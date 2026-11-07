//! Brew worker thread: transport-agnostic message loop for Brew protocol exchange
//!
//! Generic over any [`NetworkTransport`] implementation (WebSocket, QUIC, TCP, etc.).
//! The transport handles connection lifecycle and heartbeat; this worker handles
//! Brew protocol parsing, command dispatch, and event generation.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use tetra_config::bluestation::BrewDirectoryNeighbour;
use tetra_config::bluestation::CfgBrew;
use tetra_config::bluestation::Mni;
use tetra_config::bluestation::SharedConfig;
use tetra_config::bluestation::StackConfig;
use tetra_core::CellId;
use uuid::Uuid;

use crate::net_brew;
use crate::network::transports::NetworkTransport;

use super::protocol::*;

// ─── Events passed from worker to entity ─────────────────────────

/// Capacity of the worker→entity event channel.
///
/// The channel MUST stay bounded: the entity drains it from the single TDMA thread, so an
/// unbounded queue lets a remote core (or an on-path attacker when `tls = false`) stream
/// frames faster than one tick can drain and grow it until the box runs out of memory.
/// Sized for roughly a second of worst-case traffic — the entity drains MAX_EVENTS_PER_TICK
/// events per timeslot (see `entity.rs`), so it empties far faster than that.
pub const BREW_EVENT_CHANNEL_CAPACITY: usize = 512;

/// How long a *signalling* event may wait for room on a full event channel before being
/// dropped. Blocking here is deliberate: it stops us reading the socket, which is what
/// applies backpressure to the peer. Bounded so the worker still reaches its heartbeat.
const EVENT_SEND_BACKPRESSURE: Duration = Duration::from_millis(100);

/// Minimum spacing between "voice dropped" warnings — the condition is per-frame, the log
/// must not become the next flood.
const VOICE_DROP_WARN_INTERVAL: Duration = Duration::from_secs(5);

/// Events the Brew worker sends to the BrewEntity
#[derive(Debug)]
pub enum BrewEvent {
    /// Successfully connected to TetraPack server
    Connected { server_version: u8 },
    /// Brew protocol version detected from incoming message length (mnemonic presence)
    VersionDetected { version: u8 },

    /// Disconnected (with reason)
    Disconnected(String),

    /// Group call started
    GroupCallStart {
        uuid: Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
        service: u16,
    },

    /// Group call ended
    GroupCallEnd { uuid: Uuid, cause: u8 },

    /// Voice frame received (ACELP traffic)
    VoiceFrame { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// Subscriber event received
    SubscriberEvent { msg_type: u8, issi: u32, groups: Vec<u32> },

    /// SDS transfer received (SHORT_TRANSFER + SDS_TRANSFER combined)
    SdsTransfer {
        uuid: Uuid,
        source: u32,
        destination: u32,
        data: Vec<u8>,
        length_bits: u16,
    },

    /// SDS report received
    SdsReport { uuid: Uuid, status: u8 },

    // ── Circuit / individual call events ──────────────────────────
    /// TetraPack initiates a circuit call to a local MS (inbound SETUP_REQUEST)
    CircuitSetupRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// TetraPack accepted our outbound circuit call (SETUP_ACCEPT)
    CircuitSetupAccept { uuid: Uuid },

    /// TetraPack rejected our outbound circuit call (SETUP_REJECT)
    CircuitSetupReject { uuid: Uuid, cause: u8 },

    /// Remote side is alerting/ringing (CALL_ALERT)
    CircuitCallAlert { uuid: Uuid },

    /// Remote side accepted the call and sent CONNECT_REQUEST
    CircuitConnectRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// TetraPack confirmed connect (CONNECT_CONFIRM)
    CircuitConnectConfirm { uuid: Uuid, grant: u8, permission: u8 },

    /// Network granted simplex floor to the local side.
    CircuitSimplexGranted { uuid: Uuid, grant: u8, permission: u8 },

    /// Network released simplex floor / returned to idle.
    CircuitSimplexIdle { uuid: Uuid, grant: u8, permission: u8 },

    /// Circuit call released (CALL_RELEASE, inbound)
    CircuitCallRelease { uuid: Uuid, cause: u8 },

    /// DTMF digits received from network
    CircuitDtmf { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// A serving station asks this station (the target) to prepare an announced cell reselection
    /// (`SERVICE_HO_PREPARE` / `SERVICE_HO_FORWARD_REG`, relayed by the server)
    HandoverRequest(BrewHoRequest),

    /// Answer to a handover request this station sent (`SERVICE_HO_RESULT`)
    HandoverResult(BrewHoResult),

    /// A radio registered here registered on another station without a prepared handover
    /// (`SERVICE_MS_MOVED`, only from a server that offered `ms-moved`)
    MsMoved { issi: u32 },

    /// The server's answer to `RequestCallRestore` (`SERVICE_CALL_RESTORE`), or the worker's own
    /// "no call" when the server cannot move calls
    CallRestoreAnswer {
        issi: u32,
        peer: u32,
        call: Option<tetra_saps::control::call_control::NetworkRestoredCall>,
    },

    /// The server asks for the local individual call between `issi` (gone to another station)
    /// and `peer` (`SERVICE_CALL_HANDOFF`, only from a server that offered `call-handoff`)
    CallHandoffRequest { issi: u32, peer: u32 },

    /// Error from server
    ServerError { error_type: u8, data: Vec<u8> },
}

/// Commands the BrewEntity sends to the worker
#[derive(Debug)]
pub enum BrewCommand {
    /// Register a subscriber (ISSI)
    RegisterSubscriber { issi: u32 },

    /// Tell the server which cell of this station a just registered subscriber is on (only to a
    /// server that offered `ms-cell`)
    ReportMsCell { issi: u32, cell: CellId },

    /// Tell the server how a radio left this station (only to a server that offered
    /// `ms-reselection`)
    ReportReselection {
        issi: u32,
        reselection: tetra_saps::control::call_control::CellReselection,
    },

    /// Ask the server for the individual call `issi` restores here with `peer` (only a server
    /// that offered `call-restore`; otherwise answered "no call" at once)
    RequestCallRestore { issi: u32, peer: u32 },
    /// `issi` registered with a plain roaming location update: it has no call to restore
    /// (`SERVICE_CALL_NOT_RESTORED`, only to a server that moves calls)
    ReportRegisteredWithoutCall { issi: u32 },
    /// Answer a `CallHandoffRequest`: the call handed to the network, or `None`
    /// (`SERVICE_CALL_HANDOFF`, only to a server that offered `call-handoff`)
    SendCallHandoff {
        issi: u32,
        peer: u32,
        call: Option<tetra_saps::control::call_control::HandedOffCall>,
    },

    /// Deregister a subscriber (ISSI)
    DeregisterSubscriber { issi: u32 },

    /// Affiliate subscriber to groups
    AffiliateGroups { issi: u32, groups: Vec<u32> },

    /// Deaffiliate subscriber from groups
    DeaffiliateGroups { issi: u32, groups: Vec<u32> },

    /// Send GROUP_TX to TetraPack (local radio started transmitting on subscribed group)
    SendGroupTx {
        uuid: Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
        service: u16,
    },

    /// Send a voice frame to TetraPack (ACELP data from UL)
    SendVoiceFrame { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// Send GROUP_IDLE to TetraPack (transmission ended)
    SendGroupIdle { uuid: Uuid, cause: u8 },

    /// Send SDS to TetraPack (SHORT_TRANSFER + SDS_TRANSFER)
    SendSds {
        uuid: Uuid,
        source: u32,
        destination: u32,
        data: Vec<u8>,
        length_bits: u16,
    },

    /// Send SDS report to Brew (delivery acknowledgement)
    SendSdsReport { uuid: Uuid, status: u8 },

    // ── Circuit / individual call commands ────────────────────────
    /// CMCE → Brew: initiate a circuit call setup to TetraPack (outbound)
    SendSetupRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// CMCE → Brew: accept an inbound circuit call (BS is the called side)
    SendSetupAccept { uuid: Uuid },

    /// CMCE → Brew: reject an inbound circuit call
    SendSetupReject { uuid: Uuid, cause: u8 },

    /// CMCE → Brew: called MS is ringing
    SendCallAlert { uuid: Uuid },

    /// CMCE → Brew: called MS accepted the call
    SendConnectRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// CMCE → Brew: call confirmed and connected
    SendConnectConfirm { uuid: Uuid, grant: u8, permission: u8 },

    /// CMCE → Brew: local simplex side granted the floor.
    SendSimplexGranted { uuid: Uuid, grant: u8, permission: u8 },

    /// CMCE → Brew: local simplex side released the floor.
    SendSimplexIdle { uuid: Uuid, grant: u8, permission: u8 },

    /// CMCE → Brew: release/terminate a circuit call
    SendCallRelease { uuid: Uuid, cause: u8 },

    /// CMCE → Brew: forward DTMF digits from MS
    SendDtmf { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// MM → Brew: send RSSI measurement for an MS to the Brew server (Service 0xf4)
    SendRssiUpdate { issi: u32, rssi_dbfs: f32 },

    /// Answer a handover request the server relayed to this station (only to a server that
    /// offered `handover-v1`)
    SendHandoverResult(BrewHoResult),

    /// Ask the station serving a neighbour cell to prepare an MS's arrival (serving side). To a
    /// server without `handover-v1` nothing is sent; the worker answers "no target" itself.
    SendHandoverRequest(BrewHoRequest),

    /// Disconnect gracefully
    Disconnect,
}

/// Offer one event to the bounded entity channel. Returns the event back on failure.
///
/// Voice never waits: it is lossy by nature and a stalled entity must not stall the socket
/// reader. Signalling waits up to `backpressure` for room — that pause is the point, it stops
/// us draining the socket and lets TCP push back on the peer instead of us buffering for it.
fn offer_event(sender: &Sender<BrewEvent>, event: BrewEvent, backpressure: Duration) -> Result<(), BrewEvent> {
    if matches!(event, BrewEvent::VoiceFrame { .. }) {
        return sender.try_send(event).map_err(|e| e.into_inner());
    }
    sender.send_timeout(event, backpressure).map_err(|e| e.into_inner())
}

// ─── Worker ───────────────────────────────────────────────────────

/// SERVICE_BTS_INFO announcing every cell of this station (one Brew connection serves them all),
/// or `None` when the server did not offer the `bts-info` extension: TetraPack and BrandMeister
/// never get it. A cell whose frequency offset has no band-plan code is left out.
fn bts_info_message(config: &StackConfig, server_extensions: &[String]) -> Option<Vec<u8>> {
    if !server_extensions.iter().any(|e| e == BREW_EXT_BTS_INFO) {
        return None;
    }
    let directory = server_extensions.iter().any(|e| e == BREW_EXT_CELL_DIRECTORY);
    // Cells of another network (own `[cells.net_info]`) announce themselves on their own
    // connection.
    let cells: Vec<BrewBtsCell> = config
        .cells_of_station_network()
        .into_iter()
        .filter_map(|(id, cell, _)| {
            let carrier_ext = tetra_config::bluestation::carrier_band_plan(cell)?;
            let mut entry = BrewBtsCell { cell: id.0, carrier: cell.main_carrier, carrier_ext, location_area: cell.location_area, ..Default::default() };
            if directory {
                // What the cell broadcasts, for the other stations' neighbour lists.
                broadcast_values(config, cell, carrier_ext, &mut entry);
            }
            Some(entry)
        })
        .collect();
    let mut capabilities: Vec<&str> = Vec::new();
    if directory {
        capabilities.push(BREW_EXT_CELL_DIRECTORY);
    }
    // Who may use which group: the station refuses attachments and requests to talk itself. The
    // rules are shared by all cells of the station.
    if server_extensions.iter().any(|e| e == BREW_EXT_GROUP_ACCESS) {
        capabilities.push(BREW_EXT_GROUP_ACCESS);
    }
    // Radios of other networks by migration (Multi Network, server mode).
    if config.mni_enabled() && server_extensions.iter().any(|e| e == BREW_EXT_MNI) {
        capabilities.push(BREW_EXT_MNI);
    }
    if handles_handover(config, server_extensions) {
        capabilities.push(BREW_EXT_HANDOVER);
        if server_extensions.iter().any(|e| e == BREW_EXT_MS_MOVED) {
            capabilities.push(BREW_EXT_MS_MOVED);
        }
        if server_extensions.iter().any(|e| e == BREW_EXT_CALL_RESTORE) {
            capabilities.push(BREW_EXT_CALL_RESTORE);
        }
        if hands_off_calls(config, server_extensions) {
            capabilities.push(BREW_EXT_CALL_HANDOFF);
        }
    }
    Some(build_service_bts_info(config.net.mcc, config.net.mnc, &cells, &capabilities))
}

/// The optional `SERVICE_BTS_INFO` fields of `cell`: MS_TXPWR_MAX_CELL (code 1-7 = 15-45 dBm,
/// table 18.57; 0 is reserved and left out), RXLEV_ACCESS_MIN, the BS service details as broadcast
/// (system wide services while the network link is configured, as the SYSINFO sends them) and the
/// neighbour list with siblings. An element a neighbour entry leaves out is the serving cell's own
/// (TS 100 392-2 table 18.64 notes 1 and 2), so the network, band plan and LA are filled in.
fn broadcast_values(config: &StackConfig, cell: &tetra_config::bluestation::CfgCellInfo, carrier_ext: u16, entry: &mut BrewBtsCell) {
    entry.ms_txpwr_max_dbm = (1..=7).contains(&cell.ms_txpwr_max_cell).then(|| 10 + 5 * cell.ms_txpwr_max_cell);
    entry.rxlev_access_min_dbm = Some(cell.rxlev_access_min_dbm);
    entry.service_details = Some(vec![
        ("registration", cell.registration),
        ("deregistration", cell.deregistration),
        ("priority_cell", cell.priority_cell),
        ("no_minimum_mode", cell.no_minimum_mode),
        ("migration", cell.migration || config.multi_network.enabled),
        ("system_wide_services", cell.system_wide_services || config.has_network_link()),
        ("voice_service", cell.voice_service),
        ("circuit_mode_data_service", cell.circuit_mode_data_service),
        ("sndcp_service", cell.sndcp_service),
        ("aie_service", cell.aie_service),
        ("advanced_link", cell.advanced_link),
    ]);
    entry.neighbours = Some(
        cell.neighbor_cells_ca
            .iter()
            .map(|n| BrewDirectoryNeighbour {
                mcc: n.mcc.unwrap_or(config.net.mcc),
                mnc: n.mnc.unwrap_or(config.net.mnc),
                carrier: n.main_carrier_number,
                carrier_ext: n.main_carrier_number_extension.unwrap_or(carrier_ext),
                location_area: Some(n.location_area.unwrap_or(cell.location_area)),
            })
            .collect(),
    );
}

/// This station takes part in announced reselection with other stations: the server relays the
/// requests (`handover-v1` offered). A linked multi-cell station too: its site switch hands each
/// request to the cell of the target carrier and each answer back to the asking cell (between its
/// own cells it never asks the server).
fn handles_handover(_config: &StackConfig, server_extensions: &[String]) -> bool {
    server_extensions.iter().any(|e| e == BREW_EXT_HANDOVER)
}

/// This station restores individual calls of radios that come from another station: it takes
/// part in handover and the server moves calls (`call-restore`).
fn restores_calls(config: &StackConfig, server_extensions: &[String]) -> bool {
    handles_handover(config, server_extensions) && server_extensions.iter().any(|e| e == BREW_EXT_CALL_RESTORE)
}

/// This station hands local individual calls to the network and joins network calls of two of
/// its radios locally (`call-handoff`): it restores calls and the server offers the extension.
/// Not on a multi-cell station yet: its site switch does not route the handoff to a cell.
pub(crate) fn hands_off_calls(config: &StackConfig, server_extensions: &[String]) -> bool {
    !config.is_site_linked()
        && restores_calls(config, server_extensions)
        && server_extensions.iter().any(|e| e == BREW_EXT_CALL_HANDOFF)
}

/// SERVICE_MS_CELL placing `issi` on this station's cell `cell`, or `None` when the server did not
/// offer the `ms-cell` extension (TetraPack and BrandMeister never get it), or the cell is unknown
/// or has no band-plan code.
fn ms_cell_message(config: &StackConfig, server_extensions: &[String], issi: u32, cell: CellId) -> Option<Vec<u8>> {
    if !server_extensions.iter().any(|e| e == BREW_EXT_MS_CELL) {
        return None;
    }
    let (_, info, _) = config.cells().into_iter().find(|(id, _, _)| *id == cell)?;
    Some(build_service_ms_cell(
        issi,
        info.main_carrier,
        tetra_config::bluestation::carrier_band_plan(info)?,
    ))
}

/// Pending SDS header data (from CALL_STATE_SHORT_TRANSFER), awaiting matching FRAME_TYPE_SDS_TRANSFER
#[derive(Debug)]
struct PendingSds {
    source: u32,
    destination: u32,
    received_at: Instant,
}

/// Brew protocol worker, generic over the network transport.
///
/// Runs in a separate thread. Communicates with [`super::entity::BrewEntity`] via
/// crossbeam channels ([`BrewEvent`] and [`BrewCommand`]).
pub struct BrewWorker<T: NetworkTransport> {
    config: SharedConfig,
    brew_config: CfgBrew,
    /// Network transport (WebSocket, QUIC, TCP, …)
    transport: T,
    /// Send events to the BrewEntity
    event_sender: Sender<BrewEvent>,
    /// Receive commands from the BrewEntity
    command_receiver: Receiver<BrewCommand>,
    /// Registered subscribers and their affiliated groups (tracked from commands)
    subscriber_groups: HashMap<u32, HashSet<u32>>,
    /// Pending SDS transfers keyed by UUID, awaiting matching SDS_TRANSFER frame
    pending_sds: HashMap<Uuid, PendingSds>,
    /// Voice frames dropped because the entity channel was full, since the last warning
    voice_dropped: u64,
    /// When we last warned about dropped voice
    last_voice_drop_warn: Option<Instant>,
    /// Multi Network: visitors this connection registered under their home ISSI ((V)ASSI ->
    /// (home network, home ISSI)), kept until their deregistration is sent even when MM already
    /// forgot them.
    mni_visitors: HashMap<u32, (Mni, u32)>,
    /// `SERVICE_CALL_NETWORK` from the server, waiting for its call (uuid)
    mni_calls_in: HashMap<Uuid, (BrewCallNetwork, Instant)>,
    /// Group calls of another network running here: uuid -> ((V)GSSI, last over)
    mni_group_calls: HashMap<Uuid, (u32, Instant)>,
    /// Calls of visitors that stay inside the station (server without `mni`): nothing of them
    /// goes to the network
    mni_local_calls: HashSet<Uuid>,
}

/// How long a `SERVICE_CALL_NETWORK` waits for its call, and a group call of another network is
/// remembered after its last over.
const MNI_CALL_NOTE_TTL: Duration = Duration::from_secs(30);
const MNI_GROUP_CALL_TTL: Duration = Duration::from_secs(600);
const MNI_MAX_CALLS: usize = 4096;

impl<T: NetworkTransport> BrewWorker<T> {
    pub fn new(config: SharedConfig, event_sender: Sender<BrewEvent>, command_receiver: Receiver<BrewCommand>, transport: T) -> Self {
        let brew_config = config.config().brew.clone().unwrap(); // Never fails
        Self {
            config,
            brew_config,
            transport,
            event_sender,
            command_receiver,
            subscriber_groups: HashMap::new(),
            pending_sds: HashMap::new(),
            voice_dropped: 0,
            last_voice_drop_warn: None,
            mni_visitors: HashMap::new(),
            mni_calls_in: HashMap::new(),
            mni_group_calls: HashMap::new(),
            mni_local_calls: HashSet::new(),
        }
    }

    // ── Multi Network (`mni`) ──

    /// Multi Network on here and the server takes radios of other networks.
    fn mni_on(&self) -> bool {
        self.config.config().mni_enabled() && self.transport.server_brew_extensions().iter().any(|e| e == BREW_EXT_MNI)
    }

    /// The network this connection's cell broadcasts, when it is not the server's own (a cell
    /// with its own `[cells.net_info]`, or a whole station of another network): its radios and
    /// groups are that network's, and the server is told so (`mni`). `None` until the server
    /// named its network (`SERVICE_NETWORKS`), or when it is the same.
    fn mni_station_network(&self) -> Option<Mni> {
        if !self.config.config().mni_enabled() {
            return None;
        }
        let own = self.own_mni();
        let server = self.config.state_read().multi_network.server.as_ref().map(|(this, _)| *this)?;
        (own != server).then_some(own)
    }

    /// The home identity of a visitor's (V)ASSI; on a cell of another network than the
    /// server's, every radio is that network's under its own ISSI.
    fn mni_visitor(&self, ssi: u32) -> Option<(Mni, u32)> {
        if !self.config.config().mni_enabled() {
            return None;
        }
        self.config
            .state_read()
            .multi_network
            .visitors
            .get(&ssi)
            .copied()
            .or_else(|| self.mni_visitors.get(&ssi).copied())
            .or_else(|| self.mni_station_network().map(|n| (n, ssi)))
    }

    /// The home group behind a (V)GSSI; on a cell of another network than the server's, every
    /// group is that network's under its own GSSI.
    fn mni_home_group(&self, gssi: u32) -> Option<(Mni, u32)> {
        if !self.config.config().mni_enabled() {
            return None;
        }
        self.config
            .state_read()
            .multi_network
            .groups
            .get(&gssi)
            .copied()
            .or_else(|| self.mni_station_network().map(|n| (n, gssi)))
    }

    fn own_mni(&self) -> Mni {
        let cfg = self.config.config();
        Mni { mcc: cfg.net.mcc, mnc: cfg.net.mnc }
    }

    /// Visitor `ssi` towards the server: its home ISSI, or `None` when it has to stay inside the
    /// station (server without `mni`).
    fn mni_outbound_ssi(&self, ssi: u32) -> Option<u32> {
        match self.mni_visitor(ssi) {
            Some((_, home)) if self.mni_on() => Some(home),
            Some(_) => None,
            None => Some(ssi),
        }
    }

    fn send_logged(&mut self, msg: &[u8], what: &str) {
        if let Err(e) = self.transport.send_reliable(msg) {
            tracing::error!("BrewWorker: failed to send {}: {}", what, e);
        }
    }

    /// Groups of visitor `issi` (home network `home`) as the server knows them: a (V)GSSI becomes
    /// its home GSSI; the others are this network's groups, named in `SERVICE_GROUP_NETWORK`.
    fn mni_groups(&self, home: Mni, groups: &[u32]) -> (Vec<u32>, Vec<(u32, Mni)>) {
        let own = self.own_mni();
        let mut out = Vec::with_capacity(groups.len());
        let mut foreign = Vec::new();
        for &g in groups {
            match self.mni_home_group(g) {
                Some((mni, gssi)) => {
                    out.push(gssi);
                    if mni != home {
                        foreign.push((gssi, mni));
                    }
                }
                None => {
                    out.push(g);
                    foreign.push((g, own));
                }
            }
        }
        (out, foreign)
    }

    /// This cell turned out to be of another network than the server's: radios registered
    /// before that are moved over to it (deregistered as the server's, registered again as
    /// that network's with their groups).
    fn mni_reannounce_station(&mut self, net: Mni) {
        let state = self.config.state_read();
        let local: Vec<(u32, Vec<u32>)> = self
            .subscriber_groups
            .iter()
            .filter(|(issi, _)| !state.multi_network.visitors.contains_key(*issi) && !self.mni_visitors.contains_key(*issi))
            .map(|(issi, groups)| {
                let mut g: Vec<u32> = groups.iter().copied().collect();
                g.sort_unstable();
                (*issi, g)
            })
            .collect();
        drop(state);
        if !local.is_empty() {
            tracing::info!("BrewWorker: cell of network {}: {} radios registered again under it", net, local.len());
        }
        for (issi, groups) in local {
            self.send_logged(&build_subscriber_deregister(issi), "deregistration");
            self.send_logged(&build_service_ms_network(issi, net), "MS network");
            self.send_logged(&build_subscriber_register(issi, &[]), "registration");
            if !groups.is_empty() {
                self.send_logged(&build_subscriber_affiliate(issi, &groups), "affiliation");
            }
        }
    }

    fn mni_prune(&mut self) {
        let now = Instant::now();
        self.mni_calls_in.retain(|_, (_, t)| now.duration_since(*t) < MNI_CALL_NOTE_TTL);
        self.mni_group_calls.retain(|_, (_, t)| now.duration_since(*t) < MNI_GROUP_CALL_TTL);
        if self.mni_local_calls.len() > MNI_MAX_CALLS {
            self.mni_local_calls.clear();
        }
    }

    /// Hand an event to the entity over the bounded channel.
    ///
    /// Voice is lossy — a dropped ACELP frame is a click the jitter buffer hides — so it is
    /// discarded immediately when the entity is behind. Signalling is not lossy: it waits a
    /// short, bounded moment for room (which stops us draining the socket and pushes back on
    /// the peer) and is only dropped if the entity is still stuck after that.
    fn send_event(&mut self, event: BrewEvent) {
        let lossy = matches!(event, BrewEvent::VoiceFrame { .. });
        if offer_event(&self.event_sender, event, EVENT_SEND_BACKPRESSURE).is_err() {
            if lossy {
                self.voice_dropped += 1;
                let due = self
                    .last_voice_drop_warn
                    .is_none_or(|last| last.elapsed() >= VOICE_DROP_WARN_INTERVAL);
                if due {
                    tracing::warn!(
                        "BrewWorker: entity event channel full — dropped {} voice frames (core sending faster than the TDMA tick drains)",
                        self.voice_dropped
                    );
                    self.voice_dropped = 0;
                    self.last_voice_drop_warn = Some(Instant::now());
                }
            } else {
                tracing::error!("BrewWorker: entity event channel still full after backpressure — dropped a signalling event");
            }
        }
    }

    /// Main worker entry point — runs until disconnect or fatal error
    pub fn run(&mut self) {
        tracing::info!("BrewWorker: starting");

        loop {
            // A (re)connection to the core starts with a clean registration slate: the core may have
            // restarted while we were disconnected and no longer knows any of our subscribers.
            // Clearing this map makes the post-reconnect resync (BrewEntity::resync_subscribers) emit
            // REGISTER — not REREGISTER — for every subscriber, so a REREGISTER-for-unknown is never
            // sent to a fresh core (which can leave local MS unregistered until a power-cycle). The
            // entity re-drives all affiliations right after Connected, so no group state is lost.
            self.subscriber_groups.clear();
            self.mni_visitors.clear();
            self.mni_calls_in.clear();
            self.mni_group_calls.clear();
            self.mni_local_calls.clear();

            // Attempt connection via transport
            match self.transport.connect() {
                Ok(()) => {
                    tracing::info!("BrewWorker: transport connected");
                    let server_version = self.transport.server_brew_version();
                    {
                        let mut state = self.config.state_write();
                        state.brew_server_extensions = self.transport.server_brew_extensions().to_vec();
                        // A new list comes after the announcement, if the server offers one.
                        state.brew_cell_directory = None;
                        // Likewise the group rules (a server without `group-access`: all open).
                        state.brew_group_access.clear();
                        // And the network list (`mni`).
                        state.multi_network.server = None;
                    }
                    self.send_event(BrewEvent::Connected { server_version });
                    self.announce_cells();
                }
                Err(e) => {
                    tracing::error!(
                        "BrewWorker: connection error: {}, reconnecting in {:?}",
                        e,
                        self.brew_config.reconnect_delay
                    );
                    self.send_event(BrewEvent::Disconnected(e.to_string()));
                    std::thread::sleep(self.brew_config.reconnect_delay);
                    continue;
                }
            }

            // Run the message loop until error or clean shutdown
            match self.message_loop() {
                Ok(()) => {
                    tracing::info!("BrewWorker: connection closed normally");
                    break;
                }
                Err(e) => {
                    tracing::error!(
                        "BrewWorker: connection error: {}, reconnecting in {:?}",
                        e,
                        self.brew_config.reconnect_delay
                    );
                    self.send_event(BrewEvent::Disconnected(e));
                    std::thread::sleep(self.brew_config.reconnect_delay);
                }
            }
        }
    }

    /// Tell the server which cells this station serves, if it understands that (see
    /// [`bts_info_message`]).
    fn announce_cells(&mut self) {
        let Some(msg) = bts_info_message(&self.config.config(), self.transport.server_brew_extensions()) else {
            return;
        };
        if let Err(e) = self.transport.send_reliable(&msg) {
            tracing::error!("BrewWorker: failed to send BTS info: {}", e);
        } else {
            tracing::info!("BrewWorker: announced cells to the server (BTS info)");
        }
    }

    /// Graceful teardown: DEAFFILIATE → DEREGISTER → transport disconnect
    fn graceful_teardown(&mut self) {
        let subscribers: Vec<(u32, Vec<u32>)> = self
            .subscriber_groups
            .iter()
            .map(|(issi, groups)| (*issi, groups.iter().copied().collect()))
            .collect();
        for (local, groups) in subscribers {
            // A visitor under its home identities, or not at all (server without `mni`).
            let Some(issi) = self.mni_outbound_ssi(local) else { continue };
            let mut group_list = match self.mni_visitor(local) {
                Some((mni, _)) => self.mni_groups(mni, &groups).0,
                None => groups,
            };
            if !group_list.is_empty() {
                group_list.sort_unstable();
                let deaff_msg = build_subscriber_deaffiliate(issi, &group_list);
                if let Err(e) = self.transport.send_reliable(&deaff_msg) {
                    tracing::error!("BrewWorker: failed to send deaffiliation: {}", e);
                } else {
                    tracing::info!("BrewWorker: deaffiliated issi={} groups={:?}", issi, group_list);
                }
            }

            let dereg_msg = build_subscriber_deregister(issi);
            if let Err(e) = self.transport.send_reliable(&dereg_msg) {
                tracing::error!("BrewWorker: failed to send deregistration: {}", e);
            } else {
                tracing::info!("BrewWorker: deregistered ISSI {}", issi);
            }
        }
        self.transport.disconnect();
    }

    /// Main message processing loop (transport-agnostic)
    fn message_loop(&mut self) -> Result<(), String> {
        loop {
            let now = Instant::now();

            // Expire stale pending SDS entries (SHORT_TRANSFER without matching SDS_TRANSFER)
            self.pending_sds.retain(|uuid, pending| {
                let age = now.duration_since(pending.received_at);
                if age > Duration::from_secs(30) {
                    tracing::warn!("BrewWorker: expiring stale pending SDS uuid={}", uuid);
                    false
                } else {
                    true
                }
            });

            // ── Receive incoming messages from transport ──
            let messages = self.transport.receive_reliable();
            for msg in messages {
                self.handle_incoming_binary(&msg.payload);
            }

            // Check if transport is still connected (may have been dropped during receive)
            if !self.transport.is_connected() {
                return Err("transport disconnected".to_string());
            }

            // ── Check for commands from the BrewEntity ──
            loop {
                let cmd = match self.command_receiver.try_recv() {
                    Ok(cmd) => cmd,
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        // Entity was dropped — do graceful teardown
                        tracing::info!("BrewWorker: command channel closed, performing graceful teardown");
                        self.graceful_teardown();
                        return Ok(());
                    }
                };
                match cmd {
                    BrewCommand::RegisterSubscriber { issi } => {
                        let local = issi;
                        let issi = match self.mni_visitor(local) {
                            Some((mni, home)) => {
                                if !self.mni_on() {
                                    tracing::debug!("BrewWorker: visitor {} stays inside the station (server without mni)", local);
                                    continue;
                                }
                                self.mni_visitors.insert(local, (mni, home));
                                self.send_logged(&build_service_ms_network(home, mni), "MS network");
                                if local == home && self.mni_station_network() == Some(mni) {
                                    // Not a visitor: a radio of this cell's own network.
                                    tracing::info!("BrewWorker: radio {} of this cell's network registers as {}:{}", local, mni, home);
                                } else {
                                    tracing::info!("BrewWorker: visitor (V)ASSI {} registers as {}:{}", local, mni, home);
                                }
                                home
                            }
                            None => local,
                        };
                        let already_registered = self.subscriber_groups.contains_key(&local);
                        self.subscriber_groups.entry(local).or_insert_with(HashSet::new);
                        let msg = if already_registered {
                            build_subscriber_reregister(issi)
                        } else {
                            build_subscriber_register(issi, &[])
                        };
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send registration: {}", e);
                        } else {
                            tracing::debug!(
                                "BrewWorker: sent {} issi={}",
                                if already_registered { "REREGISTER" } else { "REGISTER" },
                                issi
                            );
                        }
                    }
                    BrewCommand::DeregisterSubscriber { issi } => {
                        self.subscriber_groups.remove(&issi);
                        let local = issi;
                        let Some(issi) = self.mni_outbound_ssi(local) else { continue };
                        self.mni_visitors.remove(&local);
                        let msg = build_subscriber_deregister(issi);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send deregistration: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent DEREGISTER issi={}", issi);
                        }
                    }
                    BrewCommand::AffiliateGroups { issi, groups } => {
                        let entry = self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                        for gssi in &groups {
                            entry.insert(*gssi);
                        }
                        let (issi, groups) = match self.mni_visitor(issi) {
                            Some((mni, home)) => {
                                if !self.mni_on() {
                                    continue;
                                }
                                let (groups, foreign) = self.mni_groups(mni, &groups);
                                if !foreign.is_empty() {
                                    self.send_logged(&build_service_group_network(home, &foreign), "group network");
                                }
                                (home, groups)
                            }
                            None => (issi, groups),
                        };
                        let msg = build_subscriber_affiliate(issi, &groups);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send affiliation: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent AFFILIATE issi={} groups={:?}", issi, groups);
                        }
                    }
                    BrewCommand::DeaffiliateGroups { issi, groups } => {
                        if let Some(entry) = self.subscriber_groups.get_mut(&issi) {
                            for gssi in &groups {
                                entry.remove(gssi);
                            }
                        }
                        let (issi, groups) = match self.mni_visitor(issi) {
                            Some((mni, home)) => {
                                if !self.mni_on() {
                                    continue;
                                }
                                (home, self.mni_groups(mni, &groups).0)
                            }
                            None => (issi, groups),
                        };
                        let msg = build_subscriber_deaffiliate(issi, &groups);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send deaffiliation: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent DEAFFILIATE issi={} groups={:?}", issi, groups);
                        }
                    }
                    BrewCommand::SendGroupTx {
                        uuid,
                        source_issi,
                        dest_gssi,
                        priority,
                        service,
                    } => {
                        self.mni_prune();
                        let group = self.mni_home_group(dest_gssi);
                        let visitor = self.mni_visitor(source_issi);
                        if (group.is_some() || visitor.is_some()) && !self.mni_on() {
                            tracing::debug!("BrewWorker: call {} of a visitor or visitor group stays inside the station", uuid);
                            self.mni_local_calls.insert(uuid);
                            continue;
                        }
                        let source_issi = visitor.map(|(_, home)| home).unwrap_or(source_issi);
                        let dest_gssi = match group {
                            Some((mni, gssi)) => {
                                self.send_logged(&build_service_call_network_group(&uuid, gssi, mni), "call network");
                                gssi
                            }
                            None => dest_gssi,
                        };
                        let msg = build_group_tx(&uuid, source_issi, dest_gssi, priority, service, None);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send GROUP_TX: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent GROUP_TX uuid={} src={} dst={}", uuid, source_issi, dest_gssi);
                        }
                    }
                    BrewCommand::SendVoiceFrame { uuid, length_bits, data } => {
                        if self.mni_local_calls.contains(&uuid) {
                            continue;
                        }
                        let msg = build_voice_frame(&uuid, length_bits, &data);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send voice frame: {}", e);
                        }
                    }
                    BrewCommand::SendGroupIdle { uuid, cause } => {
                        if self.mni_local_calls.remove(&uuid) {
                            continue;
                        }
                        let msg = build_group_idle(&uuid, cause);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send GROUP_IDLE: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent GROUP_IDLE uuid={} cause={}", uuid, cause);
                        }
                    }
                    BrewCommand::SendSds {
                        uuid,
                        source,
                        destination,
                        data,
                        length_bits,
                    } => {
                        if !net_brew::feature_sds_enabled(&self.config) {
                            tracing::warn!("BrewWorker: ignoring SendSds command because SDS over Brew is disabled in config");
                            continue;
                        }

                        let visitor = self.mni_visitor(source);
                        let source = match visitor {
                            Some((mni, home)) => {
                                if !self.mni_on() {
                                    continue;
                                }
                                // On a cell of another network an ISSI without network is one of it.
                                let called = self.mni_station_network();
                                self.send_logged(&build_service_call_network_parties(&uuid, Some(mni), called), "call network");
                                home
                            }
                            None => source,
                        };
                        // Send SHORT_TRANSFER first (header with source/dest)
                        let short_msg = build_short_transfer(&uuid, source, destination);
                        if let Err(e) = self.transport.send_reliable(&short_msg) {
                            tracing::error!("BrewWorker: failed to send SHORT_TRANSFER: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent SHORT_TRANSFER uuid={} src={} dst={}", uuid, source, destination);
                            // Then send SDS_TRANSFER with the payload
                            let sds_msg = build_sds_frame(&uuid, length_bits, &data);
                            if let Err(e) = self.transport.send_reliable(&sds_msg) {
                                tracing::error!("BrewWorker: failed to send SDS_TRANSFER: {}", e);
                            } else {
                                tracing::debug!("BrewWorker: sent SDS_TRANSFER uuid={} {} bytes", uuid, data.len());
                            }
                        }
                    }
                    BrewCommand::SendSdsReport { uuid, status } => {
                        if !net_brew::feature_sds_enabled(&self.config) {
                            tracing::warn!("BrewWorker: ignoring SendSdsReport command because SDS over Brew is disabled in config");
                            continue;
                        }

                        let msg = build_sds_report(&uuid, status);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::warn!("BrewWorker: failed to send SDS_REPORT: {}", e);
                        } else {
                            tracing::debug!("BrewWorker: sent SDS_REPORT uuid={} status={}", uuid, status);
                        }
                    }
                    BrewCommand::SendSetupRequest { uuid, mut call } => {
                        if let Some((mni, home)) = self.mni_visitor(call.source) {
                            if !self.mni_on() {
                                self.mni_local_calls.insert(uuid);
                                continue;
                            }
                            let called = self.mni_station_network();
                            self.send_logged(&build_service_call_network_parties(&uuid, Some(mni), called), "call network");
                            call.source = home;
                        }
                        let data = build_setup_request(&uuid, &call);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send SETUP_REQUEST: {}", e);
                        } else {
                            tracing::debug!("Brew: sent SETUP_REQUEST uuid={}", uuid);
                        }
                    }
                    BrewCommand::SendSetupAccept { uuid } => {
                        let data = build_setup_accept(&uuid);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send SETUP_ACCEPT: {}", e);
                        } else {
                            tracing::debug!("Brew: sent SETUP_ACCEPT uuid={}", uuid);
                        }
                    }
                    BrewCommand::SendSetupReject { uuid, cause } => {
                        let data = build_setup_reject(&uuid, cause);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send SETUP_REJECT: {}", e);
                        } else {
                            tracing::debug!("Brew: sent SETUP_REJECT uuid={} cause={}", uuid, cause);
                        }
                    }
                    BrewCommand::SendCallAlert { uuid } => {
                        let data = build_call_alert(&uuid);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send CALL_ALERT: {}", e);
                        } else {
                            tracing::debug!("Brew: sent CALL_ALERT uuid={}", uuid);
                        }
                    }
                    BrewCommand::SendConnectRequest { uuid, mut call } => {
                        if let Some(home) = self.mni_visitor(call.source).map(|v| v.1) {
                            call.source = home;
                        }
                        if let Some(home) = self.mni_visitor(call.destination).map(|v| v.1) {
                            call.destination = home;
                        }
                        let data = build_connect_request(&uuid, &call);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send CONNECT_REQUEST: {}", e);
                        } else {
                            tracing::debug!("Brew: sent CONNECT_REQUEST uuid={}", uuid);
                        }
                    }
                    BrewCommand::SendConnectConfirm { uuid, grant, permission } => {
                        let data = build_connect_confirm(&uuid, grant, permission);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send CONNECT_CONFIRM: {}", e);
                        } else {
                            tracing::debug!("Brew: sent CONNECT_CONFIRM uuid={} grant={} perm={}", uuid, grant, permission);
                        }
                    }
                    BrewCommand::SendSimplexGranted { uuid, grant, permission } => {
                        let data = build_simplex_granted(&uuid, grant, permission);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send SIMPLEX_GRANTED: {}", e);
                        } else {
                            tracing::debug!("Brew: sent SIMPLEX_GRANTED uuid={} grant={} perm={}", uuid, grant, permission);
                        }
                    }
                    BrewCommand::SendSimplexIdle { uuid, grant, permission } => {
                        let data = build_simplex_idle(&uuid, grant, permission);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send SIMPLEX_IDLE: {}", e);
                        } else {
                            tracing::debug!("Brew: sent SIMPLEX_IDLE uuid={} grant={} perm={}", uuid, grant, permission);
                        }
                    }
                    BrewCommand::SendCallRelease { uuid, cause } => {
                        let data = build_call_release(&uuid, cause);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("BrewWorker: failed to send CALL_RELEASE: {}", e);
                        } else {
                            tracing::debug!("Brew: sent CALL_RELEASE uuid={} cause={}", uuid, cause);
                        }
                    }
                    BrewCommand::SendDtmf { uuid, length_bits, data } => {
                        let msg = build_dtmf_frame(&uuid, length_bits, &data);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send DTMF: {}", e);
                        } else {
                            tracing::debug!("Brew: sent DTMF uuid={} bits={}", uuid, length_bits);
                        }
                    }
                    BrewCommand::ReportMsCell { issi, cell } => {
                        let Some(issi) = self.mni_outbound_ssi(issi) else { continue };
                        let Some(msg) = ms_cell_message(&self.config.config(), self.transport.server_brew_extensions(), issi, cell) else {
                            continue;
                        };
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send MS cell for ISSI {}: {}", issi, e);
                        } else {
                            tracing::debug!("BrewWorker: sent MS cell issi={} cell={}", issi, cell.0);
                        }
                    }
                    BrewCommand::ReportReselection { issi, reselection } => {
                        if !self.transport.server_brew_extensions().iter().any(|e| e == BREW_EXT_MS_RESELECTION) {
                            continue;
                        }
                        let msg = build_service_ms_reselection(issi, reselection);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send MS reselection for ISSI {}: {}", issi, e);
                        } else {
                            tracing::debug!("BrewWorker: sent MS reselection issi={} type={}", issi, reselection_name(reselection));
                        }
                    }
                    BrewCommand::SendRssiUpdate { issi, rssi_dbfs } => {
                        let msg = build_service_rssi(issi, rssi_dbfs);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send RSSI update for ISSI {}: {}", issi, e);
                        } else {
                            tracing::debug!("BrewWorker: sent RSSI issi={} rssi={:.1}dBFS", issi, rssi_dbfs);
                        }
                    }
                    BrewCommand::RequestCallRestore { issi, peer } => {
                        if !restores_calls(&self.config.config(), self.transport.server_brew_extensions()) {
                            self.send_event(BrewEvent::CallRestoreAnswer { issi, peer, call: None });
                            continue;
                        }
                        let msg = build_service_call_restore(issi, peer);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send call restore request for ISSI {}: {}", issi, e);
                            self.send_event(BrewEvent::CallRestoreAnswer { issi, peer, call: None });
                        } else {
                            tracing::info!("BrewWorker: asked the server for ISSI {}'s call with peer {}", issi, peer);
                        }
                    }
                    BrewCommand::SendCallHandoff { issi, peer, call } => {
                        if !hands_off_calls(&self.config.config(), self.transport.server_brew_extensions()) {
                            continue;
                        }
                        let msg = build_service_call_handoff(issi, peer, call.as_ref());
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to answer the call handoff for ISSI {}: {}", issi, e);
                        } else {
                            tracing::info!("BrewWorker: call handoff answer issi={} peer={} call={:?}", issi, peer, call);
                        }
                    }
                    BrewCommand::ReportRegisteredWithoutCall { issi } => {
                        if !restores_calls(&self.config.config(), self.transport.server_brew_extensions()) {
                            continue;
                        }
                        let msg = build_service_call_not_restored(issi);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to report ISSI {} registered without a call: {}", issi, e);
                        } else {
                            tracing::debug!("BrewWorker: ISSI {} registered by roaming, no call to restore", issi);
                        }
                    }
                    BrewCommand::SendHandoverResult(result) => {
                        if !handles_handover(&self.config.config(), self.transport.server_brew_extensions()) {
                            continue;
                        }
                        let msg = build_service_ho_result(&result);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send handover result for ISSI {}: {}", result.issi, e);
                        } else {
                            tracing::info!(
                                "BrewWorker: sent handover result issi={} transaction={} status={}",
                                result.issi,
                                result.transaction_id,
                                result.status
                            );
                        }
                    }
                    BrewCommand::SendHandoverRequest(req) => {
                        if !handles_handover(&self.config.config(), self.transport.server_brew_extensions()) {
                            self.send_event(BrewEvent::HandoverResult(BrewHoResult {
                                transaction_id: req.transaction_id,
                                issi: req.issi,
                                status: BREW_HO_STATUS_NO_TARGET,
                                reason: Some("not_capable".to_string()),
                                sdu: None,
                                channel: None,
                            }));
                            continue;
                        }
                        let msg = build_service_ho_request(&req);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("BrewWorker: failed to send handover request for ISSI {}: {}", req.issi, e);
                        } else {
                            tracing::info!(
                                "BrewWorker: sent handover request issi={} transaction={} target_carrier={}",
                                req.issi,
                                req.transaction_id,
                                req.target_carrier
                            );
                        }
                    }
                    BrewCommand::Disconnect => {
                        self.graceful_teardown();
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Parse an incoming binary Brew message and forward as event
    fn handle_incoming_binary(&mut self, data: &[u8]) {
        match parse_brew_message(data) {
            Ok(msg) => match msg {
                BrewMessage::CallControl(cc) => self.handle_call_control(cc),
                BrewMessage::Frame(frame) => self.handle_frame(frame),
                BrewMessage::Subscriber(sub) => {
                    tracing::debug!("BrewWorker: subscriber event type={}", sub.msg_type);
                    // TODO FIXME we could check whether this call is indeed a brew ssi here
                    self.send_event(BrewEvent::SubscriberEvent {
                        msg_type: sub.msg_type,
                        issi: sub.number,
                        groups: sub.groups,
                    });
                }
                BrewMessage::Error(err) => {
                    // Log the payload, not just its length: Brew echoes the offending request in
                    // the error (e.g. f1 03 = CALL_CONTROL/GROUP_IDLE + the 16-byte call UUID + 00),
                    // which is what you need to tie a rejection to a specific call (#48). Hex is the
                    // primary view; the lossy text is handy when the payload is actually a string.
                    tracing::warn!(
                        "BrewWorker: server error type={}: {} bytes: {:?} | hex={:02x?}",
                        err.error_type,
                        err.data.len(),
                        String::from_utf8_lossy(&err.data),
                        err.data
                    );
                    // TODO FIXME we could check whether this call is indeed a brew ssi here
                    self.send_event(BrewEvent::ServerError {
                        error_type: err.error_type,
                        data: err.data,
                    });
                }
                BrewMessage::Service(svc) => self.handle_service(svc),
            },
            Err(e) => {
                tracing::warn!("BrewWorker: failed to parse message ({} bytes): {}", data.len(), e);
            }
        }
    }

    /// Handle a service message. Only the handover messages are acted on, and only when this
    /// station announced `handover-v1` (the server sends them to no one else).
    fn handle_service(&mut self, svc: BrewServiceMessage) {
        match svc.service_type {
            BREW_SERVICE_HO_PREPARE | BREW_SERVICE_HO_FORWARD_REG | BREW_SERVICE_HO_RESULT
                if handles_handover(&self.config.config(), self.transport.server_brew_extensions()) =>
            {
                let parsed = if svc.service_type == BREW_SERVICE_HO_RESULT {
                    parse_ho_result(&svc.json_data).map(BrewEvent::HandoverResult)
                } else {
                    parse_ho_request(svc.service_type, &svc.json_data).map(BrewEvent::HandoverRequest)
                };
                match parsed {
                    Ok(event) => self.send_event(event),
                    Err(e) => tracing::warn!("BrewWorker: malformed handover message type={:#x}: {}", svc.service_type, e),
                }
            }
            BREW_SERVICE_CALL_RESTORE if restores_calls(&self.config.config(), self.transport.server_brew_extensions()) => {
                match parse_call_restore_answer(&svc.json_data) {
                    Ok((issi, peer, call)) => {
                        tracing::info!("BrewWorker: call restore answer issi={} peer={} call={:?}", issi, peer, call);
                        self.send_event(BrewEvent::CallRestoreAnswer { issi, peer, call });
                    }
                    Err(e) => tracing::warn!("BrewWorker: malformed call restore answer: {}", e),
                }
            }
            BREW_SERVICE_CALL_HANDOFF if hands_off_calls(&self.config.config(), self.transport.server_brew_extensions()) => {
                match parse_call_handoff_request(&svc.json_data) {
                    Ok((issi, peer)) => {
                        tracing::info!("BrewWorker: the server asks for the local call of ISSI {} with peer {}", issi, peer);
                        self.send_event(BrewEvent::CallHandoffRequest { issi, peer });
                    }
                    Err(e) => tracing::warn!("BrewWorker: malformed call handoff request: {}", e),
                }
            }
            BREW_SERVICE_MS_MOVED
                if handles_handover(&self.config.config(), self.transport.server_brew_extensions())
                    && self.transport.server_brew_extensions().iter().any(|e| e == BREW_EXT_MS_MOVED) =>
            {
                match parse_ms_moved(&svc.json_data) {
                    Ok(issi) => self.send_event(BrewEvent::MsMoved { issi }),
                    Err(e) => tracing::warn!("BrewWorker: malformed MS moved message: {}", e),
                }
            }
            BREW_SERVICE_CELL_DIRECTORY if self.transport.server_brew_extensions().iter().any(|e| e == BREW_EXT_CELL_DIRECTORY) => {
                match parse_cell_directory(&svc.json_data) {
                    Ok(cells) => {
                        tracing::info!("BrewWorker: cell directory from the server: {} cells", cells.len());
                        self.config.state_write().brew_cell_directory = Some(cells);
                    }
                    Err(e) => tracing::warn!("BrewWorker: malformed cell directory: {}", e),
                }
            }
            BREW_SERVICE_NETWORKS if self.mni_on() => match parse_networks(&svc.json_data) {
                Ok((this, networks)) => {
                    tracing::info!(
                        "BrewWorker: server network {} reaches {:?}",
                        this,
                        networks.iter().map(|n| format!("{} {} {}", n.name, n.mni, if n.up { "up" } else { "down" })).collect::<Vec<_>>()
                    );
                    let before = self.mni_station_network();
                    self.config.state_write().multi_network.server = Some((this, networks));
                    if before.is_none() {
                        if let Some(net) = self.mni_station_network() {
                            self.mni_reannounce_station(net);
                        }
                    }
                }
                Err(e) => tracing::warn!("BrewWorker: malformed networks message: {}", e),
            },
            BREW_SERVICE_CALL_NETWORK if self.mni_on() => match parse_call_network(&svc.json_data) {
                Ok((uuid, note)) => {
                    self.mni_prune();
                    if self.mni_calls_in.len() < MNI_MAX_CALLS {
                        self.mni_calls_in.insert(uuid, (note, Instant::now()));
                    }
                }
                Err(e) => tracing::warn!("BrewWorker: malformed call network message: {}", e),
            },
            BREW_SERVICE_GROUP_ACCESS if self.transport.server_brew_extensions().iter().any(|e| e == BREW_EXT_GROUP_ACCESS) => {
                match parse_group_access(&svc.json_data) {
                    Ok(groups) => {
                        tracing::info!("BrewWorker: group access rules from the server: {} groups limited", groups.len());
                        self.config.state_write().brew_group_access = groups;
                    }
                    Err(e) => tracing::warn!("BrewWorker: malformed group access message: {}", e),
                }
            }
            _ => tracing::debug!("BrewWorker: service type={}: {}", svc.service_type, svc.json_data),
        }
    }

    /// The (V)GSSI for an inbound GROUP_TX of call `uuid`: `None` = a group of this network,
    /// `Some(None)` = a group of another network no visitor here has (dropped).
    fn mni_inbound_group(&mut self, uuid: Uuid, gssi: u32) -> Option<Option<u32>> {
        if !self.mni_on() {
            return None;
        }
        if let Some((vgssi, at)) = self.mni_group_calls.get_mut(&uuid) {
            *at = Instant::now();
            return Some(Some(*vgssi));
        }
        // Without a note the group is of the server's own network (the server names only other
        // networks, and a server or terminal without `mni` names none). On a cell of another
        // network it must not go out as that network's group of the same number: only a migrated
        // radio of the server's network that attached it with its network has it, on a (V)GSSI.
        let server_group = |w: &mut Self| -> Option<Option<u32>> {
            w.mni_station_network()?;
            let vgssi = {
                let state = w.config.state_read();
                let (this, _) = state.multi_network.server.as_ref()?;
                state.multi_network.vgssi_of(*this, gssi)
            };
            match vgssi {
                Some(v) => {
                    w.mni_group_calls.insert(uuid, (v, Instant::now()));
                    Some(Some(v))
                }
                None => {
                    tracing::info!("BrewWorker: call {} on group {} of the server's network: not this cell's network, dropped", uuid, gssi);
                    Some(None)
                }
            }
        };
        let Some((note, _)) = self.mni_calls_in.remove(&uuid) else {
            return server_group(self);
        };
        let Some((g, mni)) = note.group else {
            return server_group(self);
        };
        if g != gssi {
            tracing::warn!("BrewWorker: call network names group {} but GROUP_TX {} is on {}", g, uuid, gssi);
        }
        let vgssi = self.config.state_read().multi_network.vgssi_of(mni, g);
        // A group of the network this cell broadcasts keeps its GSSI.
        let vgssi = vgssi.or_else(|| (self.mni_station_network() == Some(mni)).then_some(g));
        match vgssi {
            Some(v) => {
                tracing::info!("BrewWorker: call {} on group {}:{} = (V)GSSI {}", uuid, mni, g, v);
                self.mni_group_calls.insert(uuid, (v, Instant::now()));
                Some(Some(v))
            }
            None => {
                tracing::info!("BrewWorker: call {} on group {}:{}: no visitor here has it, dropped", uuid, mni, g);
                Some(None)
            }
        }
    }

    /// The (V)ASSI for an inbound individual call or SDS `uuid` to `called`: `None` = a radio of
    /// this network, `Some(None)` = a visitor that is not here (dropped).
    fn mni_inbound_called(&mut self, uuid: Uuid, called: u32) -> Option<Option<u32>> {
        if !self.mni_on() {
            return None;
        }
        // Without a note the called party is of the server's own network (the server names only
        // other networks). On a cell of another network a migrated radio of the server's network
        // is reached on its (V)ASSI; any other radio keeps its ISSI.
        let server_vassi = |w: &Self| {
            w.mni_station_network()?;
            let state = w.config.state_read();
            let (this, _) = state.multi_network.server.as_ref()?;
            state.multi_network.vassi_of(*this, called)
        };
        let Some((note, _)) = self.mni_calls_in.remove(&uuid) else {
            return server_vassi(self).map(Some);
        };
        if let Some(mni) = note.calling {
            tracing::info!("BrewWorker: call {} comes from network {}", uuid, mni);
        }
        let Some(mni) = note.called else {
            return server_vassi(self).map(Some);
        };
        let vassi = self.config.state_read().multi_network.vassi_of(mni, called);
        // A radio of the network this cell broadcasts keeps its ISSI.
        let vassi = vassi.or_else(|| (self.mni_station_network() == Some(mni)).then_some(called));
        if vassi.is_none() {
            tracing::info!("BrewWorker: call {} to {}:{}: no such visitor here, dropped", uuid, mni, called);
        }
        Some(vassi)
    }

    /// Handle a parsed call control message
    fn handle_call_control(&mut self, cc: BrewCallControlMessage) {
        match cc.call_state {
            CALL_STATE_GROUP_TX => {
                if let BrewCallPayload::GroupTransmission(gt) = cc.payload {
                    tracing::info!(
                        "BrewWorker: GROUP_TX uuid={} src={} dst={} prio={} service={} mnemonic={}",
                        cc.identifier,
                        gt.source,
                        gt.destination,
                        gt.priority,
                        gt.service,
                        gt.mnemonic.is_some()
                    );
                    // Detect server version from mnemonic presence (v1 includes 34-byte mnemonic)
                    if gt.mnemonic.is_some() {
                        self.send_event(BrewEvent::VersionDetected { version: 1 });
                    }
                    // Multi Network: a group of another network (announced by SERVICE_CALL_NETWORK,
                    // kept for the later overs of the call) is its (V)GSSI here.
                    let mut gt = gt;
                    if let Some(vgssi) = self.mni_inbound_group(cc.identifier, gt.destination) {
                        match vgssi {
                            Some(v) => gt.destination = v,
                            None => return,
                        }
                    }
                    // Inbound admission (FH-FEAT-032 R3): a Brew-originated group call must NOT be
                    // gated by the outbound-only `whitelisted_ssis`; only `local_ssi_ranges` reject it.
                    if !net_brew::is_brew_inbound_allowed(&self.config, gt.destination) {
                        tracing::warn!("BrewWorker: dropping GROUP_TX to inactive/local-only GSSI {}", gt.destination);
                        return;
                    };
                    self.send_event(BrewEvent::GroupCallStart {
                        uuid: cc.identifier,
                        source_issi: gt.source,
                        dest_gssi: gt.destination,
                        priority: gt.priority,
                        service: gt.service,
                    });
                }
            }
            CALL_STATE_GROUP_IDLE => {
                let cause = if let BrewCallPayload::Cause(c) = cc.payload { c } else { 0 };
                tracing::info!("BrewWorker: GROUP_IDLE uuid={} cause={}", cc.identifier, cause);
                // TODO FIXME we could check whether this call is indeed a brew call here
                self.send_event(BrewEvent::GroupCallEnd {
                    uuid: cc.identifier,
                    cause,
                });
            }

            CALL_STATE_SETUP_REQUEST => {
                if let BrewCallPayload::CircularCall(call) = cc.payload {
                    tracing::info!(
                        "BrewWorker: SETUP_REQUEST uuid={} src={} dst={} number='{}' duplex={}",
                        cc.identifier,
                        call.source,
                        call.destination,
                        call.number,
                        call.duplex
                    );
                    let mut call = call;
                    match self.mni_inbound_called(cc.identifier, call.destination) {
                        Some(Some(vassi)) => call.destination = vassi,
                        Some(None) => return,
                        None => {}
                    }
                    self.send_event(BrewEvent::CircuitSetupRequest { uuid: cc.identifier, call });
                }
            }
            CALL_STATE_SETUP_ACCEPT => {
                tracing::info!("BrewWorker: SETUP_ACCEPT uuid={}", cc.identifier);
                self.send_event(BrewEvent::CircuitSetupAccept { uuid: cc.identifier });
            }
            CALL_STATE_SETUP_REJECT => {
                let cause = if let BrewCallPayload::Cause(c) = cc.payload { c } else { 0 };
                tracing::info!("BrewWorker: SETUP_REJECT uuid={} cause={}", cc.identifier, cause);
                self.send_event(BrewEvent::CircuitSetupReject {
                    uuid: cc.identifier,
                    cause,
                });
            }
            CALL_STATE_CALL_ALERT => {
                tracing::info!("BrewWorker: CALL_ALERT uuid={}", cc.identifier);
                self.send_event(BrewEvent::CircuitCallAlert { uuid: cc.identifier });
            }
            CALL_STATE_CONNECT_REQUEST => {
                if let BrewCallPayload::CircularCall(call) = cc.payload {
                    tracing::info!(
                        "BrewWorker: CONNECT_REQUEST uuid={} src={} dst={} duplex={}",
                        cc.identifier,
                        call.source,
                        call.destination,
                        call.duplex
                    );
                    self.send_event(BrewEvent::CircuitConnectRequest { uuid: cc.identifier, call });
                }
            }
            CALL_STATE_CONNECT_CONFIRM => {
                let (grant, permission) = if let BrewCallPayload::CircularGrant(g) = cc.payload {
                    (g.grant, g.permission)
                } else {
                    (0, 0)
                };
                tracing::info!(
                    "BrewWorker: CONNECT_CONFIRM uuid={} grant={} perm={}",
                    cc.identifier,
                    grant,
                    permission
                );
                self.send_event(BrewEvent::CircuitConnectConfirm {
                    uuid: cc.identifier,
                    grant,
                    permission,
                });
            }
            CALL_STATE_SIMPLEX_GRANTED => {
                let (grant, permission) = if let BrewCallPayload::CircularGrant(g) = cc.payload {
                    (g.grant, g.permission)
                } else {
                    (0, 0)
                };
                tracing::info!(
                    "BrewWorker: SIMPLEX_GRANTED uuid={} grant={} perm={}",
                    cc.identifier,
                    grant,
                    permission
                );
                self.send_event(BrewEvent::CircuitSimplexGranted {
                    uuid: cc.identifier,
                    grant,
                    permission,
                });
            }
            CALL_STATE_SIMPLEX_IDLE => {
                let (grant, permission) = if let BrewCallPayload::CircularGrant(g) = cc.payload {
                    (g.grant, g.permission)
                } else {
                    (0, 0)
                };
                tracing::info!(
                    "BrewWorker: SIMPLEX_IDLE uuid={} grant={} perm={}",
                    cc.identifier,
                    grant,
                    permission
                );
                self.send_event(BrewEvent::CircuitSimplexIdle {
                    uuid: cc.identifier,
                    grant,
                    permission,
                });
            }
            CALL_STATE_CALL_RELEASE => {
                let cause = if let BrewCallPayload::Cause(c) = cc.payload { c } else { 0 };
                tracing::info!("BrewWorker: CALL_RELEASE uuid={} cause={}", cc.identifier, cause);
                // Send both events — entity will handle whichever is relevant
                self.send_event(BrewEvent::GroupCallEnd {
                    uuid: cc.identifier,
                    cause,
                });
                self.send_event(BrewEvent::CircuitCallRelease {
                    uuid: cc.identifier,
                    cause,
                });
            }
            CALL_STATE_SHORT_TRANSFER => {
                if let BrewCallPayload::ShortTransfer { source, destination } = cc.payload {
                    tracing::info!(
                        "BrewWorker: SHORT_TRANSFER uuid={} src={} dst={}",
                        cc.identifier,
                        source,
                        destination
                    );
                    let destination = match self.mni_inbound_called(cc.identifier, destination) {
                        Some(Some(vassi)) => vassi,
                        Some(None) => return,
                        None => destination,
                    };
                    // Stash for matching with upcoming SDS_TRANSFER frame
                    self.pending_sds.insert(
                        cc.identifier,
                        PendingSds {
                            source,
                            destination,
                            received_at: Instant::now(),
                        },
                    );
                }
            }
            state => {
                tracing::debug!("BrewWorker: unhandled call state {} uuid={}", state, cc.identifier);
            }
        }
    }

    /// Handle a parsed voice/data frame
    fn handle_frame(&mut self, frame: BrewFrameMessage) {
        match frame.frame_type {
            FRAME_TYPE_TRAFFIC_CHANNEL => {
                // Forward ACELP voice frame to entity
                // TODO FIXME we could check whether this call is indeed a brew call here
                self.send_event(BrewEvent::VoiceFrame {
                    uuid: frame.identifier,
                    length_bits: frame.length_bits,
                    data: frame.data,
                });
            }
            FRAME_TYPE_SDS_TRANSFER => {
                if !net_brew::feature_sds_enabled(&self.config) {
                    tracing::warn!("BrewWorker: ignoring incoming SDS_TRANSFER because SDS over Brew is disabled in config");
                    return;
                }

                if frame.length_bits > 2047 {
                    // TODO FIXME we could split into multiple SDS messages here
                    tracing::warn!(
                        "BrewWorker: ignoring SDS_TRANSFER with excessive length_bits={} ({} bytes)",
                        frame.length_bits,
                        frame.data.len()
                    );
                    return;
                }

                // Reject frames whose declared bit-length exceeds the actual payload:
                // length_bits and data are independent wire fields, and the downstream
                // D-SDS-DATA serializer indexes the payload by length_bits — an
                // over-claimed length would otherwise panic the base station (OOB).
                if (frame.length_bits as usize).div_ceil(8) > frame.data.len() {
                    tracing::warn!(
                        "BrewWorker: ignoring SDS_TRANSFER with length_bits={} exceeding payload of {} bytes",
                        frame.length_bits,
                        frame.data.len()
                    );
                    return;
                }

                // Match with pending SHORT_TRANSFER by UUID
                if let Some(pending) = self.pending_sds.remove(&frame.identifier) {
                    tracing::info!(
                        "BrewWorker: SDS_TRANSFER uuid={} src={} dst={} {} bytes",
                        frame.identifier,
                        pending.source,
                        pending.destination,
                        frame.data.len()
                    );
                    self.send_event(BrewEvent::SdsTransfer {
                        uuid: frame.identifier,
                        source: pending.source,
                        destination: pending.destination,
                        data: frame.data,
                        length_bits: frame.length_bits,
                    });
                } else {
                    tracing::warn!(
                        "BrewWorker: SDS_TRANSFER uuid={} without matching SHORT_TRANSFER, {} bytes",
                        frame.identifier,
                        frame.data.len()
                    );
                }
            }
            FRAME_TYPE_SDS_REPORT => {
                let status = if frame.data.is_empty() { 0 } else { frame.data[0] };
                tracing::debug!("BrewWorker: SDS_REPORT uuid={} status={}", frame.identifier, status);
                self.send_event(BrewEvent::SdsReport {
                    uuid: frame.identifier,
                    status,
                });
            }
            ft => {
                tracing::debug!("BrewWorker: unhandled frame type {} uuid={}", ft, frame.identifier);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::transports::mock::MockTransport;
    use crossbeam_channel::bounded;

    fn voice() -> BrewEvent {
        BrewEvent::VoiceFrame {
            uuid: Uuid::nil(),
            length_bits: 288,
            data: vec![0u8; 36],
        }
    }

    fn signalling() -> BrewEvent {
        BrewEvent::GroupCallEnd {
            uuid: Uuid::nil(),
            cause: 0,
        }
    }

    #[test]
    fn full_channel_drops_voice_instead_of_growing() {
        let (tx, rx) = bounded::<BrewEvent>(2);
        assert!(offer_event(&tx, voice(), Duration::ZERO).is_ok());
        assert!(offer_event(&tx, voice(), Duration::ZERO).is_ok());

        // Third frame has nowhere to go — it is dropped and the queue does NOT grow.
        assert!(offer_event(&tx, voice(), Duration::ZERO).is_err());
        assert_eq!(rx.len(), 2);
    }

    #[test]
    fn signalling_is_delivered_while_there_is_room() {
        let (tx, rx) = bounded::<BrewEvent>(1);
        assert!(offer_event(&tx, signalling(), Duration::ZERO).is_ok());
        // Full: with no backpressure window it is reported as undelivered rather than queued.
        assert!(offer_event(&tx, signalling(), Duration::ZERO).is_err());
        assert_eq!(rx.len(), 1);
    }

    // ── Multi Network (mni) ──

    const HOME: Mni = Mni { mcc: 901, mnc: 1234 };
    const VASSI: u32 = 0xF0_0000;
    const VGSSI: u32 = 0xF1_0000;
    const HOME_ISSI: u32 = 2320972;

    /// Worker with Multi Network on, one visitor (VASSI = HOME/2320972) with home group 9 as VGSSI.
    fn mni_worker(extensions: &[&str]) -> (BrewWorker<MockTransport>, crossbeam_channel::Receiver<BrewEvent>, Sender<BrewCommand>) {
        let cfg = config("\n[multi_network]\nenabled = true\nnetworks = [{ mcc = 901, mnc = 1234 }]\n");
        {
            let mut st = cfg.state_write();
            st.multi_network.visitors.insert(VASSI, (HOME, HOME_ISSI));
            st.multi_network.groups.insert(VGSSI, (HOME, 9));
        }
        let (event_tx, event_rx) = bounded(64);
        let (command_tx, command_rx) = bounded(64);
        let mut w = BrewWorker::new(cfg, event_tx, command_rx, MockTransport::new().with_brew_extensions(extensions));
        w.transport.connect().unwrap();
        (w, event_rx, command_tx)
    }

    fn run(w: &mut BrewWorker<MockTransport>, tx: &Sender<BrewCommand>, commands: Vec<BrewCommand>) -> Vec<Vec<u8>> {
        for c in commands {
            tx.send(c).unwrap();
        }
        tx.send(BrewCommand::Disconnect).unwrap();
        w.message_loop().unwrap();
        w.transport.sent_payloads().to_vec()
    }

    fn service_body(msg: &[u8]) -> serde_json::Value {
        serde_json::from_slice(&msg[2..msg.len() - 1]).unwrap()
    }

    fn visitor_commands(uuid: Uuid) -> Vec<BrewCommand> {
        vec![
            BrewCommand::RegisterSubscriber { issi: VASSI },
            BrewCommand::AffiliateGroups { issi: VASSI, groups: vec![VGSSI, 10] },
            BrewCommand::SendGroupTx { uuid, source_issi: VASSI, dest_gssi: VGSSI, priority: 0, service: 0 },
            BrewCommand::SendVoiceFrame { uuid, length_bits: 8, data: vec![1] },
            BrewCommand::DeregisterSubscriber { issi: VASSI },
        ]
    }

    /// Towards a server with `mni` a visitor is its home ISSI, a (V)GSSI its home group, each
    /// named by the messages of section 12b before the Brew message they belong to.
    #[test]
    fn visitor_goes_to_the_server_under_its_home_identities() {
        let uuid = Uuid::new_v4();
        let (mut w, _rx, tx) = mni_worker(&["bts-info", BREW_EXT_MNI]);
        let sent = run(&mut w, &tx, visitor_commands(uuid));
        assert_eq!((sent[0][0], sent[0][1]), (BREW_CLASS_SERVICE, BREW_SERVICE_MS_NETWORK));
        assert_eq!(service_body(&sent[0]), serde_json::json!({ "issi": HOME_ISSI, "mcc": 901, "mnc": 1234 }));
        assert_eq!(sent[1][..6], build_subscriber_register(HOME_ISSI, &[])[..6]);
        assert_eq!((sent[2][0], sent[2][1]), (BREW_CLASS_SERVICE, BREW_SERVICE_GROUP_NETWORK));
        assert_eq!(service_body(&sent[2]), serde_json::json!({ "issi": HOME_ISSI, "groups": [{ "gssi": 10, "mcc": 232, "mnc": 5 }] }));
        let aff = build_subscriber_affiliate(HOME_ISSI, &[9, 10]);
        assert_eq!((&sent[3][..6], &sent[3][18..]), (&aff[..6], &aff[18..]), "timestamp aside");
        assert_eq!(service_body(&sent[4])["group"], serde_json::json!({ "gssi": 9, "mcc": 901, "mnc": 1234 }));
        assert_eq!(sent[5], build_group_tx(&uuid, HOME_ISSI, 9, 0, 0, None));
        assert_eq!(sent[7][..6], build_subscriber_deregister(HOME_ISSI)[..6]);
    }

    /// Internal simulation: 10 stations with 25 radios each, every third one of this network, the
    /// others visitors with their own (V)ASSI. Each visitor is registered under its home ISSI
    /// after its `SERVICE_MS_NETWORK`, each own radio as it is.
    #[test]
    fn simulation_ten_stations_twenty_five_radios_register_under_the_right_identity() {
        for station in 0..10u32 {
            let (mut w, _rx, tx) = mni_worker(&["bts-info", BREW_EXT_MNI]);
            let mut expected = Vec::new();
            let mut commands = Vec::new();
            for radio in 0..25u32 {
                let home = 2_400_000 + station * 100 + radio;
                if radio % 3 == 2 {
                    commands.push(BrewCommand::RegisterSubscriber { issi: home });
                    expected.push((None, home));
                } else {
                    let vassi = 0xF0_0100 + radio * 8;
                    w.config.state_write().multi_network.visitors.insert(vassi, (HOME, home));
                    commands.push(BrewCommand::RegisterSubscriber { issi: vassi });
                    expected.push((Some(home), home));
                }
            }
            let sent = run(&mut w, &tx, commands);
            let mut it = sent.iter();
            for (note, issi) in expected {
                if let Some(home) = note {
                    let m = it.next().unwrap();
                    assert_eq!(service_body(m)["issi"], home);
                }
                let m = it.next().unwrap();
                assert_eq!(m[..6], build_subscriber_register(issi, &[])[..6]);
            }
        }
    }

    /// A server without `mni` hears nothing of a visitor; own radios work as before.
    #[test]
    fn visitor_stays_inside_the_station_without_mni() {
        let uuid = Uuid::new_v4();
        let (mut w, _rx, tx) = mni_worker(&["bts-info"]);
        let mut commands = visitor_commands(uuid);
        commands.push(BrewCommand::RegisterSubscriber { issi: 2320980 });
        let sent = run(&mut w, &tx, commands);
        let heads: Vec<&[u8]> = sent.iter().map(|m| &m[..6]).collect();
        let own = build_subscriber_register(2320980, &[]);
        let own_dereg = build_subscriber_deregister(2320980);
        assert_eq!(heads, vec![&own[..6], &own_dereg[..6]], "own radio registered, deregistered at teardown");
        assert!(bts_info_message(&w.config.config(), &["bts-info".to_string()]).is_some_and(|m| !String::from_utf8_lossy(&m).contains("\"mni\"")));
        let offered = bts_info_message(&w.config.config(), &["bts-info".to_string(), BREW_EXT_MNI.to_string()]).unwrap();
        assert!(String::from_utf8_lossy(&offered).contains("\"mni\""), "capability only where offered");
    }

    fn networks_message(this: Mni) -> Vec<u8> {
        let networks = serde_json::json!({ "this": { "mcc": this.mcc, "mnc": this.mnc }, "networks": [] });
        let mut svc = vec![BREW_CLASS_SERVICE, BREW_SERVICE_NETWORKS];
        svc.extend_from_slice(networks.to_string().as_bytes());
        svc.push(0);
        svc
    }

    /// A cell of another network than the server's (own `[cells.net_info]`, here the test
    /// station's 232/5 against a server of 901/9999): every radio and group is that network's.
    /// Radios registered before the server named its network are moved over; calls, SDS and
    /// affiliations are named with it; calls from the server keep their ISSI / GSSI.
    #[test]
    fn cell_of_another_network_names_its_radios_and_groups_with_it() {
        const OWN: Mni = Mni { mcc: 232, mnc: 5 };
        let cfg = config("\n[multi_network]\nenabled = true\n");
        let (event_tx, rx) = bounded(64);
        let (tx, command_rx) = bounded(64);
        let mut w = BrewWorker::new(cfg, event_tx, command_rx, MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_MNI]));
        w.transport.connect().unwrap();
        // Registered before the server named its network: plain.
        tx.send(BrewCommand::RegisterSubscriber { issi: 2320971 }).unwrap();
        tx.send(BrewCommand::AffiliateGroups { issi: 2320971, groups: vec![9] }).unwrap();
        tx.send(BrewCommand::Disconnect).unwrap();
        w.message_loop().unwrap();
        w.transport.connect().unwrap();
        let before = w.transport.sent_payloads().len();
        w.handle_incoming_binary(&networks_message(Mni { mcc: 901, mnc: 9999 }));
        let moved = w.transport.sent_payloads()[before..].to_vec();
        assert_eq!(moved[0][..6], build_subscriber_deregister(2320971)[..6]);
        assert_eq!(service_body(&moved[1]), serde_json::json!({ "issi": 2320971, "mcc": 232, "mnc": 5 }));
        assert_eq!(moved[2][..6], build_subscriber_register(2320971, &[])[..6]);
        let aff = build_subscriber_affiliate(2320971, &[9]);
        assert_eq!((&moved[3][..6], &moved[3][18..]), (&aff[..6], &aff[18..]));

        let uuid = Uuid::new_v4();
        let sds = Uuid::new_v4();
        let start = w.transport.sent_payloads().len();
        for c in [
            BrewCommand::RegisterSubscriber { issi: 2320972 },
            BrewCommand::AffiliateGroups { issi: 2320972, groups: vec![9] },
            BrewCommand::SendGroupTx { uuid, source_issi: 2320972, dest_gssi: 9, priority: 0, service: 0 },
            BrewCommand::SendSds { uuid: sds, source: 2320972, destination: 200999, data: vec![1, 2], length_bits: 16 },
        ] {
            tx.send(c).unwrap();
        }
        tx.send(BrewCommand::Disconnect).unwrap();
        w.message_loop().unwrap();
        let sent = w.transport.sent_payloads()[start..].to_vec();
        assert_eq!(service_body(&sent[0]), serde_json::json!({ "issi": 2320972, "mcc": 232, "mnc": 5 }));
        assert_eq!(sent[1][..6], build_subscriber_register(2320972, &[])[..6]);
        let aff = build_subscriber_affiliate(2320972, &[9]);
        assert_eq!((&sent[2][..6], &sent[2][18..]), (&aff[..6], &aff[18..]), "its groups are its network's: no group network");
        assert_eq!(service_body(&sent[3])["group"], serde_json::json!({ "gssi": 9, "mcc": 232, "mnc": 5 }));
        assert_eq!(sent[4], build_group_tx(&uuid, 2320972, 9, 0, 0, None));
        let parties = service_body(&sent[5]);
        assert_eq!(parties["calling"], serde_json::json!({ "mcc": 232, "mnc": 5 }));
        assert_eq!(parties["called"], serde_json::json!({ "mcc": 232, "mnc": 5 }), "an ISSI without network is one of this cell's");

        // From the server: a call of this network's group and to radio 2320972 keep their ids
        // (2_000_009: GSSI 9 is a local-only range in the test config).
        let call = Uuid::new_v4();
        w.handle_incoming_binary(&build_service_call_network_group(&call, 2_000_009, OWN));
        w.handle_incoming_binary(&build_group_tx(&call, 5551234, 2_000_009, 0, 0, None));
        let individual = Uuid::new_v4();
        w.handle_incoming_binary(&build_service_call_network_parties(&individual, Some(Mni { mcc: 901, mnc: 9999 }), Some(OWN)));
        let c = BrewCircularCall {
            source: 200999, destination: 2320972, number: String::new(), priority: 0, service: 0, mode: 0, duplex: 1,
            method: 0, communication: 0, grant: 0, permission: 0, timeout: 0, ownership: 0, queued: 0, mnemonic: None,
        };
        w.handle_incoming_binary(&build_setup_request(&individual, &c));
        let (mut groups, mut called) = (Vec::new(), Vec::new());
        while let Ok(e) = rx.try_recv() {
            match e {
                BrewEvent::GroupCallStart { dest_gssi, .. } => groups.push(dest_gssi),
                BrewEvent::CircuitSetupRequest { call, .. } => called.push(call.destination),
                _ => {}
            }
        }
        assert_eq!(groups, vec![2_000_009]);
        assert_eq!(called, vec![2320972]);
    }

    /// The server's own network: nothing changes (no network messages for own radios).
    #[test]
    fn cell_of_the_servers_network_sends_no_network_messages() {
        let cfg = config("\n[multi_network]\nenabled = true\n");
        let (event_tx, _rx) = bounded(64);
        let (tx, command_rx) = bounded(64);
        let mut w = BrewWorker::new(cfg, event_tx, command_rx, MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_MNI]));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&networks_message(Mni { mcc: 232, mnc: 5 }));
        let sent = run(&mut w, &tx, vec![BrewCommand::RegisterSubscriber { issi: 2320972 }]);
        assert!(sent.iter().all(|m| !(m[0] == BREW_CLASS_SERVICE && m[1] == BREW_SERVICE_MS_NETWORK)));
    }

    /// A cell of a network reached `via` the home network's Brew server (no `[multi_network]`):
    /// it offers `mni` and names its radios with its own network, so the server registers them
    /// there.
    #[test]
    fn cell_of_a_network_via_the_server_names_its_radios_with_it() {
        let text = CONFIG.replace("[brew]", "[networks.home.brew]");
        let text = format!(
            "network = \"home\"\n{text}\n[networks.home]\n[networks.tp]\nmcc = 901\nmnc = 1638\nvia = \"home\"\n\
             [[cells]]\nid = 1\nnetwork = \"tp\"\n[cells.cell_info]\nmain_carrier = 1532\n"
        );
        let station = tetra_config::bluestation::parsing::from_toml_str(&text).expect("test config parses");
        assert!(!station.mni_enabled(), "the home network's cell is unchanged");
        let cell = station.for_extra_cell(tetra_core::CellId(1)).unwrap();
        assert!(cell.mni_enabled() && cell.brew.as_ref().is_some_and(|b| b.host == "brew.example"));
        let offered = bts_info_message(&cell, &["bts-info".to_string(), BREW_EXT_MNI.to_string()]).unwrap();
        assert_eq!(json_of(&offered)["mnc"], 1638);
        assert!(json_of(&offered)["capabilities"].as_array().unwrap().iter().any(|c| c == BREW_EXT_MNI));
        assert_eq!(json_of(&offered)["cells"].as_array().unwrap().len(), 1);

        let (event_tx, _rx) = bounded(64);
        let (tx, command_rx) = bounded(64);
        let cfg = SharedConfig::from_parts(cell, None);
        let mut w = BrewWorker::new(cfg, event_tx, command_rx, MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_MNI]));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&networks_message(Mni { mcc: 232, mnc: 5 }));
        let sent = run(&mut w, &tx, vec![BrewCommand::RegisterSubscriber { issi: 2320972 }]);
        let named = sent.iter().find(|m| m[0] == BREW_CLASS_SERVICE && m[1] == BREW_SERVICE_MS_NETWORK).expect("MS_NETWORK sent");
        assert_eq!(service_body(named), serde_json::json!({ "issi": 2320972, "mcc": 901, "mnc": 1638 }));
    }

    /// On a cell of another network, a call from the server without a network note is for a
    /// radio of the server's own network: one that migrated here is reached on its (V)ASSI.
    #[test]
    fn call_for_a_migrated_radio_of_the_servers_network_reaches_its_vassi() {
        let text = CONFIG.replace("[brew]", "[networks.home.brew]");
        let text = format!(
            "network = \"home\"\n{text}\n[networks.home]\n[networks.tp]\nmcc = 901\nmnc = 1638\nvia = \"home\"\n\
             [[cells]]\nid = 1\nnetwork = \"tp\"\n[cells.cell_info]\nmain_carrier = 1532\n"
        );
        let station = tetra_config::bluestation::parsing::from_toml_str(&text).expect("test config parses");
        let cell = station.for_extra_cell(tetra_core::CellId(1)).unwrap();
        let (event_tx, rx) = bounded(64);
        let (_tx, command_rx) = bounded(64);
        let cfg = SharedConfig::from_parts(cell, None);
        let mut w = BrewWorker::new(cfg, event_tx, command_rx, MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_MNI]));
        w.transport.connect().unwrap();
        let server = Mni { mcc: 232, mnc: 5 };
        w.handle_incoming_binary(&networks_message(server));
        w.config.state_write().multi_network.visitors.insert(VASSI, (server, 2320972));
        for destination in [2320972, 2320973] {
            let c = BrewCircularCall {
                source: 2329999, destination, number: String::new(), priority: 0, service: 0, mode: 0, duplex: 1, method: 0,
                communication: 0, grant: 0, permission: 0, timeout: 0, ownership: 0, queued: 0, mnemonic: None,
            };
            w.handle_incoming_binary(&build_setup_request(&Uuid::new_v4(), &c));
        }
        let mut called = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let BrewEvent::CircuitSetupRequest { call, .. } = e {
                called.push(call.destination);
            }
        }
        assert_eq!(called, vec![VASSI, 2320973], "the migrated radio on its (V)ASSI, others unchanged");

        // A group call without a network note (e.g. from an app without `mni`) is of the
        // server's network: not sent out as the cell network's group 232; a migrated radio that
        // attached group 9 of the server's network gets it on its (V)GSSI.
        w.config.state_write().multi_network.groups.insert(VGSSI, (server, 9));
        let (leak, own) = (Uuid::new_v4(), Uuid::new_v4());
        w.handle_incoming_binary(&build_group_tx(&leak, 2329999, 232, 0, 0, None));
        w.handle_incoming_binary(&build_group_tx(&own, 2329999, 9, 0, 0, None));
        w.handle_incoming_binary(&build_group_tx(&own, 2329998, 9, 0, 0, None));
        let mut groups = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let BrewEvent::GroupCallStart { uuid, dest_gssi, .. } = e {
                groups.push((uuid, dest_gssi));
            }
        }
        assert_eq!(groups, vec![(own, VGSSI), (own, VGSSI)], "group 232 of the server's network not sent out here");
    }

    /// From the server: the network list is kept, a group call of the visitor's home group comes
    /// in on its (V)GSSI (later overs too), a call to the visitor on its (V)ASSI, and a call of
    /// another network's group nobody here has is dropped.
    #[test]
    fn server_calls_reach_visitors_on_their_local_identities() {
        let (mut w, rx, _tx) = mni_worker(&["bts-info", BREW_EXT_MNI]);
        let networks = serde_json::json!({ "this": { "mcc": 232, "mnc": 5 }, "networks": [{ "mcc": 901, "mnc": 1234, "name": "Home", "up": true }] });
        let mut svc = vec![BREW_CLASS_SERVICE, BREW_SERVICE_NETWORKS];
        svc.extend_from_slice(networks.to_string().as_bytes());
        svc.push(0);
        w.handle_incoming_binary(&svc);
        let server = w.config.state_read().multi_network.server.clone().expect("network list kept");
        assert_eq!(server.1[0].mni, HOME);
        assert!(server.1[0].up);

        let call = Uuid::new_v4();
        w.handle_incoming_binary(&build_service_call_network_group(&call, 9, HOME));
        w.handle_incoming_binary(&build_group_tx(&call, 5551234, 9, 0, 0, None));
        w.handle_incoming_binary(&build_group_tx(&call, 5551235, 9, 0, 0, None));
        let other = Uuid::new_v4();
        w.handle_incoming_binary(&build_service_call_network_group(&other, 77, HOME));
        w.handle_incoming_binary(&build_group_tx(&other, 5551234, 77, 0, 0, None));
        let own = Uuid::new_v4();
        w.handle_incoming_binary(&build_group_tx(&own, 5551234, 2_000_009, 0, 0, None));

        let individual = Uuid::new_v4();
        w.handle_incoming_binary(&build_service_call_network_parties(&individual, Some(HOME), Some(HOME)));
        let c = BrewCircularCall {
            source: 5551234,
            destination: HOME_ISSI,
            number: String::new(),
            priority: 0,
            service: 0,
            mode: 0,
            duplex: 1,
            method: 0,
            communication: 0,
            grant: 0,
            permission: 0,
            timeout: 0,
            ownership: 0,
            queued: 0,
            mnemonic: None,
        };
        w.handle_incoming_binary(&build_setup_request(&individual, &c));

        let mut groups = Vec::new();
        let mut called = Vec::new();
        while let Ok(e) = rx.try_recv() {
            match e {
                BrewEvent::GroupCallStart { uuid, dest_gssi, .. } => groups.push((uuid, dest_gssi)),
                BrewEvent::CircuitSetupRequest { call, .. } => called.push(call.destination),
                _ => {}
            }
        }
        assert_eq!(groups, vec![(call, VGSSI), (call, VGSSI), (own, 2_000_009)], "group 77 of the home network dropped");
        assert_eq!(called, vec![VASSI]);
    }

    const CONFIG: &str = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 232
mnc = 5

[cell_info]
main_carrier = 1528
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 10

[brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"
"#;

    fn config(extra: &str) -> SharedConfig {
        let cfg = tetra_config::bluestation::parsing::from_toml_str(&format!("{CONFIG}{extra}")).expect("test config parses");
        SharedConfig::from_parts(cfg, None)
    }

    fn json_of(msg: &[u8]) -> serde_json::Value {
        assert_eq!(&msg[..2], &[BREW_CLASS_SERVICE, BREW_SERVICE_BTS_INFO]);
        assert_eq!(msg.last(), Some(&0), "NULL-terminated");
        serde_json::from_slice(&msg[2..msg.len() - 1]).unwrap()
    }

    #[test]
    fn bts_info_lists_the_cell_with_its_absolute_band_plan() {
        let msg = bts_info_message(&config("").config(), &["bts-info".to_string()]).unwrap();
        // band 4, offset 0, duplex spacing 4, no reverse operation: (4 << 6) | (4 << 1)
        assert_eq!(
            json_of(&msg),
            serde_json::json!({
                "mcc": 232, "mnc": 5,
                "cells": [{ "cell": 0, "carrier": 1528, "carrier_ext": 264, "location_area": 10 }],
                "capabilities": []
            })
        );
    }

    #[test]
    fn bts_info_lists_every_cell_of_a_multi_cell_station() {
        let cfg = config("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1530\nlocation_area = 11\n");
        let msg = bts_info_message(&cfg.config(), &["bts-info".to_string()]).unwrap();
        let cells = json_of(&msg)["cells"].clone();
        assert_eq!(cells[0]["carrier"], 1528);
        assert_eq!(cells[1]["carrier"], 1530);
        assert_eq!(cells[1]["cell"], 3, "the [[cells]] id, not the position");
        assert_eq!(cells[1]["location_area"], 11);
        assert_eq!(cells[1]["carrier_ext"], 264);
    }

    #[test]
    fn ms_cell_names_the_cell_by_carrier_and_band_plan() {
        let cfg = config("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1530\nlocation_area = 11\n");
        let offered = ["bts-info".to_string(), "ms-cell".to_string()];
        let msg = ms_cell_message(&cfg.config(), &offered, 2320001, CellId(3)).unwrap();
        assert_eq!(&msg[..2], &[BREW_CLASS_SERVICE, BREW_SERVICE_MS_CELL]);
        assert_eq!(msg.last(), Some(&0), "NULL-terminated");
        let json: serde_json::Value = serde_json::from_slice(&msg[2..msg.len() - 1]).unwrap();
        assert_eq!(json, serde_json::json!({ "issi": 2320001, "carrier": 1530, "carrier_ext": 264 }));
        let primary = ms_cell_message(&cfg.config(), &offered, 7, CellId::PRIMARY).unwrap();
        assert!(String::from_utf8_lossy(&primary).contains("\"carrier\":1528"));
        assert!(ms_cell_message(&cfg.config(), &offered, 7, CellId(5)).is_none(), "no such cell");
    }

    #[test]
    fn ms_cell_only_for_a_server_that_offers_it() {
        let cfg = config("").config();
        assert!(
            ms_cell_message(&cfg, &[], 7, CellId::PRIMARY).is_none(),
            "TetraPack / BrandMeister offer no extensions"
        );
        assert!(
            ms_cell_message(&cfg, &["bts-info".to_string()], 7, CellId::PRIMARY).is_none(),
            "a server with bts-info only (milestone 2) does not get it"
        );
    }

    /// Runs the worker's message loop over `commands` (ending with a disconnect) and returns what
    /// it sent.
    fn run_commands(transport: MockTransport, commands: Vec<BrewCommand>) -> Vec<Vec<u8>> {
        let (event_tx, _event_rx) = bounded(16);
        let (command_tx, command_rx) = bounded(16);
        let mut worker = BrewWorker::new(config(""), event_tx, command_rx, transport);
        worker.transport.connect().unwrap();
        for c in commands {
            command_tx.send(c).unwrap();
        }
        command_tx.send(BrewCommand::Disconnect).unwrap();
        worker.message_loop().unwrap();
        worker.transport.sent_payloads().to_vec()
    }

    #[test]
    fn registration_is_followed_by_the_cell_only_where_offered() {
        let commands = || {
            vec![
                BrewCommand::RegisterSubscriber { issi: 7 },
                BrewCommand::ReportMsCell {
                    issi: 7,
                    cell: CellId::PRIMARY,
                },
            ]
        };
        let offered = run_commands(MockTransport::new().with_brew_extensions(&["bts-info", "ms-cell"]), commands());
        let kinds: Vec<(u8, u8)> = offered.iter().map(|p| (p[0], p[1])).collect();
        assert_eq!(kinds[0].0, BREW_CLASS_SUBSCRIBER);
        assert_eq!(kinds[1], (BREW_CLASS_SERVICE, BREW_SERVICE_MS_CELL));

        let plain = run_commands(MockTransport::new(), commands());
        assert!(
            plain.iter().all(|p| p[0] != BREW_CLASS_SERVICE),
            "nothing new towards a plain Brew server"
        );
        assert_eq!(plain.len(), offered.len() - 1, "otherwise the same messages");
    }

    #[test]
    fn reselection_reports_go_only_to_a_server_that_offers_them() {
        use tetra_saps::control::call_control::CellReselection;
        let commands = || {
            vec![
                BrewCommand::ReportReselection {
                    issi: 7,
                    reselection: CellReselection::AnnouncedType3,
                },
                BrewCommand::ReportReselection {
                    issi: 8,
                    reselection: CellReselection::Undeclared,
                },
            ]
        };
        let offered = run_commands(MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_MS_RESELECTION]), commands());
        assert_eq!(offered.len(), 2);
        assert_eq!((offered[0][0], offered[0][1]), (BREW_CLASS_SERVICE, BREW_SERVICE_MS_RESELECTION));
        let json = |m: &[u8]| serde_json::from_slice::<serde_json::Value>(&m[2..m.len() - 1]).unwrap();
        assert_eq!(json(&offered[0]), serde_json::json!({ "issi": 7, "type": "announced_type_3" }));
        assert_eq!(json(&offered[1]), serde_json::json!({ "issi": 8, "type": "undeclared" }));

        assert!(run_commands(MockTransport::new(), commands()).is_empty(), "nothing towards a plain Brew server");
        let older = run_commands(MockTransport::new().with_brew_extensions(&["bts-info", "handover-v1", "ms-cell", "ms-moved"]), commands());
        assert!(older.is_empty(), "nor towards a server without the extension");
    }

    #[test]
    fn bts_info_only_for_a_server_that_offers_it() {
        let cfg = config("").config();
        assert!(bts_info_message(&cfg, &[]).is_none(), "TetraPack / BrandMeister offer no extensions");
        assert!(bts_info_message(&cfg, &["something-else".to_string()]).is_none());
    }

    fn worker(transport: MockTransport) -> BrewWorker<MockTransport> {
        let (event_tx, _event_rx) = bounded(16);
        let (_command_tx, command_rx) = bounded(16);
        BrewWorker::new(config(""), event_tx, command_rx, transport)
    }

    #[test]
    fn connect_announces_cells_only_where_offered() {
        let mut offered = worker(MockTransport::new().with_brew_extensions(&["bts-info"]));
        offered.transport.connect().unwrap();
        offered.announce_cells();
        assert_eq!(offered.transport.sent_payloads().len(), 1);
        assert_eq!(json_of(&offered.transport.sent_payloads()[0])["cells"][0]["carrier"], 1528);

        let mut plain = worker(MockTransport::new());
        plain.transport.connect().unwrap();
        plain.announce_cells();
        assert!(plain.transport.sent_payloads().is_empty(), "nothing new towards a plain Brew server");
    }

    // ── Cell directory (cell-directory) ──

    const NEIGHBOUR: &str = "ms_txpwr_max_cell = 4\nrxlev_access_min_dbm = -100\n\n[[cell_info.neighbor_cells_ca]]\ncell_identifier_ca = 1\ncell_reselection_types_supported = 2\nneighbor_cell_synchronized = false\ncell_load_ca = 0\nmain_carrier_number = 1530\nlocation_area = 1\n\n[[cell_info.neighbor_cells_ca]]\ncell_identifier_ca = 2\ncell_reselection_types_supported = 2\nneighbor_cell_synchronized = false\ncell_load_ca = 0\nmain_carrier_number = 1600\nmain_carrier_number_extension = 280\nmcc = 901\nmnc = 9\n";

    /// `NEIGHBOUR` goes into `[cell_info]`, before `[brew]`.
    fn config_with_neighbours(extra: &str) -> SharedConfig {
        let base = CONFIG.replace("location_area = 10\n", &format!("location_area = 10\n{NEIGHBOUR}"));
        let mut cfg = tetra_config::bluestation::parsing::from_toml_str(&format!("{base}{extra}")).expect("test config parses");
        cfg.add_sibling_neighbours();
        SharedConfig::from_parts(cfg, None)
    }

    #[test]
    fn bts_info_reports_what_the_cells_broadcast_only_with_cell_directory() {
        let offered = ["bts-info".to_string(), BREW_EXT_CELL_DIRECTORY.to_string()];
        let msg = json_of(&bts_info_message(&config_with_neighbours("").config(), &offered).unwrap());
        assert_eq!(msg["capabilities"], serde_json::json!(["cell-directory"]));
        let cell = &msg["cells"][0];
        assert_eq!(cell["ms_txpwr_max_dbm"], 30, "code 4 = 30 dBm (table 18.57)");
        assert_eq!(cell["rxlev_access_min_dbm"], -100);
        assert_eq!(cell["service_details"]["registration"], true);
        assert_eq!(cell["service_details"]["system_wide_services"], true, "broadcast while the Brew link is configured");
        assert_eq!(cell["service_details"].as_object().unwrap().len(), 11);
        assert_eq!(
            cell["neighbours"],
            serde_json::json!([
                {"mcc": 232, "mnc": 5, "carrier": 1530, "carrier_ext": 264, "location_area": 1},
                {"mcc": 901, "mnc": 9, "carrier": 1600, "carrier_ext": 280, "location_area": 10},
            ]),
            "an element left out is the serving cell's own (table 18.64 notes 1, 2)"
        );

        // A second cell lists its sibling.
        let multi = config_with_neighbours("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1532\n");
        let msg = json_of(&bts_info_message(&multi.config(), &offered).unwrap());
        assert!(msg["cells"][1]["neighbours"].as_array().unwrap().iter().any(|n| n["carrier"] == 1528), "{msg}");

        // Not offered: the message stays as before.
        let plain = json_of(&bts_info_message(&config_with_neighbours("").config(), &["bts-info".to_string()]).unwrap());
        assert_eq!(plain["capabilities"], serde_json::json!([]));
        assert_eq!(plain["cells"][0], serde_json::json!({"cell": 0, "carrier": 1528, "carrier_ext": 264, "location_area": 10}));
    }

    fn cell_directory(json: &str) -> Vec<u8> {
        let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_CELL_DIRECTORY];
        msg.extend_from_slice(json.as_bytes());
        msg.push(0);
        msg
    }

    const DIRECTORY: &str = r#"{"cells":[
        {"id":"1000002/0","station":"1000002","cell":0,"online":true,"mcc":232,"mnc":5,"carrier":1530,"carrier_ext":264,"location_area":1,
         "rxlev_access_min_dbm":-100,"ms_txpwr_max_dbm":32,"neighbours":[{"mcc":232,"mnc":5,"carrier":1528,"carrier_ext":264}],"later":1},
        {"id":"1000003/0","station":"1000003","cell":0,"online":false,"last_seen_ms":5,"mcc":232,"mnc":5,"carrier":1532,"carrier_ext":264,"location_area":1},
        {"id":"bad","station":"x","cell":0,"online":true,"mcc":232,"mnc":5,"carrier":5000,"carrier_ext":264,"location_area":1},
        {"id":"also bad"}
    ]}"#;

    #[test]
    fn the_cell_directory_reaches_the_dashboard_state_only_where_offered() {
        let mut w = worker(MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_CELL_DIRECTORY]));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&cell_directory(DIRECTORY));
        let dir = w.config.state_read().brew_cell_directory.clone().expect("stored");
        assert_eq!(dir.iter().map(|c| (c.id.as_str(), c.online)).collect::<Vec<_>>(), [("1000002/0", true), ("1000003/0", false)]);
        assert_eq!(dir[0].rxlev_access_min_dbm, Some(-100));
        assert_eq!(dir[0].ms_txpwr_max_dbm, None, "32 dBm is no table 18.57 value");
        assert_eq!(dir[0].neighbours.as_ref().unwrap()[0].carrier, 1528);
        assert_eq!(dir[1].last_seen_ms, Some(5));
        w.handle_incoming_binary(&cell_directory("not json"));
        assert_eq!(w.config.state_read().brew_cell_directory.as_ref().map(|d| d.len()), Some(2), "a broken list keeps the last one");

        let mut plain = worker(MockTransport::new().with_brew_extensions(&["bts-info"]));
        plain.transport.connect().unwrap();
        plain.handle_incoming_binary(&cell_directory(DIRECTORY));
        assert!(plain.config.state_read().brew_cell_directory.is_none());
    }

    // ── Group access (group-access) ──

    fn group_access(json: &str) -> Vec<u8> {
        let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_GROUP_ACCESS];
        msg.extend_from_slice(json.as_bytes());
        msg.push(0);
        msg
    }

    const ACCESS: &str = r#"{"groups":[
        {"gssi":2321,"mode":"allow","issis":[2320972],"rx_only":false,"rx_only_issis":[]},
        {"gssi":2322,"mode":"block","issis":[4013],"rx_only":true,"rx_only_issis":[2320972]},
        {"gssi":0,"mode":"allow","issis":[1]},
        {"gssi":2323,"issis":[16777216]}
    ]}"#;

    #[test]
    fn group_access_is_announced_and_stored_only_where_offered() {
        let offered = ["bts-info".to_string(), BREW_EXT_GROUP_ACCESS.to_string()];
        let msg = json_of(&bts_info_message(&config("").config(), &offered).unwrap());
        assert_eq!(msg["capabilities"], serde_json::json!(["group-access"]));
        let plain = json_of(&bts_info_message(&config("").config(), &["bts-info".to_string()]).unwrap());
        assert_eq!(plain["capabilities"], serde_json::json!([]));

        let mut w = worker(MockTransport::new().with_brew_extensions(&["bts-info", BREW_EXT_GROUP_ACCESS]));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&group_access(ACCESS));
        {
            let state = w.config.state_read();
            assert_eq!(state.brew_group_access.keys().copied().collect::<Vec<_>>(), [2321, 2322], "bad entries stay open");
            assert!(state.group_may_join(2321, 2320972) && !state.group_may_join(2321, 4013));
            assert!(state.group_may_talk(2321, 2320972));
            assert!(!state.group_may_join(2322, 4013) && state.group_may_join(2322, 2320972));
            assert!(!state.group_may_talk(2322, 2320972) && state.group_may_talk(2322, 5), "listen only for the listed one");
            assert!(state.group_may_join(2323, 4013) && state.group_may_talk(9999, 4013), "no rule = open");
        }
        w.handle_incoming_binary(&group_access("not json"));
        assert_eq!(w.config.state_read().brew_group_access.len(), 2, "a broken message keeps the last table");
        w.handle_incoming_binary(&group_access(r#"{"groups":[]}"#));
        assert!(w.config.state_read().brew_group_access.is_empty(), "an empty table opens every group");

        let mut plain = worker(MockTransport::new().with_brew_extensions(&["bts-info"]));
        plain.transport.connect().unwrap();
        plain.handle_incoming_binary(&group_access(ACCESS));
        assert!(plain.config.state_read().brew_group_access.is_empty(), "not offered: ignored");
    }

    // ── Handover messages (target side) ──

    const HANDOVER: [&str; 3] = ["bts-info", "handover-v1", "ms-cell"];

    #[test]
    fn bts_info_announces_handover_only_where_offered() {
        let offered: Vec<String> = HANDOVER.iter().map(|e| e.to_string()).collect();
        let single = bts_info_message(&config("").config(), &offered).unwrap();
        assert_eq!(json_of(&single)["capabilities"], serde_json::json!(["handover-v1"]));

        let no_ho = bts_info_message(&config("").config(), &["bts-info".to_string(), "ms-cell".to_string()]).unwrap();
        assert_eq!(json_of(&no_ho)["capabilities"], serde_json::json!([]), "milestone 3 server: no capability");

        let multi = config("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1530\nlocation_area = 11\n");
        assert!(multi.config().is_site_linked());
        let multi = bts_info_message(&multi.config(), &offered).unwrap();
        assert_eq!(json_of(&multi)["capabilities"], serde_json::json!(["handover-v1"]), "a multi-cell station is a target too");
    }

    #[test]
    fn bts_info_asks_for_ms_moved_only_where_offered() {
        let mut offered: Vec<String> = HANDOVER.iter().map(|e| e.to_string()).collect();
        offered.push(BREW_EXT_MS_MOVED.to_string());
        let single = bts_info_message(&config("").config(), &offered).unwrap();
        assert_eq!(json_of(&single)["capabilities"], serde_json::json!(["handover-v1", "ms-moved"]));

        let multi = config("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1530\nlocation_area = 11\n");
        let multi = bts_info_message(&multi.config(), &offered).unwrap();
        assert_eq!(json_of(&multi)["capabilities"], serde_json::json!(["handover-v1", "ms-moved"]), "multi-cell too");

        offered.retain(|e| e != BREW_EXT_HANDOVER);
        let no_ho = bts_info_message(&config("").config(), &offered).unwrap();
        assert_eq!(json_of(&no_ho)["capabilities"], serde_json::json!([]), "only with handover-v1");
    }

    fn ms_moved(json: &str) -> Vec<u8> {
        let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_MS_MOVED];
        msg.extend_from_slice(json.as_bytes());
        msg.push(0);
        msg
    }

    #[test]
    fn ms_moved_reaches_the_entity_only_where_offered() {
        let mut offered: Vec<&str> = HANDOVER.to_vec();
        offered.push(BREW_EXT_MS_MOVED);
        let (mut w, events) = worker_with_events(MockTransport::new().with_brew_extensions(&offered));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&ms_moved(r#"{"issi":2320001,"later":true}"#));
        assert!(matches!(events.try_recv(), Ok(BrewEvent::MsMoved { issi: 2320001 })));
        for bad in [r#"{"issi":"x"}"#, r#"{"issi":16777216}"#, r#"{}"#, "not json"] {
            w.handle_incoming_binary(&ms_moved(bad));
            assert!(events.try_recv().is_err(), "malformed {bad} is dropped");
        }

        // A server that did not offer ms-moved cannot have meant this station.
        let (mut plain, events) = worker_with_events(MockTransport::new().with_brew_extensions(&HANDOVER));
        plain.transport.connect().unwrap();
        plain.handle_incoming_binary(&ms_moved(r#"{"issi":2320001}"#));
        assert!(events.try_recv().is_err());
    }

    // ── Individual call restore (call-restore) ──

    fn call_restore(json: &str) -> Vec<u8> {
        let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_CALL_RESTORE];
        msg.extend_from_slice(json.as_bytes());
        msg.push(0);
        msg
    }

    #[test]
    fn bts_info_asks_for_call_restore_only_where_offered() {
        let mut offered: Vec<String> = HANDOVER.iter().map(|e| e.to_string()).collect();
        offered.push(BREW_EXT_MS_MOVED.to_string());
        offered.push(BREW_EXT_CALL_RESTORE.to_string());
        let single = bts_info_message(&config("").config(), &offered).unwrap();
        assert_eq!(json_of(&single)["capabilities"], serde_json::json!(["handover-v1", "ms-moved", "call-restore"]));
        let multi = config("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1530\nlocation_area = 11\n");
        let multi = bts_info_message(&multi.config(), &offered).unwrap();
        assert_eq!(json_of(&multi)["capabilities"], serde_json::json!(["handover-v1", "ms-moved", "call-restore"]), "multi-cell too");
    }

    #[test]
    fn a_call_restore_request_goes_only_to_a_server_that_moves_calls() {
        let commands = || vec![BrewCommand::RequestCallRestore { issi: 2320972, peer: 2320997 }];
        let offered = run_commands(
            MockTransport::new().with_brew_extensions(&["bts-info", "handover-v1", "ms-moved", BREW_EXT_CALL_RESTORE]),
            commands(),
        );
        assert_eq!(offered.len(), 1);
        assert_eq!((offered[0][0], offered[0][1]), (BREW_CLASS_SERVICE, BREW_SERVICE_CALL_RESTORE));
        let json = serde_json::from_slice::<serde_json::Value>(&offered[0][2..offered[0].len() - 1]).unwrap();
        assert_eq!(json, serde_json::json!({ "issi": 2320972, "peer": 2320997 }));
        assert!(run_commands(MockTransport::new().with_brew_extensions(&["bts-info", "handover-v1", "ms-moved"]), commands()).is_empty());
        assert!(run_commands(MockTransport::new(), commands()).is_empty());
    }

    #[test]
    fn a_registration_without_a_call_is_reported_only_to_a_server_that_moves_calls() {
        let commands = || vec![BrewCommand::ReportRegisteredWithoutCall { issi: 2320972 }];
        let offered = run_commands(
            MockTransport::new().with_brew_extensions(&["bts-info", "handover-v1", "ms-moved", BREW_EXT_CALL_RESTORE]),
            commands(),
        );
        assert_eq!(offered.len(), 1);
        assert_eq!((offered[0][0], offered[0][1]), (BREW_CLASS_SERVICE, BREW_SERVICE_CALL_NOT_RESTORED));
        let json = serde_json::from_slice::<serde_json::Value>(&offered[0][2..offered[0].len() - 1]).unwrap();
        assert_eq!(json, serde_json::json!({ "issi": 2320972 }));
        assert!(run_commands(MockTransport::new().with_brew_extensions(&["bts-info", "handover-v1", "ms-moved"]), commands()).is_empty());
        assert!(run_commands(MockTransport::new(), commands()).is_empty());
    }

    #[test]
    fn without_call_restore_the_worker_answers_no_call_itself() {
        let (event_tx, event_rx) = bounded(16);
        let (command_tx, command_rx) = bounded(16);
        let mut worker = BrewWorker::new(config(""), event_tx, command_rx, MockTransport::new().with_brew_extensions(&HANDOVER));
        worker.transport.connect().unwrap();
        command_tx.send(BrewCommand::RequestCallRestore { issi: 7, peer: 8 }).unwrap();
        command_tx.send(BrewCommand::Disconnect).unwrap();
        worker.message_loop().unwrap();
        let answer = std::iter::from_fn(|| event_rx.try_recv().ok()).find(|e| matches!(e, BrewEvent::CallRestoreAnswer { .. }));
        assert!(matches!(answer, Some(BrewEvent::CallRestoreAnswer { issi: 7, peer: 8, call: None })));
    }

    #[test]
    fn call_restore_answers_reach_the_entity_only_where_offered() {
        let mut offered: Vec<&str> = HANDOVER.to_vec();
        offered.push(BREW_EXT_CALL_RESTORE);
        let (mut w, events) = worker_with_events(MockTransport::new().with_brew_extensions(&offered));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&call_restore(
            r#"{"issi":2320972,"peer":2320997,"uuid":"57c22f65-c8cb-4008-ac84-6fb79023888f","duplex":false,"priority":2,"floor":2320997,"later":1}"#,
        ));
        let Ok(BrewEvent::CallRestoreAnswer { issi, peer, call: Some(call) }) = events.try_recv() else { panic!("call expected") };
        assert_eq!((issi, peer), (2320972, 2320997));
        assert_eq!(call.brew_uuid.to_string(), "57c22f65-c8cb-4008-ac84-6fb79023888f");
        assert_eq!((call.simplex_duplex, call.priority, call.floor_issi), (false, 2, Some(2320997)));
        w.handle_incoming_binary(&call_restore(r#"{"issi":2320972,"peer":2320997,"uuid":"57c22f65-c8cb-4008-ac84-6fb79023888f","duplex":true,"priority":0,"floor":0}"#));
        assert!(matches!(events.try_recv(), Ok(BrewEvent::CallRestoreAnswer { call: Some(c), .. }) if c.floor_issi.is_none() && c.simplex_duplex));
        w.handle_incoming_binary(&call_restore(r#"{"issi":2320972,"peer":2320997}"#));
        assert!(matches!(events.try_recv(), Ok(BrewEvent::CallRestoreAnswer { issi: 2320972, peer: 2320997, call: None })));
        for bad in [r#"{"peer":1}"#, r#"{"issi":1,"uuid":"nope","duplex":true}"#, r#"{"issi":1,"uuid":"57c22f65-c8cb-4008-ac84-6fb79023888f"}"#, "x"] {
            w.handle_incoming_binary(&call_restore(bad));
            assert!(events.try_recv().is_err(), "malformed {bad} is dropped");
        }
        let (mut plain, events) = worker_with_events(MockTransport::new().with_brew_extensions(&HANDOVER));
        plain.transport.connect().unwrap();
        plain.handle_incoming_binary(&call_restore(r#"{"issi":2320972,"peer":2320997}"#));
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn call_handoff_is_announced_and_used_only_where_offered() {
        const RESTORE: [&str; 4] = ["bts-info", "handover-v1", "ms-moved", BREW_EXT_CALL_RESTORE];
        let mut offered: Vec<String> = RESTORE.iter().map(|e| e.to_string()).collect();
        offered.push(BREW_EXT_CALL_HANDOFF.to_string());
        let info = bts_info_message(&config("").config(), &offered).unwrap();
        assert_eq!(json_of(&info)["capabilities"], serde_json::json!(["handover-v1", "ms-moved", "call-restore", "call-handoff"]));
        // Without call-restore the station restores nothing, so it hands nothing off either.
        let no_restore: Vec<String> = ["bts-info", "handover-v1", BREW_EXT_CALL_HANDOFF].iter().map(|e| e.to_string()).collect();
        let info = bts_info_message(&config("").config(), &no_restore).unwrap();
        assert_eq!(json_of(&info)["capabilities"], serde_json::json!(["handover-v1"]));
        // A multi-cell station restores calls, but its site switch does not route a handoff yet.
        let multi = config("\n[[cells]]\nid = 3\n[cells.cell_info]\nmain_carrier = 1530\nlocation_area = 11\n");
        let info = bts_info_message(&multi.config(), &offered).unwrap();
        assert_eq!(json_of(&info)["capabilities"], serde_json::json!(["handover-v1", "ms-moved", "call-restore"]));

        // The answer goes out only to a server that offered the extension.
        let call = tetra_saps::control::call_control::HandedOffCall {
            brew_uuid: Uuid::from_u128(9),
            calling_issi: 2320997,
            simplex_duplex: true,
            priority: 0,
            floor_issi: None,
        };
        let commands = || vec![BrewCommand::SendCallHandoff { issi: 2320972, peer: 2320997, call: Some(call) }];
        let mut with: Vec<&str> = RESTORE.to_vec();
        with.push(BREW_EXT_CALL_HANDOFF);
        let sent = run_commands(MockTransport::new().with_brew_extensions(&with), commands());
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0][0], sent[0][1]), (BREW_CLASS_SERVICE, BREW_SERVICE_CALL_HANDOFF));
        let json = serde_json::from_slice::<serde_json::Value>(&sent[0][2..sent[0].len() - 1]).unwrap();
        assert_eq!(json["uuid"], Uuid::from_u128(9).to_string());
        assert_eq!(json["calling"], 2320997);
        assert!(run_commands(MockTransport::new().with_brew_extensions(&RESTORE), commands()).is_empty());

        // Requests reach the entity only from a server that offered it.
        let request = |json: &str| {
            let mut msg = vec![BREW_CLASS_SERVICE, BREW_SERVICE_CALL_HANDOFF];
            msg.extend_from_slice(json.as_bytes());
            msg.push(0);
            msg
        };
        let (mut w, events) = worker_with_events(MockTransport::new().with_brew_extensions(&with));
        w.transport.connect().unwrap();
        w.handle_incoming_binary(&request(r#"{"issi":2320972,"peer":2320997}"#));
        assert!(matches!(events.try_recv(), Ok(BrewEvent::CallHandoffRequest { issi: 2320972, peer: 2320997 })));
        w.handle_incoming_binary(&request(r#"{"issi":2320972}"#));
        assert!(events.try_recv().is_err(), "malformed request dropped");
        let (mut plain, events) = worker_with_events(MockTransport::new().with_brew_extensions(&RESTORE));
        plain.transport.connect().unwrap();
        plain.handle_incoming_binary(&request(r#"{"issi":2320972,"peer":2320997}"#));
        assert!(events.try_recv().is_err());
    }

    fn worker_with_events(transport: MockTransport) -> (BrewWorker<MockTransport>, Receiver<BrewEvent>) {
        let (event_tx, event_rx) = bounded(16);
        let (_command_tx, command_rx) = bounded(16);
        (BrewWorker::new(config(""), event_tx, command_rx, transport), event_rx)
    }

    fn forward_reg() -> BrewHoRequest {
        BrewHoRequest {
            transaction_id: 77,
            issi: 2320001,
            target_carrier: 1528,
            target_carrier_ext: 264,
            target_mcc: Some(232),
            target_mnc: Some(5),
            target_location_area: Some(10),
            sdu: Some(BrewHoSdu { bytes: vec![0xa1, 0xb2, 0xc0], len_bits: 18 }),
            call_gssi: None,
        }
    }

    #[test]
    fn handover_request_reaches_the_entity_only_where_offered() {
        let (mut offered, events) = worker_with_events(MockTransport::new().with_brew_extensions(&HANDOVER));
        offered.transport.connect().unwrap();
        offered.handle_incoming_binary(&build_service_ho_request(&forward_reg()));
        match events.try_recv() {
            Ok(BrewEvent::HandoverRequest(req)) => assert_eq!(req, forward_reg()),
            other => panic!("expected the handover request, got {other:?}"),
        }

        // A server that never offered handover-v1 cannot have meant this station.
        let (mut plain, events) = worker_with_events(MockTransport::new().with_brew_extensions(&["bts-info", "ms-cell"]));
        plain.transport.connect().unwrap();
        plain.handle_incoming_binary(&build_service_ho_request(&forward_reg()));
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn malformed_handover_request_is_dropped() {
        let (mut w, events) = worker_with_events(MockTransport::new().with_brew_extensions(&HANDOVER));
        w.transport.connect().unwrap();
        let mut bad = vec![BREW_CLASS_SERVICE, BREW_SERVICE_HO_FORWARD_REG];
        let json = r#"{"transaction_id":1,"issi":5,"target_carrier":1528,"target_carrier_ext":264,"sdu":"a1","sdu_len_bits":18}"#;
        bad.extend_from_slice(json.as_bytes());
        bad.push(0);
        w.handle_incoming_binary(&bad);
        assert!(events.try_recv().is_err(), "sdu shorter than sdu_len_bits");
    }

    #[test]
    fn handover_result_is_sent_only_where_offered() {
        let result = || {
            vec![BrewCommand::SendHandoverResult(BrewHoResult {
                transaction_id: 77,
                issi: 2320001,
                status: BREW_HO_STATUS_OK,
                reason: None,
                sdu: Some(BrewHoSdu { bytes: vec![0x0f, 0x80], len_bits: 9 }),
                channel: None,
            })]
        };
        let sent = run_commands(MockTransport::new().with_brew_extensions(&HANDOVER), result());
        let msg = sent.iter().find(|p| p[..2] == [BREW_CLASS_SERVICE, BREW_SERVICE_HO_RESULT]).expect("HO_RESULT sent");
        let json: serde_json::Value = serde_json::from_slice(&msg[2..msg.len() - 1]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "transaction_id": 77, "issi": 2320001, "status": 0, "sdu": "0f80", "sdu_len_bits": 9 })
        );

        let plain = run_commands(MockTransport::new(), result());
        assert!(plain.iter().all(|p| p[0] != BREW_CLASS_SERVICE), "never towards TetraPack / BrandMeister");
    }

    // ── Handover messages (serving side) ──

    #[test]
    fn handover_request_is_sent_only_where_offered_else_answered_not_capable() {
        let sent = run_commands(
            MockTransport::new().with_brew_extensions(&HANDOVER),
            vec![BrewCommand::SendHandoverRequest(forward_reg())],
        );
        assert!(sent.contains(&build_service_ho_request(&forward_reg())), "HO_FORWARD_REG sent");

        let (event_tx, event_rx) = bounded(16);
        let (command_tx, command_rx) = bounded(16);
        let mut plain = BrewWorker::new(config(""), event_tx, command_rx, MockTransport::new());
        plain.transport.connect().unwrap();
        command_tx.send(BrewCommand::SendHandoverRequest(forward_reg())).unwrap();
        command_tx.send(BrewCommand::Disconnect).unwrap();
        plain.message_loop().unwrap();
        assert!(plain.transport.sent_payloads().iter().all(|p| p[0] != BREW_CLASS_SERVICE), "never towards TetraPack / BrandMeister");
        let answer = std::iter::from_fn(|| event_rx.try_recv().ok()).find_map(|e| match e {
            BrewEvent::HandoverResult(r) => Some(r),
            _ => None,
        });
        assert_eq!(
            answer,
            Some(BrewHoResult {
                transaction_id: 77,
                issi: 2320001,
                status: BREW_HO_STATUS_NO_TARGET,
                reason: Some("not_capable".into()),
                sdu: None,
                channel: None,
            }),
            "answered locally, so MLE does not wait for its fallback"
        );
    }
}
