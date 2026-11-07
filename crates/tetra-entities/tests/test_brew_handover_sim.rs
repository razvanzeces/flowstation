//! Simulation: ten separate FlowStations (MLE, MM, CMCE and the real Brew entity with its worker
//! thread) on one Brew network, with thirty radios moving between them during group calls.
//!
//! The network is a small in-test relay that follows brew-server's rules where they matter here:
//! a registration moves the ISSI to the registering station and keeps its groups, a group call
//! goes to every station with a radio of that group, a station that gains such a radio mid-call
//! gets the call's GROUP_TX (late entry), and handover requests reach only stations that
//! announced `handover-v1`.
//!
//! Air interface per TS 100 392-2: U-RESTORE / U-CALL RESTORE after cell reselection (clauses
//! 18.4.1.4.7, 14.7.2.2), D-RESTORE-ACK with D-CALL RESTORE (18.4.1.4.4, 14.7.1.3), registration
//! only on a location area change (clause 16), D-LOCATION UPDATE COMMAND to ask for one.

mod common;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tetra_config::bluestation::{CfgBrew, CfgExtraCell, CfgNeighborCellCa, StackConfig, StackMode, WhitelistMode};
use tetra_core::ranges::SortedDisjointSsiRanges;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, CellId, Sap, SsiType, TdmaTime, TetraAddress};
use tetra_entities::net_brew::entity::BrewEntity;
use tetra_entities::net_brew::protocol::*;
use tetra_entities::net_site::{SharedDirectory, SiteSwitch, site_links};
use tetra_entities::network::transports::{NetworkAddress, NetworkError, NetworkMessage, NetworkTransport};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::{
    cmce_pdu_type_dl::CmcePduTypeDl, party_type_identifier::PartyTypeIdentifier, transmission_grant::TransmissionGrant,
};
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::{
    d_call_restore::DCallRestore, d_connect::DConnect, d_info::DInfo, d_setup::DSetup, u_call_restore::UCallRestore, u_connect::UConnect,
    u_disconnect::UDisconnect, u_setup::USetup, u_tx_ceased::UTxCeased,
};
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::mle::pdus::{u_prepare::UPrepare, u_restore::URestore};
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use tetra_pdus::mm::fields::group_identity_location_demand::GroupIdentityLocationDemand;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_saps::control::call_control::CallControl;
use tetra_saps::control::enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType};
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tla::{TlaTlDataIndBl, TlaTlDataReqBl};
use tetra_saps::tmd::TmdCircuitDataInd;
use uuid::Uuid;

use crate::common::ComponentTest;

const STATIONS: usize = 10;
const RADIOS: u32 = 30;
const FIRST_ISSI: u32 = 3_000_001;
const GROUPS: [u32; 5] = [2001, 2002, 2003, 2004, 2005];
const EXTENSIONS: [&str; 3] = ["bts-info", "handover-v1", "ms-cell"];

// ─── Transport: one Brew connection per station, wired to the relay ─────────────────────────────

#[derive(Default)]
struct Wire {
    to_server: VecDeque<Vec<u8>>,
    to_station: VecDeque<Vec<u8>>,
}

struct ChannelTransport {
    wire: Arc<Mutex<Wire>>,
    extensions: Vec<String>,
    connected: bool,
}

impl NetworkTransport for ChannelTransport {
    fn connect(&mut self) -> Result<(), NetworkError> {
        self.connected = true;
        Ok(())
    }
    fn send_reliable(&mut self, payload: &[u8]) -> Result<(), NetworkError> {
        self.wire.lock().unwrap().to_server.push_back(payload.to_vec());
        Ok(())
    }
    fn send_unreliable(&mut self, payload: &[u8]) -> Result<(), NetworkError> {
        self.send_reliable(payload)
    }
    fn receive_reliable(&mut self) -> Vec<NetworkMessage> {
        let inbound: Vec<Vec<u8>> = self.wire.lock().unwrap().to_station.drain(..).collect();
        if inbound.is_empty() {
            // The worker polls in a tight loop; don't burn a core per station.
            std::thread::sleep(Duration::from_micros(300));
        }
        inbound
            .into_iter()
            .map(|payload| NetworkMessage {
                source: NetworkAddress::Custom {
                    scheme: "sim".into(),
                    address: "relay".into(),
                },
                payload,
                timestamp: Instant::now(),
            })
            .collect()
    }
    fn receive_unreliable(&mut self) -> Vec<NetworkMessage> {
        vec![]
    }
    fn wait_for_response_reliable(&mut self) -> Result<NetworkMessage, NetworkError> {
        Err(NetworkError::Timeout)
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
    fn server_brew_extensions(&self) -> &[String] {
        &self.extensions
    }
}

// ─── The relay (brew-server rules, reduced to what the simulation needs) ────────────────────────

struct RelayCall {
    gssi: u32,
    owner: usize,
    talker: u32,
    group_tx: Vec<u8>,
    peers: HashSet<usize>,
}

/// An individual call from a network party (an app behind the server) to a radio.
struct RelayPrivate {
    app: u32,
    radio: u32,
    /// Station the radio's leg is on
    owner: usize,
    duplex: bool,
    connected: bool,
}

#[derive(Default)]
struct Relay {
    /// ISSI -> station it is registered on
    location: HashMap<u32, usize>,
    /// ISSI -> groups (kept across registrations, as brew-server does)
    groups: HashMap<u32, HashSet<u32>>,
    /// station -> announced carriers and capabilities
    cells: HashMap<usize, (Vec<u16>, Vec<String>)>,
    calls: HashMap<Uuid, RelayCall>,
    /// Handover results the target stations sent: (station, result)
    ho_results: Vec<(usize, BrewHoResult)>,
    /// Every registration move: (issi, from, to)
    moves: Vec<(u32, Option<usize>, usize)>,
    /// Requests the relay forwarded for a serving station: relay id -> (serving station, its id)
    ho_open: HashMap<u32, (usize, u32)>,
    ho_last: u32,
    /// Every DEREGISTER a station sent: (issi, station)
    deregistrations: Vec<(u32, usize)>,
    /// Carriers whose station "goes away" after answering: the serving station gets status 2
    /// `gone` instead of the answer (the target may already have registered the radio).
    gone_carriers: HashSet<u16>,
    /// ISSIs a station asked to hand over (HO_PREPARE / HO_FORWARD_REG): their next move is a
    /// prepared one
    prepared: HashSet<u32>,
    /// Every MS_MOVED notice sent: (issi, station told)
    ms_moved: Vec<(u32, usize)>,
    /// GROUP_TX refused because another station's over held the group's floor: (station, gssi)
    floor_refused: Vec<(usize, u32)>,
    /// Every MS_RESELECTION report: (issi, station that reported, type)
    reselections: Vec<(u32, usize, String)>,
    /// Individual calls (brew-server `call_restore`)
    private: HashMap<Uuid, RelayPrivate>,
    /// Moved individual calls whose radio has not restored them yet
    restoring: HashSet<Uuid>,
    /// Every CALL_RELEASE a station sent: (station, call, cause)
    call_releases: Vec<(usize, Uuid, u8)>,
    /// Every SERVICE_CALL_RESTORE request: (station, issi, peer)
    restore_requests: Vec<(usize, u32, u32)>,
    /// Every SERVICE_CALL_NOT_RESTORED report: (station, issi)
    not_restored: Vec<(usize, u32)>,
    /// Voice frames the stations sent: (station, call)
    voice_up: Vec<(usize, Uuid)>,
    /// Calls between two radios on two stations (brew-server private calls made by
    /// `call-handoff`): call -> each party as (station, radio)
    bridges: HashMap<Uuid, Vec<(usize, u32)>>,
    /// Handoffs asked and not answered yet: (station asked, issi) -> (station that asked, peer)
    handoffs: HashMap<(usize, u32), (usize, u32)>,
    /// Every SERVICE_CALL_HANDOFF answer: (station, issi, call)
    handoff_answers: Vec<(usize, u32, Option<Uuid>)>,
}

impl Relay {
    fn handle(&mut self, from: usize, data: &[u8], wires: &[Arc<Mutex<Wire>>]) {
        let send = |to: usize, msg: &[u8]| wires[to].lock().unwrap().to_station.push_back(msg.to_vec());
        match parse_brew_message(data) {
            Ok(BrewMessage::Subscriber(sub)) => match sub.msg_type {
                BREW_SUBSCRIBER_REGISTER | BREW_SUBSCRIBER_REREGISTER => {
                    let old = self.location.insert(sub.number, from);
                    if old != Some(from) {
                        self.moves.push((sub.number, old, from));
                    }
                    self.groups.entry(sub.number).or_default().extend(sub.groups.iter().copied());
                    self.late_entry(sub.number, from, &send);
                    let prepared = self.prepared.remove(&sub.number);
                    if let Some(left) = old.filter(|o| *o != from) {
                        self.move_calls(sub.number, left, from, &send);
                        self.join_calls(sub.number, left, from, &send);
                    }
                    if let Some(left) = old.filter(|o| *o != from && !prepared) {
                        self.radio_moved(sub.number, left, from, &send);
                    }
                }
                BREW_SUBSCRIBER_DEREGISTER => {
                    self.deregistrations.push((sub.number, from));
                    if self.location.get(&sub.number) == Some(&from) {
                        self.location.remove(&sub.number);
                    }
                }
                BREW_SUBSCRIBER_AFFILIATE if self.location.get(&sub.number) == Some(&from) => {
                    self.groups.entry(sub.number).or_default().extend(sub.groups.iter().copied());
                    self.late_entry(sub.number, from, &send);
                }
                BREW_SUBSCRIBER_DEAFFILIATE if self.location.get(&sub.number) == Some(&from) => {
                    if let Some(g) = self.groups.get_mut(&sub.number) {
                        for gssi in &sub.groups {
                            g.remove(gssi);
                        }
                    }
                }
                _ => {}
            },
            Ok(BrewMessage::CallControl(cc)) => match cc.payload {
                BrewCallPayload::GroupTransmission(gt) if cc.call_state == CALL_STATE_GROUP_TX => {
                    // brew-server: one over per group; another station's over keeps the floor.
                    if self.calls.iter().any(|(id, c)| *id != cc.identifier && c.gssi == gt.destination && c.owner != from) {
                        self.floor_refused.push((from, gt.destination));
                        return;
                    }
                    let peers: HashSet<usize> = self
                        .location
                        .iter()
                        .filter(|(issi, s)| **s != from && self.groups.get(*issi).is_some_and(|g| g.contains(&gt.destination)))
                        .map(|(_, s)| *s)
                        .collect();
                    for p in &peers {
                        send(*p, data);
                    }
                    self.calls.insert(
                        cc.identifier,
                        RelayCall {
                            gssi: gt.destination,
                            owner: from,
                            talker: gt.source,
                            group_tx: data.to_vec(),
                            peers,
                        },
                    );
                }
                BrewCallPayload::CircularCall(_) if cc.call_state == CALL_STATE_CONNECT_REQUEST => {
                    if let Some(call) = self.private.get_mut(&cc.identifier).filter(|c| c.owner == from) {
                        call.connected = true;
                        send(from, &build_connect_confirm(&cc.identifier, TransmissionGrant::Granted.into_raw() as u8, 0));
                    }
                }
                BrewCallPayload::Cause(cause) if cc.call_state == CALL_STATE_CALL_RELEASE => {
                    self.call_releases.push((from, cc.identifier, cause));
                    if self.bridges.get(&cc.identifier).is_some_and(|b| b.iter().any(|(st, _)| *st == from)) {
                        for (st, _) in self.bridges.remove(&cc.identifier).unwrap() {
                            if st != from {
                                send(st, data);
                            }
                        }
                    }
                    // brew-server: control from a station that is not in the call is ignored.
                    if self.private.get(&cc.identifier).is_some_and(|c| c.owner == from) {
                        self.private.remove(&cc.identifier);
                        self.restoring.remove(&cc.identifier);
                    }
                }
                _ if (cc.call_state == CALL_STATE_SIMPLEX_GRANTED || cc.call_state == CALL_STATE_SIMPLEX_IDLE)
                    && self.bridges.contains_key(&cc.identifier) =>
                {
                    for (st, _) in &self.bridges[&cc.identifier] {
                        if *st != from {
                            send(*st, data);
                        }
                    }
                }
                _ if cc.call_state == CALL_STATE_GROUP_IDLE => {
                    if let Some(call) = self.calls.remove(&cc.identifier) {
                        for p in &call.peers {
                            send(*p, data);
                        }
                    }
                }
                _ => {}
            },
            Ok(BrewMessage::Frame(frame)) => {
                if self.private.get(&frame.identifier).is_some_and(|c| c.owner == from) {
                    self.voice_up.push((from, frame.identifier));
                }
                if let Some(bridge) = self.bridges.get(&frame.identifier).filter(|b| b.iter().any(|(st, _)| *st == from)) {
                    self.voice_up.push((from, frame.identifier));
                    for (st, _) in bridge {
                        if *st != from {
                            send(*st, data);
                        }
                    }
                }
                if let Some(call) = self.calls.get(&frame.identifier) {
                    for p in &call.peers {
                        send(*p, data);
                    }
                }
            }
            Ok(BrewMessage::Service(svc)) => match svc.service_type {
                BREW_SERVICE_BTS_INFO => {
                    let json: serde_json::Value = serde_json::from_str(&svc.json_data).unwrap();
                    let carriers = json["cells"].as_array().unwrap().iter().map(|c| c["carrier"].as_u64().unwrap() as u16).collect();
                    let caps = json["capabilities"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
                    self.cells.insert(from, (carriers, caps));
                }
                BREW_SERVICE_CALL_RESTORE => {
                    // brew-server `call_restore::handle_request`: the moved call between the
                    // radio and that peer, held for this station, or no call.
                    let json: serde_json::Value = serde_json::from_str(&svc.json_data).unwrap();
                    let (issi, peer) = (json["issi"].as_u64().unwrap() as u32, json["peer"].as_u64().unwrap() as u32);
                    self.restore_requests.push((from, issi, peer));
                    let found = self
                        .private
                        .iter()
                        .find(|(id, c)| c.radio == issi && c.app == peer && c.owner == from && self.restoring.contains(*id))
                        .map(|(id, c)| (*id, c.duplex));
                    // brew-server: no such call, but the peer is on a station with `call-handoff`:
                    // ask that one for its local call and answer when it hands it over.
                    let holder = self
                        .location
                        .get(&peer)
                        .copied()
                        .filter(|st| *st != from && self.cells.get(st).is_some_and(|(_, c)| c.iter().any(|c| c == BREW_EXT_CALL_HANDOFF)));
                    if let (None, Some(holder)) = (found, holder) {
                        self.handoffs.insert((holder, issi), (from, peer));
                        let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_CALL_HANDOFF];
                        msg.extend_from_slice(serde_json::json!({"issi": issi, "peer": peer}).to_string().as_bytes());
                        msg.push(0);
                        send(holder, &msg);
                        return;
                    }
                    let answer = match found {
                        Some((id, duplex)) => {
                            self.restoring.remove(&id);
                            serde_json::json!({"issi": issi, "peer": peer, "uuid": id.to_string(), "duplex": duplex, "priority": 0, "floor": 0})
                        }
                        None => serde_json::json!({"issi": issi, "peer": peer}),
                    };
                    let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_CALL_RESTORE];
                    msg.extend_from_slice(answer.to_string().as_bytes());
                    msg.push(0);
                    send(from, &msg);
                }
                BREW_SERVICE_CALL_HANDOFF => {
                    // brew-server `call_restore::handle_handoff`: the local call, bridged now,
                    // becomes a call between the two stations and answers the restore request.
                    let json: serde_json::Value = serde_json::from_str(&svc.json_data).unwrap();
                    let (issi, peer) = (json["issi"].as_u64().unwrap() as u32, json["peer"].as_u64().unwrap() as u32);
                    let id = json["uuid"].as_str().map(|u| Uuid::parse_str(u).unwrap());
                    self.handoff_answers.push((from, issi, id));
                    let Some((asker, _)) = self.handoffs.remove(&(from, issi)) else { return };
                    let answer = match id {
                        Some(id) => {
                            self.bridges.insert(id, vec![(from, peer), (asker, issi)]);
                            serde_json::json!({"issi": issi, "peer": peer, "uuid": id.to_string(), "duplex": json["duplex"],
                                "priority": 0, "floor": json["floor"]})
                        }
                        None => serde_json::json!({"issi": issi, "peer": peer}),
                    };
                    let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_CALL_RESTORE];
                    msg.extend_from_slice(answer.to_string().as_bytes());
                    msg.push(0);
                    send(asker, &msg);
                }
                BREW_SERVICE_CALL_NOT_RESTORED => {
                    // brew-server `call_restore::handle_not_restored`: the moved call ends at once.
                    let json: serde_json::Value = serde_json::from_str(&svc.json_data).unwrap();
                    let issi = json["issi"].as_u64().unwrap() as u32;
                    self.not_restored.push((from, issi));
                    let ended: Vec<Uuid> =
                        self.private.iter().filter(|(id, c)| c.radio == issi && c.owner == from && self.restoring.contains(*id)).map(|(id, _)| *id).collect();
                    for id in ended {
                        self.restoring.remove(&id);
                        self.private.remove(&id);
                    }
                }
                BREW_SERVICE_MS_RESELECTION => {
                    let json: serde_json::Value = serde_json::from_str(&svc.json_data).unwrap();
                    let issi = json["issi"].as_u64().unwrap() as u32;
                    self.reselections.push((issi, from, json["type"].as_str().unwrap().to_string()));
                }
                BREW_SERVICE_HO_RESULT => {
                    let result = parse_ho_result(&svc.json_data).unwrap();
                    self.ho_results.push((from, result.clone()));
                    // brew-server: relay the target's answer to the serving station under its id.
                    if let Some((serving, id)) = self.ho_open.remove(&result.transaction_id) {
                        let gone = self.cells.get(&from).is_some_and(|(c, _)| c.iter().any(|c| self.gone_carriers.contains(c)));
                        let result = if gone {
                            BrewHoResult {
                                status: BREW_HO_STATUS_NO_TARGET,
                                reason: Some("gone".into()),
                                sdu: None,
                                channel: None,
                                ..result
                            }
                        } else {
                            result
                        };
                        send(serving, &build_service_ho_result(&BrewHoResult { transaction_id: id, ..result }));
                    }
                }
                BREW_SERVICE_HO_PREPARE | BREW_SERVICE_HO_FORWARD_REG => {
                    // brew-server's relay: the capable station serving the carrier gets the request
                    // under the relay's own id; nobody → "no target" at once.
                    let mut req = parse_ho_request(svc.service_type, &svc.json_data).unwrap();
                    self.prepared.insert(req.issi);
                    let target = self.cells.iter().find(|(st, (carriers, caps))| {
                        **st != from && carriers.contains(&req.target_carrier) && caps.iter().any(|c| c == BREW_EXT_HANDOVER)
                    });
                    match target {
                        Some((&target, _)) => {
                            self.ho_last += 1;
                            let id = 1_000_000 + self.ho_last;
                            self.ho_open.insert(id, (from, req.transaction_id));
                            req.transaction_id = id;
                            send(target, &build_service_ho_request(&req));
                        }
                        None => send(
                            from,
                            &build_service_ho_result(&BrewHoResult {
                                transaction_id: req.transaction_id,
                                issi: req.issi,
                                status: BREW_HO_STATUS_NO_TARGET,
                                reason: Some("unknown".into()),
                                sdu: None,
                                channel: None,
                            }),
                        ),
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// brew-server late entry: a station that gains a radio of a running call's group gets the call.
    fn late_entry(&mut self, issi: u32, station: usize, send: &impl Fn(usize, &[u8])) {
        let groups = self.groups.get(&issi).cloned().unwrap_or_default();
        for call in self.calls.values_mut() {
            if groups.contains(&call.gssi) && call.owner != station && call.peers.insert(station) {
                send(station, &call.group_tx);
            }
        }
    }

    /// brew-server `call_restore::move_calls`: a radio that registers on a station with
    /// `call-restore` takes its connected individual calls along; the station it left lets them
    /// go (CALL_RELEASE, SwMI requested disconnection) and the radio restores them there.
    fn move_calls(&mut self, issi: u32, left: usize, to: usize, send: &impl Fn(usize, &[u8])) {
        let capable = self.cells.get(&to).is_some_and(|(_, c)| c.iter().any(|c| c == BREW_EXT_CALL_RESTORE));
        if !capable {
            return;
        }
        for (id, call) in self.private.iter_mut().filter(|(_, c)| c.radio == issi && c.owner == left && c.connected) {
            call.owner = to;
            self.restoring.insert(*id);
            send(left, &build_call_release(id, 14));
        }
    }

    /// brew-server `call_restore::move_calls`, join case: a radio of a call between two stations
    /// registers on its peer's station, which announced `call-handoff`: the station it left lets
    /// its leg go, and the peer's station joins both legs itself.
    fn join_calls(&mut self, issi: u32, left: usize, to: usize, send: &impl Fn(usize, &[u8])) {
        if !self.cells.get(&to).is_some_and(|(_, c)| c.iter().any(|c| c == BREW_EXT_CALL_HANDOFF)) {
            return;
        }
        for (id, parties) in self.bridges.iter_mut() {
            let joins = parties.contains(&(left, issi)) && parties.iter().any(|(st, r)| *st == to && *r != issi);
            if joins {
                for p in parties.iter_mut().filter(|p| **p == (left, issi)) {
                    p.0 = to;
                }
                send(left, &build_call_release(id, 14));
            }
        }
    }

    /// A network party calls `radio` on `station`.
    fn call_radio(&mut self, id: Uuid, app: u32, radio: u32, station: usize, duplex: bool, wires: &[Arc<Mutex<Wire>>]) {
        let call = BrewCircularCall {
            source: app,
            destination: radio,
            number: String::new(),
            priority: 0,
            service: 0,
            mode: 0,
            duplex: duplex as u8,
            method: 0,
            communication: 0,
            grant: 0,
            permission: 0,
            timeout: 0,
            ownership: 0,
            queued: 0,
            mnemonic: None,
        };
        self.private.insert(
            id,
            RelayPrivate {
                app,
                radio,
                owner: station,
                duplex,
                connected: false,
            },
        );
        wires[station].lock().unwrap().to_station.push_back(build_setup_request(&id, &call));
    }

    /// brew-server `handover::radio_moved`: the station a radio left unprepared is told (if it
    /// asked for `ms-moved`), and an over the radio was talking in there ends on the network (if
    /// the new station has handover-v1), so the radio can talk on from the new station.
    fn radio_moved(&mut self, issi: u32, left: usize, to: usize, send: &impl Fn(usize, &[u8])) {
        let caps = |st: usize| self.cells.get(&st).map(|(_, c)| c.clone()).unwrap_or_default();
        if caps(left).iter().any(|c| c == BREW_EXT_MS_MOVED) {
            let mut notice = vec![BREW_CLASS_SERVICE, BREW_SERVICE_MS_MOVED];
            notice.extend_from_slice(format!(r#"{{"issi":{issi}}}"#).as_bytes());
            notice.push(0);
            send(left, &notice);
            self.ms_moved.push((issi, left));
        }
        if !caps(to).iter().any(|c| c == BREW_EXT_HANDOVER) {
            return;
        }
        let talking: Vec<Uuid> = self.calls.iter().filter(|(_, c)| c.owner == left && c.talker == issi).map(|(id, _)| *id).collect();
        for id in talking {
            let call = self.calls.remove(&id).unwrap();
            for p in &call.peers {
                send(*p, &build_group_idle(&id, 0));
            }
        }
    }

    /// The server side of a forwarded registration towards the target (milestone 3 relay).
    fn forward_registration(&self, target_carrier: u16, req: &BrewHoRequest, wires: &[Arc<Mutex<Wire>>]) -> usize {
        let (target, _) = self
            .cells
            .iter()
            .find(|(_, (carriers, caps))| carriers.contains(&target_carrier) && caps.iter().any(|c| c == BREW_EXT_HANDOVER))
            .expect("a capable station serves the carrier");
        wires[*target].lock().unwrap().to_station.push_back(build_service_ho_request(req));
        *target
    }
}

// ─── Stations ───────────────────────────────────────────────────────────────────────────────────

fn relay_brew() -> CfgBrew {
    CfgBrew {
        host: "relay.sim".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        whitelisted_ssis: None,
        feature_rssi_export: false,
        feature_lip_forward: false,
        lip_forward_issi: None,
        pbx_gateway_issis: None,
        backhaul_hysteresis_secs: 0,
    }
}

struct Station {
    test: ComponentTest,
    wire: Arc<Mutex<Wire>>,
    carrier: u16,
    /// Everything this station put on air (LLC sink), oldest first
    air: Vec<SapMsg>,
    /// Downlink voice the station played out on a traffic channel: (carrier, timeslot)
    voice_down: Vec<(u16, u8)>,
}

fn station(index: usize, location_area: u16, extensions: &[&str], configure: &dyn Fn(usize, &mut StackConfig)) -> Station {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    let carrier = 1521 + 2 * index as u16;
    config.cell.main_carrier = carrier;
    config.cell.location_area = location_area;
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_ssirange(vec![]);
    config.brew = Some(relay_brew());
    configure(index, &mut config);
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Mle, TetraEntity::Mm, TetraEntity::Cmce],
        vec![TetraEntity::Llc, TetraEntity::Umac],
    );
    let wire = Arc::new(Mutex::new(Wire::default()));
    let transport = ChannelTransport {
        wire: wire.clone(),
        extensions: extensions.iter().map(|e| e.to_string()).collect(),
        connected: false,
    };
    test.register_entity(BrewEntity::new(test.get_shared_config(), transport));
    Station {
        test,
        wire,
        carrier,
        air: Vec::new(),
        voice_down: Vec::new(),
    }
}

/// Uplink PDU from `issi` as LLC hands it to MLE.
fn uplink(issi: u32, discriminator: MleProtocolDiscriminator, write: impl FnOnce(&mut BitBuffer)) -> SapMsg {
    let mut b = BitBuffer::new_autoexpand(128);
    b.write_bits(discriminator.into_raw(), 3);
    write(&mut b);
    let len = b.get_pos();
    b.seek(0);
    let mut sdu = BitBuffer::new(len);
    sdu.copy_bits(&mut b, len);
    sdu.seek(0);
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
            main_address: TetraAddress::new(issi, SsiType::Issi),
            link_id: 0,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(sdu),
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            req_handle: 0,
        }),
    }
}

fn location_update_pdu(issi: u32, lu_type: LocationUpdateType, groups: &[u32]) -> ULocationUpdateDemand {
    ULocationUpdateDemand {
        location_update_type: lu_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode: None,
        la_information: None,
        ssi: Some(issi as u64),
        address_extension: None,
        group_identity_location_demand: (!groups.is_empty()).then(|| GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 0,
            group_identity_uplink: Some(
                groups
                    .iter()
                    .map(|g| GroupIdentityUplink {
                        class_of_usage: Some(4),
                        group_identity_detachment_uplink: None,
                        gssi: Some(*g),
                        address_extension: None,
                        vgssi: None,
                    })
                    .collect(),
            ),
        }),
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    }
}

/// U-ATTACH/DETACH GROUP IDENTITY with "detach all and attach" (table 16.49), as a radio sends
/// its group report.
fn u_attach_detach_all(issi: u32, groups: &[u32]) -> SapMsg {
    let lu = location_update_pdu(issi, LocationUpdateType::DemandLocationUpdating, groups);
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: false,
        group_identity_attach_detach_mode: true,
        group_report_response: None,
        group_identity_uplink: lu.group_identity_location_demand.and_then(|d| d.group_identity_uplink),
        proprietary: None,
    };
    uplink(issi, MleProtocolDiscriminator::Mm, |b| pdu.to_bitbuf(b).unwrap())
}

fn u_setup(issi: u32, gssi: u32) -> SapMsg {
    u_setup_prio(issi, gssi, 0)
}

fn u_setup_prio(issi: u32, gssi: u32, call_priority: u8) -> SapMsg {
    let pdu = USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: true,
        call_priority,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(gssi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    uplink(issi, MleProtocolDiscriminator::Cmce, |b| pdu.to_bitbuf(b).unwrap())
}

/// U-SETUP of an individual call from `issi` to radio `called` (TS 100 392-2 clause 14.7.2.1).
fn u_setup_individual(issi: u32, called: u32, duplex: bool) -> SapMsg {
    let pdu = USetup {
        area_selection: 0,
        hook_method_selection: true,
        simplex_duplex_selection: duplex,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(called as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    uplink(issi, MleProtocolDiscriminator::Cmce, |b| pdu.to_bitbuf(b).unwrap())
}

fn u_disconnect(issi: u32, call_id: u16) -> SapMsg {
    let pdu = UDisconnect {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    };
    uplink(issi, MleProtocolDiscriminator::Cmce, |b| pdu.to_bitbuf(b).unwrap())
}

fn u_tx_ceased(issi: u32, call_id: u16) -> SapMsg {
    let pdu = UTxCeased {
        call_identifier: call_id,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    uplink(issi, MleProtocolDiscriminator::Cmce, |b| pdu.to_bitbuf(b).unwrap())
}

/// U-RESTORE carrying U-CALL RESTORE. `old_la` is given when the radio moved to another LA
/// (note 1 to table 18.13); without it the PDU has O-bit 0 and the SDU right after (table E.29).
fn u_restore(issi: u32, call_id: u16, gssi: u32, old_la: Option<u64>) -> SapMsg {
    let call_restore = UCallRestore {
        call_identifier: call_id,
        // A listener: "request that other MS may transmit" (table 14.74).
        request_to_transmit_send_data: true,
        other_party_type_identifier: 1,
        other_party_short_number_address: None,
        other_party_ssi: Some(gssi as u64),
        other_party_extension: None,
        basic_service_information: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(64);
    call_restore.to_bitbuf(&mut sdu).unwrap();
    let len = sdu.get_pos();
    sdu.seek(0);
    let mut exact = BitBuffer::new(len);
    exact.copy_bits(&mut sdu, len);
    exact.seek(0);
    let restore = URestore {
        mcc: None,
        mnc: None,
        la: old_la,
        sdu: Some(exact),
    };
    uplink(issi, MleProtocolDiscriminator::Mle, |b| restore.to_bitbuf(b).unwrap())
}

/// What a station sent on air to `ssi`, as bit strings (discriminator included).
fn air_to(air: &[SapMsg], ssi: u32) -> Vec<String> {
    air.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::TlaTlDataReqBl(r) if r.main_address.ssi == ssi => Some(r.tl_sdu.to_bitstr()),
            SapMsgInner::TlaTlUnitdataReqBl(r) if r.main_address.ssi == ssi => Some(r.tl_sdu.to_bitstr()),
            _ => None,
        })
        .collect()
}

const PD_MM: &str = "001";
const PD_CMCE: &str = "010";
const PD_MLE: &str = "101";

fn mm_pdu(t: MmPduTypeDl) -> String {
    format!("{PD_MM}{:04b}", t.into_raw())
}

/// The call identifier `issi` uses after D-RESTORE-ACK (the D-CALL RESTORE's new call
/// identifier, else its call identifier), if one was sent. Checks that the identifier the radio
/// restored with is echoed and that a channel allocation goes with it (clause 14.5.2.2.4).
fn restore_ack(air: &[SapMsg], issi: u32) -> Option<u16> {
    air.iter().find_map(|m| {
        let SapMsgInner::TlaTlDataReqBl(r) = &m.msg else {
            return None;
        };
        if r.main_address.ssi != issi {
            return None;
        }
        // D-RESTORE-ACK = MLE type 4, O-bit 0, then D-CALL RESTORE (table E.23).
        let rest = r.tl_sdu.to_bitstr().strip_prefix(&format!("{PD_MLE}1000"))?.to_string();
        let mut b = BitBuffer::from_bitstr(&rest);
        let restore = DCallRestore::from_bitbuf(&mut b).ok()?;
        assert!(r.chan_alloc.is_some(), "D-RESTORE-ACK to {issi} without channel allocation");
        Some(restore.new_call_identifier.map_or(restore.call_identifier, |id| id as u16))
    })
}

/// The D-CALL RESTORE `issi` got in a D-RESTORE-ACK, with the timeslot of its channel allocation.
fn call_restore_to(air: &[SapMsg], issi: u32) -> Option<(DCallRestore, u8)> {
    air.iter().find_map(|m| {
        let SapMsgInner::TlaTlDataReqBl(r) = &m.msg else {
            return None;
        };
        if r.main_address.ssi != issi {
            return None;
        }
        let rest = r.tl_sdu.to_bitstr().strip_prefix(&format!("{PD_MLE}1000"))?.to_string();
        let restore = DCallRestore::from_bitbuf(&mut BitBuffer::from_bitstr(&rest)).ok()?;
        let alloc = r.chan_alloc.as_ref().expect("D-CALL RESTORE with the U-plane on carries a channel allocation (14.5.1.2.4)");
        let ts = alloc.timeslots.iter().position(|t| *t).expect("a timeslot is allocated") as u8 + 1;
        Some((restore, ts))
    })
}

/// The call identifier of the last D-SETUP sent to radio `issi` (an incoming individual call).
fn individual_setup_call_id(air: &[SapMsg], issi: u32) -> Option<u16> {
    group_setup_call_id(air, issi)
}

fn cmce_pdu_to(air: &[SapMsg], ssi: u32, pdu: CmcePduTypeDl) -> bool {
    air_to(air, ssi)
        .iter()
        .any(|bits| bits.strip_prefix(PD_CMCE).is_some_and(|rest| BitBuffer::from_bitstr(rest).peek_bits(5) == Some(pdu.into_raw())))
}

fn u_connect(issi: u32, call_id: u16, duplex: bool) -> SapMsg {
    let pdu = UConnect {
        call_identifier: call_id,
        hook_method_selection: true,
        simplex_duplex_selection: duplex,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    };
    uplink(issi, MleProtocolDiscriminator::Cmce, |b| pdu.to_bitbuf(b).unwrap())
}

/// U-RESTORE carrying the U-CALL RESTORE of an individual call with `peer` (clause 14.5.1.2.4:
/// the other party's address, and the basic service information, mandatory in a U-CALL RESTORE
/// to a station that does not know the call, note 3 to table 14.22).
fn u_restore_individual(issi: u32, call_id: u16, peer: u32, request_to_transmit: bool, old_la: u64) -> SapMsg {
    let call_restore = UCallRestore {
        call_identifier: call_id,
        // Table 14.74: 0 = request to transmit, 1 = request that other MS may transmit.
        request_to_transmit_send_data: !request_to_transmit,
        other_party_type_identifier: 1,
        other_party_short_number_address: None,
        other_party_ssi: Some(peer as u64),
        other_party_extension: None,
        basic_service_information: Some(BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        }),
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(64);
    call_restore.to_bitbuf(&mut sdu).unwrap();
    let len = sdu.get_pos();
    sdu.seek(0);
    let mut exact = BitBuffer::new(len);
    exact.copy_bits(&mut sdu, len);
    exact.seek(0);
    let restore = URestore {
        mcc: None,
        mnc: None,
        la: Some(old_la),
        sdu: Some(exact),
    };
    uplink(issi, MleProtocolDiscriminator::Mle, |b| restore.to_bitbuf(b).unwrap())
}

fn restore_fail(air: &[SapMsg], issi: u32) -> bool {
    air_to(air, issi).iter().any(|bits| bits.starts_with(&format!("{PD_MLE}101")))
}

fn group_release_sent(air: &[SapMsg], gssi: u32) -> bool {
    air_to(air, gssi).iter().any(|bits| {
        bits.strip_prefix(PD_CMCE)
            .is_some_and(|rest| BitBuffer::from_bitstr(rest).peek_bits(5) == Some(CmcePduTypeDl::DRelease.into_raw()))
    })
}

fn group_setups(air: &[SapMsg], gssi: u32) -> usize {
    air_to(air, gssi)
        .iter()
        .filter(|bits| {
            bits.strip_prefix(PD_CMCE)
                .is_some_and(|rest| BitBuffer::from_bitstr(rest).peek_bits(5) == Some(CmcePduTypeDl::DSetup.into_raw()))
        })
        .count()
}

fn group_setup_call_id(air: &[SapMsg], gssi: u32) -> Option<u16> {
    air_to(air, gssi).iter().rev().find_map(|bits| {
        let rest = bits.strip_prefix(PD_CMCE)?;
        let mut b = BitBuffer::from_bitstr(rest);
        (b.peek_bits(5) == Some(CmcePduTypeDl::DSetup.into_raw())).then_some(())?;
        DSetup::from_bitbuf(&mut b).ok().map(|s| s.call_identifier)
    })
}

// ─── The network ────────────────────────────────────────────────────────────────────────────────

struct Sim {
    stations: Vec<Station>,
    wires: Vec<Arc<Mutex<Wire>>>,
    relay: Relay,
    /// TDMA ticks run so far
    ticks: usize,
}

impl Sim {
    /// `location_areas[i]` is station i's LA; neighbouring stations sharing an LA model the
    /// "reselection within the registered area" case.
    fn new(location_areas: &[u16], extensions: &[&str]) -> Self {
        Self::with_config(location_areas, extensions, &|_, _| {})
    }

    fn with_config(location_areas: &[u16], extensions: &[&str], configure: &dyn Fn(usize, &mut StackConfig)) -> Self {
        let stations: Vec<Station> = location_areas
            .iter()
            .enumerate()
            .map(|(i, la)| station(i, *la, extensions, configure))
            .collect();
        let wires = stations.iter().map(|s| s.wire.clone()).collect();
        let mut sim = Sim {
            stations,
            wires,
            relay: Relay::default(),
            ticks: 0,
        };
        sim.run_until("every station announced its cell", |s| s.relay.cells.len() == location_areas.len());
        sim
    }

    /// One TDMA tick on every station, then deliver what the stations sent to the relay. Paced so
    /// the stations' TDMA time does not outrun the worker threads (CMCE timers run on TDMA time).
    fn step(&mut self) {
        self.ticks += 1;
        for s in &mut self.stations {
            s.test.run_stack(Some(1));
            for m in s.test.dump_sinks() {
                match &m.msg {
                    SapMsgInner::TmdCircuitDataReq(v) => s.voice_down.push((v.carrier_num, v.ts)),
                    _ if m.dest == TetraEntity::Llc => s.air.push(m),
                    _ => {}
                }
            }
        }
        for i in 0..self.wires.len() {
            let out: Vec<Vec<u8>> = self.wires[i].lock().unwrap().to_server.drain(..).collect();
            for msg in out {
                self.relay.handle(i, &msg, &self.wires);
            }
        }
        std::thread::sleep(Duration::from_micros(500));
    }

    fn run_until(&mut self, what: &str, done: impl Fn(&Sim) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            self.step();
        }
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn submit(&mut self, station: usize, msg: SapMsg) {
        self.stations[station].test.submit_message(msg);
    }

    fn register(&mut self, station: usize, issi: u32, lu_type: LocationUpdateType, groups: &[u32]) {
        let lu = location_update_pdu(issi, lu_type, groups);
        self.submit(station, uplink(issi, MleProtocolDiscriminator::Mm, |b| lu.to_bitbuf(b).unwrap()));
        self.run_until(&format!("ISSI {issi} is registered on station {station} on the network"), |s| {
            s.relay.location.get(&issi) == Some(&station)
        });
    }

    /// A radio of `gssi` on `station` starts a group call; returns the Brew session.
    fn start_call(&mut self, station: usize, talker: u32, gssi: u32) -> Uuid {
        self.submit(station, u_setup(talker, gssi));
        self.run_until(&format!("the call on {gssi} reached the network"), |s| {
            s.relay.calls.values().any(|c| c.gssi == gssi && c.owner == station)
        });
        *self.relay.calls.iter().find(|(_, c)| c.gssi == gssi && c.owner == station).unwrap().0
    }

    /// The talker releases PTT: the station ends the over on the network (GROUP_IDLE) and every
    /// station releases its leg after hang time.
    fn end_call(&mut self, station: usize, talker: u32, gssi: u32, session: Uuid) {
        let call_id = self.local_call_id(station, gssi).expect("the talker's station set the call up");
        self.submit(station, u_tx_ceased(talker, call_id));
        self.run_until(&format!("the over on {gssi} ended on the network"), |s| !s.relay.calls.contains_key(&session));
        self.steps(600);
    }

    fn local_call_id(&self, station: usize, gssi: u32) -> Option<u16> {
        group_setup_call_id(&self.stations[station].air, gssi)
    }

    fn clear_air(&mut self) {
        for s in &mut self.stations {
            s.air.clear();
        }
    }
}

// ─── Scenarios ──────────────────────────────────────────────────────────────────────────────────

/// Ten stations, each its own location area. Thirty radios register; group calls run; every
/// listener moves to another station mid-call (unannounced reselection with a registration,
/// since the LA changes) and must be restored into the call there, although the new station had
/// no listener for the group before (the milestone 0 field failure).
#[test]
fn sim_ten_stations_thirty_radios_unannounced_reselection_keeps_calls() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let mut sim = Sim::new(&las, &EXTENSIONS);
    assert!(
        sim.relay.cells.values().all(|(_, caps)| caps == &vec![BREW_EXT_HANDOVER.to_string()]),
        "single-cell stations announce handover-v1"
    );

    // Radio r sits on station r % 10, member of group r % 5.
    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    // Three rounds: in each, talkers on three groups start calls and every other member of those
    // groups moves two stations on and restores.
    let mut restored = 0;
    for round in 0..3usize {
        sim.clear_air();
        let groups = [GROUPS[round], GROUPS[(round + 1) % 5], GROUPS[(round + 3) % 5]];
        let mut talkers = HashSet::new();
        let mut sessions = Vec::new();
        for gssi in groups {
            // A different member talks each round.
            let &(talker, _, _) = radios.iter().filter(|(_, _, g)| *g == gssi).nth(round).unwrap();
            let st = sim.relay.location[&talker];
            talkers.insert(talker);
            sessions.push((gssi, st, talker, sim.start_call(st, talker, gssi)));
        }
        sim.steps(20);

        for &(issi, _, gssi) in &radios {
            if talkers.contains(&issi) || !groups.contains(&gssi) {
                continue;
            }
            let from = sim.relay.location[&issi];
            let to = (from + 2 + round) % STATIONS;
            // The call id the radio knows: its old station's (it listened there).
            let old_call_id = sim.local_call_id(from, gssi).unwrap_or(1);

            // New LA: the radio registers first (clause 16), then restores its call. Half the
            // radios report their group in the registration; the others leave it out, so the
            // station's MM asks for a group report (D-LOCATION UPDATE COMMAND), which those
            // radios answer only after the restore.
            let reports_group = issi % 2 == 0;
            let groups: &[u32] = if reports_group { &[gssi] } else { &[] };
            sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, groups);
            // Half the radios give the old LA in U-RESTORE, the others leave it out (both codings).
            let old_la = (issi % 4 < 2).then_some(from as u64 + 1);
            sim.submit(to, u_restore(issi, old_call_id, gssi, old_la));
            sim.run_until(&format!("ISSI {issi} gets an answer to its U-RESTORE on station {to}"), |s| {
                restore_ack(&s.stations[to].air, issi).is_some() || restore_fail(&s.stations[to].air, issi)
            });
            let ack = restore_ack(&sim.stations[to].air, issi).unwrap_or_else(|| panic!("ISSI {issi} was refused on station {to}"));
            assert_eq!(Some(ack), sim.local_call_id(to, gssi), "restored into station {to}'s call");
            if !reports_group {
                sim.register(to, issi, LocationUpdateType::DemandLocationUpdating, &[gssi]);
            }
            restored += 1;
        }

        // Calls end.
        for (gssi, st, talker, session) in sessions {
            sim.end_call(st, talker, gssi, session);
        }
    }
    assert!(restored >= 25, "only {restored} restores exercised");

    assert!(sim.relay.ms_moved.is_empty(), "nobody is told without ms-moved");
    // The network has every radio where it last went.
    for &(issi, _, _) in &radios {
        assert!(sim.relay.location.contains_key(&issi), "ISSI {issi} still registered");
    }
}

/// Stations 0..4 share LA 1 and 5..9 share LA 2. A radio moving within its LA does not register
/// (clause 16): the new station asks it to (D-LOCATION UPDATE COMMAND), the network moves the ISSI
/// on that registration and hands the station the call.
#[test]
fn sim_reselection_within_the_location_area_registers_and_restores() {
    let las = [1, 1, 1, 1, 1, 2, 2, 2, 2, 2];
    let mut sim = Sim::new(&las, &EXTENSIONS);

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let mut restored = 0;
    for gssi in GROUPS {
        sim.clear_air();
        let &(talker, st, _) = radios.iter().find(|(_, _, g)| *g == gssi).unwrap();
        let st = sim.relay.location.get(&talker).copied().unwrap_or(st);
        let session = sim.start_call(st, talker, gssi);
        sim.steps(20);

        for &(issi, _, g) in &radios {
            if g != gssi || issi == talker {
                continue;
            }
            let from = sim.relay.location[&issi];
            // Next station in the same LA (0→1→…→4→0, 5→6→…→9→5).
            let to = (from / 5) * 5 + (from % 5 + 1) % 5;
            let old_call_id = sim.local_call_id(from, gssi).unwrap_or(1);

            // Same LA: U-RESTORE without MCC/MNC/LA (table E.29).
            sim.submit(to, u_restore(issi, old_call_id, gssi, None));
            sim.steps(3);
            assert!(
                air_to(&sim.stations[to].air, issi).iter().any(|b| b.starts_with(&mm_pdu(MmPduTypeDl::DLocationUpdateCommand))),
                "station {to} asks unknown ISSI {issi} to register"
            );
            assert!(!restore_fail(&sim.stations[to].air, issi), "not refused while waiting");
            // The radio answers the command with a demand location update and its groups.
            sim.register(to, issi, LocationUpdateType::DemandLocationUpdating, &[gssi]);
            sim.run_until(&format!("ISSI {issi} restored on station {to}"), |s| {
                restore_ack(&s.stations[to].air, issi).is_some() || restore_fail(&s.stations[to].air, issi)
            });
            assert!(restore_ack(&sim.stations[to].air, issi).is_some(), "ISSI {issi} refused on station {to}");
            restored += 1;
        }
        sim.end_call(st, talker, gssi, session);
    }
    assert!(restored >= 20, "only {restored} restores exercised");
}

/// Forwarded registrations (announced reselection type 1) from the network: every target station
/// hands the MM PDU to its MM, registers the radio (which moves the ISSI on the network, keeping
/// its groups, and brings the running call along) and answers with MM's D-LOCATION UPDATE ACCEPT.
#[test]
fn sim_forwarded_registrations_are_answered_with_the_mm_accept() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let mut sim = Sim::new(&las, &EXTENSIONS);

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }
    // A call on group 2001 runs while its members are handed over.
    let talker = radios[0].0;
    sim.start_call(radios[0].1, talker, GROUPS[0]);

    for (n, &(issi, _, gssi)) in radios.iter().enumerate().skip(1) {
        let from = sim.relay.location[&issi];
        let to = (from + 1 + n % 4) % STATIONS;
        let lu = location_update_pdu(issi, LocationUpdateType::RoamingLocationUpdating, &[]);
        let mut sdu = BitBuffer::new_autoexpand(64);
        lu.to_bitbuf(&mut sdu).unwrap();
        let len = sdu.get_pos();
        sdu.seek(0);
        let mut exact = BitBuffer::new(len);
        exact.copy_bits(&mut sdu, len);
        let req = BrewHoRequest {
            transaction_id: 1000 + n as u32,
            issi,
            target_carrier: sim.stations[to].carrier,
            target_carrier_ext: 264,
            target_mcc: Some(204),
            target_mnc: Some(1337),
            target_location_area: Some(las[to]),
            sdu: Some(BrewHoSdu::from_bitbuffer(&exact)),
            call_gssi: None,
        };
        let target = sim.relay.forward_registration(sim.stations[to].carrier, &req, &sim.wires.clone());
        assert_eq!(target, to);
        sim.run_until(&format!("station {to} answers ISSI {issi}'s forwarded registration"), |s| {
            s.relay.ho_results.iter().any(|(_, r)| r.transaction_id == 1000 + n as u32)
        });
        let (answered_by, result) = sim.relay.ho_results.iter().find(|(_, r)| r.transaction_id == 1000 + n as u32).unwrap().clone();
        assert_eq!(answered_by, to);
        assert_eq!((result.issi, result.status), (issi, BREW_HO_STATUS_OK));
        let answer = result.sdu.expect("MM answer for D-NEW-CELL").to_bitbuffer().to_bitstr();
        assert!(answer.starts_with(&format!("{:04b}", MmPduTypeDl::DLocationUpdateAccept.into_raw())), "D-LOCATION UPDATE ACCEPT");
        assert!(
            !air_to(&sim.stations[to].air, issi).iter().any(|b| b.starts_with(&mm_pdu(MmPduTypeDl::DLocationUpdateAccept))),
            "MM's accept goes back to the serving station, not on air at the target"
        );
        assert_eq!(sim.relay.location.get(&issi), Some(&to), "the ISSI moved before the radio left");
        assert!(sim.relay.groups[&issi].contains(&gssi), "groups kept");
        if gssi == GROUPS[0] {
            assert!(
                sim.relay.calls.values().any(|c| c.gssi == gssi && c.peers.contains(&to)),
                "the running call reaches the target station"
            );
        }
    }
    assert!(sim.relay.ho_results.len() >= 25);
}

/// A station on a server without `handover-v1` (or TetraPack / Brandmeister) announces nothing,
/// answers nothing, and still restores calls after an unannounced reselection.
#[test]
fn sim_without_the_extension_nothing_new_is_sent_and_restore_still_works() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let mut sim = Sim::new(&las, &["bts-info", "ms-cell"]);
    assert!(sim.relay.cells.values().all(|(_, caps)| caps.is_empty()));

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[0])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }
    // A forwarded registration sent anyway is ignored (the station never offered to answer).
    let req = BrewHoRequest {
        transaction_id: 9,
        issi: FIRST_ISSI,
        target_carrier: sim.stations[3].carrier,
        target_carrier_ext: 264,
        target_mcc: None,
        target_mnc: None,
        target_location_area: None,
        sdu: Some(BrewHoSdu { bytes: vec![0x20, 0x03], len_bits: 16 }),
        call_gssi: None,
    };
    sim.wires[3].lock().unwrap().to_station.push_back(build_service_ho_request(&req));
    sim.steps(50);
    assert!(sim.relay.ho_results.is_empty());

    let session = sim.start_call(radios[0].1, radios[0].0, GROUPS[0]);
    sim.steps(20);
    let mover = radios[1].0;
    let from = sim.relay.location[&mover];
    let to = (from + 5) % STATIONS;
    let to = if to == radios[0].1 { (to + 1) % STATIONS } else { to };
    let old = sim.local_call_id(from, GROUPS[0]).unwrap_or(1);
    sim.register(to, mover, LocationUpdateType::RoamingLocationUpdating, &[]);
    sim.submit(to, u_restore(mover, old, GROUPS[0], Some(from as u64 + 1)));
    sim.run_until("the restore is answered", |s| restore_ack(&s.stations[to].air, mover).is_some());
    let _ = session;
}

/// A target whose MM rejects the radio (here: not on station 9's access list) answers "rejected"
/// with MM's D-LOCATION UPDATE REJECT, so the serving station sends D-PREPARE-FAIL with it
/// (clause 18.4.1.4.3). The ISSI stays where it was on the network.
#[test]
fn sim_forwarded_registration_rejected_by_the_target_mm() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let allowed: Vec<u32> = (0..RADIOS / 2).map(|r| FIRST_ISSI + r).collect();
    let mut sim = Sim::with_config(&las, &EXTENSIONS, &|i, cfg| {
        if i == 9 {
            cfg.security.whitelist_mode = WhitelistMode::Enforce;
            cfg.security.issi_whitelist = allowed.clone();
        }
    });
    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % 9, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    for (n, &(issi, from, _)) in radios.iter().enumerate() {
        let lu = location_update_pdu(issi, LocationUpdateType::RoamingLocationUpdating, &[]);
        let mut sdu = BitBuffer::new_autoexpand(64);
        lu.to_bitbuf(&mut sdu).unwrap();
        let len = sdu.get_pos();
        sdu.seek(0);
        let mut exact = BitBuffer::new(len);
        exact.copy_bits(&mut sdu, len);
        let req = BrewHoRequest {
            transaction_id: 5000 + n as u32,
            issi,
            target_carrier: sim.stations[9].carrier,
            target_carrier_ext: 264,
            target_mcc: None,
            target_mnc: None,
            target_location_area: Some(10),
            sdu: Some(BrewHoSdu::from_bitbuffer(&exact)),
            call_gssi: None,
        };
        sim.relay.forward_registration(sim.stations[9].carrier, &req, &sim.wires.clone());
        sim.run_until("station 9 answers", |s| s.relay.ho_results.iter().any(|(_, r)| r.transaction_id == 5000 + n as u32));
        let (_, result) = sim.relay.ho_results.iter().find(|(_, r)| r.transaction_id == 5000 + n as u32).unwrap().clone();
        let answer = result.sdu.expect("MM answer").to_bitbuffer().to_bitstr();
        if allowed.contains(&issi) {
            assert_eq!(result.status, BREW_HO_STATUS_OK);
            assert_eq!(sim.relay.location[&issi], 9);
        } else {
            assert_eq!(result.status, BREW_HO_STATUS_REJECTED, "ISSI {issi}");
            assert!(answer.starts_with(&format!("{:04b}", MmPduTypeDl::DLocationUpdateReject.into_raw())));
            assert_eq!(sim.relay.location[&issi], from, "nothing moved");
        }
    }
}

/// Milestone 4 field test case A: the radio moves to a station where nobody listens to the running
/// call and does not restore it. It answers the station's D-LOCATION UPDATE COMMAND with a demand
/// location update (detach all and attach, its group), which brings the call up there; then it
/// sends its own roaming location update and the same group report again as U-ATTACH/DETACH
/// GROUP IDENTITY (table 16.49). The call must keep running on the station (before: MM deaffiliated
/// the group for a moment, CMCE sent D-RELEASE and set the call up again with a new identifier).
#[test]
fn sim_group_report_after_reselection_keeps_the_call_running() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let mut sim = Sim::new(&las, &EXTENSIONS);

    // Group g's members sit on stations g and g + 5, so the other eight stations have no listener.
    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let mut moved = 0;
    for (g, gssi) in GROUPS.into_iter().enumerate() {
        sim.clear_air();
        let &(talker, _, _) = radios.iter().find(|(_, _, grp)| *grp == gssi).unwrap();
        let st = sim.relay.location[&talker];
        let session = sim.start_call(st, talker, gssi);
        sim.steps(20);

        let mut targets = (0..STATIONS).filter(|s| s % 5 != g);
        for &(issi, _, grp) in &radios {
            if grp != gssi || issi == talker {
                continue;
            }
            let to = targets.next().expect("a free station per listener");
            assert_eq!(sim.local_call_id(to, gssi), None, "nobody on station {to} listened before");

            let mut lu = location_update_pdu(issi, LocationUpdateType::DemandLocationUpdating, &[gssi]);
            lu.group_identity_location_demand.as_mut().unwrap().group_identity_attach_detach_mode = 1;
            sim.submit(to, uplink(issi, MleProtocolDiscriminator::Mm, |b| lu.to_bitbuf(b).unwrap()));
            sim.run_until(&format!("station {to} sets the call on {gssi} up for ISSI {issi}"), |s| {
                s.local_call_id(to, gssi).is_some()
            });
            let call_id = sim.local_call_id(to, gssi);

            sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, &[]);
            // The radio ignored the D-SETUP sent before its own registration (field test
            // 2026-11-02, F1): its group attach must bring the late-entry D-SETUP again at once,
            // not only with the periodic one 5 multiframes later.
            let setups_before = group_setups(&sim.stations[to].air, gssi);
            sim.submit(to, u_attach_detach_all(issi, &[gssi]));
            sim.steps(4);
            assert!(
                group_setups(&sim.stations[to].air, gssi) > setups_before,
                "station {to} repeats the late-entry D-SETUP on {gssi} after ISSI {issi} attached again"
            );
            sim.steps(96);
            assert!(!group_release_sent(&sim.stations[to].air, gssi), "station {to} released the call on {gssi}");
            assert_eq!(sim.local_call_id(to, gssi), call_id, "station {to} kept the call identifier");
            assert!(sim.relay.calls.contains_key(&session), "the over on {gssi} still runs");
            moved += 1;
        }
        sim.end_call(st, talker, gssi, session);
    }
    assert!(moved >= 25, "only {moved} radios exercised");
}

/// Location update accept type of the last D-LOCATION UPDATE ACCEPT a station sent to `issi`
/// (3 bits after the PDU type, table 16.68).
fn accept_type(air: &[SapMsg], issi: u32) -> Option<u64> {
    let prefix = mm_pdu(MmPduTypeDl::DLocationUpdateAccept);
    air_to(air, issi)
        .iter()
        .rev()
        .find_map(|bits| bits.strip_prefix(&prefix).map(|rest| BitBuffer::from_bitstr(rest).peek_bits(3).unwrap()))
}

fn accepts(air: &[SapMsg], issi: u32) -> usize {
    let prefix = mm_pdu(MmPduTypeDl::DLocationUpdateAccept);
    air_to(air, issi).iter().filter(|bits| bits.starts_with(&prefix)).count()
}

/// `issi` registers on `station` with `lu_type`; returns the accept type of the answer.
fn register_accepted_as(sim: &mut Sim, station: usize, issi: u32, lu_type: LocationUpdateType, groups: &[u32]) -> Option<u64> {
    let before = accepts(&sim.stations[station].air, issi);
    sim.register(station, issi, lu_type, groups);
    sim.run_until(&format!("ISSI {issi} gets an accept on station {station}"), |s| {
        accepts(&s.stations[station].air, issi) > before
    });
    accept_type(&sim.stations[station].air, issi)
}

/// Transmission grant of the D-CALL RESTORE in the D-RESTORE-ACK a station sent to `issi`.
fn restore_grant(air: &[SapMsg], issi: u32) -> Option<u8> {
    air_to(air, issi).iter().find_map(|bits| {
        let rest = bits.strip_prefix(&format!("{PD_MLE}1000"))?;
        DCallRestore::from_bitbuf(&mut BitBuffer::from_bitstr(rest)).ok().map(|r| r.transmission_grant)
    })
}

/// Three stations (one LA each), five radios on one group. Every registration is accepted as the
/// type the radio asked for (clause 16.10.35a, table 16.68): roaming, demand (the answer to the
/// station's D-LOCATION UPDATE COMMAND) and periodic. Listeners that move mid-call restore with
/// "request that other MS may transmit" (table 14.74) and get "granted to another user" while
/// the talker talks, and "not granted" between overs (clause 14.5.2.2.4).
#[test]
fn sim_three_stations_five_radios_accept_types_and_restore_grants() {
    const GRANTED_TO_OTHER_USER: u8 = 3;
    const NOT_GRANTED: u8 = 1;
    let mut sim = Sim::new(&[1, 2, 3], &EXTENSIONS);
    let gssi = GROUPS[0];
    let radios: Vec<(u32, usize)> = (0..5u32).map(|r| (FIRST_ISSI + r, r as usize % 3)).collect();
    for &(issi, st) in &radios {
        let accepted = register_accepted_as(&mut sim, st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
        assert_eq!(accepted, Some(0b000), "roaming accept for {issi}");
    }

    // Periodic registration on the current station.
    let (periodic, st) = radios[1];
    let accepted = register_accepted_as(&mut sim, st, periodic, LocationUpdateType::PeriodicLocationUpdating, &[gssi]);
    assert_eq!(accepted, Some(0b010), "periodic accept");

    // Radio 0 talks on station 0; the others move one station on and restore as listeners.
    sim.clear_air();
    let (talker, talker_st) = radios[0];
    let session = sim.start_call(talker_st, talker, gssi);
    sim.steps(20);
    for &(issi, _) in &radios[1..] {
        let from = sim.relay.location[&issi];
        let to = (from + 1) % 3;
        let old_call_id = sim.local_call_id(from, gssi).unwrap_or(1);
        // Odd radios leave the group out, so the station commands a group report (demand).
        let reports_group = issi % 2 == 0;
        let reported = if reports_group { vec![gssi] } else { vec![] };
        let accepted = register_accepted_as(&mut sim, to, issi, LocationUpdateType::RoamingLocationUpdating, &reported);
        assert_eq!(accepted, Some(0b000), "roaming accept for {issi} on {to}");
        sim.submit(to, u_restore(issi, old_call_id, gssi, Some(from as u64 + 1)));
        sim.run_until(&format!("ISSI {issi} gets an answer on station {to}"), |s| {
            restore_ack(&s.stations[to].air, issi).is_some() || restore_fail(&s.stations[to].air, issi)
        });
        assert_eq!(restore_ack(&sim.stations[to].air, issi), sim.local_call_id(to, gssi), "{issi} restored on {to}");
        assert_eq!(restore_grant(&sim.stations[to].air, issi), Some(GRANTED_TO_OTHER_USER), "{issi} listens on {to}");
        if !reports_group {
            assert!(
                air_to(&sim.stations[to].air, issi).iter().any(|b| b.starts_with(&mm_pdu(MmPduTypeDl::DLocationUpdateCommand))),
                "station {to} asks {issi} for its groups"
            );
            let accepted = register_accepted_as(&mut sim, to, issi, LocationUpdateType::DemandLocationUpdating, &[gssi]);
            assert_eq!(accepted, Some(0b110), "demand accept for {issi} on {to}");
        }
    }

    // Between overs: the talker releases PTT, a listener moves to the talker's station within
    // hang time and restores without asking to transmit. The floor is free but not its to take.
    let call_id = sim.local_call_id(talker_st, gssi).unwrap();
    sim.submit(talker_st, u_tx_ceased(talker, call_id));
    sim.steps(2);
    let &(mover, _) = radios[1..].iter().find(|(issi, _)| sim.relay.location[issi] != talker_st).unwrap();
    let from = sim.relay.location[&mover];
    sim.stations[talker_st].air.clear();
    sim.register(talker_st, mover, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    sim.submit(talker_st, u_restore(mover, sim.local_call_id(from, gssi).unwrap_or(1), gssi, Some(from as u64 + 1)));
    sim.run_until(&format!("ISSI {mover} gets an answer between overs"), |s| {
        restore_ack(&s.stations[talker_st].air, mover).is_some() || restore_fail(&s.stations[talker_st].air, mover)
    });
    assert_eq!(restore_grant(&sim.stations[talker_st].air, mover), Some(NOT_GRANTED), "listener not granted between overs");

    sim.run_until("the over ended on the network", |s| !s.relay.calls.contains_key(&session));
    sim.steps(600);
    for &(issi, _) in &radios {
        assert!(sim.relay.location.contains_key(&issi), "ISSI {issi} still registered");
    }
}

// ─── Announced reselection between stations (milestone 5, serving side) ────────────────────────

fn neighbour(cell_identifier_ca: u8, carrier: u16, location_area: u16) -> CfgNeighborCellCa {
    CfgNeighborCellCa {
        cell_identifier_ca,
        // Forward registration supported (table 18.22).
        cell_reselection_types_supported: 2,
        neighbor_cell_synchronized: false,
        cell_load_ca: 0,
        main_carrier_number: carrier,
        main_carrier_number_extension: None,
        mcc: None,
        mnc: None,
        location_area: Some(location_area),
        maximum_ms_transmit_power: None,
        minimum_rx_access_level: None,
        subscriber_class: None,
        bs_service_details: None,
        timeshare_cell_information_or_security_parameters: None,
        tdma_frame_offset: None,
    }
}

/// Station i lists station i + 1 as neighbour 1 and station i - 1 as neighbour 2 (ring), plus
/// neighbour 3 on a carrier no station serves.
fn ring_neighbours(las: &[u16], i: usize, cfg: &mut StackConfig) {
    let n = las.len();
    let next = (i + 1) % n;
    let prev = (i + n - 1) % n;
    cfg.cell.neighbor_cells_ca = vec![
        neighbour(1, 1521 + 2 * next as u16, las[next]),
        neighbour(2, 1521 + 2 * prev as u16, las[prev]),
        neighbour(3, 1999, 99),
    ];
}

/// U-PREPARE naming neighbour `ci`, with a forward registration (roaming location update) when
/// `forward` (table 18.10; the MS supports forward registration and the LA changes, 18.3.4.7.5).
fn u_prepare(issi: u32, ci: u64, forward: bool) -> SapMsg {
    let sdu = forward.then(|| {
        let lu = location_update_pdu(issi, LocationUpdateType::RoamingLocationUpdating, &[]);
        let mut b = BitBuffer::new_autoexpand(64);
        lu.to_bitbuf(&mut b).unwrap();
        let len = b.get_pos();
        b.seek(0);
        let mut exact = BitBuffer::new(len);
        exact.copy_bits(&mut b, len);
        exact.seek(0);
        exact
    });
    let pdu = UPrepare {
        cell_identifier_ca: Some(ci),
        sdu,
    };
    uplink(issi, MleProtocolDiscriminator::Mle, |b| pdu.to_bitbuf(b).unwrap())
}

/// The serving station's answer to a U-PREPARE from `issi` (after the MLE discriminator):
/// D-NEW-CELL (type 000) or D-PREPARE-FAIL (type 001), if sent.
fn prepare_answer(air: &[SapMsg], issi: u32) -> Option<String> {
    air_to(air, issi).into_iter().find_map(|bits| {
        let rest = bits.strip_prefix(PD_MLE)?;
        (rest.starts_with("000") || rest.starts_with("001")).then(|| rest.to_string())
    })
}

const D_NEW_CELL_CHANGE_NOW: &str = "000010";
/// D-NEW-CELL "follow MAC channel change" (type 1, table 18.43) with an MM SDU.
const D_NEW_CELL_FOLLOW_MAC: &str = "000000";

/// The D-NEW-CELL `issi` got, as handed to layer 2 (with its channel allocation and STCH).
fn d_new_cells(air: &[SapMsg], issi: u32) -> Vec<&TlaTlDataReqBl> {
    air.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::TlaTlDataReqBl(r) if r.main_address.ssi == issi && r.tl_sdu.to_bitstr().starts_with(&format!("{PD_MLE}000")) => Some(r),
            _ => None,
        })
        .collect()
}

/// New call identifiers sent to `issi` in D-INFO.
fn d_info_new_call_ids(air: &[SapMsg], issi: u32) -> Vec<u16> {
    air_to(air, issi)
        .iter()
        .filter_map(|bits| {
            let rest = bits.strip_prefix(PD_CMCE)?;
            let mut b = BitBuffer::from_bitstr(rest);
            (b.peek_bits(5) == Some(CmcePduTypeDl::DInfo.into_raw())).then_some(())?;
            DInfo::from_bitbuf(&mut b).ok()?.new_call_identifier.map(|id| id as u16)
        })
        .collect()
}

/// Carrier and timeslot of the station's channel for its latest call to `gssi`.
fn group_setup_channel(air: &[SapMsg], gssi: u32) -> Option<(u16, u8)> {
    air.iter().rev().find_map(|m| match &m.msg {
        SapMsgInner::TlaTlDataReqBl(r) if r.main_address.ssi == gssi && r.tl_sdu.to_bitstr().starts_with(PD_CMCE) => {
            let ca = r.chan_alloc.as_ref()?;
            let ts = ca.timeslots.iter().position(|&t| t)? as u8 + 1;
            Some((ca.carrier?, ts))
        }
        SapMsgInner::TlaTlUnitdataReqBl(r) if r.main_address.ssi == gssi && r.tl_sdu.to_bitstr().starts_with(PD_CMCE) => {
            let ca = r.chan_alloc.as_ref()?;
            let ts = ca.timeslots.iter().position(|&t| t)? as u8 + 1;
            Some((ca.carrier?, ts))
        }
        _ => None,
    })
}
/// D-LOCATION UPDATE REJECT, roaming, forward registration failure (table 16.81), O-bit 0.
const FRF_REJECT: &str = "01110000011100";

/// Ten stations in their own location areas, thirty radios. During a group call every listener
/// announces a reselection to the next station (U-PREPARE with forward registration): the serving
/// station asks the target through the network, holds D-NEW-CELL until the target's MM accepted
/// and the target runs the call, then grants type 1 (clause 18.3.4.7.5): the target's call
/// identifier in D-INFO if it differs, then D-NEW-CELL "follow MAC channel change" with the
/// accept and a channel allocation for the target's traffic channel ("cell change flag", clause
/// 23.5.4.1), stolen on the serving call's traffic channel. The radio goes straight to the new
/// channel without U-RESTORE (figure 18.14). Every fourth grant cannot go out on the traffic
/// channel: the serving station grants type 2 instead and the radio restores the call. The ISSI is
/// on the target before the radio leaves, and the station it left drops it without deregistering
/// it on the network.
#[test]
fn sim_announced_reselection_prepares_the_other_station() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_RESELECTION];
    let mut sim = Sim::with_config(&las, &extensions, &|i, cfg| ring_neighbours(&las, i, cfg));

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let accept = format!("{:04b}", MmPduTypeDl::DLocationUpdateAccept.into_raw());
    let mut moved = Vec::new();
    let (mut type1, mut type2, mut d_infos) = (0, 0, 0);
    for gssi in GROUPS {
        sim.clear_air();
        let &(talker, _, _) = radios.iter().find(|(_, _, g)| *g == gssi).unwrap();
        let st = sim.relay.location[&talker];
        let session = sim.start_call(st, talker, gssi);
        sim.steps(20);

        for &(issi, _, g) in &radios {
            if g != gssi || issi == talker {
                continue;
            }
            let from = sim.relay.location[&issi];
            let to = (from + 1) % STATIONS;
            let old_call_id = sim.local_call_id(from, gssi).unwrap_or(1);
            let serving_channel = group_setup_channel(&sim.stations[from].air, gssi);
            sim.submit(from, u_prepare(issi, 1, true));
            sim.run_until(&format!("station {from} answers ISSI {issi}'s U-PREPARE"), |s| {
                prepare_answer(&s.stations[from].air, issi).is_some()
            });
            let answer = prepare_answer(&sim.stations[from].air, issi).unwrap();
            assert!(
                answer.starts_with(&format!("{D_NEW_CELL_FOLLOW_MAC}{accept}")),
                "ISSI {issi}: type 1 D-NEW-CELL with the target's accept, got {answer}"
            );
            assert_eq!(sim.relay.location.get(&issi), Some(&to), "registered on the target before leaving");
            let new_call_id = sim.local_call_id(to, gssi).expect("the target runs the call");
            let (carrier, ts) = group_setup_channel(&sim.stations[to].air, gssi).expect("the target's channel");

            let grant = d_new_cells(&sim.stations[from].air, issi)[0].clone();
            let alloc = grant.chan_alloc.as_ref().expect("channel allocation with the type 1 D-NEW-CELL");
            assert!(alloc.cell_change, "allocation for another cell");
            assert_eq!(alloc.carrier, Some(carrier), "the target's carrier");
            assert_eq!(alloc.carrier, Some(sim.stations[to].carrier));
            assert_eq!(alloc.timeslots.iter().position(|&t| t), Some(ts as usize - 1), "the target's timeslot");
            assert!(grant.stealing_permission);
            assert_eq!(grant.stealing_slot, serving_channel, "stolen on the call's traffic channel here");

            let infos = d_info_new_call_ids(&sim.stations[from].air, issi);
            if new_call_id != old_call_id {
                assert!(infos.len() >= 2 && infos.iter().all(|&id| id == new_call_id), "ISSI {issi}: D-INFO {infos:?}, want {new_call_id}");
                d_infos += 1;
            } else {
                assert!(infos.is_empty(), "no D-INFO when the call identifier stays");
            }

            let reporter = grant.tx_reporter.clone().expect("type 1 grant is tracked");
            if moved.len() % 4 == 3 {
                // Layer 2 could not steal the slot: type 2 with the same accept.
                reporter.mark_discarded();
                sim.run_until(&format!("station {from} falls back to type 2 for ISSI {issi}"), |s| d_new_cells(&s.stations[from].air, issi).len() == 2);
                let fallback = d_new_cells(&sim.stations[from].air, issi)[1].tl_sdu.to_bitstr();
                assert!(
                    fallback.strip_prefix(PD_MLE).unwrap().starts_with(&format!("{D_NEW_CELL_CHANGE_NOW}{accept}")),
                    "type 2 with the accept, got {fallback}"
                );
                // The radio arrives and restores its call (old LA given, table 18.13 note 1).
                sim.submit(to, u_restore(issi, new_call_id, gssi, Some(las[from] as u64)));
                sim.run_until(&format!("ISSI {issi} gets an answer to its U-RESTORE on station {to}"), |s| {
                    restore_ack(&s.stations[to].air, issi).is_some() || restore_fail(&s.stations[to].air, issi)
                });
                let ack = restore_ack(&sim.stations[to].air, issi).unwrap_or_else(|| panic!("ISSI {issi} was refused on station {to}"));
                assert_eq!(ack, new_call_id, "restored into station {to}'s call");
                type2 += 1;
            } else {
                reporter.mark_transmitted();
                sim.steps(5);
                assert_eq!(d_new_cells(&sim.stations[from].air, issi).len(), 1, "no second D-NEW-CELL after type 1 went out");
                type1 += 1;
            }
            moved.push((issi, from, to));
        }
        sim.end_call(st, talker, gssi, session);
    }
    assert!(moved.len() >= 25, "only {} announced reselections", moved.len());
    assert!(type1 >= 18 && type2 >= 6, "type 1 {type1}, type 2 {type2}");
    assert!(d_infos > 0, "no reselection changed the call identifier");
    // The network hears which type each radio got: the one that went out on the air.
    sim.steps(5);
    let reported = |t: &str| sim.relay.reselections.iter().filter(|(_, _, k)| k == t).count();
    assert_eq!((reported("announced_type_1"), reported("announced_type_2")), (type1, type2));
    assert_eq!(sim.relay.reselections.len(), moved.len(), "one report per radio");
    for (issi, from, _) in &moved {
        assert!(sim.relay.reselections.iter().any(|(i, st, _)| i == issi && st == from), "station {from} reported ISSI {issi}");
    }

    // The stations they left drop them (after the departure delay) without deregistering them.
    sim.run_until("the serving stations dropped the radios that left", |s| {
        moved
            .iter()
            .all(|(issi, from, _)| !s.stations[*from].test.get_shared_config().state_read().subscribers.is_registered(*issi))
    });
    sim.steps(50);
    for (issi, from, to) in &moved {
        assert!(!sim.relay.deregistrations.contains(&(*issi, *from)), "station {from} deregistered ISSI {issi} on the network");
        assert_eq!(sim.relay.location.get(issi), Some(to));
    }
}

/// What the serving station answers when the target does not take the radio, per clause
/// 18.3.4.7.6: target MM rejects (station 9's access list) → D-PREPARE-FAIL "MS not allowed on
/// cell" with the reject; target registered the radio but went away (station 7) → D-PREPARE-FAIL
/// "temporary break in service" and the serving station claims the ISSI back; no station serves
/// the carrier → D-NEW-CELL type 2 with "forward registration failure" (figure 18.12), the radio
/// registers on arrival; without forward registration → plain D-NEW-CELL.
#[test]
fn sim_announced_reselection_that_the_target_does_not_take() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let allowed: Vec<u32> = (0..RADIOS / 2).map(|r| FIRST_ISSI + r).collect();
    let mut sim = Sim::with_config(&las, &EXTENSIONS, &|i, cfg| {
        ring_neighbours(&las, i, cfg);
        if i == 9 {
            cfg.security.whitelist_mode = WhitelistMode::Enforce;
            cfg.security.issi_whitelist = allowed.clone();
        }
    });
    sim.relay.gone_carriers.insert(sim.stations[7].carrier);
    // Everyone on station 8, whose neighbour 1 is station 9 and neighbour 2 station 7.
    let radios: Vec<u32> = (0..RADIOS).map(|r| FIRST_ISSI + r).collect();
    for &issi in &radios {
        sim.register(8, issi, LocationUpdateType::RoamingLocationUpdating, &[GROUPS[0]]);
    }

    let mut checked = 0;
    for (n, &issi) in radios.iter().enumerate() {
        sim.clear_air();
        let (ci, forward) = match n % 4 {
            0 => (1, true),
            1 => (3, true),
            2 => (3, false),
            _ => (2, true),
        };
        sim.submit(8, u_prepare(issi, ci, forward));
        sim.run_until(&format!("station 8 answers ISSI {issi}"), |s| prepare_answer(&s.stations[8].air, issi).is_some());
        let answer = prepare_answer(&sim.stations[8].air, issi).unwrap();
        match (ci, forward) {
            (1, _) if allowed.contains(&issi) => {
                assert!(answer.starts_with(D_NEW_CELL_CHANGE_NOW), "ISSI {issi}: {answer}");
                assert_eq!(sim.relay.location[&issi], 9);
            }
            (1, _) => {
                let reject = format!("{:04b}", MmPduTypeDl::DLocationUpdateReject.into_raw());
                assert!(answer.starts_with(&format!("001100{reject}")), "ISSI {issi}: fail cause 2 with the reject, got {answer}");
                sim.run_until("the ISSI is claimed back", |s| s.relay.location[&issi] == 8);
            }
            (2, _) => {
                assert_eq!(answer, format!("001000{FRF_REJECT}"), "ISSI {issi}: fail cause 0 with forward registration failure");
                sim.run_until("the ISSI is claimed back from the station that went away", |s| s.relay.location[&issi] == 8);
                assert!(sim.relay.moves.contains(&(issi, Some(7), 8)), "it had moved to station 7 first");
            }
            (_, true) => {
                assert_eq!(answer, format!("{D_NEW_CELL_CHANGE_NOW}{FRF_REJECT}"), "ISSI {issi}");
                assert_eq!(sim.relay.location[&issi], 8, "nothing moved");
            }
            (_, false) => assert_eq!(answer, D_NEW_CELL_CHANGE_NOW, "ISSI {issi}"),
        }
        checked += 1;
    }
    assert!(checked >= 25);
}

/// Last D-CONNECT sent to `issi`: its transmission grant.
fn d_connect_grant(air: &[SapMsg], issi: u32) -> Option<TransmissionGrant> {
    air_to(air, issi).iter().rev().find_map(|bits| {
        let mut b = BitBuffer::from_bitstr(bits.strip_prefix(PD_CMCE)?);
        (b.peek_bits(5) == Some(CmcePduTypeDl::DConnect.into_raw())).then_some(())?;
        DConnect::from_bitbuf(&mut b).ok().map(|c| c.transmission_grant)
    })
}

/// Field case C: a talker changes station unannounced in the middle of its over and presses PTT
/// again on the new station (U-SETUP, as the MTP6650 does). With `ms-moved` the network ends the
/// over on the station it left and tells that station, which drops the radio without
/// deregistering it; the new station gives the talker the floor (TS 100 392-2 clause
/// 14.5.2.2.1 a)) whether the network's end of the old over reaches it before or after the
/// U-SETUP, and the new over goes out on the network. Every one of the 30 radios talks once:
/// half move to a station with listeners of the group, half to one without.
#[test]
fn sim_talker_that_moves_unannounced_keeps_talking() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED];
    let mut sim = Sim::new(&las, &extensions);
    assert!(
        sim.relay
            .cells
            .values()
            .all(|(_, caps)| caps == &vec![BREW_EXT_HANDOVER.to_string(), BREW_EXT_MS_MOVED.to_string()]),
        "stations ask for ms-moved where the server offers it"
    );

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let mut moved = Vec::new();
    for &(talker, _, gssi) in &radios {
        sim.clear_air();
        let from = sim.relay.location[&talker];
        let old_session = sim.start_call(from, talker, gssi);
        sim.steps(20);

        let to = (from + if talker % 2 == 0 { 5 } else { 3 }) % STATIONS;
        sim.register(to, talker, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
        sim.submit(to, u_setup(talker, gssi));
        sim.run_until(&format!("ISSI {talker} talks from station {to}"), |s| {
            s.relay.calls.values().any(|c| c.gssi == gssi && c.owner == to && c.talker == talker)
        });
        assert_eq!(
            d_connect_grant(&sim.stations[to].air, talker),
            Some(TransmissionGrant::Granted),
            "ISSI {talker} keeps the floor on station {to}"
        );
        assert!(!sim.relay.calls.contains_key(&old_session), "the over on station {from} ended on the network");
        assert!(sim.relay.ms_moved.contains(&(talker, from)), "station {from} was told");
        assert!(sim.relay.floor_refused.is_empty(), "no over was refused: {:?}", sim.relay.floor_refused);

        // The new over holds: the old over's late end (GROUP_IDLE from the station the radio
        // left, after it ends its own leg) must not take the floor away here.
        sim.steps(100);
        let session = *sim
            .relay
            .calls
            .iter()
            .find(|(_, c)| c.gssi == gssi && c.owner == to && c.talker == talker)
            .expect("the new over still runs")
            .0;
        sim.end_call(to, talker, gssi, session);
        moved.push((talker, from, to));
    }
    assert!(moved.len() >= 25);

    sim.run_until("the stations dropped the radios that left", |s| {
        moved.iter().all(|(issi, from, _)| {
            s.relay.location.get(issi) == Some(from)
                || !s.stations[*from].test.get_shared_config().state_read().subscribers.is_registered(*issi)
        })
    });
    for (issi, from, _) in &moved {
        assert!(!sim.relay.deregistrations.contains(&(*issi, *from)), "station {from} deregistered ISSI {issi} on the network");
    }
}

/// Announced type 3 (clause 18.3.4.7.4): a talker without a scanned or synchronized neighbour
/// announces the change with a U-PREPARE that names no cell (the MTP6650 does this while
/// transmitting when the cells are not synchronized). The serving station answers D-NEW-CELL
/// "change channel immediately", never D-PREPARE-FAIL, prepares nothing, and the radio then
/// goes on as after an unannounced change: it keeps the floor on the new station and the old
/// one is told.
#[test]
fn sim_type_3_announcement_by_a_talker_gets_d_new_cell_and_keeps_the_floor() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED];
    let mut sim = Sim::with_config(&las, &extensions, &|i, cfg| ring_neighbours(&las, i, cfg));

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let mut moved = 0;
    for &(talker, _, gssi) in &radios {
        sim.clear_air();
        let from = sim.relay.location[&talker];
        let old_session = sim.start_call(from, talker, gssi);
        sim.steps(20);

        let prepare = UPrepare {
            cell_identifier_ca: None,
            sdu: None,
        };
        sim.submit(from, uplink(talker, MleProtocolDiscriminator::Mle, |b| prepare.to_bitbuf(b).unwrap()));
        sim.run_until(&format!("station {from} answers ISSI {talker}'s type 3 U-PREPARE"), |s| {
            prepare_answer(&s.stations[from].air, talker).is_some()
        });
        assert_eq!(
            prepare_answer(&sim.stations[from].air, talker).as_deref(),
            Some(D_NEW_CELL_CHANGE_NOW),
            "ISSI {talker}: D-NEW-CELL change channel immediately"
        );
        assert!(!sim.relay.prepared.contains(&talker), "nothing is prepared for type 3");

        let to = (from + 1 + talker as usize % 3) % STATIONS;
        sim.register(to, talker, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
        sim.submit(to, u_setup(talker, gssi));
        sim.run_until(&format!("ISSI {talker} talks from station {to}"), |s| {
            s.relay.calls.values().any(|c| c.gssi == gssi && c.owner == to && c.talker == talker)
        });
        assert_eq!(d_connect_grant(&sim.stations[to].air, talker), Some(TransmissionGrant::Granted));
        assert!(!sim.relay.calls.contains_key(&old_session), "the over on station {from} ended");
        assert!(sim.relay.ms_moved.contains(&(talker, from)), "station {from} was told");
        let session = *sim
            .relay
            .calls
            .iter()
            .find(|(_, c)| c.gssi == gssi && c.owner == to && c.talker == talker)
            .expect("the new over runs")
            .0;
        sim.end_call(to, talker, gssi, session);
        moved += 1;
    }
    assert!(moved >= 25);
}

/// The Brew side of case C on the new station: once CMCE gives the moved talker the floor of the
/// over it was heard on, that network session is over here. The old station's GROUP_IDLE that
/// follows must not start a pending tail on the slot the talker now sends on (its playout and
/// NetworkCallEnd would hit the new over).
#[test]
fn moved_talker_floor_ends_its_network_session_on_the_new_station() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(relay_brew());
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![], vec![TetraEntity::Cmce, TetraEntity::Umac]);
    let wire = Arc::new(Mutex::new(Wire::default()));
    let transport = ChannelTransport {
        wire: wire.clone(),
        extensions: EXTENSIONS.iter().map(|e| e.to_string()).collect(),
        connected: false,
    };
    test.register_entity(BrewEntity::new(test.get_shared_config(), transport));
    let (talker, gssi, other) = (3_100_001, 2001, 3_100_002);
    let old = Uuid::from_u128(0xC0);
    let to_cmce = |test: &mut ComponentTest, cc: CallControl| {
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(cc),
        })
    };
    let run = |test: &mut ComponentTest, wire: &Arc<Mutex<Wire>>, ms: u64| {
        let mut out = Vec::new();
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            test.run_stack(Some(1));
            out.extend(test.dump_sinks());
            wire.lock().unwrap().to_server.clear();
            std::thread::sleep(Duration::from_millis(2));
        }
        out
    };
    let ends = |msgs: &[SapMsg], uuid: Uuid| {
        msgs.iter()
            .any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }) if *brew_uuid == uuid))
    };
    run(&mut test, &wire, 50);

    for (round, (speaker, uuid)) in [(other, Uuid::from_u128(0xC1)), (talker, old)].into_iter().enumerate() {
        let group_tx = build_group_tx(&uuid, speaker, gssi, 0, 0, None);
        wire.lock().unwrap().to_station.push_back(group_tx);
        let msgs = run(&mut test, &wire, 100);
        assert!(msgs.iter().any(
            |m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::NetworkCallStart { brew_uuid, .. }) if *brew_uuid == uuid)
        ));
        let (call_id, ts) = (5 + round as u16, 2 + round as u8);
        to_cmce(
            &mut test,
            CallControl::NetworkCallReady {
                brew_uuid: uuid,
                call_id,
                carrier_num: 1521,
                ts,
                usage: 4,
            },
        );
        run(&mut test, &wire, 50);
        // CMCE hands the floor of that over to the talker that came here.
        to_cmce(
            &mut test,
            CallControl::FloorGranted {
                call_id,
                source_issi: talker,
                dest_gssi: gssi,
                carrier_num: 1521,
                ts,
            },
        );
        run(&mut test, &wire, 50);
        wire.lock().unwrap().to_station.push_back(build_group_idle(&uuid, 0));
        let msgs = run(&mut test, &wire, 1500);
        if speaker == talker {
            assert!(!ends(&msgs, uuid), "the moved talker's old session left a tail on its new over");
        } else {
            // Somebody else's over is not touched (CMCE would not hand its floor over anyway).
            assert!(ends(&msgs, uuid), "another talker's over ends as before");
        }
    }
}

/// Field cases A and B: listeners that leave unannounced. With `ms-moved` the station they left
/// stops serving them, and once no radio of the group is left there it releases its leg of the
/// network call instead of sending it to nobody until the call ends. Stations i and i + 5 hold
/// the three radios each of group i; the talker is on station i, the three on station i + 5 move
/// to three other stations and restore there.
#[test]
fn sim_station_left_by_all_listeners_releases_the_call() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let mut sim = Sim::new(&las, &["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED]);
    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    for (i, gssi) in GROUPS.into_iter().enumerate() {
        sim.clear_air();
        let (talker, left) = (FIRST_ISSI + i as u32, i + 5);
        let session = sim.start_call(i, talker, gssi);
        let started = sim.ticks;
        sim.run_until(&format!("station {left} runs the call on {gssi}"), |s| s.local_call_id(left, gssi).is_some());
        let leavers: Vec<u32> = radios.iter().filter(|(_, st, _)| *st == left).map(|(issi, _, _)| *issi).collect();
        assert_eq!(leavers.len(), 3);
        for (n, &issi) in leavers.iter().enumerate() {
            let to = (left + 1 + n) % STATIONS;
            let old_call_id = sim.local_call_id(left, gssi).unwrap();
            assert!(!group_release_sent(&sim.stations[left].air, gssi), "station {left} released while radios of {gssi} are there");
            sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
            sim.submit(to, u_restore(issi, old_call_id, gssi, Some(las[left] as u64)));
            sim.run_until(&format!("ISSI {issi} is restored on station {to}"), |s| restore_ack(&s.stations[to].air, issi).is_some());
        }
        sim.run_until(&format!("station {left} released its leg of the call on {gssi}"), |s| {
            group_release_sent(&s.stations[left].air, gssi)
        });
        // The sim sends no voice, so CMCE's network media watchdog would release the leg after
        // 720 ticks anyway; the release must come well before that.
        assert!(sim.ticks - started < 500, "station {left} released only after {} ticks", sim.ticks - started);
        assert!(sim.relay.calls.contains_key(&session), "the call itself runs on");
        for &issi in &leavers {
            assert!(sim.relay.ms_moved.contains(&(issi, left)));
            assert!(!sim.relay.deregistrations.contains(&(issi, left)), "station {left} deregistered ISSI {issi}");
        }
        sim.end_call(i, talker, gssi, session);
    }
}

/// Ten stations, thirty radios, each leaving its station once, in turn: idle (undeclared), while
/// listening to a group call (unannounced), with a type 3 U-PREPARE (no cell), and with a U-PREPARE
/// naming the neighbour (type 2, prepared through the network). The station each radio left tells
/// the network once which category of cell reselection that was (clause 18.3.4.7.0): from
/// D-NEW-CELL for the announced ones, from its own calls when told the radio moved unprepared.
#[test]
fn sim_the_station_left_reports_how_the_radio_went() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_MS_RESELECTION];
    let mut sim = Sim::with_config(&las, &extensions, &|i, cfg| ring_neighbours(&las, i, cfg));

    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let mut expected = Vec::new();
    for (n, &(issi, _, gssi)) in radios.iter().enumerate() {
        sim.clear_air();
        let from = sim.relay.location[&issi];
        let to = (from + 1) % STATIONS;
        let kind = match n % 4 {
            0 => {
                sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
                "undeclared"
            }
            1 => {
                // Another member of the group talks from elsewhere; the call reaches `from`.
                let &(talker, _, _) = radios.iter().find(|(t, _, g)| *g == gssi && sim.relay.location[t] != from).unwrap();
                let st = sim.relay.location[&talker];
                let session = sim.start_call(st, talker, gssi);
                sim.run_until(&format!("station {from} runs the call on {gssi}"), |s| s.local_call_id(from, gssi).is_some());
                sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
                sim.run_until(&format!("station {from} reports ISSI {issi}"), |s| s.relay.reselections.iter().any(|(i, _, _)| *i == issi));
                sim.end_call(st, talker, gssi, session);
                "unannounced"
            }
            2 => {
                let prepare = UPrepare {
                    cell_identifier_ca: None,
                    sdu: None,
                };
                sim.submit(from, uplink(issi, MleProtocolDiscriminator::Mle, |b| prepare.to_bitbuf(b).unwrap()));
                sim.run_until(&format!("station {from} answers ISSI {issi}'s type 3 U-PREPARE"), |s| {
                    prepare_answer(&s.stations[from].air, issi).is_some()
                });
                sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
                assert!(sim.relay.ms_moved.contains(&(issi, from)), "unprepared for the network");
                "announced_type_3"
            }
            _ => {
                sim.submit(from, u_prepare(issi, 1, false));
                sim.run_until(&format!("station {from} answers ISSI {issi}'s U-PREPARE"), |s| {
                    prepare_answer(&s.stations[from].air, issi).is_some()
                });
                assert_eq!(prepare_answer(&sim.stations[from].air, issi).as_deref(), Some(D_NEW_CELL_CHANGE_NOW));
                sim.register(to, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
                "announced_type_2"
            }
        };
        sim.run_until(&format!("station {from} reports how ISSI {issi} left"), |s| {
            s.relay.reselections.iter().any(|(i, st, _)| *i == issi && *st == from)
        });
        sim.steps(20);
        expected.push((issi, from, kind.to_string()));
    }
    assert!(expected.len() >= 25);
    assert_eq!(sim.relay.reselections, expected, "exactly one report per move, from the station left");
}

/// Milestone 5a, the field baseline of 2026-10-30: a radio in an individual call with a network
/// party moves to another station and sends U-CALL RESTORE there (clause 14.5.1.2.4). Ten
/// stations, thirty radios, one call each, half duplex and half simplex. The network moves the
/// call with the radio's registration (brew-server `call_restore`), the station left lets its leg
/// go without ending the call on the network, and the new station, which never saw the call,
/// asks the network for it, puts it on a channel of its own (new call identifier, channel
/// allocation) and carries the voice both ways. Every sixth radio does not restore: the network
/// keeps waiting for it (its window, not modelled here, ends the call for the other party).
#[test]
fn sim_individual_calls_follow_their_radios_to_another_station() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_CALL_RESTORE];
    let mut sim = Sim::new(&las, &extensions);
    assert!(
        sim.relay.cells.values().all(|(_, caps)| caps.iter().any(|c| c == BREW_EXT_CALL_RESTORE)),
        "stations announce call-restore where the server offers it"
    );
    let radios: Vec<(u32, usize, u32)> = (0..RADIOS).map(|r| (FIRST_ISSI + r, r as usize % STATIONS, GROUPS[r as usize % 5])).collect();
    for &(issi, st, gssi) in &radios {
        sim.register(st, issi, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
    }

    let (mut restored, mut not_restored, mut new_ids) = (0, 0, HashSet::new());
    for (n, &(radio, _, gssi)) in radios.iter().enumerate() {
        sim.clear_air();
        let from = sim.relay.location[&radio];
        let to = (from + 3) % STATIONS;
        let app = 2_200_001 + n as u32;
        let duplex = n % 2 == 0;
        let id = Uuid::from_u128(0x5A00 + n as u128);
        sim.relay.call_radio(id, app, radio, from, duplex, &sim.wires);
        sim.run_until(&format!("station {from} pages ISSI {radio}"), |s| individual_setup_call_id(&s.stations[from].air, radio).is_some());
        let old_id = individual_setup_call_id(&sim.stations[from].air, radio).unwrap();
        sim.submit(from, u_connect(radio, old_id, duplex));
        sim.run_until(&format!("ISSI {radio}'s call is connected on station {from}"), |s| {
            cmce_pdu_to(&s.stations[from].air, radio, CmcePduTypeDl::DConnectAcknowledge)
        });
        sim.steps(10);

        // A radio that keeps its call registers with service restoration roaming, one that gave
        // it up (every sixth) with plain roaming (TS 100 392-2 clause 16.4 a)).
        let gives_up = n % 6 == 5;
        let lu_type = if gives_up {
            LocationUpdateType::RoamingLocationUpdating
        } else {
            LocationUpdateType::ServiceRestorationRoamingLocationUpdating
        };
        let reports_before = sim.relay.not_restored.len();
        sim.register(to, radio, lu_type, &[gssi]);
        sim.run_until(&format!("station {from} lets ISSI {radio}'s call go"), |s| {
            cmce_pdu_to(&s.stations[from].air, radio, CmcePduTypeDl::DRelease)
        });
        if gives_up {
            sim.steps(100);
            assert!(sim.relay.not_restored.contains(&(to, radio)), "station {to} reports ISSI {radio} has no call");
            assert!(!sim.relay.restoring.contains(&id), "the network ended ISSI {radio}'s call at once");
            assert!(call_restore_to(&sim.stations[to].air, radio).is_none());
            assert!(sim.relay.restore_requests.iter().all(|(_, issi, _)| *issi != radio));
            not_restored += 1;
            continue;
        }
        assert_eq!(sim.relay.private[&id].owner, to, "the call moved with the registration");
        assert!(sim.relay.not_restored[reports_before..].iter().all(|(_, issi)| *issi != radio), "a radio with a call is not reported");

        // Simplex: the radio asks for the floor in every other restore; the floor is free.
        let request = duplex || n % 4 == 1;
        sim.submit(to, u_restore_individual(radio, old_id, app, request, las[from] as u64));
        sim.run_until(&format!("ISSI {radio} is restored on station {to}"), |s| call_restore_to(&s.stations[to].air, radio).is_some());
        let (pdu, ts) = call_restore_to(&sim.stations[to].air, radio).unwrap();
        assert_eq!(pdu.call_identifier, old_id, "D-CALL RESTORE names the call the radio restored");
        // The station's own identifier; "new call identifier" only when it differs (14.5.1.2.4).
        let new_id = pdu.new_call_identifier.map_or(pdu.call_identifier, |id| id as u16);
        new_ids.insert(new_id != old_id);
        let expected = if request { TransmissionGrant::Granted } else { TransmissionGrant::NotGranted };
        assert_eq!(pdu.transmission_grant, expected.into_raw() as u8, "grant for ISSI {radio} (duplex {duplex})");
        assert_eq!(sim.relay.restore_requests.last(), Some(&(to, radio, app)));
        assert!(!sim.relay.restoring.contains(&id));

        // Voice both ways on the new station's channel.
        let carrier = sim.stations[to].carrier;
        for _ in 0..8 {
            sim.wires[to].lock().unwrap().to_station.push_back(build_voice_frame(&id, 274, &[0u8; 36]));
        }
        sim.run_until(&format!("station {to} plays the network party's voice to ISSI {radio}"), |s| {
            s.stations[to].voice_down.contains(&(carrier, ts))
        });
        sim.submit(
            to,
            SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Umac,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
                    carrier_num: carrier,
                    ts,
                    data: vec![0u8; 35],
                }),
            },
        );
        sim.run_until(&format!("ISSI {radio}'s voice reaches the network from station {to}"), |s| {
            s.relay.voice_up.contains(&(to, id))
        });
        assert!(
            !sim.relay.call_releases.iter().any(|(st, c, _)| *st == from && *c == id),
            "station {from} ended the moved call on the network"
        );
        assert!(!sim.relay.call_releases.iter().any(|(_, c, _)| *c == id), "nobody ended the call");

        // The network party hangs up; the new station releases the radio on the new call id.
        sim.wires[to].lock().unwrap().to_station.push_back(build_call_release(&id, 1));
        sim.relay.private.remove(&id);
        sim.run_until(&format!("station {to} releases ISSI {radio}"), |s| {
            cmce_pdu_to(&s.stations[to].air, radio, CmcePduTypeDl::DRelease)
        });
        sim.steps(20);
        restored += 1;
    }
    assert!(restored >= 25, "{restored} restored");
    assert_eq!(not_restored, 5);
    assert!(new_ids.contains(&true), "some restores ran on a call identifier of the new station's own");
}

/// Uplink voice of the radio on `station`'s traffic channel `ts`, as UMAC hands it to Brew.
fn uplink_voice(carrier_num: u16, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Brew,
        msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
            carrier_num,
            ts,
            data: vec![0u8; 35],
        }),
    }
}

/// Calls between two radios across station changes (extension `call-handoff`). Ten stations, 26
/// radios in 13 pairs. Each pair starts a local call on its home station; the caller moves to
/// another station and restores the call there (the home station hands the call to the network,
/// TS 100 392-2 clause 14.5.1.2.4); voice runs between the stations; then the caller comes back to
/// its peer's station and restores again, and the home station joins both legs into a local call.
/// Duplex and simplex alternate.
#[test]
fn sim_radio_to_radio_calls_survive_station_changes_both_ways() {
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_CALL_RESTORE, BREW_EXT_CALL_HANDOFF];
    let mut sim = Sim::new(&las, &extensions);
    assert!(
        sim.relay.cells.values().all(|(_, caps)| caps.iter().any(|c| c == BREW_EXT_CALL_HANDOFF)),
        "stations announce call-handoff where the server offers it"
    );
    const PAIRS: u32 = 13;
    let pairs: Vec<(u32, u32, usize)> = (0..PAIRS).map(|k| (FIRST_ISSI + 2 * k, FIRST_ISSI + 2 * k + 1, k as usize % STATIONS)).collect();
    for &(a, b, home) in &pairs {
        sim.register(home, a, LocationUpdateType::RoamingLocationUpdating, &[]);
        sim.register(home, b, LocationUpdateType::RoamingLocationUpdating, &[]);
    }

    let (mut handed, mut joined) = (0, 0);
    for (k, &(a, b, home)) in pairs.iter().enumerate() {
        sim.clear_air();
        let duplex = k % 2 == 0;
        let away = (home + 3) % STATIONS;

        // A calls B on their home station: a local call, the network knows nothing of it.
        sim.submit(home, u_setup_individual(a, b, duplex));
        sim.run_until(&format!("station {home} pages ISSI {b}"), |s| individual_setup_call_id(&s.stations[home].air, b).is_some());
        let home_id = individual_setup_call_id(&sim.stations[home].air, b).unwrap();
        sim.submit(home, u_connect(b, home_id, duplex));
        sim.run_until(&format!("ISSI {a} hears the call is connected"), |s| {
            cmce_pdu_to(&s.stations[home].air, a, CmcePduTypeDl::DConnect)
        });
        sim.steps(10);
        assert!(sim.relay.private.is_empty() && sim.relay.bridges.is_empty(), "a local call stays off the network");

        // Direction 1: A goes to another station and restores the call there.
        sim.register(away, a, LocationUpdateType::ServiceRestorationRoamingLocationUpdating, &[]);
        sim.submit(away, u_restore_individual(a, home_id, b, duplex, las[home] as u64));
        sim.run_until(&format!("ISSI {a} is restored on station {away}"), |s| call_restore_to(&s.stations[away].air, a).is_some());
        let (pdu, a_ts) = call_restore_to(&sim.stations[away].air, a).unwrap();
        assert_eq!(pdu.call_identifier, home_id, "D-CALL RESTORE names the call the radio restored");
        if duplex {
            assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted.into_raw() as u8, "duplex: always granted");
        }
        assert_eq!(sim.relay.handoff_answers.last().map(|(st, issi, id)| (*st, *issi, id.is_some())), Some((home, a, true)));
        let id = *sim.relay.bridges.keys().next().expect("the call runs between the two stations now");
        assert!(!cmce_pdu_to(&sim.stations[home].air, b, CmcePduTypeDl::DRelease), "B keeps its call");
        handed += 1;

        // Voice from A on the away station reaches B on the home station.
        let away_carrier = sim.stations[away].carrier;
        sim.stations[home].voice_down.clear();
        for _ in 0..8 {
            sim.submit(away, uplink_voice(away_carrier, a_ts));
        }
        sim.run_until(&format!("station {home} plays ISSI {a}'s voice to ISSI {b}"), |s| !s.stations[home].voice_down.is_empty());
        assert!(sim.relay.voice_up.contains(&(away, id)));

        // Direction 2: A comes back to B's station and restores the call there; the home station
        // joins both legs, the network call ends.
        sim.clear_air();
        let away_id = pdu.new_call_identifier.map_or(pdu.call_identifier, |n| n as u16);
        sim.register(home, a, LocationUpdateType::ServiceRestorationRoamingLocationUpdating, &[]);
        sim.run_until(&format!("station {away} lets ISSI {a}'s leg go"), |s| {
            cmce_pdu_to(&s.stations[away].air, a, CmcePduTypeDl::DRelease)
        });
        sim.submit(home, u_restore_individual(a, away_id, b, duplex, las[away] as u64));
        sim.run_until(&format!("ISSI {a} is restored on station {home}"), |s| call_restore_to(&s.stations[home].air, a).is_some());
        let (back, back_ts) = call_restore_to(&sim.stations[home].air, a).unwrap();
        assert_eq!(back.call_identifier, away_id);
        assert_eq!(back.new_call_identifier.map_or(back.call_identifier, |n| n as u16), home_id, "the home station's own call again");
        sim.run_until(&format!("station {home} ends the network call of pair {k}"), |s| {
            s.relay.call_releases.iter().any(|(st, c, cause)| *st == home && *c == id && *cause == 14)
        });
        assert!(sim.relay.bridges.is_empty(), "the call is local again");
        assert!(!sim.relay.restore_requests.iter().any(|(st, issi, _)| *st == home && *issi == a), "nothing asked of the network");
        assert!(!cmce_pdu_to(&sim.stations[home].air, b, CmcePduTypeDl::DRelease), "B keeps its call");
        let _ = back_ts;
        joined += 1;

        // B hangs up; both radios are released on the home station.
        sim.submit(home, u_disconnect(b, home_id));
        sim.run_until(&format!("ISSI {a} is released on station {home}"), |s| {
            cmce_pdu_to(&s.stations[home].air, a, CmcePduTypeDl::DRelease)
        });
        sim.steps(20);
    }
    assert_eq!((handed, joined), (PAIRS, PAIRS));
}

// ─── Multi-cell station (milestone 7) ───────────────────────────────────────────────────────────

/// Index of station 0's second cell in `Sim::stations` (after the ten stations). It shares station
/// 0's Brew connection through the site switch and has no wire of its own.
const SIBLING: usize = STATIONS;
const SIBLING_CARRIER: u16 = 1599;

/// Ten stations; station 0 runs a second cell (carrier 1599, location area `cell1_la`), wired as
/// `bluestation-bs` does: the real Brew entity inside the site switch on cell 0, a cell link in
/// cell 1's network slot.
fn multi_cell_sim(cell1_la: u16, extensions: &[&str]) -> Sim {
    multi_cell_sim_with(cell1_la, extensions, Vec::new(), &|_, _| {})
}

/// `multi_cell_sim` with neighbour lists: `cell1_neighbours` for station 0's second cell,
/// `configure` for stations 1 to 9.
fn multi_cell_sim_with(
    cell1_la: u16,
    extensions: &[&str],
    cell1_neighbours: Vec<CfgNeighborCellCa>,
    configure: &dyn Fn(usize, &mut StackConfig),
) -> Sim {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.main_carrier = 1521;
    config.cell.location_area = 1;
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_ssirange(vec![]);
    config.brew = Some(relay_brew());
    let mut cell1 = config.cell.clone();
    cell1.main_carrier = SIBLING_CARRIER;
    cell1.location_area = cell1_la;
    cell1.neighbor_cells_ca = cell1_neighbours;
    config.extra_cells = vec![CfgExtraCell {
        id: CellId(1),
        cell: cell1,
        soapysdr: None,
        network: None,
    }];
    assert!(config.is_site_linked());
    let sibling_config = config.for_extra_cell(CellId(1)).unwrap();
    let start = Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    let entities = || vec![TetraEntity::Mle, TetraEntity::Mm, TetraEntity::Cmce];
    let mut primary = ComponentTest::from_config(config, start);
    primary.populate_entities(entities(), vec![TetraEntity::Llc, TetraEntity::Umac]);
    let mut sibling = ComponentTest::from_config(sibling_config, start);
    sibling.populate_entities(entities(), vec![TetraEntity::Llc, TetraEntity::Umac]);

    let (ports, mut links) = site_links(&[CellId(1)]);
    sibling.register_entity(links.remove(&CellId(1)).unwrap());
    let wire = Arc::new(Mutex::new(Wire::default()));
    let transport = ChannelTransport {
        wire: wire.clone(),
        extensions: extensions.iter().map(|e| e.to_string()).collect(),
        connected: false,
    };
    let brew = BrewEntity::new(primary.get_shared_config(), transport);
    let switch = SiteSwitch::new(
        Box::new(brew),
        primary.get_shared_config(),
        vec![(CellId(1), sibling.get_shared_config())],
        ports,
        SharedDirectory::default(),
    );
    primary.register_entity(switch);

    let cell = |test, wire, carrier| Station {
        test,
        wire,
        carrier,
        air: Vec::new(),
        voice_down: Vec::new(),
    };
    let mut stations = vec![cell(primary, wire, 1521)];
    stations.extend((1..STATIONS).map(|i| station(i, i as u16 + 1, extensions, configure)));
    stations.push(cell(sibling, Arc::new(Mutex::new(Wire::default())), SIBLING_CARRIER));
    let wires = stations.iter().map(|s| s.wire.clone()).collect();
    let mut sim = Sim {
        stations,
        wires,
        relay: Relay::default(),
        ticks: 0,
    };
    sim.run_until("every station announced its cells", |s| s.relay.cells.len() == STATIONS);
    sim
}

/// `issi` registers on cell `cell` of `sim` (a station index or `SIBLING`).
fn register_on_cell(sim: &mut Sim, cell: usize, issi: u32, lu_type: LocationUpdateType, groups: &[u32]) {
    let before = accepts(&sim.stations[cell].air, issi);
    let lu = location_update_pdu(issi, lu_type, groups);
    sim.submit(cell, uplink(issi, MleProtocolDiscriminator::Mm, |b| lu.to_bitbuf(b).unwrap()));
    sim.run_until(&format!("ISSI {issi} gets an accept on cell {cell}"), |s| accepts(&s.stations[cell].air, issi) > before);
    let station = if cell == SIBLING { 0 } else { cell };
    sim.run_until(&format!("ISSI {issi} is on station {station} on the network"), |s| s.relay.location.get(&issi) == Some(&station));
}

/// Milestone 7: individual calls with a network party (the TetraLink app behind the server) follow
/// their radios between the two cells of one station. The radio sends U-CALL RESTORE on the new
/// cell (TS 100 392-2 clause 14.5.1.2.4); the new cell asks the site switch, which moves the call
/// from its sibling cell without the network noticing: the old leg goes without a release to the
/// network, the new cell answers with D-CALL RESTORE on its own channel, and the voice runs both
/// ways from there. With an own location area per cell the radio registers first (service
/// restoration roaming, clause 16.4 a)); every sixth gives the call up (plain roaming) and the
/// call ends at once for the network party with "call restoration of the other user failed".
fn individual_calls_follow_their_radios_between_cells(cell1_la: u16) {
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_CALL_RESTORE];
    let mut sim = multi_cell_sim(cell1_la, &extensions);
    let own_la = cell1_la != 1;
    let radios: Vec<u32> = (0..26).map(|r| FIRST_ISSI + 500 + r).collect();
    for (n, &radio) in radios.iter().enumerate() {
        let home = if n % 2 == 0 { 0 } else { SIBLING };
        register_on_cell(&mut sim, home, radio, LocationUpdateType::RoamingLocationUpdating, &[GROUPS[n % 5]]);
    }

    let (mut restored, mut given_up) = (0, 0);
    for (n, &radio) in radios.iter().enumerate() {
        sim.clear_air();
        let (from, to) = if n % 2 == 0 { (0, SIBLING) } else { (SIBLING, 0) };
        let app = 2_200_501 + n as u32;
        let duplex = n % 2 == 0 || n % 3 == 0;
        let id = Uuid::from_u128(0x7A00 + n as u128);
        sim.relay.call_radio(id, app, radio, 0, duplex, &sim.wires);
        sim.run_until(&format!("cell {from} pages ISSI {radio}"), |s| individual_setup_call_id(&s.stations[from].air, radio).is_some());
        let old_id = individual_setup_call_id(&sim.stations[from].air, radio).unwrap();
        sim.submit(from, u_connect(radio, old_id, duplex));
        sim.run_until(&format!("ISSI {radio}'s call is connected on cell {from}"), |s| {
            cmce_pdu_to(&s.stations[from].air, radio, CmcePduTypeDl::DConnectAcknowledge)
        });
        sim.steps(10);

        if own_la {
            let gives_up = n % 6 == 5;
            let lu_type = if gives_up {
                LocationUpdateType::RoamingLocationUpdating
            } else {
                LocationUpdateType::ServiceRestorationRoamingLocationUpdating
            };
            register_on_cell(&mut sim, to, radio, lu_type, &[GROUPS[n % 5]]);
            sim.run_until(&format!("cell {from} lets ISSI {radio}'s leg go"), |s| {
                cmce_pdu_to(&s.stations[from].air, radio, CmcePduTypeDl::DRelease)
            });
            if gives_up {
                sim.run_until(&format!("the network ends ISSI {radio}'s given up call"), |s| {
                    s.relay.call_releases.iter().any(|(st, c, cause)| *st == 0 && *c == id && *cause == 19)
                });
                sim.relay.private.remove(&id);
                sim.steps(20);
                given_up += 1;
                continue;
            }
        }

        // Simplex: the radio asks for the floor in every other restore; the floor is free.
        let request = duplex || n % 4 == 1;
        let old_la = if from == 0 { 1 } else { cell1_la };
        sim.submit(to, u_restore_individual(radio, old_id, app, request, old_la as u64));
        sim.run_until(&format!("ISSI {radio} is restored on cell {to}"), |s| call_restore_to(&s.stations[to].air, radio).is_some());
        let (pdu, ts) = call_restore_to(&sim.stations[to].air, radio).unwrap();
        assert_eq!(pdu.call_identifier, old_id, "D-CALL RESTORE names the call the radio restored");
        let expected = if request { TransmissionGrant::Granted } else { TransmissionGrant::NotGranted };
        assert_eq!(pdu.transmission_grant, expected.into_raw() as u8, "grant for ISSI {radio} (duplex {duplex})");
        sim.run_until(&format!("cell {from} drops ISSI {radio}'s old leg"), |s| {
            cmce_pdu_to(&s.stations[from].air, radio, CmcePduTypeDl::DRelease)
        });
        assert!(sim.relay.restore_requests.iter().all(|(_, issi, _)| *issi != radio), "the network was not asked");

        // Voice both ways on the new cell's channel.
        let carrier = sim.stations[to].carrier;
        for _ in 0..8 {
            sim.wires[0].lock().unwrap().to_station.push_back(build_voice_frame(&id, 274, &[0u8; 36]));
        }
        sim.run_until(&format!("cell {to} plays the network party's voice to ISSI {radio}"), |s| {
            s.stations[to].voice_down.contains(&(carrier, ts))
        });
        sim.relay.voice_up.retain(|(_, c)| *c != id);
        sim.submit(
            to,
            SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Umac,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
                    carrier_num: carrier,
                    ts,
                    data: vec![0u8; 35],
                }),
            },
        );
        sim.run_until(&format!("ISSI {radio}'s voice reaches the network from cell {to}"), |s| {
            s.relay.voice_up.contains(&(0, id))
        });
        assert!(!sim.relay.call_releases.iter().any(|(_, c, _)| *c == id), "nobody ended the call on the network");

        // The network party hangs up; the new cell releases the radio.
        sim.wires[0].lock().unwrap().to_station.push_back(build_call_release(&id, 1));
        sim.relay.private.remove(&id);
        let releases_before = air_to(&sim.stations[from].air, radio).len();
        sim.run_until(&format!("cell {to} releases ISSI {radio}"), |s| {
            cmce_pdu_to(&s.stations[to].air, radio, CmcePduTypeDl::DRelease)
        });
        sim.steps(20);
        assert_eq!(air_to(&sim.stations[from].air, radio).len(), releases_before, "the old cell stays out of it");
        restored += 1;
    }
    if own_la {
        assert_eq!((restored, given_up), (22, 4));
    } else {
        assert_eq!(restored, 26);
    }
}

#[test]
fn sim_multi_cell_individual_calls_follow_their_radios_between_cells_with_own_location_areas() {
    individual_calls_follow_their_radios_between_cells(11);
}

#[test]
fn sim_multi_cell_individual_calls_follow_their_radios_between_cells_in_one_location_area() {
    individual_calls_follow_their_radios_between_cells(1);
}

/// Milestone 7: calls between two radios of a multi-cell station survive a change of cell, as
/// between stations (`call-handoff`), but through the site switch alone. 26 radios in 13 pairs,
/// homes alternating between the two cells. Each pair starts a local call on its home cell; the
/// caller moves to the other cell and restores the call there (U-CALL RESTORE, TS 100 392-2 clause
/// 14.5.1.2.4): the home cell hands its call to the switch and the two cells carry it, voice both
/// ways. Then the caller comes back to its peer's cell and restores again: the home cell joins
/// both legs into its own local call. The network never hears of any of it. Duplex and simplex
/// alternate; with an own location area per cell the radio registers first.
fn radio_to_radio_calls_survive_cell_changes(cell1_la: u16) {
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_CALL_RESTORE, BREW_EXT_CALL_HANDOFF];
    let mut sim = multi_cell_sim(cell1_la, &extensions);
    let own_la = cell1_la != 1;
    let la_of = |cell: usize| if cell == SIBLING { cell1_la } else { 1 };
    const PAIRS: u32 = 13;
    let pairs: Vec<(u32, u32, usize, usize)> = (0..PAIRS)
        .map(|k| {
            let (home, away) = if k % 2 == 0 { (0, SIBLING) } else { (SIBLING, 0) };
            (FIRST_ISSI + 700 + 2 * k, FIRST_ISSI + 701 + 2 * k, home, away)
        })
        .collect();
    for &(a, b, home, _) in &pairs {
        register_on_cell(&mut sim, home, a, LocationUpdateType::RoamingLocationUpdating, &[]);
        register_on_cell(&mut sim, home, b, LocationUpdateType::RoamingLocationUpdating, &[]);
    }

    let (mut handed, mut joined) = (0, 0);
    for (k, &(a, b, home, away)) in pairs.iter().enumerate() {
        sim.clear_air();
        let duplex = k % 2 == 0 || k % 3 == 0;

        // A calls B on their home cell: a local call.
        sim.submit(home, u_setup_individual(a, b, duplex));
        sim.run_until(&format!("cell {home} pages ISSI {b}"), |s| individual_setup_call_id(&s.stations[home].air, b).is_some());
        let home_id = individual_setup_call_id(&sim.stations[home].air, b).unwrap();
        sim.submit(home, u_connect(b, home_id, duplex));
        sim.run_until(&format!("ISSI {a} hears the call is connected"), |s| {
            cmce_pdu_to(&s.stations[home].air, a, CmcePduTypeDl::DConnect)
        });
        sim.steps(10);

        // A goes to the other cell and restores the call there.
        if own_la {
            register_on_cell(&mut sim, away, a, LocationUpdateType::ServiceRestorationRoamingLocationUpdating, &[]);
        }
        sim.submit(away, u_restore_individual(a, home_id, b, duplex, la_of(home) as u64));
        sim.run_until(&format!("ISSI {a} is restored on cell {away}"), |s| call_restore_to(&s.stations[away].air, a).is_some());
        let (pdu, a_ts) = call_restore_to(&sim.stations[away].air, a).unwrap();
        assert_eq!(pdu.call_identifier, home_id, "D-CALL RESTORE names the call the radio restored");
        if duplex {
            assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted.into_raw() as u8, "duplex: always granted");
        }
        assert!(!cmce_pdu_to(&sim.stations[home].air, b, CmcePduTypeDl::DRelease), "B keeps its call");
        assert!(!restore_fail(&sim.stations[away].air, a));
        handed += 1;

        // Voice from A on the other cell reaches B on the home cell, and back.
        let (home_carrier, away_carrier) = (sim.stations[home].carrier, sim.stations[away].carrier);
        sim.steps(100); // earlier pairs' voice played out
        sim.stations[home].voice_down.clear();
        for _ in 0..16 {
            sim.submit(away, uplink_voice(away_carrier, a_ts));
        }
        sim.run_until(&format!("cell {home} plays ISSI {a}'s voice to ISSI {b}"), |s| !s.stations[home].voice_down.is_empty());
        let (_, b_ts) = sim.stations[home].voice_down[0];
        sim.steps(100);
        sim.stations[away].voice_down.clear();
        for _ in 0..16 {
            sim.submit(home, uplink_voice(home_carrier, b_ts));
        }
        sim.run_until(&format!("cell {away} plays ISSI {b}'s voice to ISSI {a}"), |s| {
            s.stations[away].voice_down.contains(&(away_carrier, a_ts))
        });

        // A comes back to B's cell and restores again: the home cell joins both legs.
        sim.clear_air();
        let away_id = pdu.new_call_identifier.map_or(pdu.call_identifier, |n| n as u16);
        if own_la {
            register_on_cell(&mut sim, home, a, LocationUpdateType::ServiceRestorationRoamingLocationUpdating, &[]);
        }
        sim.submit(home, u_restore_individual(a, away_id, b, duplex, la_of(away) as u64));
        sim.run_until(&format!("ISSI {a} is restored on cell {home}"), |s| call_restore_to(&s.stations[home].air, a).is_some());
        let (back, _) = call_restore_to(&sim.stations[home].air, a).unwrap();
        assert_eq!(back.call_identifier, away_id);
        assert_eq!(back.new_call_identifier.map_or(back.call_identifier, |n| n as u16), home_id, "the home cell's own call again");
        sim.run_until(&format!("cell {away} lets ISSI {a}'s leg go"), |s| {
            cmce_pdu_to(&s.stations[away].air, a, CmcePduTypeDl::DRelease)
        });
        assert!(!cmce_pdu_to(&sim.stations[home].air, b, CmcePduTypeDl::DRelease), "B keeps its call");
        joined += 1;

        // B hangs up; both radios are released on the home cell.
        sim.submit(home, u_disconnect(b, home_id));
        sim.run_until(&format!("ISSI {a} is released on cell {home}"), |s| {
            cmce_pdu_to(&s.stations[home].air, a, CmcePduTypeDl::DRelease)
        });
        sim.steps(20);
        for &r in &[a, b] {
            assert!(!sim.relay.restore_requests.iter().any(|(_, issi, _)| *issi == r), "nothing asked of the network");
        }
        assert!(sim.relay.private.is_empty() && sim.relay.bridges.is_empty(), "the calls stay on the station");
    }
    assert_eq!((handed, joined), (PAIRS, PAIRS));
}

#[test]
fn sim_multi_cell_radio_to_radio_calls_survive_cell_changes_in_one_location_area() {
    radio_to_radio_calls_survive_cell_changes(1);
}

#[test]
fn sim_multi_cell_radio_to_radio_calls_survive_cell_changes_with_own_location_areas() {
    radio_to_radio_calls_survive_cell_changes(11);
}

/// Milestone 7: within its location area a radio changes cell without registering (TS 100 392-2
/// clause 18.3.4.7.1a), so a group call to the members of one cell of a multi-cell station runs on
/// every cell of that cell's location area; with an own location area per cell it stays on the
/// members' cell. 26 radios, all registered on the primary cell; the talkers are on station 2.
fn group_calls_on_the_cells_of_the_members_location_area(cell1_la: u16) {
    let extensions = ["bts-info", "handover-v1", "ms-cell"];
    let mut sim = multi_cell_sim(cell1_la, &extensions);
    let same_la = cell1_la == 1;
    for n in 0..26u32 {
        register_on_cell(&mut sim, 0, FIRST_ISSI + 600 + n, LocationUpdateType::RoamingLocationUpdating, &[GROUPS[n as usize % 5]]);
    }
    for (n, &gssi) in GROUPS.iter().enumerate() {
        let talker = FIRST_ISSI + 700 + n as u32;
        sim.register(2, talker, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
        sim.clear_air();
        let session = sim.start_call(2, talker, gssi);
        sim.run_until(&format!("the primary cell sets up {gssi}"), |s| group_setups(&s.stations[0].air, gssi) > 0);
        sim.steps(40);
        assert_eq!(
            group_setups(&sim.stations[SIBLING].air, gssi) > 0,
            same_la,
            "group {gssi} on the sibling cell (location area {cell1_la})"
        );
        sim.end_call(2, talker, gssi, session);
    }
}

#[test]
fn sim_multi_cell_group_calls_run_on_every_cell_of_one_location_area() {
    group_calls_on_the_cells_of_the_members_location_area(1);
}

#[test]
fn sim_multi_cell_group_calls_stay_on_the_members_cell_with_own_location_areas() {
    group_calls_on_the_cells_of_the_members_location_area(11);
}

/// Milestone 7: an individual call from the network (the TetraLink app) to a radio that changed
/// cell idle within the location area of a multi-cell station, without registering (TS 100 392-2
/// clause 18.3.4.7.1a). Both cells page it; it answers on the cell it is on, which takes the call,
/// and the other cell lets go without a release to the network. 26 radios registered on the
/// primary cell; every second one has moved to the sibling cell.
#[test]
fn sim_multi_cell_individual_call_reaches_a_radio_that_moved_within_the_location_area() {
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_CALL_RESTORE];
    let mut sim = multi_cell_sim(1, &extensions);
    let radios: Vec<u32> = (0..26).map(|r| FIRST_ISSI + 800 + r).collect();
    for (n, &radio) in radios.iter().enumerate() {
        register_on_cell(&mut sim, 0, radio, LocationUpdateType::RoamingLocationUpdating, &[GROUPS[n % 5]]);
    }
    for (n, &radio) in radios.iter().enumerate() {
        sim.clear_air();
        let on = if n % 2 == 1 { SIBLING } else { 0 };
        let other = if on == 0 { SIBLING } else { 0 };
        let duplex = n % 3 != 0;
        let id = Uuid::from_u128(0x7B00 + n as u128);
        sim.relay.call_radio(id, 2_200_801 + n as u32, radio, 0, duplex, &sim.wires);
        sim.run_until(&format!("both cells page ISSI {radio}"), |s| {
            individual_setup_call_id(&s.stations[0].air, radio).is_some() && individual_setup_call_id(&s.stations[SIBLING].air, radio).is_some()
        });
        let call_id = individual_setup_call_id(&sim.stations[on].air, radio).unwrap();
        sim.submit(on, u_connect(radio, call_id, duplex));
        sim.run_until(&format!("ISSI {radio}'s call is connected on cell {on}"), |s| {
            cmce_pdu_to(&s.stations[on].air, radio, CmcePduTypeDl::DConnectAcknowledge)
        });
        sim.run_until(&format!("cell {other} stops paging ISSI {radio}"), |s| {
            cmce_pdu_to(&s.stations[other].air, radio, CmcePduTypeDl::DRelease)
        });
        assert!(!sim.relay.call_releases.iter().any(|(_, c, _)| *c == id), "nobody ended the call on the network");

        // Voice from the network reaches the cell the radio answered on.
        let carrier = sim.stations[on].carrier;
        for _ in 0..8 {
            sim.wires[0].lock().unwrap().to_station.push_back(build_voice_frame(&id, 274, &[0u8; 36]));
        }
        sim.run_until(&format!("cell {on} plays the app's voice to ISSI {radio}"), |s| {
            s.stations[on].voice_down.iter().any(|(c, _)| *c == carrier)
        });

        sim.wires[0].lock().unwrap().to_station.push_back(build_call_release(&id, 1));
        sim.relay.private.remove(&id);
        sim.run_until(&format!("cell {on} releases ISSI {radio}"), |s| {
            cmce_pdu_to(&s.stations[on].air, radio, CmcePduTypeDl::DRelease)
        });
        sim.steps(20);
    }
}

/// Milestone 7 step 3: a multi-cell station takes part in roaming between stations like a
/// single-cell one; its site switch hands each request to the cell concerned. 26 radios, station
/// 0's second cell (own location area 11) lists stations 1 to 6, which list it back. Each radio:
///
/// 1. announced reselection (U-PREPARE with forward registration, TS 100 392-2 clause 18.3.4.7.5)
///    from station k to the second cell: the second cell's MM registers it, k sends D-NEW-CELL
///    with that accept;
/// 2. an individual call from the network reaches it there; it moves unannounced to station m with
///    service restoration roaming (clause 16.4 a)): the network moves the call, the second cell
///    lets its leg go (`call-restore`), drops the radio (`ms-moved`) without deregistering it,
///    and the radio restores the call on m (clause 14.5.1.2.4);
/// 3. every second radio: a call on m, then it moves unannounced back to the second cell and
///    restores the call there (voice both ways on the second cell's channel), then leaves by
///    announced reselection to another station, which registers it; the second cell drops it.
#[test]
fn sim_multi_cell_station_roams_with_other_stations() {
    let extensions = ["bts-info", "handover-v1", "ms-cell", BREW_EXT_MS_MOVED, BREW_EXT_MS_RESELECTION, BREW_EXT_CALL_RESTORE];
    let cell1_la = 11;
    let neighbours: Vec<CfgNeighborCellCa> = (1..=6u8).map(|i| neighbour(i, 1521 + 2 * i as u16, i as u16 + 1)).collect();
    let mut sim = multi_cell_sim_with(cell1_la, &extensions, neighbours, &|i, cfg| {
        cfg.cell.neighbor_cells_ca = vec![neighbour(1, SIBLING_CARRIER, 11), neighbour(2, 1521 + 2 * ((i % 9) + 1) as u16, (i % 9) as u16 + 2)];
    });
    let caps = &sim.relay.cells[&0].1;
    for cap in [BREW_EXT_HANDOVER, BREW_EXT_MS_MOVED, BREW_EXT_CALL_RESTORE] {
        assert!(caps.iter().any(|c| c == cap), "the multi-cell station announces {cap}: {caps:?}");
    }
    assert!(sim.relay.cells[&0].0.contains(&SIBLING_CARRIER), "both cells announced");

    let accept = format!("{:04b}", MmPduTypeDl::DLocationUpdateAccept.into_raw());
    let registered = |sim: &Sim, cell: usize, issi: u32| sim.stations[cell].test.get_shared_config().state_read().subscribers.is_registered(issi);
    let (mut arrived, mut restored_away, mut restored_here, mut left) = (0, 0, 0, 0);
    for n in 0..26usize {
        let radio = FIRST_ISSI + 900 + n as u32;
        let gssi = GROUPS[n % 5];
        let k = 1 + n % 6;
        let m = 1 + (n + 3) % 6;
        sim.clear_air();

        // 1. Announced from station k to the second cell.
        sim.register(k, radio, LocationUpdateType::RoamingLocationUpdating, &[gssi]);
        sim.submit(k, u_prepare(radio, 1, true));
        sim.run_until(&format!("station {k} answers ISSI {radio}'s U-PREPARE"), |s| prepare_answer(&s.stations[k].air, radio).is_some());
        let answer = prepare_answer(&sim.stations[k].air, radio).unwrap();
        assert!(answer.starts_with(&format!("{D_NEW_CELL_CHANGE_NOW}{accept}")), "ISSI {radio}: D-NEW-CELL with the second cell's accept, got {answer}");
        assert_eq!(sim.relay.location.get(&radio), Some(&0), "registered on the multi-cell station before leaving");
        assert!(registered(&sim, SIBLING, radio) && !registered(&sim, 0, radio), "the second cell registered it");
        arrived += 1;

        // 2. A network call on the second cell, then unannounced to station m.
        let app = 2_200_901 + n as u32;
        let duplex = n % 3 != 0;
        let id = Uuid::from_u128(0x7C00 + n as u128);
        sim.relay.call_radio(id, app, radio, 0, duplex, &sim.wires);
        sim.run_until(&format!("the second cell pages ISSI {radio}"), |s| individual_setup_call_id(&s.stations[SIBLING].air, radio).is_some());
        assert!(individual_setup_call_id(&sim.stations[0].air, radio).is_none(), "own location area: only its cell pages");
        let old_id = individual_setup_call_id(&sim.stations[SIBLING].air, radio).unwrap();
        sim.submit(SIBLING, u_connect(radio, old_id, duplex));
        sim.run_until(&format!("ISSI {radio}'s call is connected on the second cell"), |s| {
            cmce_pdu_to(&s.stations[SIBLING].air, radio, CmcePduTypeDl::DConnectAcknowledge)
        });
        sim.steps(10);
        sim.register(m, radio, LocationUpdateType::ServiceRestorationRoamingLocationUpdating, &[gssi]);
        sim.run_until(&format!("the second cell lets ISSI {radio}'s leg go"), |s| {
            cmce_pdu_to(&s.stations[SIBLING].air, radio, CmcePduTypeDl::DRelease)
        });
        assert!(sim.relay.ms_moved.contains(&(radio, 0)), "the network told the multi-cell station");
        sim.run_until(&format!("the second cell drops ISSI {radio}"), |s| !registered(s, SIBLING, radio));
        sim.submit(m, u_restore_individual(radio, old_id, app, true, cell1_la as u64));
        sim.run_until(&format!("ISSI {radio} is restored on station {m}"), |s| call_restore_to(&s.stations[m].air, radio).is_some());
        sim.wires[m].lock().unwrap().to_station.push_back(build_call_release(&id, 1));
        sim.relay.private.remove(&id);
        sim.run_until(&format!("station {m} releases ISSI {radio}"), |s| cmce_pdu_to(&s.stations[m].air, radio, CmcePduTypeDl::DRelease));
        restored_away += 1;
        sim.steps(20);
        assert!(!sim.relay.deregistrations.contains(&(radio, 0)), "the multi-cell station deregistered ISSI {radio} on the network");
        assert_eq!(sim.relay.location.get(&radio), Some(&m));
        if n % 2 == 1 {
            continue;
        }

        // 3. A call on m, back to the second cell unannounced, restored there.
        sim.clear_air();
        let id = Uuid::from_u128(0x7D00 + n as u128);
        sim.relay.call_radio(id, app, radio, m, duplex, &sim.wires);
        sim.run_until(&format!("station {m} pages ISSI {radio}"), |s| individual_setup_call_id(&s.stations[m].air, radio).is_some());
        let old_id = individual_setup_call_id(&sim.stations[m].air, radio).unwrap();
        sim.submit(m, u_connect(radio, old_id, duplex));
        sim.run_until(&format!("ISSI {radio}'s call is connected on station {m}"), |s| {
            cmce_pdu_to(&s.stations[m].air, radio, CmcePduTypeDl::DConnectAcknowledge)
        });
        sim.steps(10);
        register_on_cell(&mut sim, SIBLING, radio, LocationUpdateType::ServiceRestorationRoamingLocationUpdating, &[gssi]);
        sim.run_until(&format!("station {m} lets ISSI {radio}'s call go"), |s| cmce_pdu_to(&s.stations[m].air, radio, CmcePduTypeDl::DRelease));
        assert_eq!(sim.relay.private[&id].owner, 0, "the call moved to the multi-cell station");
        sim.submit(SIBLING, u_restore_individual(radio, old_id, app, true, (m + 1) as u64));
        sim.run_until(&format!("ISSI {radio} is restored on the second cell"), |s| call_restore_to(&s.stations[SIBLING].air, radio).is_some());
        let (pdu, ts) = call_restore_to(&sim.stations[SIBLING].air, radio).unwrap();
        assert_eq!(pdu.call_identifier, old_id, "D-CALL RESTORE names the call the radio restored");
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
        assert_eq!(sim.relay.restore_requests.last(), Some(&(0, radio, app)), "the second cell asked the network");
        assert!(call_restore_to(&sim.stations[0].air, radio).is_none(), "the primary cell stays out of it");
        for _ in 0..8 {
            sim.wires[0].lock().unwrap().to_station.push_back(build_voice_frame(&id, 274, &[0u8; 36]));
        }
        sim.run_until(&format!("the second cell plays the app's voice to ISSI {radio}"), |s| {
            s.stations[SIBLING].voice_down.contains(&(SIBLING_CARRIER, ts))
        });
        sim.relay.voice_up.retain(|(_, c)| *c != id);
        sim.submit(
            SIBLING,
            SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Umac,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
                    carrier_num: SIBLING_CARRIER,
                    ts,
                    data: vec![0u8; 35],
                }),
            },
        );
        sim.run_until(&format!("ISSI {radio}'s voice reaches the network from the second cell"), |s| s.relay.voice_up.contains(&(0, id)));
        assert!(!sim.relay.call_releases.iter().any(|(_, c, _)| *c == id), "nobody ended the call on the network");
        sim.wires[0].lock().unwrap().to_station.push_back(build_call_release(&id, 1));
        sim.relay.private.remove(&id);
        sim.run_until(&format!("the second cell releases ISSI {radio}"), |s| {
            cmce_pdu_to(&s.stations[SIBLING].air, radio, CmcePduTypeDl::DRelease)
        });
        restored_here += 1;
        sim.steps(20);

        //    Announced from the second cell to station j (one the radio has not been on: k
        //    may still be dropping it, after its departure delay).
        let j = 1 + (n + 1) % 6;
        sim.clear_air();
        sim.submit(SIBLING, u_prepare(radio, j as u64, true));
        sim.run_until(&format!("the second cell answers ISSI {radio}'s U-PREPARE"), |s| prepare_answer(&s.stations[SIBLING].air, radio).is_some());
        let answer = prepare_answer(&sim.stations[SIBLING].air, radio).unwrap();
        assert!(answer.starts_with(&format!("{D_NEW_CELL_CHANGE_NOW}{accept}")), "ISSI {radio}: D-NEW-CELL with station {j}'s accept, got {answer}");
        assert!(prepare_answer(&sim.stations[0].air, radio).is_none(), "the primary cell stays out of it");
        assert_eq!(sim.relay.location.get(&radio), Some(&j));
        sim.run_until(&format!("the second cell drops ISSI {radio} after it left"), |s| !registered(s, SIBLING, radio));
        sim.steps(20);
        assert!(!sim.relay.deregistrations.contains(&(radio, 0)), "the multi-cell station deregistered ISSI {radio} on the network");
        assert!(sim.relay.reselections.iter().any(|(i, st, t)| *i == radio && *st == 0 && t == "announced_type_2"), "station 0 reported the announced reselection");
        left += 1;
    }
    assert_eq!((arrived, restored_away, restored_here, left), (26, 26, 13, 13));
}

// ─── Group access (brew-server `group-access`) ──────────────────────────────────────────────────

/// The server sends who may use which group to every station that announced `group-access`.
fn send_group_access(sim: &mut Sim, json: &str, groups: usize) {
    let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_GROUP_ACCESS];
    msg.extend_from_slice(json.as_bytes());
    msg.push(0);
    for (i, wire) in sim.wires.iter().enumerate() {
        if sim.relay.cells[&i].1.iter().any(|c| c == BREW_EXT_GROUP_ACCESS) {
            wire.lock().unwrap().to_station.push_back(msg.clone());
        }
    }
    sim.run_until("every station has the group rules", |s| {
        s.stations.iter().all(|st| st.test.config.state_read().brew_group_access.len() == groups)
    });
}

/// The CMCE PDU of `pdu` type a station last sent to `ssi`, parsed.
fn last_cmce_to<T>(air: &[SapMsg], ssi: u32, pdu: CmcePduTypeDl, parse: fn(&mut BitBuffer) -> Option<T>) -> Option<T> {
    air_to(air, ssi).iter().rev().find_map(|bits| {
        let mut b = BitBuffer::from_bitstr(bits.strip_prefix(PD_CMCE)?);
        (b.peek_bits(5) == Some(pdu.into_raw())).then_some(())?;
        parse(&mut b)
    })
}

/// Ten stations, thirty radios on five groups, rules from the server: 2001 admits four radios,
/// 2002 is listen-only for everyone, 2003 blocks one radio and makes two listen-only. Stations
/// refuse the attachment (clause 16.8.2, in the D-LOCATION UPDATE ACCEPT, clause 16.10.23), a
/// call set-up from a radio that may not talk (D-RELEASE), the request to talk in a running call
/// (D-TX GRANTED "not granted", clause 14.5.2.2.1 b)) and let listener-only radios join a call
/// without the right to ask for the floor (clause 14.8.43). Emergency calls pass, and a changed
/// table applies at once.
#[test]
fn sim_ten_stations_thirty_radios_group_access() {
    use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
    use tetra_pdus::cmce::pdus::{d_release::DRelease, d_tx_granted::DTxGranted, u_tx_demand::UTxDemand};
    let issi = |r: u32| FIRST_ISSI + r;
    let mut extensions = EXTENSIONS.to_vec();
    extensions.push(BREW_EXT_GROUP_ACCESS);
    let las: Vec<u16> = (1..=STATIONS as u16).collect();
    let mut sim = Sim::new(&las, &extensions);
    assert!(sim.relay.cells.values().all(|(_, caps)| caps.iter().any(|c| c == BREW_EXT_GROUP_ACCESS)));

    let rules = format!(
        r#"{{"groups":[
            {{"gssi":2001,"mode":"allow","issis":[{},{},{},{}]}},
            {{"gssi":2002,"mode":"allow","issis":[],"rx_only":true,"rx_only_issis":[]}},
            {{"gssi":2003,"mode":"block","issis":[{}],"rx_only":true,"rx_only_issis":[{},{}]}}]}}"#,
        issi(0), issi(5), issi(10), issi(15), issi(27), issi(7), issi(12)
    );
    send_group_access(&mut sim, &rules, 3);

    // Radio r sits on station r % 10, member of group r % 5.
    for r in 0..RADIOS {
        sim.register(r as usize % STATIONS, issi(r), LocationUpdateType::RoamingLocationUpdating, &[GROUPS[r as usize % 5]]);
    }
    sim.steps(10);
    for r in 0..RADIOS {
        let (st, gssi) = (r as usize % STATIONS, GROUPS[r as usize % 5]);
        let refused = [20, 25, 27].contains(&r);
        let accept = {
            let prefix = mm_pdu(MmPduTypeDl::DLocationUpdateAccept);
            let bits = air_to(&sim.stations[st].air, issi(r)).into_iter().rev().find(|b| b.starts_with(&prefix)).expect("accepted");
            DLocationUpdateAccept::from_bitbuf(&mut BitBuffer::from_bitstr(&bits[3..])).expect("parses")
        };
        let gila = accept.group_identity_location_accept.expect("group identity location accept");
        assert_eq!(gila.group_identity_accept_reject, u8::from(refused), "radio {r}");
        let reason = gila.group_identity_downlink.unwrap_or_default().iter().find(|g| g.gssi == Some(gssi)).and_then(|g| g.group_identity_detachment_uplink);
        assert_eq!(reason, refused.then_some(1), "radio {r}: temporary 1 detachment");
        assert_eq!(sim.relay.groups[&issi(r)].contains(&gssi), !refused, "radio {r} affiliated on the network");
        assert_eq!(sim.stations[st].test.config.state_read().subscribers.attached_groups_of(issi(r)).contains(&gssi), !refused);
    }

    // 2002 is listen-only: a call set-up is refused, nothing reaches the network.
    sim.clear_air();
    sim.submit(1, u_setup(issi(1), 2002));
    sim.steps(20);
    let release = last_cmce_to(&sim.stations[1].air, issi(1), CmcePduTypeDl::DRelease, |b| DRelease::from_bitbuf(b).ok()).expect("D-RELEASE");
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert!(!sim.relay.calls.values().any(|c| c.gssi == 2002));

    // 2003: radio 2 talks; on station 7, radio 7 (listen-only) and 17 (may talk) join, radio 27
    // (blocked) is refused, and radio 7's request to talk is not granted.
    let session = sim.start_call(2, issi(2), 2003);
    sim.run_until("station 7 has the call on 2003", |s| s.local_call_id(7, 2003).is_some());
    let call_id = sim.local_call_id(7, 2003).unwrap();
    let joined = |sim: &mut Sim, r: u32| -> DConnect {
        sim.submit(7, u_setup(issi(r), 2003));
        sim.steps(10);
        last_cmce_to(&sim.stations[7].air, issi(r), CmcePduTypeDl::DConnect, |b| DConnect::from_bitbuf(b).ok()).expect("D-CONNECT")
    };
    let listener = joined(&mut sim, 7);
    assert_eq!((listener.call_identifier, listener.transmission_grant, listener.transmission_request_permission), (call_id, TransmissionGrant::GrantedToOtherUser, true));
    let talker = joined(&mut sim, 17);
    assert_eq!((talker.transmission_grant, talker.transmission_request_permission), (TransmissionGrant::GrantedToOtherUser, false));
    sim.submit(7, u_setup(issi(27), 2003));
    sim.steps(10);
    assert!(last_cmce_to(&sim.stations[7].air, issi(27), CmcePduTypeDl::DRelease, |b| DRelease::from_bitbuf(b).ok()).is_some());
    let demand = UTxDemand { call_identifier: call_id, tx_demand_priority: 0, encryption_control: false, reserved: false, facility: None, dm_ms_address: None, proprietary: None };
    sim.submit(7, uplink(issi(7), MleProtocolDiscriminator::Cmce, |b| demand.to_bitbuf(b).unwrap()));
    sim.steps(10);
    let answer = last_cmce_to(&sim.stations[7].air, issi(7), CmcePduTypeDl::DTxGranted, |b| DTxGranted::from_bitbuf(b).ok()).expect("D-TX GRANTED");
    assert_eq!(answer.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert_eq!(sim.relay.calls[&session].talker, issi(2), "the talker keeps the floor");
    sim.end_call(2, issi(2), 2003, session);

    // Emergency on the listen-only group passes.
    sim.submit(6, u_setup_prio(issi(6), 2002, 15));
    sim.run_until("the emergency call on 2002 reached the network", |s| s.relay.calls.values().any(|c| c.gssi == 2002 && c.owner == 6));

    // The server opens every group: radio 20 attaches 2001 now.
    send_group_access(&mut sim, r#"{"groups":[]}"#, 0);
    sim.register(0, issi(20), LocationUpdateType::PeriodicLocationUpdating, &[2001]);
    sim.run_until("radio 20 is affiliated to 2001", |s| s.relay.groups[&issi(20)].contains(&2001));
}
