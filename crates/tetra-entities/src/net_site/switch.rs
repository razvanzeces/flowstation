use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use tetra_config::bluestation::{SharedConfig, StackConfig};
use tetra_core::{CellId, Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::{
        brew::{BrewSubscriberAction, MmSubscriberUpdate},
        call_control::{CallControl, HandedOffCall, NetworkRestoredCall},
    },
    tmd::TmdCircuitDataReq,
};
use uuid::Uuid;

use super::SharedDirectory;
use crate::net_brew::components::jitter_buffer::VoiceJitterBuffer;
use crate::{MessageQueue, TetraEntityTrait, net_brew};

/// Sessions with no traffic for this long are dropped (the network entity or a cell lost track
/// of them, e.g. an end message that never came).
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// An announced handover the MS doesn't complete (no registration on the target cell) within
/// this time is undone on the target cell.
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(30);

/// A radio that left the cell of its individual call with the network (registered on another
/// cell, or its leg there ended by uplink inactivity) has this long to restore the call on another
/// cell before the network is told it ended. Covers T306 (4 s to 6 s, TS 100 392-2 clause 14.6)
/// after the radio reaches the new cell; the same window brew-server keeps between stations.
const CIRCUIT_RESTORE_WINDOW: Duration = Duration::from_secs(8);
/// How long an announced reselection to or from another station may take through the site switch
/// (the radio waits T370 = 5 s for its answer, TS 100 392-2 clause 18.6.2).
const STATION_HANDOVER_LIFETIME: Duration = Duration::from_secs(10);

/// Idle playout buffers are dropped after this long.
const PLAYOUT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// What the switch sends a cell: a stack message, or a voice frame copied from another cell.
pub(super) enum LinkMsg {
    Msg(SapMsg),
    Voice { carrier_num: u16, ts: u8, data: Vec<u8> },
}

/// Plays voice copied from another cell out on this cell's own TDMA timing. The source cell's
/// clock is independent (separate SDR), so frames are buffered per circuit and released one per
/// traffic slot, like Brew does for network voice.
#[derive(Default)]
struct VoicePlayout {
    circuits: HashMap<(u16, u8), (VoiceJitterBuffer, Instant)>,
}

impl VoicePlayout {
    fn push(&mut self, carrier_num: u16, ts: u8, data: Vec<u8>) {
        let (buf, last) = self
            .circuits
            .entry((carrier_num, ts))
            .or_insert_with(|| (VoiceJitterBuffer::with_initial_latency(0), Instant::now()));
        buf.push(data);
        *last = Instant::now();
    }

    /// One frame per circuit whose timeslot is now (no traffic in frame 18).
    fn drain(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) {
        self.circuits
            .retain(|_, (buf, last)| !buf.is_empty() || last.elapsed() < PLAYOUT_IDLE_TIMEOUT);
        if dltime.f == 18 {
            return;
        }
        for (&(carrier_num, ts), (buf, _)) in self.circuits.iter_mut() {
            if ts != dltime.t {
                continue;
            }
            if let Some(frame) = buf.pop_ready() {
                queue.push_back(SapMsg::new(
                    Sap::TmdSap,
                    TetraEntity::Brew,
                    TetraEntity::Umac,
                    SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                        carrier_num,
                        ts,
                        data: frame.acelp_data,
                    }),
                ));
            }
        }
    }
}

/// Stand-in for the network entity in an additional cell's router. Everything the cell sends to
/// `TetraEntity::Brew` goes to the [`SiteSwitch`]; whatever the switch routes to this cell is
/// injected into the cell's queue at the start of each tick.
pub struct CellLink {
    id: CellId,
    /// Router slot this link fills: `Brew` (site switch) or `Asterisk` (Asterisk relay).
    slot: TetraEntity,
    to_switch: Sender<(CellId, SapMsg)>,
    to_asterisk: Sender<(CellId, SapMsg)>,
    /// Only the Brew-slot link receives; the Asterisk-slot link just sends.
    from_switch: Option<Receiver<LinkMsg>>,
    playout: VoicePlayout,
}

impl CellLink {
    /// The link for this cell's `Asterisk` slot, towards the primary's Asterisk relay. What the
    /// relay sends back arrives through this (Brew-slot) link.
    pub fn asterisk_link(&self) -> CellLink {
        CellLink {
            id: self.id,
            slot: TetraEntity::Asterisk,
            to_switch: self.to_asterisk.clone(),
            to_asterisk: self.to_asterisk.clone(),
            from_switch: None,
            playout: VoicePlayout::default(),
        }
    }
}

impl TetraEntityTrait for CellLink {
    fn entity(&self) -> TetraEntity {
        self.slot
    }

    fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        // Unbounded: the switch drains every primary tick, so this can't grow without bound
        // unless the primary stack is stalled (which the health watchdog handles).
        let _ = self.to_switch.send((self.id, message));
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        let Some(from_switch) = &self.from_switch else {
            return;
        };
        while let Ok(message) = from_switch.try_recv() {
            match message {
                LinkMsg::Msg(m) => queue.push_back(m),
                LinkMsg::Voice { carrier_num, ts, data } => self.playout.push(carrier_num, ts, data),
            }
        }
        self.playout.drain(queue, ts);
    }
}

/// Switch-side ends of the channels to the additional cells.
pub struct SitePorts {
    to_cells: HashMap<CellId, Sender<LinkMsg>>,
    from_cells: Receiver<(CellId, SapMsg)>,
    from_cells_asterisk: Option<Receiver<(CellId, SapMsg)>>,
}

/// Asterisk relay's ends: what the cells' Asterisk-slot links send, and the way back to them.
pub struct RelayPorts {
    pub(super) to_cells: HashMap<CellId, Sender<LinkMsg>>,
    pub(super) from_cells: Receiver<(CellId, SapMsg)>,
}

impl SitePorts {
    /// Split off the Asterisk relay's ports (once).
    pub fn take_asterisk(&mut self) -> Option<RelayPorts> {
        Some(RelayPorts {
            to_cells: self.to_cells.clone(),
            from_cells: self.from_cells_asterisk.take()?,
        })
    }
}

/// Create the channels between the switch and one [`CellLink`] per additional cell.
pub fn site_links(extra_cells: &[CellId]) -> (SitePorts, HashMap<CellId, CellLink>) {
    let (to_switch, from_cells) = unbounded();
    let (to_asterisk, from_cells_asterisk) = unbounded();
    let mut to_cells = HashMap::new();
    let mut links = HashMap::new();
    for &id in extra_cells {
        let (tx, rx) = unbounded();
        to_cells.insert(id, tx);
        links.insert(
            id,
            CellLink {
                id,
                slot: TetraEntity::Brew,
                to_switch: to_switch.clone(),
                to_asterisk: to_asterisk.clone(),
                from_switch: Some(rx),
                playout: VoicePlayout::default(),
            },
        );
    }
    let ports = SitePorts {
        to_cells,
        from_cells,
        from_cells_asterisk: Some(from_cells_asterisk),
    };
    (ports, links)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellCircuit {
    cell: CellId,
    carrier_num: u16,
    ts: u8,
}

/// One group call spread over several cells. Either it comes from the network (`origin` None:
/// the first cell's circuit is reported to the network entity as the `anchor` and its downlink
/// voice is copied to the `replicas`), or a radio on `origin` is talking and its uplink voice is
/// copied to the `replicas` on the other cells, which see it as a network call.
#[derive(Debug)]
struct GroupSession {
    gssi: u32,
    source_issi: u32,
    priority: u8,
    origin: Option<CellCircuit>,
    /// Cells sent a NetworkCallStart that have not answered Ready/Hold/End yet.
    pending: BTreeSet<CellId>,
    anchor: Option<CellCircuit>,
    /// False once the anchor's cell left the call (its voice is still copied to the others).
    anchor_live: bool,
    replicas: Vec<CellCircuit>,
    last_activity: Instant,
}

impl GroupSession {
    fn new(gssi: u32, source_issi: u32, priority: u8, origin: Option<CellCircuit>) -> Self {
        Self {
            gssi,
            source_issi,
            priority,
            origin,
            pending: BTreeSet::new(),
            anchor: None,
            anchor_live: false,
            replicas: Vec::new(),
            last_activity: Instant::now(),
        }
    }

    /// Cells taking part as listeners (for a local session: not the origin).
    fn cells(&self) -> BTreeSet<CellId> {
        let mut cells = self.pending.clone();
        if let Some(a) = self.anchor.filter(|_| self.anchor_live) {
            cells.insert(a.cell);
        }
        cells.extend(self.replicas.iter().map(|r| r.cell));
        cells
    }

    fn involves(&self, cell: CellId) -> bool {
        self.origin.is_some_and(|o| o.cell == cell) || self.cells().contains(&cell)
    }

    fn remove_cell(&mut self, cell: CellId) {
        self.pending.remove(&cell);
        self.replicas.retain(|r| r.cell != cell);
        if self.anchor.is_some_and(|a| a.cell == cell) {
            self.anchor_live = false;
        }
    }

    fn start_msg(&self, uuid: Uuid) -> CallControl {
        CallControl::NetworkCallStart {
            brew_uuid: uuid,
            source_issi: self.source_issi,
            dest_gssi: self.gssi,
            priority: self.priority,
        }
    }
}

/// An individual call between radios on two cells: each side's CMCE sees the other as the
/// network, so the switch passes the circuit-call signalling across and copies the voice.
#[derive(Debug)]
struct CellToCellCall {
    caller: CellId,
    called: CellId,
    caller_issi: u32,
    called_issi: u32,
    simplex_duplex: bool,
    priority: u8,
    circuits: HashMap<CellId, (u16, u8)>,
    last_activity: Instant,
    /// The leg on this cell ended by uplink inactivity: its radio may be on its way to the
    /// other cell, which joins both legs (the release is held until the deadline).
    leg_lost: Option<(CellId, Instant)>,
}

impl CellToCellCall {
    fn peer(&self, cell: CellId) -> CellId {
        if cell == self.caller { self.called } else { self.caller }
    }
    /// The cell a given party's ISSI is currently on, if it is one of this call's two parties.
    fn cell_of_issi(&self, issi: u32) -> Option<CellId> {
        if issi == self.caller_issi {
            Some(self.caller)
        } else if issi == self.called_issi {
            Some(self.called)
        } else {
            None
        }
    }
    /// The other party's ISSI, given one party's ISSI.
    fn peer_issi(&self, issi: u32) -> Option<u32> {
        if issi == self.caller_issi {
            Some(self.called_issi)
        } else if issi == self.called_issi {
            Some(self.caller_issi)
        } else {
            None
        }
    }
}

/// An individual call between a radio on one of the cells and the network (Brew / LST).
#[derive(Debug)]
struct CircuitCall {
    /// Cell holding the call's leg; only this cell speaks for the call to the network.
    cell: CellId,
    /// The radio on site (0 when the switch did not see the setup).
    issi: u32,
    /// The other party on the network (0 for an external number or when not known).
    peer: u32,
    duplex: bool,
    priority: u8,
    /// Party holding a simplex call's floor.
    floor: Option<u32>,
    /// The radio's leg is gone (it registered on another cell, or uplink inactivity ended it):
    /// the call waits until the deadline for the radio's U-CALL RESTORE on another cell, then the
    /// network gets the release (`cause`).
    awaiting_restore: Option<(Instant, u8)>,
    /// Network call to a radio registered on a cell that shares its location area with others:
    /// every cell of the area pages it, until it answers on one (empty = not paging).
    paging: BTreeSet<CellId>,
}

impl CircuitCall {
    fn new(cell: CellId) -> Self {
        Self {
            cell,
            issi: 0,
            peer: 0,
            duplex: false,
            priority: 0,
            floor: None,
            awaiting_restore: None,
            paging: BTreeSet::new(),
        }
    }

    /// Cells the network's messages for this call go to.
    fn cells(&self) -> BTreeSet<CellId> {
        if self.paging.is_empty() {
            BTreeSet::from([self.cell])
        } else {
            self.paging.clone()
        }
    }
}

/// Call identifiers are allocated per cell; the network entity needs them unique site-wide.
#[derive(Debug, Default)]
struct CallIdMap {
    to_global: HashMap<(CellId, u16), u16>,
    to_local: HashMap<u16, (CellId, u16)>,
    next: u16,
}

impl CallIdMap {
    fn global(&mut self, cell: CellId, local: u16) -> u16 {
        if let Some(&g) = self.to_global.get(&(cell, local)) {
            return g;
        }
        loop {
            self.next = self.next.wrapping_add(1);
            if self.next != 0 && !self.to_local.contains_key(&self.next) {
                break;
            }
        }
        self.to_global.insert((cell, local), self.next);
        self.to_local.insert(self.next, (cell, local));
        self.next
    }

    fn local(&self, global: u16) -> Option<(CellId, u16)> {
        self.to_local.get(&global).copied()
    }

    fn release(&mut self, cell: CellId, local: u16) {
        if let Some(g) = self.to_global.remove(&(cell, local)) {
            self.to_local.remove(&g);
        }
    }
}

/// Link state the network entity keeps in the primary cell's `StackState`, mirrored to the others.
type LinkState = (bool, bool, Option<Instant>);

/// Wraps the network entity (Brew / LST Dispatch) in the primary router; see the module docs.
pub struct SiteSwitch {
    inner: Box<dyn TetraEntityTrait>,
    /// The cell whose router runs this switch; messages for it go straight onto that queue.
    host: CellId,
    primary: SharedConfig,
    extra: Vec<(CellId, SharedConfig)>,
    all_cells: Vec<CellId>,
    carrier_cell: HashMap<u16, CellId>,
    ports: SitePorts,
    directory: SharedDirectory,
    sessions: HashMap<Uuid, GroupSession>,
    /// Individual calls with the network, by Brew session.
    circuit_calls: HashMap<Uuid, CircuitCall>,
    /// Individual call restores a cell asked the network about: ISSI → asking cell.
    restore_requests: HashMap<u32, CellId>,
    cell_calls: HashMap<Uuid, CellToCellCall>,
    call_ids: CallIdMap,
    /// Group of each call a cell reported, to apply the Brew routing rules to its floor events.
    call_gssi: HashMap<(CellId, u16), u32>,
    /// Local floors that lost to a talker on another cell: their events never reach the network.
    suppressed: HashSet<(CellId, u16)>,
    /// Raised priority of local group calls, from `SiteCallPriority` (absent = 0).
    call_priority: HashMap<(CellId, u16), u8>,
    mirrored_link: Option<LinkState>,
    /// Copied voice for the primary cell's circuits.
    playout: VoicePlayout,
    /// Announced handovers in progress: (ISSI, target cell, deadline).
    handovers: Vec<(u32, CellId, Instant)>,
    /// Forwarded registrations awaiting the target cell's answer: ISSI → serving cell.
    forwards: HashMap<u32, CellId>,
    /// Announced reselections to another station a cell asked the network about: ISSI → asking
    /// cell, which gets the answer (`HandoverResult`).
    station_asks: HashMap<u32, (CellId, Instant)>,
    /// Registrations another station forwarded to a cell here: ISSI → target cell.
    station_forwards: HashMap<u32, (CellId, Instant)>,
    /// Groups last sent to a cell as `SiteAreaMembers` for an ISSI (absent = none).
    area_sent: HashMap<(CellId, u32), Vec<u32>>,
    /// Local calls asked of a cell for a radio restoring on a sibling cell (`call-handoff` within
    /// the station): (cell asked, issi) → (asking cell, peer, asked at).
    sibling_handoffs: HashMap<(CellId, u32), (CellId, u32, Instant)>,
}

impl SiteSwitch {
    /// `primary` is the config of the cell whose router runs the switch and the network entity
    /// (the network's home cell: the primary cell, or the first cell of a network the primary
    /// does not serve); `extra` are the network's other cells, whose link state the switch keeps
    /// in sync.
    pub fn new(
        inner: Box<dyn TetraEntityTrait>,
        primary: SharedConfig,
        extra: Vec<(CellId, SharedConfig)>,
        ports: SitePorts,
        directory: SharedDirectory,
    ) -> Self {
        let host = primary.config().cell_id;
        let carrier_cell = carrier_map(&primary, &extra);
        let mut all_cells = vec![host];
        all_cells.extend(extra.iter().map(|(id, _)| *id));
        Self {
            inner,
            host,
            primary,
            extra,
            all_cells,
            carrier_cell,
            ports,
            directory,
            sessions: HashMap::new(),
            circuit_calls: HashMap::new(),
            restore_requests: HashMap::new(),
            cell_calls: HashMap::new(),
            call_ids: CallIdMap::default(),
            call_gssi: HashMap::new(),
            suppressed: HashSet::new(),
            call_priority: HashMap::new(),
            mirrored_link: None,
            playout: VoicePlayout::default(),
            handovers: Vec::new(),
            forwards: HashMap::new(),
            station_asks: HashMap::new(),
            station_forwards: HashMap::new(),
            area_sent: HashMap::new(),
            sibling_handoffs: HashMap::new(),
        }
    }

    // ── Directory (shared with the Asterisk relay) ──────────────────────────────────────────

    fn location(&self, issi: u32) -> Option<CellId> {
        self.directory.read().expect("site directory").location(issi)
    }

    /// Cells a call to `gssi` must run on: those with a registered member, and the other cells
    /// of their location areas, where a member may be without registering there.
    fn group_cells(&self, gssi: u32) -> BTreeSet<CellId> {
        let members = self.directory.read().expect("site directory").group_cells(gssi);
        let mut cells = members.clone();
        for cell in members {
            cells.extend(self.area_siblings(cell));
        }
        cells
    }

    // ── Location areas ──────────────────────────────────────────────────────────────────────

    fn location_area(&self, cell: CellId) -> Option<u16> {
        if cell.is_primary() {
            return Some(self.primary.config().cell.location_area);
        }
        self.extra
            .iter()
            .find(|(id, _)| *id == cell)
            .map(|(_, cfg)| cfg.config().cell.location_area)
    }

    /// The other cells of `cell`'s location area. A radio changes between them without
    /// registering (TS 100 392-2 clause 18.3.4.7.1a), so the switch only knows the area.
    fn area_siblings(&self, cell: CellId) -> Vec<CellId> {
        let Some(la) = self.location_area(cell) else {
            return Vec::new();
        };
        self.all_cells
            .iter()
            .copied()
            .filter(|c| *c != cell && self.location_area(*c) == Some(la))
            .collect()
    }

    /// Tell every cell whether `issi` may be there without being registered: the sibling cells
    /// of its registered cell's location area count it as a listener of its groups and accept
    /// its individual calls.
    fn refresh_area_members(&mut self, queue: &mut MessageQueue, issi: u32) {
        let (siblings, groups) = match self.location(issi) {
            Some(home) => (self.area_siblings(home), self.groups_of(home, issi)),
            None => (Vec::new(), Vec::new()),
        };
        for cell in self.all_cells.clone() {
            let want = siblings.contains(&cell).then(|| groups.clone());
            let had = self.area_sent.get(&(cell, issi)).cloned();
            if want == had {
                continue;
            }
            match &want {
                Some(groups) => self.area_sent.insert((cell, issi), groups.clone()),
                None => self.area_sent.remove(&(cell, issi)),
            };
            self.deliver_cc(queue, cell, CallControl::SiteAreaMembers { issi, groups: want });
        }
    }

    /// MM on `cell` sent `issi` a presence check; the radio may be on another cell of the area.
    fn probe_area(&self, queue: &mut MessageQueue, cell: CellId, issi: u32) {
        for sibling in self.area_siblings(cell) {
            let ask = SapMsg::new(Sap::Control, TetraEntity::Brew, TetraEntity::Mm, SapMsgInner::MmRequestLocationUpdate { issi });
            self.deliver(queue, sibling, ask);
        }
    }

    fn groups_of(&self, cell: CellId, issi: u32) -> Vec<u32> {
        self.directory.read().expect("site directory").groups_of(cell, issi)
    }

    // ── Delivery helpers ────────────────────────────────────────────────────────────────────

    fn deliver(&self, queue: &mut MessageQueue, cell: CellId, message: SapMsg) {
        if cell == self.host {
            queue.push_back(message);
        } else if let Some(tx) = self.ports.to_cells.get(&cell) {
            let _ = tx.send(LinkMsg::Msg(message));
        } else {
            tracing::warn!("SiteSwitch: no link to {cell}, dropping {:?}", message.msg);
        }
    }

    fn deliver_all(&self, queue: &mut MessageQueue, cells: impl IntoIterator<Item = CellId>, message: &SapMsg) {
        for cell in cells {
            self.deliver(queue, cell, message.clone());
        }
    }

    /// Call control to a cell's CMCE, as if from the network.
    fn deliver_cc(&self, queue: &mut MessageQueue, cell: CellId, cc: CallControl) {
        let msg = SapMsg::new(Sap::Control, TetraEntity::Brew, TetraEntity::Cmce, SapMsgInner::CmceCallControl(cc));
        self.deliver(queue, cell, msg);
    }

    /// Downlink voice onto a cell's circuit, via that cell's playout buffer.
    fn deliver_voice(&mut self, to: CellCircuit, data: &[u8]) {
        if to.cell == self.host {
            self.playout.push(to.carrier_num, to.ts, data.to_vec());
        } else if let Some(tx) = self.ports.to_cells.get(&to.cell) {
            let _ = tx.send(LinkMsg::Voice {
                carrier_num: to.carrier_num,
                ts: to.ts,
                data: data.to_vec(),
            });
        }
    }

    fn cell_of_carrier(&self, carrier_num: u16) -> CellId {
        self.carrier_cell.get(&carrier_num).copied().unwrap_or(self.host)
    }

    /// Run the network entity with a private queue and route what it emits.
    fn with_inner(&mut self, queue: &mut MessageQueue, f: impl FnOnce(&mut dyn TetraEntityTrait, &mut MessageQueue)) {
        let mut out = MessageQueue::new();
        f(self.inner.as_mut(), &mut out);
        while let Some(message) = out.pop_front() {
            self.route_from_network(queue, message);
        }
    }

    fn to_inner(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        self.with_inner(queue, |inner, out| inner.rx_prim(out, message));
    }

    // ── Brew routing rules (the cells report everything once site-linked) ───────────────────

    /// Group floor events reach the network entity only for groups it routes.
    fn group_to_network(&self, gssi: u32) -> bool {
        net_brew::is_brew_gssi_routable(&self.primary, gssi) || net_brew::is_lst_dispatch_active(&self.primary)
    }

    /// MM's Brew filter: what of a subscriber update the network entity may see.
    fn subscriber_update_to_network(&self, update: &mut MmSubscriberUpdate) -> bool {
        if !net_brew::is_active(&self.primary) {
            return false;
        }
        update.groups.retain(|g| net_brew::is_brew_gssi_routable(&self.primary, *g));
        match update.action {
            BrewSubscriberAction::Register | BrewSubscriberAction::Deregister => {
                net_brew::is_brew_issi_routable(&self.primary, update.issi)
            }
            BrewSubscriberAction::Affiliate | BrewSubscriberAction::Deaffiliate => !update.groups.is_empty(),
        }
    }

    /// CMCE's routing check for an individual call leaving the site.
    fn individual_call_to_network(&self, source_issi: u32, destination: u32, number: &str) -> bool {
        if net_brew::is_lst_dispatch_active(&self.primary) {
            return true;
        }
        net_brew::is_brew_issi_routable(&self.primary, source_issi)
            && (destination == 0 || !number.is_empty() || net_brew::is_brew_issi_routable(&self.primary, destination))
    }

    // ── Cell → network ──────────────────────────────────────────────────────────────────────

    /// A cell (primary included) sent `message` to the network entity.
    fn handle_from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, mut message: SapMsg) {
        let forward = match &mut message.msg {
            SapMsgInner::MmSubscriberUpdate(update) => {
                let moved = self.directory.write().expect("site directory").apply(cell, update);
                if let Some(old) = moved {
                    // Reselected from `old`: drop the stale registration there (silently).
                    tracing::info!("SiteSwitch: ISSI {} moved {old} → {cell}", update.issi);
                    let drop = SapMsg::new(
                        Sap::Control,
                        TetraEntity::Brew,
                        TetraEntity::Mm,
                        SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                            issi: update.issi,
                            groups: Vec::new(),
                            action: BrewSubscriberAction::Deregister,
                        }),
                    );
                    self.deliver(queue, old, drop);
                    self.radio_left_cell(queue, update.issi, old);
                }
                // A cell the radio has left (its cleanup after a move) must not undo the
                // registration it now has on another cell, as far as the network is concerned.
                self.refresh_area_members(queue, update.issi);
                let current = self.location(update.issi);
                let from_old_cell = matches!(
                    update.action,
                    BrewSubscriberAction::Deregister | BrewSubscriberAction::Deaffiliate
                ) && current.is_some_and(|c| c != cell);
                !from_old_cell && self.subscriber_update_to_network(update)
            }
            SapMsgInner::CmceSdsData(sds) => {
                let dest = sds.dest_issi;
                if let Some(at) = self.location(dest)
                    && at != cell
                {
                    // Radio on a sibling cell: deliver directly, never via the network. `src` is
                    // CMCE (not Brew) so the receiving cell doesn't log it again as network SDS.
                    let copy = SapMsg::new(message.sap, TetraEntity::Cmce, TetraEntity::Cmce, message.msg.clone());
                    self.deliver(queue, at, copy);
                    return;
                }
                let members = self.group_cells(dest);
                let others: Vec<CellId> = members.iter().copied().filter(|c| *c != cell).collect();
                if !others.is_empty() {
                    let copy = SapMsg::new(message.sap, TetraEntity::Cmce, TetraEntity::Cmce, message.msg.clone());
                    self.deliver_all(queue, others, &copy);
                }
                // The single-cell rule, site-wide: only an SDS for nobody on site goes to the
                // network, and only with Brew's SDS feature on.
                self.location(dest).is_none() && members.is_empty() && net_brew::feature_sds_enabled(&self.primary)
            }
            SapMsgInner::TmdCircuitDataInd(ind) => {
                self.copy_uplink_voice(cell, ind.carrier_num, ind.ts, &ind.data);
                true
            }
            SapMsgInner::CmceCallControl(cc) => self.handle_call_control_from_cell(queue, cell, cc),
            _ => true,
        };
        if forward {
            self.to_inner(queue, message);
        }
    }

    /// A radio's uplink voice: copy it to the listening circuits on the other cells.
    fn copy_uplink_voice(&mut self, cell: CellId, carrier_num: u16, ts: u8, data: &[u8]) {
        let from = CellCircuit { cell, carrier_num, ts };
        let mut targets = Vec::new();
        for s in self.sessions.values_mut().filter(|s| s.origin == Some(from)) {
            s.last_activity = Instant::now();
            targets.extend(s.replicas.iter().copied());
        }
        for call in self.cell_calls.values_mut() {
            if call.circuits.get(&cell) == Some(&(carrier_num, ts)) {
                call.last_activity = Instant::now();
                let peer = call.peer(cell);
                if let Some(&(carrier_num, ts)) = call.circuits.get(&peer) {
                    targets.push(CellCircuit { cell: peer, carrier_num, ts });
                }
            }
        }
        for to in targets {
            self.deliver_voice(to, data);
        }
    }

    /// Returns false when the message is consumed by the switch and must not reach the network
    /// entity. Renumbers call identifiers in place.
    fn handle_call_control_from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, cc: &mut CallControl) -> bool {
        if let Some(uuid) = circuit_uuid(cc)
            && self.cell_calls.contains_key(&uuid)
        {
            self.cell_call_signal(queue, cell, uuid, cc);
            return false;
        }
        if let Some(uuid) = circuit_uuid(cc)
            && !matches!(cc, CallControl::NetworkCircuitSetupRequest { .. })
            && !self.circuit_call_from_cell(queue, cell, uuid, cc)
        {
            return false;
        }

        let forward = match cc {
            CallControl::SitePresenceProbe { issi } => {
                self.probe_area(queue, cell, *issi);
                false
            }
            CallControl::IndividualCallRestoreRequest { issi, peer } => {
                if self.restore_from_sibling(queue, cell, *issi, *peer) {
                    return false;
                }
                if self.ask_sibling_for_local_call(queue, cell, *issi, *peer) {
                    return false;
                }
                // Not on another cell here: the network may hold it (another station).
                self.restore_requests.insert(*issi, cell);
                true
            }
            CallControl::IndividualCallHandoffAnswer { issi, peer, call } => {
                self.sibling_handoff_answered(queue, cell, *issi, *peer, *call);
                return false;
            }
            CallControl::RegisteredWithoutCall { issi } => {
                // A plain roaming registration: the radio gave up a circuit mode call it had on
                // another cell (it would have asked for service restoration roaming, TS 100
                // 392-2 clause 16.4 a)), so that call ends now.
                self.end_given_up_calls(queue, cell, *issi);
                true
            }
            CallControl::SiteCallPriority { call_id, priority } => {
                self.call_priority.insert((cell, *call_id), *priority);
                return false;
            }
            CallControl::SiteHandoverPrepare { issi, target_carrier } => {
                self.prepare_handover(queue, cell, *issi, *target_carrier);
                return false;
            }
            CallControl::SiteForwardRegistration { issi, target_carrier, .. } => {
                // Type 1 announced reselection: the target cell's MLE/MM registers the MS.
                match self.carrier_cell.get(target_carrier).copied().filter(|t| *t != cell) {
                    Some(target) => {
                        self.forwards.insert(*issi, cell);
                        self.deliver(queue, target, to_mle(cc.clone()));
                    }
                    None => {
                        // Not one of our cells: answer at once, the MS registers on arrival.
                        let result = CallControl::SiteForwardRegistrationResult { issi: *issi, sdu: None };
                        self.deliver(queue, cell, to_mle(result));
                    }
                }
                return false;
            }
            CallControl::SiteForwardRegistrationResult { issi, .. } => {
                if let Some(serving) = self.forwards.remove(issi) {
                    self.deliver(queue, serving, to_mle(cc.clone()));
                    return false;
                }
                // A registration another station forwarded: its answer goes back through the
                // network entity.
                self.station_forwards.get(issi).is_some_and(|(target, _)| *target == cell)
            }
            CallControl::HandoverRequest { issi, .. } => {
                // Announced reselection to another station's cell: the network entity asks, the
                // answer comes back to this cell.
                self.station_asks.retain(|_, (_, at)| at.elapsed() < STATION_HANDOVER_LIFETIME);
                self.station_asks.insert(*issi, (cell, Instant::now()));
                true
            }
            CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                carrier_num,
                ts,
            } => {
                self.call_gssi.insert((cell, *call_id), *dest_gssi);
                let origin = CellCircuit {
                    cell,
                    carrier_num: *carrier_num,
                    ts: *ts,
                };
                self.local_floor_granted(queue, *call_id, *source_issi, *dest_gssi, origin) && self.group_to_network(*dest_gssi)
            }
            CallControl::FloorReleased {
                call_id,
                carrier_num,
                ts,
            }
            | CallControl::CallEnded {
                call_id,
                carrier_num,
                ts,
            } => {
                let origin = CellCircuit {
                    cell,
                    carrier_num: *carrier_num,
                    ts: *ts,
                };
                self.local_floor_released(queue, origin);
                let key = (cell, *call_id);
                !self.suppressed.remove(&key) && self.call_gssi.get(&key).is_none_or(|g| self.group_to_network(*g))
            }
            CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
                ..
            } => {
                let circuit = CellCircuit {
                    cell,
                    carrier_num: *carrier_num,
                    ts: *ts,
                };
                match self.sessions.get_mut(brew_uuid) {
                    Some(s) => {
                        self.call_gssi.insert((cell, *call_id), s.gssi);
                        s.pending.remove(&cell);
                        s.last_activity = Instant::now();
                        if s.origin.is_some() || s.anchor.is_some_and(|a| a.cell != cell) {
                            s.replicas.retain(|r| r.cell != cell);
                            s.replicas.push(circuit);
                            false
                        } else {
                            // First cell (or the anchor refreshing on a speaker change).
                            s.anchor = Some(circuit);
                            s.anchor_live = true;
                            true
                        }
                    }
                    None => true,
                }
            }
            CallControl::NetworkCallHold { brew_uuid, .. } => match self.sessions.get_mut(brew_uuid) {
                Some(s) => {
                    s.pending.remove(&cell);
                    // Only the last cell of a network call to answer may put it on hold.
                    s.origin.is_none() && s.anchor.is_none() && s.pending.is_empty()
                }
                None => true,
            },
            CallControl::NetworkCallEnd { brew_uuid } => match self.sessions.get_mut(brew_uuid) {
                Some(s) => {
                    s.remove_cell(cell);
                    if s.origin.is_some() || !s.cells().is_empty() {
                        false
                    } else {
                        self.sessions.remove(brew_uuid);
                        true
                    }
                }
                None => true,
            },
            CallControl::GroupListenersAvailable { gssi } => {
                // A cell gained its first member of a group whose call is already on air
                // elsewhere: bring this cell into the call too.
                let gssi = *gssi;
                let join = self
                    .sessions
                    .iter_mut()
                    .find(|(_, s)| s.gssi == gssi && !s.involves(cell))
                    .map(|(uuid, s)| {
                        s.pending.insert(cell);
                        s.start_msg(*uuid)
                    });
                if let Some(start) = join {
                    self.deliver_cc(queue, cell, start);
                }
                true
            }
            CallControl::NetworkCircuitSetupRequest { brew_uuid, call } => {
                let (uuid, source, destination) = (*brew_uuid, call.source_issi, call.destination);
                let to_network = self.individual_call_to_network(source, destination, &call.number);
                match self.location(destination) {
                    Some(called) if called != cell => {
                        // Called radio is on a sibling cell: connect the two cells directly.
                        self.cell_calls.insert(
                            uuid,
                            CellToCellCall {
                                caller: cell,
                                called,
                                caller_issi: source,
                                called_issi: destination,
                                simplex_duplex: call.duplex == 1,
                                priority: call.priority,
                                circuits: HashMap::new(),
                                last_activity: Instant::now(),
                                leg_lost: None,
                            },
                        );
                        self.deliver_cc(queue, called, cc.clone());
                        return false;
                    }
                    _ if !to_network => {
                        self.deliver_cc(
                            queue,
                            cell,
                            CallControl::NetworkCircuitSetupReject {
                                brew_uuid: uuid,
                                cause: DisconnectCause::CalledPartyNotReachable.into_raw() as u8,
                            },
                        );
                        return false;
                    }
                    _ => {
                        let entry = self.circuit_calls.entry(uuid).or_insert_with(|| CircuitCall::new(cell));
                        entry.cell = cell;
                        entry.issi = source;
                        entry.peer = destination;
                        entry.duplex = call.duplex == 1;
                        entry.priority = call.priority;
                        true
                    }
                }
            }
            _ => {
                if let Some(uuid) = circuit_uuid(cc) {
                    self.circuit_calls.entry(uuid).or_insert_with(|| CircuitCall::new(cell));
                }
                true
            }
        };

        let ended = matches!(cc, CallControl::CallEnded { .. });
        if let Some(id) = call_id_mut(cc) {
            let local = *id;
            // Only what goes to the network entity is renumbered.
            if forward {
                *id = self.call_ids.global(cell, local);
            }
            if ended {
                self.call_ids.release(cell, local);
                self.call_gssi.remove(&(cell, local));
                self.call_priority.remove(&(cell, local));
            }
        }
        forward
    }

    /// Circuit-call signalling of an individual call with the network from `cell`. Returns false
    /// when it must not reach the network: only the cell holding the call's leg speaks for it (a
    /// cell the radio left may still end its old leg), and a leg that ended by uplink inactivity
    /// is held back while the radio may restore the call on another cell.
    fn circuit_call_from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, uuid: Uuid, cc: &CallControl) -> bool {
        let Some(call) = self.circuit_calls.get_mut(&uuid) else {
            return true;
        };
        if !call.paging.is_empty() {
            return self.paged_call_from_cell(queue, cell, uuid, cc);
        }
        if call.cell != cell {
            tracing::debug!("SiteSwitch: {cell} is no longer in individual call {uuid} (now {}), dropping {cc:?}", call.cell);
            return false;
        }
        match cc {
            CallControl::NetworkCircuitSimplexGranted { .. } => call.floor = Some(call.issi),
            CallControl::NetworkCircuitSimplexIdle { .. } => call.floor = None,
            CallControl::NetworkCircuitRelease { cause, .. }
                if *cause == DisconnectCause::ExpiryOfTimer.into_raw() as u8 && call.issi != 0 && self.all_cells.len() > 1 =>
            {
                // Uplink inactivity: the radio may have reselected and restore on another cell.
                tracing::info!(
                    "SiteSwitch: individual call {uuid} of ISSI {} lost its leg on {cell}, waiting {}s for a restore",
                    call.issi,
                    CIRCUIT_RESTORE_WINDOW.as_secs()
                );
                call.awaiting_restore = Some((Instant::now() + CIRCUIT_RESTORE_WINDOW, *cause));
                return false;
            }
            CallControl::NetworkCircuitRelease { .. } => {
                self.circuit_calls.remove(&uuid);
            }
            _ => {}
        }
        true
    }

    /// A paging cell reported on a network call to a radio of its location area. The cell where
    /// the radio answers (alert or connect) takes the call and the others let it go silently; a
    /// cell that gives up drops out, and only the last one's failure reaches the network.
    fn paged_call_from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, uuid: Uuid, cc: &CallControl) -> bool {
        let Some(call) = self.circuit_calls.get_mut(&uuid) else {
            return true;
        };
        if !call.paging.contains(&cell) {
            return false;
        }
        match cc {
            CallControl::NetworkCircuitAlert { .. } | CallControl::NetworkCircuitConnectRequest { .. } => {
                let others: Vec<CellId> = call.paging.iter().copied().filter(|c| *c != cell).collect();
                tracing::info!("SiteSwitch: ISSI {} answered individual call {uuid} on {cell}", call.issi);
                call.cell = cell;
                call.paging.clear();
                for other in others {
                    self.deliver_cc(queue, other, silent_circuit_release(uuid));
                }
                true
            }
            CallControl::NetworkCircuitSetupReject { .. } | CallControl::NetworkCircuitRelease { .. } => {
                call.paging.remove(&cell);
                if call.paging.is_empty() {
                    self.circuit_calls.remove(&uuid);
                    return true;
                }
                if call.cell == cell {
                    call.cell = *call.paging.iter().next().expect("paging cells left");
                }
                tracing::debug!("SiteSwitch: {cell} stops paging for individual call {uuid}: {cc:?}");
                false
            }
            // One answer per call for the network (every paging cell accepts the setup).
            _ => call.cell == cell,
        }
    }

    /// A cell asks for `issi`'s individual call with `peer` (0 = any) that it does not hold: the
    /// radio reselected mid-call and sent U-CALL RESTORE there (TS 100 392-2 clause 14.5.1.2.4).
    /// If the call runs with the network from another cell of this site, move it: the old leg is
    /// released without a release to the network (cause "SwMI requested disconnection", which the
    /// old cell's CMCE does not echo) and the asking cell builds the new leg, whose media ready
    /// re-points the network's voice. True if answered here.
    fn restore_from_sibling(&mut self, queue: &mut MessageQueue, cell: CellId, issi: u32, peer: u32) -> bool {
        let found = self
            .circuit_calls
            .iter_mut()
            .find(|(_, c)| c.issi == issi && c.cell != cell && (peer == 0 || c.peer == 0 || c.peer == peer));
        let Some((&uuid, call)) = found else {
            return false;
        };
        let old = call.cell;
        let old_leg_live = call.awaiting_restore.take().is_none();
        call.cell = cell;
        let answer = NetworkRestoredCall {
            brew_uuid: uuid,
            simplex_duplex: call.duplex,
            priority: call.priority,
            floor_issi: call.floor,
        };
        let peer = if peer == 0 { call.peer } else { peer };
        tracing::info!("SiteSwitch: ISSI {issi} restores individual call {uuid} (peer {peer}) {old} → {cell}");
        if old_leg_live {
            self.deliver_cc(queue, old, silent_circuit_release(uuid));
        }
        self.deliver_cc(queue, cell, CallControl::IndividualCallRestoreAnswer { issi, peer, call: Some(answer) });
        true
    }

    /// A cell asks for `issi`'s call with `peer` that nobody holds as a network call, and `peer` is
    /// a radio of a sibling cell: the call is local there (both radios were on that cell). Ask
    /// that cell to hand it over (`IndividualCallHandoffRequest`); its answer becomes a call
    /// between the two cells. True if asked.
    fn ask_sibling_for_local_call(&mut self, queue: &mut MessageQueue, cell: CellId, issi: u32, peer: u32) -> bool {
        if peer == 0 || peer == issi {
            return false;
        }
        let Some(holder) = self.location(peer).filter(|c| *c != cell) else {
            return false;
        };
        self.sibling_handoffs.retain(|_, (_, _, at)| at.elapsed() < STATION_HANDOVER_LIFETIME);
        self.sibling_handoffs.insert((holder, issi), (cell, peer, Instant::now()));
        tracing::info!("SiteSwitch: ISSI {issi} restores on {cell}, asking {holder} for its local call with {peer}");
        self.deliver_cc(queue, holder, CallControl::IndividualCallHandoffRequest { issi, peer });
        true
    }

    /// `holder` answered a handoff asked by `ask_sibling_for_local_call`: the call, now bridged
    /// under `call.brew_uuid`, runs between the two cells; the asking cell restores it.
    fn sibling_handoff_answered(&mut self, queue: &mut MessageQueue, holder: CellId, issi: u32, peer: u32, call: Option<HandedOffCall>) {
        let pending = self.sibling_handoffs.remove(&(holder, issi)).filter(|(_, p, _)| *p == peer);
        let Some((asker, _, _)) = pending else {
            if let Some(call) = call {
                // Nobody waits for it any more: end the leg the cell just bridged.
                tracing::info!("SiteSwitch: handoff of call {} from {holder} came too late, ending it", call.brew_uuid);
                let cause = DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8;
                self.deliver_cc(queue, holder, CallControl::NetworkCircuitRelease { brew_uuid: call.brew_uuid, cause });
            }
            return;
        };
        let Some(call) = call else {
            self.deliver_cc(queue, asker, CallControl::IndividualCallRestoreAnswer { issi, peer, call: None });
            return;
        };
        let (caller, called, caller_issi, called_issi) = if call.calling_issi == issi {
            (asker, holder, issi, peer)
        } else {
            (holder, asker, peer, issi)
        };
        tracing::info!("SiteSwitch: local call of {peer} on {holder} handed over, ISSI {issi} restores it on {asker} (call {})", call.brew_uuid);
        self.cell_calls.insert(
            call.brew_uuid,
            CellToCellCall {
                caller,
                called,
                caller_issi,
                called_issi,
                simplex_duplex: call.simplex_duplex,
                priority: call.priority,
                circuits: HashMap::new(),
                last_activity: Instant::now(),
                leg_lost: None,
            },
        );
        let restored = NetworkRestoredCall {
            brew_uuid: call.brew_uuid,
            simplex_duplex: call.simplex_duplex,
            priority: call.priority,
            floor_issi: call.floor_issi,
        };
        self.deliver_cc(queue, asker, CallControl::IndividualCallRestoreAnswer { issi, peer, call: Some(restored) });
    }

    /// Calls between two cells whose lost leg was not joined on the other cell in time end there.
    fn expire_lost_cell_legs(&mut self, queue: &mut MessageQueue) {
        let now = Instant::now();
        let expired: Vec<(Uuid, CellId)> = self
            .cell_calls
            .iter()
            .filter_map(|(&uuid, c)| c.leg_lost.filter(|(_, deadline)| *deadline <= now).map(|(lost, _)| (uuid, c.peer(lost))))
            .collect();
        for (uuid, peer) in expired {
            tracing::info!("SiteSwitch: call {uuid} was not restored on {peer}, releasing it there");
            self.cell_calls.remove(&uuid);
            let cause = DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8;
            self.deliver_cc(queue, peer, CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause });
        }
    }

    /// `issi` registered on another cell than `old`: its individual calls with the network on
    /// `old` lose their leg there (as brew-server does between stations) and wait for the radio's
    /// restore on the new cell.
    fn radio_left_cell(&mut self, queue: &mut MessageQueue, issi: u32, old: CellId) {
        let mut released = Vec::new();
        for (&uuid, call) in self.circuit_calls.iter_mut() {
            if call.issi == issi && call.cell == old && call.awaiting_restore.is_none() {
                tracing::info!(
                    "SiteSwitch: ISSI {issi} left {old} during individual call {uuid}, waiting {}s for a restore",
                    CIRCUIT_RESTORE_WINDOW.as_secs()
                );
                call.awaiting_restore = Some((
                    Instant::now() + CIRCUIT_RESTORE_WINDOW,
                    DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8,
                ));
                released.push(uuid);
            }
        }
        for uuid in released {
            self.deliver_cc(queue, old, silent_circuit_release(uuid));
        }
    }

    /// `issi` registered on `cell` by plain roaming: end its individual calls with the network
    /// held on other cells.
    fn end_given_up_calls(&mut self, queue: &mut MessageQueue, cell: CellId, issi: u32) {
        let given_up: Vec<(Uuid, CellId, bool)> = self
            .circuit_calls
            .iter()
            .filter(|(_, c)| c.issi == issi && c.cell != cell)
            .map(|(&uuid, c)| (uuid, c.cell, c.awaiting_restore.is_none()))
            .collect();
        for (uuid, old, leg_live) in given_up {
            tracing::info!("SiteSwitch: ISSI {issi} registered on {cell} without restoring individual call {uuid}, ending it");
            self.circuit_calls.remove(&uuid);
            if leg_live {
                self.deliver_cc(queue, old, silent_circuit_release(uuid));
            }
            let cause = DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8;
            self.release_to_network(queue, uuid, cause);
        }
    }

    /// Tell the network entity an individual call ended.
    fn release_to_network(&mut self, queue: &mut MessageQueue, uuid: Uuid, cause: u8) {
        let release = SapMsg::new(
            Sap::Control,
            TetraEntity::Cmce,
            TetraEntity::Brew,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause }),
        );
        self.to_inner(queue, release);
    }

    /// Individual calls whose radio did not restore them in time end for the network.
    fn expire_circuit_restores(&mut self, queue: &mut MessageQueue) {
        let now = Instant::now();
        let expired: Vec<(Uuid, u8)> = self
            .circuit_calls
            .iter()
            .filter_map(|(&uuid, c)| c.awaiting_restore.filter(|(deadline, _)| *deadline <= now).map(|(_, cause)| (uuid, cause)))
            .collect();
        for (uuid, cause) in expired {
            tracing::info!("SiteSwitch: individual call {uuid} was not restored, releasing it (cause {cause})");
            self.circuit_calls.remove(&uuid);
            self.release_to_network(queue, uuid, cause);
        }
    }

    /// A radio on `origin` got the floor of `gssi`. Returns false if it lost to a talker on
    /// another cell (the grant is then withdrawn by pulling its cell into that call).
    fn local_floor_granted(&mut self, queue: &mut MessageQueue, call_id: u16, source_issi: u32, gssi: u32, origin: CellCircuit) -> bool {
        let cell = origin.cell;
        let priority = self.call_priority.get(&(cell, call_id)).copied().unwrap_or(0);

        // Someone on another cell is already talking in this group: they keep the floor, unless
        // this call has a higher priority (e.g. emergency), which takes it over site-wide.
        let busy = self
            .sessions
            .iter()
            .find(|(_, s)| s.gssi == gssi && s.origin.is_some_and(|o| o.cell != cell))
            .map(|(uuid, s)| (*uuid, s.priority, s.origin.map(|o| o.cell)));
        if let Some((busy_uuid, busy_priority, talker)) = busy {
            if priority <= busy_priority {
                tracing::info!("SiteSwitch: {cell} floor for gssi={gssi} (issi={source_issi}) loses to {talker:?}");
                let start = self.sessions.get_mut(&busy_uuid).and_then(|s| {
                    (!s.cells().contains(&cell)).then(|| {
                        s.pending.insert(cell);
                        s.start_msg(busy_uuid)
                    })
                });
                if let Some(start) = start {
                    self.deliver_cc(queue, cell, start);
                }
                self.suppressed.insert((cell, call_id));
                return false;
            }
            // Pre-empt: the old talker's cell gets the new call below (its CMCE cuts its speaker).
            tracing::info!("SiteSwitch: {cell} priority {priority} call on gssi={gssi} pre-empts {talker:?} (priority {busy_priority})");
            self.sessions.remove(&busy_uuid);
        }

        // Same circuit talking again (speaker change on this cell): update the other cells.
        let refresh = self.sessions.iter_mut().find(|(_, s)| s.origin == Some(origin)).map(|(uuid, s)| {
            s.source_issi = source_issi;
            s.priority = priority;
            s.last_activity = Instant::now();
            (s.cells(), s.start_msg(*uuid))
        });
        if let Some((cells, start)) = refresh {
            for c in cells {
                self.deliver_cc(queue, c, start.clone());
            }
            return true;
        }

        // New talker: every other cell with members of the group hears it as a network call.
        let uuid = Uuid::new_v4();
        let mut s = GroupSession::new(gssi, source_issi, priority, Some(origin));
        let targets: Vec<CellId> = self.group_cells(gssi).into_iter().filter(|c| *c != cell).collect();
        for &t in &targets {
            s.pending.insert(t);
            self.deliver_cc(queue, t, s.start_msg(uuid));
        }
        tracing::info!("SiteSwitch: {cell} talking on gssi={gssi} (issi={source_issi}, priority {priority}) → {targets:?}");
        self.sessions.insert(uuid, s);
        true
    }

    /// Announced cell reselection: `issi` got D-NEW-CELL on `from` to move to the cell with
    /// `target_carrier`. Make the target cell's CMCE count it as a listener of its groups now, so
    /// that cell joins the group calls the MS is in (via `GroupListenersAvailable`) and its
    /// U-CALL RESTORE finds the call on arrival. Undone if the MS doesn't register there in time.
    fn prepare_handover(&mut self, queue: &mut MessageQueue, from: CellId, issi: u32, target_carrier: u16) {
        let Some(&target) = self.carrier_cell.get(&target_carrier) else {
            return; // Not one of ours (a configured external neighbour).
        };
        if target == from {
            return;
        }
        let groups = self.groups_of(from, issi);
        tracing::info!("SiteHandover: ISSI {issi} announced {from} → {target}, groups {groups:?}");
        if !groups.is_empty() {
            self.deliver(queue, target, subscriber_update_from_mm(issi, groups, BrewSubscriberAction::Affiliate));
        }

        // Individual (private/duplex) call: if the mover is a party to a cell-to-cell call,
        // pre-provision a restoring leg on the target cell so the MS's U-CALL RESTORE on arrival is
        // answered with D-CALL RESTORE (seamless) instead of being rejected. The call's routing is
        // NOT re-pointed yet — that happens only once the MS actually restores on the target, so a
        // handover the MS never completes cannot break the call.
        let mover_ind: Option<(Uuid, u32, bool, u8)> = self
            .cell_calls
            .iter()
            .find(|(_, c)| {
                // To the peer's own cell the legs are joined on arrival (`call-handoff`), no
                // second leg is needed.
                c.cell_of_issi(issi) == Some(from) && c.peer_issi(issi).is_some_and(|p| c.cell_of_issi(p) != Some(target))
            })
            .map(|(&uuid, c)| (uuid, c.peer_issi(issi).unwrap(), c.simplex_duplex, c.priority));
        if let Some((uuid, peer_issi, simplex_duplex, priority)) = mover_ind {
            tracing::info!("SiteHandover: ISSI {issi} in individual call {uuid} (peer {peer_issi}) {from} → {target}; pre-provisioning restoring leg");
            self.deliver_cc(
                queue,
                target,
                CallControl::SiteHandoverIndividualCall { brew_uuid: uuid, issi, peer_issi, simplex_duplex, priority },
            );
        }

        self.handovers.retain(|(i, _, _)| *i != issi);
        self.handovers.push((issi, target, Instant::now() + HANDOVER_TIMEOUT));
    }

    /// Undo announced handovers the MS never completed (it didn't register on the target cell).
    fn expire_handovers(&mut self, queue: &mut MessageQueue) {
        let now = Instant::now();
        let (expired, pending): (Vec<_>, Vec<_>) = self.handovers.drain(..).partition(|(_, _, deadline)| *deadline <= now);
        self.handovers = pending;
        for (issi, target, _) in expired {
            if self.location(issi) != Some(target) {
                tracing::info!("SiteHandover: ISSI {issi} never arrived on {target}, releasing");
                self.deliver(queue, target, subscriber_update_from_mm(issi, Vec::new(), BrewSubscriberAction::Deregister));
            }
        }
    }

    /// The talker on `origin` released the floor (or its call ended): the listening cells go to
    /// hangtime, from where any of their radios may take the floor next.
    fn local_floor_released(&mut self, queue: &mut MessageQueue, origin: CellCircuit) {
        let ended: Vec<Uuid> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.origin == Some(origin))
            .map(|(u, _)| *u)
            .collect();
        for uuid in ended {
            if let Some(s) = self.sessions.remove(&uuid) {
                for c in s.cells() {
                    self.deliver_cc(queue, c, CallControl::NetworkCallEnd { brew_uuid: uuid });
                }
            }
        }
    }

    /// Circuit-call signalling of a call between two cells: pass it to the other side.
    fn cell_call_signal(&mut self, queue: &mut MessageQueue, cell: CellId, uuid: Uuid, cc: &CallControl) {
        let Some(call) = self.cell_calls.get_mut(&uuid) else {
            return;
        };
        call.last_activity = Instant::now();
        let peer = call.peer(cell);
        match cc {
            CallControl::NetworkCircuitMediaReady { carrier_num, ts, .. } => {
                call.circuits.insert(cell, (*carrier_num, *ts));
            }
            CallControl::NetworkCircuitRelease { cause, .. }
                if *cause == DisconnectCause::ExpiryOfTimer.into_raw() as u8 && call.leg_lost.is_none() =>
            {
                // Uplink inactivity on this cell's leg: its radio may restore the call on the
                // other cell, which then joins both legs. Hold the release until then.
                tracing::info!(
                    "SiteSwitch: leg of call {uuid} on {cell} lost its uplink, holding the call {}s for a restore on {peer}",
                    CIRCUIT_RESTORE_WINDOW.as_secs()
                );
                call.circuits.remove(&cell);
                call.leg_lost = Some((cell, Instant::now() + CIRCUIT_RESTORE_WINDOW));
            }
            CallControl::NetworkCircuitRelease { .. } | CallControl::NetworkCircuitSetupReject { .. } => {
                let lost = call.leg_lost.map(|(c, _)| c);
                self.cell_calls.remove(&uuid);
                if lost != Some(peer) {
                    self.deliver_cc(queue, peer, cc.clone());
                }
            }
            _ => self.deliver_cc(queue, peer, cc.clone()),
        }
    }

    // ── Network → cells ─────────────────────────────────────────────────────────────────────

    /// The network entity emitted `message` towards the stack.
    fn route_from_network(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        match &mut message.msg {
            SapMsgInner::TmdCircuitDataReq(req) => {
                let (carrier_num, ts) = (req.carrier_num, req.ts);
                let copies = self
                    .sessions
                    .values_mut()
                    .find(|s| s.origin.is_none() && s.anchor.is_some_and(|a| a.carrier_num == carrier_num && a.ts == ts))
                    .map(|s| {
                        s.last_activity = Instant::now();
                        (s.replicas.clone(), s.anchor_live)
                    });
                let cell = self.cell_of_carrier(carrier_num);
                match copies {
                    Some((replicas, anchor_live)) => {
                        if let SapMsgInner::TmdCircuitDataReq(req) = &message.msg {
                            for r in replicas {
                                self.deliver_voice(r, &req.data);
                            }
                        }
                        if anchor_live {
                            self.deliver(queue, cell, message);
                        }
                    }
                    None => self.deliver(queue, cell, message),
                }
            }
            SapMsgInner::CmceCallControl(cc) => {
                if let Some(id) = call_id_mut(cc)
                    && let Some((cell, local)) = self.call_ids.local(*id)
                {
                    *id = local;
                    self.deliver(queue, cell, message);
                    return;
                }
                if let CallControl::NetworkCallStart { brew_uuid, dest_gssi, .. } = cc
                    && !net_brew::network_group_inbound_allowed_from_network(&self.primary, *dest_gssi)
                {
                    // What the primary CMCE did before the cells were linked: refuse it.
                    tracing::info!("SiteSwitch: network call uuid={brew_uuid} gssi={dest_gssi} not allowed inbound");
                    let end = SapMsg::new(
                        Sap::Control,
                        TetraEntity::Cmce,
                        TetraEntity::Brew,
                        SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: *brew_uuid }),
                    );
                    self.to_inner(queue, end);
                    return;
                }
                let targets = self.network_call_targets(cc);
                self.deliver_all(queue, targets, &message);
            }
            SapMsgInner::CmceSdsData(sds) => {
                let dest = sds.dest_issi;
                let targets = match self.location(dest) {
                    Some(cell) => BTreeSet::from([cell]),
                    None => self.group_cells(dest),
                };
                let targets = if targets.is_empty() {
                    BTreeSet::from([self.host])
                } else {
                    targets
                };
                self.deliver_all(queue, targets, &message);
            }
            // One of our radios left for another station: only its cell drops it (the other
            // cells would answer with a deregistration of their own).
            SapMsgInner::MmSubscriberUpdate(update)
                if update.action == BrewSubscriberAction::Deregister && self.location(update.issi).is_some() =>
            {
                let cell = self.location(update.issi).expect("checked");
                self.deliver(queue, cell, message);
            }
            // External-subscriber presence from the network: every cell keeps its own view.
            SapMsgInner::MmSubscriberUpdate(_) => {
                let cells = self.all_cells.clone();
                self.deliver_all(queue, cells, &message);
            }
            _ => self.deliver(queue, self.host, message),
        }
    }

    fn network_call_targets(&mut self, cc: &CallControl) -> BTreeSet<CellId> {
        let host = self.host;
        let primary = || BTreeSet::from([host]);
        match cc {
            CallControl::NetworkCallStart {
                brew_uuid,
                source_issi,
                dest_gssi,
                priority,
            } => {
                let mut targets = self.group_cells(*dest_gssi);
                let s = self
                    .sessions
                    .entry(*brew_uuid)
                    .or_insert_with(|| GroupSession::new(*dest_gssi, *source_issi, *priority, None));
                s.gssi = *dest_gssi;
                s.source_issi = *source_issi;
                s.priority = *priority;
                s.last_activity = Instant::now();
                targets.extend(s.cells());
                if targets.is_empty() {
                    // Nobody affiliated anywhere: the primary answers (Hold) as before.
                    targets = primary();
                }
                let already = s.cells();
                s.pending.extend(targets.iter().filter(|c| !already.contains(c)));
                targets
            }
            CallControl::NetworkCallEnd { brew_uuid } => match self.sessions.remove(brew_uuid) {
                Some(s) => {
                    let mut cells = s.cells();
                    if let Some(a) = s.anchor {
                        cells.insert(a.cell);
                    }
                    cells
                }
                None => self.all_cells.iter().copied().collect(),
            },
            CallControl::NetworkCallMediaActivity { brew_uuid } => match self.sessions.get_mut(brew_uuid) {
                Some(s) => {
                    s.last_activity = Instant::now();
                    s.cells()
                }
                None => self.all_cells.iter().copied().collect(),
            },
            CallControl::NetworkCircuitSetupRequest { brew_uuid, call } => {
                let home = self.location(call.destination);
                let cell = home.unwrap_or(self.host);
                let mut entry = CircuitCall::new(cell);
                entry.issi = call.destination;
                entry.peer = call.source_issi;
                entry.duplex = call.duplex == 1;
                entry.priority = call.priority;
                let siblings = home.map(|h| self.area_siblings(h)).unwrap_or_default();
                if !siblings.is_empty() {
                    // Within its location area the radio may have changed cell without
                    // registering (clause 18.3.4.7.1a): page it on every cell of the area.
                    tracing::info!(
                        "SiteSwitch: paging ISSI {} for individual call {brew_uuid} on {cell} and {:?}",
                        call.destination,
                        siblings
                    );
                    entry.paging = std::iter::once(cell).chain(siblings).collect();
                }
                let targets = entry.cells();
                self.circuit_calls.insert(*brew_uuid, entry);
                targets
            }
            CallControl::IndividualCallRestoreAnswer { issi, peer, call } => {
                let cell = self.restore_requests.remove(issi).or_else(|| self.location(*issi)).unwrap_or(self.host);
                if let Some(call) = call {
                    // The network moved the radio's call here from another station: this cell
                    // holds it now, as if the setup had been seen here.
                    tracing::info!("SiteSwitch: ISSI {issi} restores individual call {} (peer {peer}) from another station on {cell}", call.brew_uuid);
                    let mut entry = CircuitCall::new(cell);
                    entry.issi = *issi;
                    entry.peer = *peer;
                    entry.duplex = call.simplex_duplex;
                    entry.priority = call.priority;
                    entry.floor = call.floor_issi;
                    self.circuit_calls.insert(call.brew_uuid, entry);
                }
                BTreeSet::from([cell])
            }
            CallControl::HandoverResult { issi, .. } => {
                let cell = self
                    .station_asks
                    .remove(issi)
                    .map(|(cell, _)| cell)
                    .or_else(|| self.location(*issi))
                    .unwrap_or(self.host);
                BTreeSet::from([cell])
            }
            CallControl::SiteForwardRegistration { issi, target_carrier, .. } => {
                // Another station forwarded the registration of a radio coming to our cell on
                // `target_carrier` (the server picked this station by that carrier).
                let cell = self.cell_of_carrier(*target_carrier);
                self.station_forwards.retain(|_, (_, at)| at.elapsed() < STATION_HANDOVER_LIFETIME);
                self.station_forwards.insert(*issi, (cell, Instant::now()));
                BTreeSet::from([cell])
            }
            CallControl::HandoverPrepareGroupCall { issi, .. } => {
                let cell = self.station_forwards.get(issi).map(|(cell, _)| *cell).unwrap_or(self.host);
                BTreeSet::from([cell])
            }
            _ => {
                if let Some(uuid) = circuit_uuid(cc) {
                    let cells = match cc {
                        CallControl::NetworkCircuitRelease { .. } => self.circuit_calls.remove(&uuid).map(|c| c.cells()),
                        CallControl::NetworkCircuitSimplexGranted { .. } | CallControl::NetworkCircuitSimplexIdle { .. } => {
                            self.circuit_calls.get_mut(&uuid).map(|c| {
                                let granted = matches!(cc, CallControl::NetworkCircuitSimplexGranted { .. });
                                c.floor = granted.then_some(c.peer);
                                c.cells()
                            })
                        }
                        _ => self.circuit_calls.get(&uuid).map(|c| c.cells()),
                    };
                    return cells.unwrap_or_else(|| self.all_cells.iter().copied().collect());
                }
                match carrier_of(cc) {
                    Some(carrier) => BTreeSet::from([self.cell_of_carrier(carrier)]),
                    None => primary(),
                }
            }
        }
    }

    // ── Housekeeping ────────────────────────────────────────────────────────────────────────

    /// Copy the network entity's link state (kept in the primary's `StackState`) to the other
    /// cells, so they advertise the same network connection and accept network calls.
    fn mirror_link_state(&mut self) {
        let now = {
            let s = self.primary.state_read();
            (s.network_connected, s.brew_link_up, s.brew_soft_recovery_until)
        };
        if self.mirrored_link == Some(now) {
            return;
        }
        for (_, cfg) in &self.extra {
            let mut s = cfg.state_write();
            s.network_connected = now.0;
            s.brew_link_up = now.1;
            s.brew_soft_recovery_until = now.2;
        }
        self.mirrored_link = Some(now);
    }

    fn purge_idle(&mut self) {
        let before = self.sessions.len() + self.cell_calls.len();
        self.sessions.retain(|_, s| s.last_activity.elapsed() < SESSION_IDLE_TIMEOUT);
        self.cell_calls.retain(|_, c| c.last_activity.elapsed() < SESSION_IDLE_TIMEOUT);
        let purged = before - self.sessions.len() - self.cell_calls.len();
        if purged > 0 {
            tracing::debug!("SiteSwitch: dropped {purged} idle session(s)");
        }
    }
}

impl TetraEntityTrait for SiteSwitch {
    fn entity(&self) -> TetraEntity {
        self.inner.entity()
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        self.handle_from_cell(queue, self.host, message);
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.mirror_link_state();
        self.purge_idle();
        self.expire_handovers(queue);
        self.expire_circuit_restores(queue);
        self.expire_lost_cell_legs(queue);
        let incoming: Vec<(CellId, SapMsg)> = self.ports.from_cells.try_iter().collect();
        for (cell, message) in incoming {
            self.handle_from_cell(queue, cell, message);
        }
        self.with_inner(queue, |inner, out| inner.tick_start(out, ts));
        self.playout.drain(queue, ts);
    }

    fn tick_end(&mut self, queue: &mut MessageQueue, ts: TdmaTime) -> bool {
        let mut result = false;
        self.with_inner(queue, |inner, out| result = inner.tick_end(out, ts));
        result
    }
}

/// Carrier number → cell, for every cell (carriers are unique per cell).
pub(super) fn carrier_map(primary: &SharedConfig, extra: &[(CellId, SharedConfig)]) -> HashMap<u16, CellId> {
    let cells = std::iter::once((primary.config().cell_id, primary)).chain(extra.iter().map(|(id, c)| (*id, c)));
    let mut map = HashMap::new();
    for (id, cfg) in cells {
        for (carrier, _, _) in StackConfig::cell_phase_mod_carriers(&cfg.config().cell).unwrap_or_default() {
            map.insert(carrier, id);
        }
    }
    map
}

/// Release of a cell's individual call leg the radio left: "SwMI requested disconnection", which
/// CMCE does not report back to the network.
fn silent_circuit_release(uuid: Uuid) -> CallControl {
    CallControl::NetworkCircuitRelease {
        brew_uuid: uuid,
        cause: DisconnectCause::SwmiRequestedDisconnection.into_raw() as u8,
    }
}

/// Site-internal call control for a cell's MLE.
fn to_mle(cc: CallControl) -> SapMsg {
    SapMsg::new(Sap::Control, TetraEntity::Brew, TetraEntity::Mle, SapMsgInner::CmceCallControl(cc))
}

/// A subscriber update for a cell's CMCE as if from its own MM (so it counts as a local radio).
fn subscriber_update_from_mm(issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) -> SapMsg {
    SapMsg::new(
        Sap::Control,
        TetraEntity::Mm,
        TetraEntity::Cmce,
        SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
    )
}

fn call_id_mut(cc: &mut CallControl) -> Option<&mut u16> {
    match cc {
        CallControl::FloorGranted { call_id, .. }
        | CallControl::RemoteFloorGranted { call_id, .. }
        | CallControl::FloorReleased { call_id, .. }
        | CallControl::CallEnded { call_id, .. }
        | CallControl::NetworkCallReady { call_id, .. }
        | CallControl::OngoingGroupCall { call_id, .. }
        | CallControl::NetworkCircuitMediaReady { call_id, .. } => Some(call_id),
        _ => None,
    }
}

fn carrier_of(cc: &CallControl) -> Option<u16> {
    match cc {
        CallControl::Open(c) => Some(c.carrier_num),
        CallControl::CloseSlot { carrier_num, .. }
        | CallControl::SetDlMediaSource { carrier_num, .. }
        | CallControl::FloorGranted { carrier_num, .. }
        | CallControl::RemoteFloorGranted { carrier_num, .. }
        | CallControl::FloorReleased { carrier_num, .. }
        | CallControl::CallEnded { carrier_num, .. }
        | CallControl::NetworkCallReady { carrier_num, .. }
        | CallControl::OngoingGroupCall { carrier_num, .. }
        | CallControl::UlInactivityTimeout { carrier_num, .. }
        | CallControl::TrafficUlActivity { carrier_num, .. }
        | CallControl::NetworkCircuitMediaReady { carrier_num, .. } => Some(*carrier_num),
        _ => None,
    }
}

/// Brew session of an individual (circuit-mode) call message.
pub(super) fn circuit_uuid(cc: &CallControl) -> Option<Uuid> {
    match cc {
        CallControl::NetworkCircuitSetupRequest { brew_uuid, .. }
        | CallControl::NetworkCircuitSetupAccept { brew_uuid }
        | CallControl::NetworkCircuitSetupReject { brew_uuid, .. }
        | CallControl::NetworkCircuitAlert { brew_uuid }
        | CallControl::NetworkCircuitConnectRequest { brew_uuid, .. }
        | CallControl::NetworkCircuitConnectConfirm { brew_uuid, .. }
        | CallControl::NetworkCircuitSimplexGranted { brew_uuid, .. }
        | CallControl::NetworkCircuitSimplexIdle { brew_uuid, .. }
        | CallControl::NetworkCircuitMediaReady { brew_uuid, .. }
        | CallControl::NetworkCircuitDtmf { brew_uuid, .. }
        | CallControl::NetworkCircuitRelease { brew_uuid, .. } => Some(*brew_uuid),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tetra_config::bluestation::parsing;
    use tetra_saps::control::call_control::{HandoverOutcome, NetworkCircuitCall};
    use tetra_saps::control::enums::sds_user_data::SdsUserData;
    use tetra_saps::control::sds::CmceSdsData;
    use tetra_saps::tmd::TmdCircuitDataInd;

    use super::*;

    const C0: u16 = 1521;
    const C1: u16 = 1525;
    /// Frames to queue so a playout buffer starts. Frames pushed back-to-back look maximally
    /// jittery, which drives the adaptive depth to its 12-frame ceiling.
    const PLAYOUT_FILL: usize = 12;
    /// Talkgroup inside `local_ssi_ranges`: cross-cell only, never sent to Brew.
    const LOCAL_TG: u32 = 5001;

    const CONFIG: &str = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
local_ssi_ranges = [[5000, 5999]]

[brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"

[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
"#;

    /// Network entity stand-in: records what it receives, emits `outbox` on the next tick.
    #[derive(Default, Clone)]
    struct MockNet {
        got: Arc<Mutex<Vec<SapMsg>>>,
        outbox: Arc<Mutex<Vec<SapMsg>>>,
    }

    impl TetraEntityTrait for MockNet {
        fn entity(&self) -> TetraEntity {
            TetraEntity::Brew
        }
        fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
            self.got.lock().unwrap().push(message);
        }
        fn tick_start(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) {
            for m in self.outbox.lock().unwrap().drain(..) {
                queue.push_back(m);
            }
        }
    }

    struct Site {
        net: MockNet,
        switch: SiteSwitch,
        link: CellLink,
        primary: SharedConfig,
        cell1: SharedConfig,
        q0: MessageQueue,
        q1: MessageQueue,
    }

    impl Site {
        fn new() -> Self {
            Self::with_config(CONFIG)
        }

        /// Cell 1 in its own location area (2; the primary is in 1).
        fn own_location_areas() -> Self {
            Self::with_config(&format!("{CONFIG}location_area = 2\n"))
        }

        fn with_config(config: &str) -> Self {
            let cfg = parsing::from_toml_str(config).unwrap();
            let cell1 = SharedConfig::from_parts(cfg.for_extra_cell(CellId(1)).unwrap(), None);
            let primary = SharedConfig::from_parts(cfg, None);
            let net = MockNet::default();
            let (ports, mut links) = site_links(&[CellId(1)]);
            Self {
                switch: SiteSwitch::new(
                    Box::new(net.clone()),
                    primary.clone(),
                    vec![(CellId(1), cell1.clone())],
                    ports,
                    SharedDirectory::default(),
                ),
                net,
                link: links.remove(&CellId(1)).unwrap(),
                primary,
                cell1,
                q0: MessageQueue::new(),
                q1: MessageQueue::new(),
            }
        }

        /// A cell's stack sends `msg` to the network entity.
        fn from_cell(&mut self, cell: u8, msg: SapMsgInner) {
            let m = SapMsg::new(Sap::Control, TetraEntity::Cmce, TetraEntity::Brew, msg);
            if cell == 0 {
                self.switch.rx_prim(&mut self.q0, m);
            } else {
                self.link.rx_prim(&mut self.q1, m);
            }
        }

        /// The network entity emits `msg`.
        fn from_net(&mut self, msg: SapMsgInner) {
            let m = SapMsg::new(Sap::Control, TetraEntity::Brew, TetraEntity::Cmce, msg);
            self.net.outbox.lock().unwrap().push(m);
        }

        /// One primary tick, then collect what each cell received.
        fn tick(&mut self) -> (Vec<SapMsgInner>, Vec<SapMsgInner>) {
            // Timeslot 2 = the test circuits' slot, so copied voice can play out.
            let now = TdmaTime { h: 0, m: 1, f: 1, t: 2 };
            self.switch.tick_start(&mut self.q0, now);
            self.link.tick_start(&mut self.q1, now);
            let drain = |q: &mut MessageQueue| std::iter::from_fn(|| q.pop_front()).map(|m| m.msg).collect();
            (drain(&mut self.q0), drain(&mut self.q1))
        }

        fn net_got(&self) -> Vec<SapMsgInner> {
            self.net.got.lock().unwrap().drain(..).map(|m| m.msg).collect()
        }

        /// Radio `issi` affiliated to `gssi` on `cell`; clears everything that produced.
        fn member(&mut self, cell: u8, issi: u32, gssi: u32) {
            self.from_cell(
                cell,
                SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                    issi,
                    groups: vec![gssi],
                    action: BrewSubscriberAction::Affiliate,
                }),
            );
            self.tick();
            self.net_got();
        }
    }

    fn cc(c: CallControl) -> SapMsgInner {
        SapMsgInner::CmceCallControl(c)
    }

    fn start_uuid(msgs: &[SapMsgInner]) -> Option<Uuid> {
        msgs.iter().find_map(|m| match m {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallStart { brew_uuid, .. }) => Some(*brew_uuid),
            _ => None,
        })
    }

    fn voice_on(msgs: &[SapMsgInner], carrier: u16) -> bool {
        msgs.iter()
            .any(|m| matches!(m, SapMsgInner::TmdCircuitDataReq(r) if r.carrier_num == carrier && r.ts == 2))
    }

    fn has_end(msgs: &[SapMsgInner]) -> bool {
        msgs.iter()
            .any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { .. })))
    }

    fn granted(call_id: u16, issi: u32, gssi: u32, carrier_num: u16) -> SapMsgInner {
        cc(CallControl::FloorGranted {
            call_id,
            source_issi: issi,
            dest_gssi: gssi,
            carrier_num,
            ts: 2,
        })
    }

    fn uplink(carrier_num: u16) -> SapMsgInner {
        SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
            carrier_num,
            ts: 2,
            data: vec![9; 4],
        })
    }

    // ── Network calls (phase 3) ─────────────────────────────────────────────────────────────

    #[test]
    fn network_group_call_fans_out_and_copies_voice() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        let uuid = Uuid::from_u128(1);
        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: uuid,
            source_issi: 9000,
            dest_gssi: 91,
            priority: 0,
        }));
        let (c0, c1) = site.tick();
        assert!(start_uuid(&c0).is_some() && start_uuid(&c1).is_some(), "both cells have members");

        let ready = |call_id, carrier_num| {
            cc(CallControl::NetworkCallReady {
                brew_uuid: uuid,
                call_id,
                carrier_num,
                ts: 2,
                usage: 4,
            })
        };
        // Cell 1 answers first (its link is drained on the next primary tick).
        site.from_cell(1, ready(7, C1));
        site.tick();
        site.from_cell(0, ready(7, C0));
        site.tick();
        let readies: Vec<_> = site
            .net_got()
            .into_iter()
            .filter(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { .. })))
            .collect();
        assert_eq!(readies.len(), 1, "only the first Ready reaches the network entity");
        assert!(matches!(
            readies[0],
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { carrier_num: C1, .. })
        ));

        // DL voice for the anchor circuit (cell 1) is copied to cell 0's circuit, which plays it
        // out from its own buffer once enough frames are queued.
        for _ in 0..PLAYOUT_FILL {
            site.net.outbox.lock().unwrap().push(SapMsg::new(
                Sap::TmdSap,
                TetraEntity::Brew,
                TetraEntity::Umac,
                SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                    carrier_num: C1,
                    ts: 2,
                    data: vec![1, 2, 3],
                }),
            ));
        }
        let (c0, c1) = site.tick();
        assert!(voice_on(&c1, C1), "anchor cell gets Brew's paced voice directly");
        assert!(voice_on(&c0, C0), "replica plays the copy");

        // Cell 1 leaving alone is swallowed; the call goes on for cell 0.
        site.from_cell(1, cc(CallControl::NetworkCallEnd { brew_uuid: uuid }));
        site.tick();
        assert!(site.net_got().is_empty());

        site.from_net(cc(CallControl::NetworkCallEnd { brew_uuid: uuid }));
        let (c0, _) = site.tick();
        assert!(has_end(&c0));
    }

    #[test]
    fn hold_is_forwarded_only_when_every_cell_holds() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        let uuid = Uuid::from_u128(2);
        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: uuid,
            source_issi: 9000,
            dest_gssi: 91,
            priority: 0,
        }));
        site.tick();
        let hold = cc(CallControl::NetworkCallHold {
            brew_uuid: uuid,
            dest_gssi: 91,
        });
        site.from_cell(0, hold.clone());
        site.tick();
        assert!(site.net_got().is_empty(), "cell 1 has not answered yet");
        site.from_cell(1, hold);
        site.tick();
        assert_eq!(site.net_got().len(), 1);
    }

    #[test]
    fn network_call_to_group_not_allowed_inbound_is_refused() {
        let mut site = Site::new();
        site.member(0, 100, LOCAL_TG);
        let uuid = Uuid::from_u128(3);
        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: uuid,
            source_issi: 9000,
            dest_gssi: LOCAL_TG,
            priority: 0,
        }));
        let (c0, c1) = site.tick();
        assert!(start_uuid(&c0).is_none() && start_uuid(&c1).is_none());
        assert!(has_end(&site.net_got()), "refused back to the network entity");
    }

    #[test]
    fn sds_between_cells_is_delivered_directly() {
        let mut site = Site::new();
        site.member(0, 100, 91);

        site.from_cell(
            1,
            SapMsgInner::CmceSdsData(CmceSdsData {
                source_issi: 200,
                dest_issi: 100,
                user_defined_data: SdsUserData::Type1(0x8001),
            }),
        );
        let (c0, _) = site.tick();
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceSdsData(s) if s.dest_issi == 100)));
        assert!(site.net_got().is_empty(), "local SDS must not go out to the network");
    }

    /// A network the primary does not serve: its switch runs in its home cell (cell 1 here),
    /// with cell 2 linked to it. Messages for the host go onto the host's own queue, the other
    /// cell's through its link, and a local network (no core) keeps everything on site.
    #[test]
    fn a_switch_hosted_by_an_additional_cell_routes_between_its_cells() {
        const TWO_NETWORKS: &str = r#"
network = "tetrapack"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
local_ssi_ranges = [[5000, 5999]]

[networks.tetrapack]
[networks.tetrapack.brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"

[networks.club]
mcc = 901
mnc = 1

[[cells]]
id = 1
network = "club"
[cells.cell_info]
main_carrier = 1525

[[cells]]
id = 2
network = "club"
[cells.cell_info]
main_carrier = 1529
"#;
        let cfg = parsing::from_toml_str(TWO_NETWORKS).unwrap();
        assert_eq!(cfg.network_home_cell("club"), Some(CellId(1)));
        let host = SharedConfig::from_parts(cfg.for_extra_cell(CellId(1)).unwrap(), None);
        let cell2 = SharedConfig::from_parts(cfg.for_extra_cell(CellId(2)).unwrap(), None);
        assert!(host.config().brew.is_none() && host.config().site_linked);
        let net = MockNet::default();
        let (ports, mut links) = site_links(&[CellId(2)]);
        let mut switch = SiteSwitch::new(Box::new(net.clone()), host.clone(), vec![(CellId(2), cell2)], ports, SharedDirectory::default());
        let mut link = links.remove(&CellId(2)).unwrap();
        let (mut q1, mut q2) = (MessageQueue::new(), MessageQueue::new());
        let now = TdmaTime { h: 0, m: 1, f: 1, t: 2 };
        let drain = |q: &mut MessageQueue| std::iter::from_fn(|| q.pop_front()).map(|m| m.msg).collect::<Vec<_>>();

        // A radio on the host cell affiliates to group 91; cell 2 sends it an SDS: delivered on
        // the host's queue, nothing reaches the (absent) core.
        let to_brew = |msg| SapMsg::new(Sap::Control, TetraEntity::Cmce, TetraEntity::Brew, msg);
        switch.rx_prim(&mut q1, to_brew(SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi: 100, groups: vec![91], action: BrewSubscriberAction::Affiliate })));
        switch.tick_start(&mut q1, now);
        drain(&mut q1);
        link.rx_prim(&mut q2, to_brew(SapMsgInner::CmceSdsData(CmceSdsData { source_issi: 200, dest_issi: 100, user_defined_data: SdsUserData::Type1(0x8001) })));
        switch.tick_start(&mut q1, now);
        link.tick_start(&mut q2, now);
        let on_host = drain(&mut q1);
        assert!(on_host.iter().any(|m| matches!(m, SapMsgInner::CmceSdsData(s) if s.dest_issi == 100)), "SDS reaches the host cell's radio: {on_host:?}");
        assert!(drain(&mut q2).is_empty());
        assert!(net.got.lock().unwrap().is_empty(), "a local network has no core to forward to");

        // A radio on cell 2 talks on group 91: the host cell hears it as a network call.
        link.rx_prim(&mut q2, to_brew(granted(7, 200, 91, 1529)));
        switch.tick_start(&mut q1, now);
        link.tick_start(&mut q2, now);
        let on_host = drain(&mut q1);
        assert!(start_uuid(&on_host).is_some(), "the host cell gets the call start: {on_host:?}");
        assert!(net.got.lock().unwrap().is_empty());
    }

    #[test]
    fn call_ids_are_unique_towards_the_network() {
        let mut site = Site::new();
        site.from_cell(0, granted(4, 100, 91, C0));
        site.from_cell(1, granted(4, 200, 92, C1));
        site.tick();
        let ids: Vec<u16> = site
            .net_got()
            .iter()
            .filter_map(|m| match m {
                SapMsgInner::CmceCallControl(CallControl::FloorGranted { call_id, .. }) => Some(*call_id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);

        // A reply carrying cell 1's global id goes back to cell 1 with its own id.
        site.from_net(cc(CallControl::FloorReleased {
            call_id: ids[1],
            carrier_num: C1,
            ts: 2,
        }));
        let (c0, c1) = site.tick();
        assert!(c0.is_empty());
        assert!(matches!(
            c1.as_slice(),
            [SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id: 4, .. })]
        ));
    }

    // ── Calls between cells (phase 4) ───────────────────────────────────────────────────────

    #[test]
    fn radio_group_call_is_heard_on_other_cells() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        site.from_cell(0, granted(4, 100, 91, C0));
        let (_, c1) = site.tick();
        let uuid = start_uuid(&c1).expect("cell 1 gets the call as a network call");
        assert!(
            site.net_got()
                .iter()
                .any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. }))),
            "a Brew-routable group still goes to the network"
        );

        site.from_cell(
            1,
            cc(CallControl::NetworkCallReady {
                brew_uuid: uuid,
                call_id: 9,
                carrier_num: C1,
                ts: 2,
                usage: 4,
            }),
        );
        site.tick();
        assert!(site.net_got().is_empty(), "the network entity never sees the switch's own session");

        for _ in 0..PLAYOUT_FILL {
            site.from_cell(0, uplink(C0));
        }
        let (_, c1) = site.tick();
        assert!(voice_on(&c1, C1), "talker's voice copied to cell 1");

        site.from_cell(
            0,
            cc(CallControl::FloorReleased {
                call_id: 4,
                carrier_num: C0,
                ts: 2,
            }),
        );
        let (_, c1) = site.tick();
        assert!(has_end(&c1), "cell 1 goes to hangtime");
    }

    #[test]
    fn one_talker_per_group_across_cells() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        site.from_cell(0, granted(4, 100, 91, C0));
        site.tick();
        site.net_got();

        // Cell 1's radio got a local grant before the call reached it: it loses.
        site.from_cell(1, granted(6, 200, 91, C1));
        let (c0, _) = site.tick();
        assert!(start_uuid(&c0).is_none(), "cell 0 keeps talking");
        assert!(site.net_got().is_empty(), "losing grant never reaches the network");

        site.from_cell(
            1,
            cc(CallControl::FloorReleased {
                call_id: 6,
                carrier_num: C1,
                ts: 2,
            }),
        );
        site.tick();
        assert!(site.net_got().is_empty(), "nor does its release");
    }

    #[test]
    fn local_only_group_links_cells_but_stays_off_the_network() {
        let mut site = Site::new();
        site.member(0, 100, LOCAL_TG);
        site.member(1, 200, LOCAL_TG);

        site.from_cell(0, granted(4, 100, LOCAL_TG, C0));
        let (_, c1) = site.tick();
        assert!(start_uuid(&c1).is_some());
        assert!(site.net_got().is_empty());
    }

    #[test]
    fn individual_call_between_cells() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        let uuid = Uuid::from_u128(40);
        let call = NetworkCircuitCall {
            source_issi: 200,
            destination: 100,
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
        };
        site.from_cell(1, cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call }));
        let (c0, _) = site.tick();
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. }))));

        site.from_cell(0, cc(CallControl::NetworkCircuitSetupAccept { brew_uuid: uuid }));
        let (_, c1) = site.tick();
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { .. }))));

        let media = |carrier_num| {
            cc(CallControl::NetworkCircuitMediaReady {
                brew_uuid: uuid,
                call_id: 5,
                carrier_num,
                ts: 2,
            })
        };
        site.from_cell(0, media(C0));
        site.from_cell(1, media(C1));
        site.tick();
        for _ in 0..PLAYOUT_FILL {
            site.from_cell(0, uplink(C0));
            site.from_cell(1, uplink(C1));
        }
        // Cell 1's uplink reaches the switch on this tick; cell 1 plays cell 0's copy on the next.
        site.tick();
        let (c0, c1) = site.tick();
        assert!(voice_on(&c1, C1) && voice_on(&c0, C0), "voice both ways");

        site.from_cell(0, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: 1 }));
        let (_, c1) = site.tick();
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { .. }))));
        assert!(
            !site.net_got().iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(_))),
            "the whole call stays on site"
        );
    }

    #[test]
    fn network_link_state_is_mirrored_to_other_cells() {
        let mut site = Site::new();
        {
            let mut s = site.primary.state_write();
            s.network_connected = true;
            s.brew_link_up = true;
        }
        site.tick();
        let s = site.cell1.state_read();
        assert!(s.network_connected && s.brew_link_up);
    }

    // ── Mobility (phase 5) ──────────────────────────────────────────────────────────────────

    fn register(issi: u32) -> SapMsgInner {
        SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![],
            action: BrewSubscriberAction::Register,
        })
    }

    #[test]
    fn radio_moving_between_cells_is_dropped_on_the_old_cell_only() {
        let mut site = Site::new();
        site.from_cell(0, register(100));
        site.tick();
        site.net_got();

        // Reselects to cell 1 and registers there.
        site.from_cell(1, register(100));
        let (c0, _) = site.tick();
        assert!(
            c0.iter().any(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u)
                if u.issi == 100 && u.action == BrewSubscriberAction::Deregister)),
            "cell 0 is told to drop the stale registration"
        );
        assert!(
            site.net_got().iter().any(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u)
                if u.action == BrewSubscriberAction::Register)),
            "the network sees the new registration"
        );

        // Cell 0's cleanup must not deregister the radio on the network.
        site.from_cell(
            0,
            SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                issi: 100,
                groups: vec![],
                action: BrewSubscriberAction::Deregister,
            }),
        );
        site.tick();
        assert!(site.net_got().is_empty());

        // SDS for it now goes to cell 1.
        site.from_net(SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 9000,
            dest_issi: 100,
            user_defined_data: SdsUserData::Type1(1),
        }));
        let (c0, c1) = site.tick();
        assert!(c0.is_empty() && c1.iter().any(|m| matches!(m, SapMsgInner::CmceSdsData(_))));
    }

    #[test]
    fn copied_voice_plays_out_on_the_receiving_cells_timeslot() {
        let mut p = VoicePlayout::default();
        for i in 0..PLAYOUT_FILL {
            p.push(C1, 2, vec![i as u8]);
        }
        let mut q = MessageQueue::new();
        p.drain(&mut q, TdmaTime { h: 0, m: 1, f: 1, t: 3 });
        assert!(q.pop_front().is_none(), "not this circuit's slot");
        p.drain(&mut q, TdmaTime { h: 0, m: 1, f: 18, t: 2 });
        assert!(q.pop_front().is_none(), "no traffic in frame 18");
        p.drain(&mut q, TdmaTime { h: 0, m: 1, f: 2, t: 2 });
        let m = q.pop_front().expect("one frame on its slot");
        assert!(matches!(m.msg, SapMsgInner::TmdCircuitDataReq(ref r) if r.data == vec![0]), "in order");
        assert!(q.pop_front().is_none(), "one frame per slot");
    }

    #[test]
    fn emergency_call_takes_the_floor_across_cells() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        site.from_cell(0, granted(4, 100, 91, C0));
        site.tick();
        site.net_got();

        // Cell 1 starts an emergency call on the same group while cell 0 talks.
        site.from_cell(1, cc(CallControl::SiteCallPriority { call_id: 6, priority: 15 }));
        site.from_cell(1, granted(6, 200, 91, C1));
        let (c0, _) = site.tick();
        assert!(
            c0.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCallStart { priority: 15, .. }))),
            "cell 0 gets the emergency call at its priority (its CMCE pre-empts the local talker)"
        );
        let got = site.net_got();
        assert!(
            got.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. }))),
            "the emergency floor reaches the network"
        );
        assert!(
            !got.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::SiteCallPriority { .. }))),
            "the priority hint itself never does"
        );
    }

    #[test]
    fn individual_call_handover_to_the_peers_cell_preprovisions_no_leg() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);

        // Cell-to-cell individual (duplex) call: 200 (cell 1) → 100 (cell 0), priority 3.
        let uuid = Uuid::from_u128(77);
        let call = NetworkCircuitCall {
            source_issi: 200,
            destination: 100,
            number: String::new(),
            priority: 3,
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
        };
        site.from_cell(1, cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call }));
        site.tick();

        // ISSI 100 (on cell 0) reselects to cell 1, where its peer 200 is: the target cell joins
        // the two legs locally when 100 restores there, so no second leg is pre-provisioned (it
        // would be a second call of the same pair on that cell).
        site.from_cell(0, cc(CallControl::SiteHandoverPrepare { issi: 100, target_carrier: C1 }));
        let (_, c1) = site.tick();
        assert!(
            !c1.iter().any(|m| matches!(m,
                SapMsgInner::CmceCallControl(CallControl::SiteHandoverIndividualCall { brew_uuid, .. }) if *brew_uuid == uuid)),
            "no pre-provisioned leg on the peer's own cell"
        );
    }

    #[test]
    fn announced_handover_prepares_the_target_cell_and_is_undone_if_the_ms_never_arrives() {
        let mut site = Site::new();
        site.member(0, 100, 91);

        site.from_cell(0, cc(CallControl::SiteHandoverPrepare { issi: 100, target_carrier: C1 }));
        let (_, c1) = site.tick();
        assert!(
            c1.iter().any(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u)
                if u.issi == 100 && u.action == BrewSubscriberAction::Affiliate && u.groups == vec![91])),
            "target cell counts the MS as a listener of its groups"
        );
        assert!(site.net_got().is_empty(), "handover notice never reaches the network");

        // The MS never registers on cell 1: after the timeout cell 1 releases it.
        for h in &mut site.switch.handovers {
            h.2 = Instant::now();
        }
        let (_, c1) = site.tick();
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u)
            if u.issi == 100 && u.action == BrewSubscriberAction::Deregister)));
    }

    #[test]
    fn completed_handover_is_not_undone() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.from_cell(0, cc(CallControl::SiteHandoverPrepare { issi: 100, target_carrier: C1 }));
        site.tick();
        site.from_cell(1, register(100));
        site.tick();
        for h in &mut site.switch.handovers {
            h.2 = Instant::now();
        }
        let (_, c1) = site.tick();
        assert!(!c1.iter().any(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u) if u.action == BrewSubscriberAction::Deregister)));
    }

    fn sds(source_issi: u32, dest_issi: u32) -> SapMsgInner {
        SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi,
            dest_issi,
            user_defined_data: SdsUserData::Type1(0x8001),
        })
    }

    #[test]
    fn group_sds_reaches_members_on_other_cells_but_not_the_network() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.member(1, 200, 91);
        site.from_cell(0, sds(100, 91));
        let (_, c1) = site.tick();
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::CmceSdsData(s) if s.dest_issi == 91)));
        assert!(site.net_got().is_empty(), "the group has members on site");
    }

    #[test]
    fn sds_for_nobody_on_site_goes_to_the_network() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.from_cell(0, sds(100, 777_777));
        let (_, c1) = site.tick();
        assert!(c1.is_empty());
        assert!(site.net_got().iter().any(|m| matches!(m, SapMsgInner::CmceSdsData(s) if s.dest_issi == 777_777)));
    }

    // ── Asterisk relay ──────────────────────────────────────────────────────────────────────

    #[test]
    fn asterisk_relay_serves_radios_on_every_cell() {
        use super::super::SiteRelay;

        let cfg = parsing::from_toml_str(CONFIG).unwrap();
        let cell1 = SharedConfig::from_parts(cfg.for_extra_cell(CellId(1)).unwrap(), None);
        let primary = SharedConfig::from_parts(cfg, None);
        let (mut ports, mut links) = site_links(&[CellId(1)]);
        let directory = SharedDirectory::default();
        directory.write().unwrap().apply(
            CellId(1),
            &MmSubscriberUpdate {
                issi: 200,
                groups: vec![],
                action: BrewSubscriberAction::Register,
            },
        );
        let asterisk = MockNet::default();
        let mut relay = SiteRelay::new(
            Box::new(asterisk.clone()),
            &primary,
            &[(CellId(1), cell1)],
            ports.take_asterisk().unwrap(),
            directory,
        );
        let mut link = links.remove(&CellId(1)).unwrap();
        let mut ast_link = link.asterisk_link();
        let (mut q0, mut q1) = (MessageQueue::new(), MessageQueue::new());
        let now = TdmaTime { h: 0, m: 1, f: 1, t: 2 };
        let drain = |q: &mut MessageQueue| std::iter::from_fn(|| q.pop_front()).map(|m| m.msg).collect::<Vec<_>>();

        // A radio on cell 1 calls a PBX number: the request reaches Asterisk.
        let uuid = Uuid::from_u128(77);
        let call = NetworkCircuitCall {
            source_issi: 200,
            destination: 0,
            number: "100".into(),
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
        };
        ast_link.rx_prim(
            &mut q1,
            SapMsg::new(
                Sap::Control,
                TetraEntity::Cmce,
                TetraEntity::Asterisk,
                cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call: call.clone() }),
            ),
        );
        relay.tick_start(&mut q0, now);
        assert!(asterisk.got.lock().unwrap().iter().any(|m| matches!(m.msg, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. }))));

        // Asterisk answers and sends voice: both reach cell 1 (voice via its playout buffer).
        {
            let mut out = asterisk.outbox.lock().unwrap();
            out.push(SapMsg::new(
                Sap::Control,
                TetraEntity::Asterisk,
                TetraEntity::Cmce,
                cc(CallControl::NetworkCircuitSetupAccept { brew_uuid: uuid }),
            ));
            for _ in 0..PLAYOUT_FILL {
                out.push(SapMsg::new(
                    Sap::TmdSap,
                    TetraEntity::Asterisk,
                    TetraEntity::Umac,
                    SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                        carrier_num: C1,
                        ts: 2,
                        data: vec![5; 4],
                    }),
                ));
            }
        }
        relay.tick_start(&mut q0, now);
        assert!(drain(&mut q0).is_empty(), "nothing for the primary");
        link.tick_start(&mut q1, now);
        let c1 = drain(&mut q1);
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { .. }))));
        assert!(voice_on(&c1, C1));

        // A SIP call to radio 200 goes to the cell it is registered on.
        asterisk.outbox.lock().unwrap().push(SapMsg::new(
            Sap::Control,
            TetraEntity::Asterisk,
            TetraEntity::Cmce,
            cc(CallControl::NetworkCircuitSetupRequest {
                brew_uuid: Uuid::from_u128(78),
                call: NetworkCircuitCall { destination: 200, ..call },
            }),
        ));
        relay.tick_start(&mut q0, now);
        link.tick_start(&mut q1, now);
        assert!(drain(&mut q0).is_empty());
        assert!(drain(&mut q1).iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. }))));
    }

    #[test]
    fn forwarded_registration_goes_to_the_target_cells_mle_and_back() {
        let mut site = Site::new();
        let demand = tetra_core::BitBuffer::from_bitstr("0010");
        site.from_cell(0, cc(CallControl::SiteForwardRegistration { issi: 100, target_carrier: C1, sdu: demand }));
        let (_, c1) = site.tick();
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistration { issi: 100, .. }))));

        site.from_cell(1, cc(CallControl::SiteForwardRegistrationResult { issi: 100, sdu: Some(tetra_core::BitBuffer::from_bitstr("0101")) }));
        let (c0, _) = site.tick();
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistrationResult { issi: 100, sdu: Some(_) }))));
        assert!(site.net_got().is_empty(), "site-internal, never to the network");
    }

    #[test]
    fn forwarded_registration_to_a_foreign_cell_is_answered_at_once() {
        let mut site = Site::new();
        let demand = tetra_core::BitBuffer::from_bitstr("0010");
        site.from_cell(0, cc(CallControl::SiteForwardRegistration { issi: 100, target_carrier: 3000, sdu: demand }));
        let (c0, c1) = site.tick();
        assert!(c1.is_empty());
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistrationResult { sdu: None, .. }))));
    }

    // ── Individual calls with the network across cells (milestone 7) ──────────────────────

    const RADIO: u32 = 2320972;
    const APP: u32 = 2320001;

    fn circuit(source_issi: u32, destination: u32, duplex: bool) -> NetworkCircuitCall {
        NetworkCircuitCall {
            source_issi,
            destination,
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
        }
    }

    fn connect_request(uuid: Uuid, duplex: bool) -> SapMsgInner {
        cc(CallControl::NetworkCircuitConnectRequest {
            brew_uuid: uuid,
            call: circuit(RADIO, APP, duplex),
        })
    }

    fn setup_reaches(msgs: &[SapMsgInner], uuid: Uuid) -> bool {
        msgs.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, .. }) if *brew_uuid == uuid))
    }

    fn net_got_cc(site: &Site, f: impl Fn(&CallControl) -> bool) -> usize {
        site.net_got().iter().filter(|m| matches!(m, SapMsgInner::CmceCallControl(c) if f(c))).count()
    }

    #[test]
    fn a_network_call_pages_the_radio_on_every_cell_of_its_location_area() {
        let mut site = Site::new();
        site.from_cell(0, register(RADIO));
        site.tick();
        let uuid = Uuid::from_u128(71);
        site.from_net(cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call: circuit(APP, RADIO, true) }));
        let (c0, c1) = site.tick();
        assert!(setup_reaches(&c0, uuid) && setup_reaches(&c1, uuid), "paged on both cells");

        // Both cells accept the setup; the network hears it once.
        let accept = cc(CallControl::NetworkCircuitSetupAccept { brew_uuid: uuid });
        site.from_cell(0, accept.clone());
        site.from_cell(1, accept);
        site.tick();
        assert_eq!(net_got_cc(&site, |c| matches!(c, CallControl::NetworkCircuitSetupAccept { .. })), 1);

        // The radio moved idle to cell 1 and answers there: cell 0 lets go without a release to the network.
        site.from_cell(1, connect_request(uuid, true));
        let (c0, _) = site.tick();
        assert_eq!(release_cause(&c0, uuid), Some(DisconnectCause::SwmiRequestedDisconnection.into_raw() as u8));
        assert_eq!(net_got_cc(&site, |c| matches!(c, CallControl::NetworkCircuitConnectRequest { .. })), 1);
        site.from_cell(0, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: 14 }));
        site.from_cell(1, media(uuid, 4, C1));
        site.tick();
        let got = site.net_got();
        assert!(!got.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { .. }))));
        assert!(got.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { .. }))));

        // From now on the call is cell 1's alone.
        site.from_net(cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: 1 }));
        let (c0, c1) = site.tick();
        assert!(release_cause(&c1, uuid).is_some() && release_cause(&c0, uuid).is_none());
    }

    #[test]
    fn a_paged_radio_answering_nowhere_is_rejected_once() {
        let mut site = Site::new();
        site.from_cell(0, register(RADIO));
        site.tick();
        let uuid = Uuid::from_u128(72);
        site.from_net(cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call: circuit(APP, RADIO, false) }));
        site.tick();
        site.net_got();
        let reject = cc(CallControl::NetworkCircuitSetupReject { brew_uuid: uuid, cause: 8 });
        site.from_cell(1, reject.clone());
        site.tick();
        assert_eq!(net_got_cc(&site, |c| matches!(c, CallControl::NetworkCircuitSetupReject { .. })), 0, "cell 0 still pages");
        site.from_cell(0, reject);
        site.tick();
        assert_eq!(net_got_cc(&site, |c| matches!(c, CallControl::NetworkCircuitSetupReject { .. })), 1);
    }

    #[test]
    fn with_own_location_areas_a_network_call_goes_to_the_registered_cell_only() {
        let mut site = Site::own_location_areas();
        site.from_cell(0, register(RADIO));
        site.tick();
        let uuid = Uuid::from_u128(73);
        site.from_net(cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call: circuit(APP, RADIO, true) }));
        let (c0, c1) = site.tick();
        assert!(setup_reaches(&c0, uuid) && !setup_reaches(&c1, uuid));
    }

    fn media(uuid: Uuid, call_id: u16, carrier_num: u16) -> SapMsgInner {
        cc(CallControl::NetworkCircuitMediaReady {
            brew_uuid: uuid,
            call_id,
            carrier_num,
            ts: 2,
        })
    }

    fn restore_request(issi: u32, peer: u32) -> SapMsgInner {
        cc(CallControl::IndividualCallRestoreRequest { issi, peer })
    }

    fn restore_answer(msgs: &[SapMsgInner]) -> Option<(u32, Option<NetworkRestoredCall>)> {
        msgs.iter().find_map(|m| match m {
            SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreAnswer { peer, call, .. }) => Some((*peer, call.clone())),
            _ => None,
        })
    }

    fn release_cause(msgs: &[SapMsgInner], uuid: Uuid) -> Option<u8> {
        msgs.iter().find_map(|m| match m {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }) if *brew_uuid == uuid => Some(*cause),
            _ => None,
        })
    }

    /// The app calls the radio, which answers on cell 0; the call's media runs there.
    fn app_calls_radio_on_cell0(site: &mut Site, duplex: bool) -> Uuid {
        let uuid = Uuid::from_u128(70);
        site.from_cell(0, register(RADIO));
        site.tick();
        site.from_net(cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call: circuit(APP, RADIO, duplex) }));
        let (c0, _) = site.tick();
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. }))));
        // The radio answers on cell 0 (cell 1 paged it too: same location area).
        site.from_cell(0, connect_request(uuid, duplex));
        site.tick();
        site.from_cell(0, media(uuid, 3, C0));
        site.tick();
        site.net_got();
        uuid
    }

    #[test]
    fn network_individual_call_follows_its_radio_to_a_sibling_cell() {
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, true);

        // Reselected to cell 1 (same location area, no registration) and sent U-CALL RESTORE.
        site.from_cell(1, restore_request(RADIO, APP));
        let (c0, c1) = site.tick();
        let (peer, call) = restore_answer(&c1).expect("cell 1 gets the call");
        let call = call.expect("found on cell 0");
        assert_eq!((peer, call.brew_uuid, call.simplex_duplex), (APP, uuid, true));
        assert_eq!(release_cause(&c0, uuid), Some(14), "old leg released without a release to the network");
        assert!(site.net_got().is_empty(), "the network never hears of the move");

        // Cell 1's new leg carries the voice now; the old cell's leftovers stay on site.
        site.from_cell(1, media(uuid, 5, C1));
        site.from_cell(0, cc(CallControl::NetworkCircuitSimplexIdle { brew_uuid: uuid, grant: 0, permission: 0 }));
        site.tick();
        let got = site.net_got();
        assert!(got.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { carrier_num, .. }) if *carrier_num == C1)));
        assert!(!got.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle { .. }))));

        // The app hangs up: the release goes to cell 1.
        site.from_net(cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: 1 }));
        let (c0, c1) = site.tick();
        assert_eq!(release_cause(&c1, uuid), Some(1));
        assert_eq!(release_cause(&c0, uuid), None);
    }

    #[test]
    fn restore_answer_names_the_simplex_floor_holder() {
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, false);
        site.from_net(cc(CallControl::NetworkCircuitSimplexGranted { brew_uuid: uuid, grant: 0, permission: 0 }));
        site.tick();

        site.from_cell(1, restore_request(RADIO, 0));
        let (_, c1) = site.tick();
        let (peer, call) = restore_answer(&c1).unwrap();
        let call = call.unwrap();
        assert_eq!((peer, call.simplex_duplex, call.floor_issi), (APP, false, Some(APP)), "peer filled in, app talks");
    }

    #[test]
    fn a_leg_lost_to_uplink_inactivity_waits_for_the_restore() {
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, true);

        // Cell 0 ends its leg after uplink inactivity before the radio restores on cell 1.
        let expiry = DisconnectCause::ExpiryOfTimer.into_raw() as u8;
        site.from_cell(0, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: expiry }));
        site.tick();
        assert!(release_cause(&site.net_got(), uuid).is_none(), "held back");

        site.from_cell(1, restore_request(RADIO, APP));
        let (c0, c1) = site.tick();
        assert!(restore_answer(&c1).unwrap().1.is_some());
        assert_eq!(release_cause(&c0, uuid), None, "the old leg is already gone");
        assert!(site.net_got().is_empty());
    }

    #[test]
    fn a_lost_leg_without_restore_ends_after_the_window() {
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, true);
        let expiry = DisconnectCause::ExpiryOfTimer.into_raw() as u8;
        site.from_cell(0, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: expiry }));
        site.tick();
        site.net_got();

        // Window over (wound forward instead of waiting 8 s).
        site.switch.circuit_calls.get_mut(&uuid).unwrap().awaiting_restore = Some((Instant::now(), expiry));
        site.tick();
        assert_eq!(release_cause(&site.net_got(), uuid), Some(expiry));
        assert!(site.switch.circuit_calls.is_empty());
    }

    #[test]
    fn registering_on_another_cell_releases_the_old_leg_and_waits() {
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, true);

        // Other location area: the radio registers on cell 1 first (service restoration roaming).
        site.from_cell(1, register(RADIO));
        let (c0, _) = site.tick();
        assert_eq!(release_cause(&c0, uuid), Some(14));
        assert!(release_cause(&site.net_got(), uuid).is_none());

        site.from_cell(1, restore_request(RADIO, APP));
        let (c0, c1) = site.tick();
        assert!(restore_answer(&c1).unwrap().1.is_some());
        assert_eq!(release_cause(&c0, uuid), None, "released once only");

        // Not restored in time: the network hears "call restoration of the other user failed".
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, true);
        site.from_cell(1, register(RADIO));
        site.tick();
        site.net_got();
        let (deadline, cause) = site.switch.circuit_calls[&uuid].awaiting_restore.unwrap();
        assert!(deadline > Instant::now());
        site.switch.circuit_calls.get_mut(&uuid).unwrap().awaiting_restore = Some((Instant::now(), cause));
        site.tick();
        assert_eq!(release_cause(&site.net_got(), uuid), Some(19));
    }

    #[test]
    fn plain_roaming_registration_ends_the_call_at_once() {
        let mut site = Site::new();
        let uuid = app_calls_radio_on_cell0(&mut site, true);
        site.from_cell(1, cc(CallControl::RegisteredWithoutCall { issi: RADIO }));
        let (c0, _) = site.tick();
        assert_eq!(release_cause(&c0, uuid), Some(14));
        assert_eq!(release_cause(&site.net_got(), uuid), Some(19));
        assert!(site.switch.circuit_calls.is_empty());
    }

    #[test]
    fn restore_of_a_call_not_on_site_asks_the_network_and_its_answer_returns_to_the_cell() {
        let mut site = Site::new();
        site.from_cell(1, restore_request(RADIO, APP));
        site.tick();
        assert!(site.net_got().iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreRequest { .. }))));

        site.from_net(cc(CallControl::IndividualCallRestoreAnswer { issi: RADIO, peer: APP, call: None }));
        let (c0, c1) = site.tick();
        assert!(restore_answer(&c1).is_some() && restore_answer(&c0).is_none());
    }

    #[test]
    fn a_restore_on_the_cell_holding_the_call_is_not_moved() {
        let mut site = Site::new();
        app_calls_radio_on_cell0(&mut site, true);
        site.from_cell(0, restore_request(RADIO, APP));
        let (c0, _) = site.tick();
        assert!(restore_answer(&c0).is_none());
        assert!(site.net_got().iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreRequest { .. }))));
    }

    // ── Location areas ──────────────────────────────────────────────────────────────────────

    /// Last `SiteAreaMembers` for `issi`: `None` = none sent, `Some(None)` = withdrawn.
    fn area_members(msgs: &[SapMsgInner], issi: u32) -> Option<Option<Vec<u32>>> {
        msgs.iter().rev().find_map(|m| match m {
            SapMsgInner::CmceCallControl(CallControl::SiteAreaMembers { issi: i, groups }) if *i == issi => Some(groups.clone()),
            _ => None,
        })
    }

    fn affiliate(issi: u32, groups: &[u32], action: BrewSubscriberAction) -> SapMsgInner {
        SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: groups.to_vec(),
            action,
        })
    }

    #[test]
    fn a_radio_listens_on_every_cell_of_its_location_area() {
        let mut site = Site::new();
        site.from_cell(0, affiliate(100, &[91, 92], BrewSubscriberAction::Affiliate));
        let (c0, c1) = site.tick();
        assert_eq!(area_members(&c1, 100), Some(Some(vec![91, 92])), "sibling cell counts it as a listener");
        assert_eq!(area_members(&c0, 100), None, "its own cell knows it from MM");

        site.from_cell(0, affiliate(100, &[92], BrewSubscriberAction::Deaffiliate));
        let (_, c1) = site.tick();
        assert_eq!(area_members(&c1, 100), Some(Some(vec![91])));

        // Registers on cell 1 (e.g. after a presence check there): the roles swap.
        site.from_cell(1, affiliate(100, &[91], BrewSubscriberAction::Register));
        let (c0, c1) = site.tick();
        assert_eq!(area_members(&c1, 100), Some(None));
        assert_eq!(area_members(&c0, 100), Some(Some(vec![91])));

        site.from_cell(1, affiliate(100, &[], BrewSubscriberAction::Deregister));
        let (c0, _) = site.tick();
        assert_eq!(area_members(&c0, 100), Some(None), "gone from the site");
    }

    #[test]
    fn a_network_group_call_runs_on_every_cell_of_the_members_location_area() {
        let mut site = Site::new();
        site.member(0, 100, 91);
        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: Uuid::from_u128(1),
            source_issi: 9000,
            dest_gssi: 91,
            priority: 0,
        }));
        let (c0, c1) = site.tick();
        assert!(start_uuid(&c0).is_some() && start_uuid(&c1).is_some());
    }

    #[test]
    fn a_presence_check_is_repeated_on_the_other_cells_of_the_location_area() {
        let mut site = Site::new();
        site.from_cell(0, cc(CallControl::SitePresenceProbe { issi: 100 }));
        let (_, c1) = site.tick();
        assert!(c1.iter().any(|m| matches!(m, SapMsgInner::MmRequestLocationUpdate { issi: 100 })));
        assert!(site.net_got().is_empty(), "never reaches the network");
    }

    #[test]
    fn cells_with_own_location_areas_do_not_share_listeners_calls_or_presence_checks() {
        let mut site = Site::own_location_areas();
        site.from_cell(0, affiliate(100, &[91], BrewSubscriberAction::Affiliate));
        let (c0, c1) = site.tick();
        assert_eq!(area_members(&c0, 100), None);
        assert_eq!(area_members(&c1, 100), None);
        site.net_got();

        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: Uuid::from_u128(1),
            source_issi: 9000,
            dest_gssi: 91,
            priority: 0,
        }));
        let (c0, c1) = site.tick();
        assert!(start_uuid(&c0).is_some() && start_uuid(&c1).is_none());

        site.from_cell(0, cc(CallControl::SitePresenceProbe { issi: 100 }));
        let (_, c1) = site.tick();
        assert!(!c1.iter().any(|m| matches!(m, SapMsgInner::MmRequestLocationUpdate { .. })));
        assert!(site.net_got().is_empty());
    }

    // ── Handover with other stations (through the network entity) ──

    fn got_cc(msgs: &[SapMsgInner], f: impl Fn(&CallControl) -> bool) -> bool {
        msgs.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(c) if f(c)))
    }

    #[test]
    fn a_reselection_to_another_station_is_answered_on_the_asking_cell() {
        let mut site = Site::new();
        site.from_cell(
            1,
            cc(CallControl::HandoverRequest {
                issi: 100,
                target_carrier: 1600,
                target_carrier_ext: 264,
                target_mcc: None,
                target_mnc: None,
                target_location_area: None,
                sdu: None,
            }),
        );
        site.tick();
        assert!(got_cc(&site.net_got(), |c| matches!(c, CallControl::HandoverRequest { issi: 100, target_carrier: 1600, .. })));

        site.from_net(cc(CallControl::HandoverResult { issi: 100, outcome: HandoverOutcome::Accepted, sdu: None, channel: None }));
        let (c0, c1) = site.tick();
        assert!(got_cc(&c1, |c| matches!(c, CallControl::HandoverResult { issi: 100, .. })), "the asking cell answers the radio");
        assert!(!got_cc(&c0, |c| matches!(c, CallControl::HandoverResult { .. })));
    }

    #[test]
    fn a_registration_forwarded_by_another_station_goes_to_the_cell_of_its_carrier() {
        let mut site = Site::new();
        let demand = tetra_core::BitBuffer::from_bitstr("0010");
        site.from_net(cc(CallControl::SiteForwardRegistration { issi: 100, target_carrier: C1, sdu: demand }));
        let (c0, c1) = site.tick();
        assert!(got_cc(&c1, |c| matches!(c, CallControl::SiteForwardRegistration { issi: 100, .. })));
        assert!(!got_cc(&c0, |c| matches!(c, CallControl::SiteForwardRegistration { .. })));

        // Cell 1's MM answer goes back to the other station through the network entity.
        site.from_cell(1, cc(CallControl::SiteForwardRegistrationResult { issi: 100, sdu: Some(tetra_core::BitBuffer::from_bitstr("0101")) }));
        site.tick();
        assert!(got_cc(&site.net_got(), |c| matches!(c, CallControl::SiteForwardRegistrationResult { issi: 100, sdu: Some(_) })));

        // So does the question about the radio's group call.
        site.from_net(cc(CallControl::HandoverPrepareGroupCall { issi: 100, gssi: 9 }));
        let (c0, c1) = site.tick();
        assert!(got_cc(&c1, |c| matches!(c, CallControl::HandoverPrepareGroupCall { issi: 100, gssi: 9 })));
        assert!(!got_cc(&c0, |c| matches!(c, CallControl::HandoverPrepareGroupCall { .. })));

        // A result for a radio no one forwarded stays on site.
        site.from_cell(0, cc(CallControl::SiteForwardRegistrationResult { issi: 300, sdu: None }));
        site.tick();
        assert!(site.net_got().is_empty());
    }

    #[test]
    fn a_call_restored_from_another_station_is_held_by_the_asking_cell() {
        let mut site = Site::new();
        site.from_cell(1, restore_request(RADIO, APP));
        site.tick();
        assert!(got_cc(&site.net_got(), |c| matches!(c, CallControl::IndividualCallRestoreRequest { .. })), "not on site: ask the network");

        let uuid = Uuid::new_v4();
        let call = NetworkRestoredCall { brew_uuid: uuid, simplex_duplex: true, priority: 0, floor_issi: None };
        site.from_net(cc(CallControl::IndividualCallRestoreAnswer { issi: RADIO, peer: APP, call: Some(call) }));
        let (c0, c1) = site.tick();
        assert!(restore_answer(&c1).is_some_and(|(_, c)| c.is_some()));
        assert!(restore_answer(&c0).is_none());

        // Cell 1 speaks for the call now; the network's messages for it go there only.
        site.from_cell(1, media(uuid, 5, C1));
        site.from_cell(0, cc(CallControl::NetworkCircuitSimplexIdle { brew_uuid: uuid, grant: 0, permission: 0 }));
        site.tick();
        let got = site.net_got();
        assert!(got_cc(&got, |c| matches!(c, CallControl::NetworkCircuitMediaReady { carrier_num, .. } if *carrier_num == C1)));
        assert!(!got_cc(&got, |c| matches!(c, CallControl::NetworkCircuitSimplexIdle { .. })));
        site.from_net(cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: 1 }));
        let (c0, c1) = site.tick();
        assert_eq!(release_cause(&c1, uuid), Some(1));
        assert_eq!(release_cause(&c0, uuid), None);
    }

    #[test]
    fn a_radio_gone_to_another_station_is_dropped_on_its_own_cell_only() {
        let mut site = Site::new();
        site.from_cell(1, register(100));
        site.tick();
        site.net_got();
        let drop = SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi: 100, groups: vec![], action: BrewSubscriberAction::Deregister });
        site.from_net(drop.clone());
        let (c0, c1) = site.tick();
        let dropped = |msgs: &[SapMsgInner]| msgs.iter().any(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u) if u.issi == 100));
        assert!(dropped(&c1));
        assert!(!dropped(&c0), "a cell that never had it would report a deregistration of its own");

        // Presence of a radio that is not ours still reaches every cell.
        site.from_net(SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi: 777, groups: vec![], action: BrewSubscriberAction::Deregister }));
        let (c0, c1) = site.tick();
        assert!(c0.iter().chain(c1.iter()).filter(|m| matches!(m, SapMsgInner::MmSubscriberUpdate(u) if u.issi == 777)).count() == 2);
    }

    // ── Calls between two radios of the station across a change of cell ─────────────────────

    const MTH: u32 = 2320973;

    fn handoff_request(msgs: &[SapMsgInner]) -> Option<(u32, u32)> {
        msgs.iter().find_map(|m| match m {
            SapMsgInner::CmceCallControl(CallControl::IndividualCallHandoffRequest { issi, peer }) => Some((*issi, *peer)),
            _ => None,
        })
    }

    fn handed(uuid: Uuid, calling_issi: u32, duplex: bool, floor_issi: Option<u32>) -> HandedOffCall {
        HandedOffCall {
            brew_uuid: uuid,
            calling_issi,
            simplex_duplex: duplex,
            priority: 0,
            floor_issi,
        }
    }

    /// Both radios registered on cell 0.
    fn both_on_cell0(site: &mut Site) {
        site.from_cell(0, register(RADIO));
        site.from_cell(0, register(MTH));
        site.tick();
        site.net_got();
    }

    #[test]
    fn a_local_call_follows_its_radio_to_a_sibling_cell() {
        let mut site = Site::new();
        both_on_cell0(&mut site);

        // RADIO, in a local duplex call it set up to MTH on cell 0, restores on cell 1.
        site.from_cell(1, restore_request(RADIO, MTH));
        let (c0, c1) = site.tick();
        assert_eq!(handoff_request(&c0), Some((RADIO, MTH)), "the cell holding MTH is asked");
        assert!(restore_answer(&c1).is_none(), "no answer before the holder's");

        let uuid = Uuid::from_u128(90);
        site.from_cell(0, cc(CallControl::IndividualCallHandoffAnswer { issi: RADIO, peer: MTH, call: Some(handed(uuid, RADIO, true, None)) }));
        site.from_cell(0, media(uuid, 4, C0));
        let (_, c1) = site.tick();
        let (peer, call) = restore_answer(&c1).expect("cell 1 gets the answer");
        let call = call.expect("handed over");
        assert_eq!((peer, call.brew_uuid, call.simplex_duplex), (MTH, uuid, true));
        let c = &site.switch.cell_calls[&uuid];
        assert_eq!((c.caller, c.caller_issi, c.called_issi), (CellId(1), RADIO, MTH), "the caller restored on cell 1");

        // Voice both ways between the two cells, nothing on the network.
        site.from_cell(1, media(uuid, 6, C1));
        site.tick();
        for _ in 0..PLAYOUT_FILL {
            site.from_cell(0, uplink(C0));
            site.from_cell(1, uplink(C1));
        }
        site.tick();
        let (c0, c1) = site.tick();
        assert!(voice_on(&c1, C1) && voice_on(&c0, C0), "voice both ways");
        assert!(!site.net_got().iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(_))), "the call stays on site");
    }

    #[test]
    fn a_handoff_without_a_local_call_rejects_the_restore() {
        let mut site = Site::new();
        both_on_cell0(&mut site);
        site.from_cell(1, restore_request(RADIO, MTH));
        site.tick();
        site.from_cell(0, cc(CallControl::IndividualCallHandoffAnswer { issi: RADIO, peer: MTH, call: None }));
        let (_, c1) = site.tick();
        assert_eq!(restore_answer(&c1), Some((MTH, None)));
        assert!(site.switch.cell_calls.is_empty());
    }

    #[test]
    fn a_handoff_nobody_waits_for_is_ended_on_its_cell() {
        let mut site = Site::new();
        both_on_cell0(&mut site);
        let uuid = Uuid::from_u128(91);
        site.from_cell(0, cc(CallControl::IndividualCallHandoffAnswer { issi: RADIO, peer: MTH, call: Some(handed(uuid, RADIO, false, None)) }));
        let (c0, c1) = site.tick();
        assert_eq!(release_cause(&c0, uuid), Some(DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8));
        assert!(restore_answer(&c1).is_none());
    }

    /// RADIO on cell 0 calls MTH on cell 1 (simplex unless `duplex`); both legs have media.
    fn call_between_cells(site: &mut Site, duplex: bool) -> Uuid {
        site.from_cell(0, register(RADIO));
        site.from_cell(1, register(MTH));
        site.tick();
        let uuid = Uuid::from_u128(92);
        site.from_cell(0, cc(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call: circuit(RADIO, MTH, duplex) }));
        site.tick();
        site.from_cell(1, connect_request(uuid, duplex));
        site.tick();
        site.from_cell(0, media(uuid, 3, C0));
        site.from_cell(1, media(uuid, 5, C1));
        site.tick();
        site.net_got();
        assert!(site.switch.cell_calls.contains_key(&uuid));
        uuid
    }

    #[test]
    fn a_call_between_cells_is_joined_when_a_radio_comes_to_its_peers_cell() {
        for duplex in [true, false] {
            let mut site = Site::new();
            let uuid = call_between_cells(&mut site, duplex);

            // RADIO leaves cell 0: its leg ends by uplink inactivity, the call is held.
            let expiry = DisconnectCause::ExpiryOfTimer.into_raw() as u8;
            site.from_cell(0, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: expiry }));
            let (_, c1) = site.tick();
            assert_eq!(release_cause(&c1, uuid), None, "MTH's leg waits for the restore");

            // RADIO restores on cell 1, which joins both legs locally and lets the bridge go.
            let joined = DisconnectCause::SwmiRequestedDisconnection.into_raw() as u8;
            site.from_cell(1, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: joined }));
            let (c0, _) = site.tick();
            assert_eq!(release_cause(&c0, uuid), None, "cell 0's leg is gone already");
            assert!(site.switch.cell_calls.is_empty());
            assert!(!site.net_got().iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(_))));
        }
    }

    #[test]
    fn a_join_before_the_old_leg_noticed_releases_it() {
        let mut site = Site::new();
        let uuid = call_between_cells(&mut site, true);
        let joined = DisconnectCause::SwmiRequestedDisconnection.into_raw() as u8;
        site.from_cell(1, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: joined }));
        let (c0, _) = site.tick();
        assert_eq!(release_cause(&c0, uuid), Some(joined), "the old leg ends quietly");
        assert!(site.switch.cell_calls.is_empty());
    }

    #[test]
    fn a_lost_cell_leg_not_joined_ends_after_the_window() {
        let mut site = Site::new();
        let uuid = call_between_cells(&mut site, true);
        let expiry = DisconnectCause::ExpiryOfTimer.into_raw() as u8;
        site.from_cell(0, cc(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: expiry }));
        site.tick();

        // Window over (wound forward instead of waiting 8 s).
        site.switch.cell_calls.get_mut(&uuid).unwrap().leg_lost = Some((CellId(0), Instant::now()));
        let (c0, c1) = site.tick();
        assert_eq!(release_cause(&c1, uuid), Some(DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8));
        assert_eq!(release_cause(&c0, uuid), None);
        assert!(site.switch.cell_calls.is_empty());
    }
}
