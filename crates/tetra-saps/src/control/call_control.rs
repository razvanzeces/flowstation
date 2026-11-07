use tetra_core::{BitBuffer, Direction};

use crate::control::enums::circuit_mode_type::CircuitModeType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitDlMediaSource {
    /// Downlink media comes from local UL loopback (classic BS behavior).
    LocalLoopback,
    /// Downlink media is supplied by SwMI over the network bridge.
    SwMI,
}

#[derive(Debug, Clone)]
pub struct Circuit {
    /// Direction
    pub direction: Direction,

    /// Timeslot in which this circuit exists
    pub ts: u8,

    /// Carrier number in which this circuit exists.
    pub carrier_num: u16,

    /// Optional peer carrier for duplex cross-routing.
    pub peer_carrier_num: Option<u16>,

    /// Optional peer timeslot for duplex cross-routing (UL on ts -> DL on peer_ts)
    pub peer_ts: Option<u8>,

    /// Usage number, between 4 and 63
    pub usage: u8,

    /// Traffic channel type
    pub circuit_mode: CircuitModeType,

    // pub comm_type: CommunicationType,

    // pub simplex_duplex: bool,

    // pub slots_per_frame: Option<u8>, // only relevant for circuit data
    /// 2 opt, 00 = TETRA encoded speech, 1|2 = reserved, 3 = proprietary
    pub speech_service: Option<u8>,
    /// Whether end-to-end encryption is enabled on this circuit
    pub etee_encrypted: bool,
    /// Downlink media source policy for this circuit.
    pub dl_media_source: CircuitDlMediaSource,
}

#[derive(Debug, Clone)]
pub struct NetworkCircuitCall {
    /// Calling party ISSI
    pub source_issi: u32,
    /// Called party ISSI/GSSI when available
    pub destination: u32,
    /// External number for PBX/phone calls (ASCII, may be empty)
    pub number: String,
    /// Call priority
    pub priority: u8,
    /// Speech service (Table 14.79)
    pub service: u8,
    /// Circuit mode (Table 14.52)
    pub mode: u8,
    /// Duplex flag (0 = simplex, 1 = duplex)
    pub duplex: u8,
    /// Hook method (Table 14.62)
    pub method: u8,
    /// Communication type (Table 14.54)
    pub communication: u8,
    /// Transmission grant (Table 14.80)
    pub grant: u8,
    /// Transmission request permission (Table 14.81)
    pub permission: u8,
    /// Call timeout (Table 14.50)
    pub timeout: u8,
    /// Call ownership (Table 14.38)
    pub ownership: u8,
    /// Call queued (Table 14.48)
    pub queued: u8,
}

#[derive(Debug, Clone)]
pub enum CallControl {
    /// Signals to set up a circuit
    /// Created by CMCE, sent to Umac
    /// Umac forwards to Lmac
    Open(Circuit),
    /// Signals to release a circuit
    /// Created by CMCE, sent to Umac
    /// Umac forwards to Lmac
    /// Contains (Direction, timeslot) of associated circuit
    Close(Direction, u8),
    /// Carrier-aware close for exact resource release.
    CloseSlot { direction: Direction, carrier_num: u16, ts: u8 },
    /// Switch DL media source on an existing circuit without teardown (no close/reopen).
    /// Used when a network speaker (LST/Brew) takes the floor over a LocalLoopback circuit.
    SetDlMediaSource {
        carrier_num: u16,
        ts: u8,
        source: CircuitDlMediaSource,
    },
    /// Floor granted: a speaker has been given transmission permission.
    /// Sent to UMAC to exit hangtime (resume traffic mode) and to Brew to start forwarding voice.
    FloorGranted {
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        carrier_num: u16,
        ts: u8,
    },
    /// Remote floor granted: a network/Brew speaker has been given transmission permission.
    /// Sent to UMAC to exit hangtime without arming local stuck-uplink detection.
    RemoteFloorGranted { call_id: u16, carrier_num: u16, ts: u8 },
    /// Floor released: speaker stopped transmitting (entering hangtime).
    /// Sent to UMAC to enter hangtime signalling mode and to Brew to stop forwarding audio.
    FloorReleased { call_id: u16, carrier_num: u16, ts: u8 },
    /// Call ended: the call is being torn down.
    /// Sent to UMAC to clear hangtime state and to Brew to clean up call tracking.
    CallEnded { call_id: u16, carrier_num: u16, ts: u8 },
    /// Request CMCE to start a network-initiated group call
    /// Sent by Brew when TetraPack sends GROUP_TX
    NetworkCallStart {
        brew_uuid: uuid::Uuid, // Brew session UUID for tracking
        source_issi: u32,      // Current speaker
        dest_gssi: u32,        // Target group
        priority: u8,          // Call priority
    },
    /// Notify Brew that network call is ready with allocated resources
    /// Response from CMCE after circuit allocation
    NetworkCallReady {
        brew_uuid: uuid::Uuid, // Matches request
        call_id: u16,          // CMCE-allocated call identifier
        carrier_num: u16,      // Allocated carrier
        ts: u8,                // Allocated timeslot
        usage: u8,             // Usage number
    },
    /// Request ending a network call
    /// Sent by Brew when TetraPack sends GROUP_IDLE, or by CMCE to make Brew drop a call
    NetworkCallEnd {
        brew_uuid: uuid::Uuid, // Identifies the call to end
    },
    /// CMCE has no local listeners yet — Brew holds the inbound call (pending) instead of teardown.
    /// Voice/GROUP_TX keepalives continue until listeners appear or idle reap.
    NetworkCallHold {
        brew_uuid: uuid::Uuid,
        dest_gssi: u32,
    },
    /// First local listener(s) appeared for this GSSI — Brew may promote a held inbound call.
    GroupListenersAvailable { gssi: u32 },
    /// Snapshot of an ongoing group call for LST console late-entry RX (reuse FloorGranted path).
    OngoingGroupCall {
        gssi: u32,
        call_id: u16,
        carrier_num: u16,
        ts: u8,
        source_issi: u32,
        tx_active: bool,
    },
    /// Notify CMCE that network media is still arriving for an active group call.
    /// Used to refresh the BS-side call timeout while the backhaul is still sending voice.
    NetworkCallMediaActivity {
        brew_uuid: uuid::Uuid, // Identifies the active network group call
    },
    /// UL inactivity detected on a traffic timeslot: no voice frames received
    /// for the timeout period. Sent by UMAC to CMCE.
    UlInactivityTimeout { carrier_num: u16, ts: u8 },
    /// UL voice presence on a traffic slot while hangtime is held (e.g. after
    /// network hard-preempt). Sent by UMAC to CMCE so Ready/talk-permit can wait
    /// for the local MS to actually drop UL.
    TrafficUlActivity {
        carrier_num: u16,
        ts: u8,
        active: bool,
    },
    /// Circuit-call setup request over Brew (individual/PBX/phone)
    NetworkCircuitSetupRequest { brew_uuid: uuid::Uuid, call: NetworkCircuitCall },
    /// Circuit-call setup accepted
    NetworkCircuitSetupAccept { brew_uuid: uuid::Uuid },
    /// Circuit-call setup rejected
    NetworkCircuitSetupReject { brew_uuid: uuid::Uuid, cause: u8 },
    /// Circuit-call alerting (ringing)
    NetworkCircuitAlert { brew_uuid: uuid::Uuid },
    /// Circuit-call connect request from remote side
    NetworkCircuitConnectRequest { brew_uuid: uuid::Uuid, call: NetworkCircuitCall },
    /// Circuit-call connect confirm from local side
    NetworkCircuitConnectConfirm { brew_uuid: uuid::Uuid, grant: u8, permission: u8 },
    /// Circuit-call simplex floor grant
    NetworkCircuitSimplexGranted { brew_uuid: uuid::Uuid, grant: u8, permission: u8 },
    /// Circuit-call simplex floor idle/release
    NetworkCircuitSimplexIdle { brew_uuid: uuid::Uuid, grant: u8, permission: u8 },
    /// Circuit-call media is active on this local timeslot
    NetworkCircuitMediaReady {
        brew_uuid: uuid::Uuid,
        call_id: u16,
        carrier_num: u16,
        ts: u8,
    },
    /// Circuit-call INFO/DTMF payload from MS to SwMI/Brew
    NetworkCircuitDtmf {
        brew_uuid: uuid::Uuid,
        length_bits: u16,
        data: Vec<u8>,
    },
    /// Circuit-call release
    NetworkCircuitRelease { brew_uuid: uuid::Uuid, cause: u8 },
    /// Multi-cell only (CMCE → site switch, never forwarded to Brew/LST): priority of the local
    /// group call whose `FloorGranted` follows, so the other cells get it at the same priority.
    SiteCallPriority { call_id: u16, priority: u8 },
    /// Multi-cell only (MLE → site switch, never forwarded to Brew/LST): the MS was cleared by
    /// D-NEW-CELL to move to the sibling cell on `target_carrier` (announced cell reselection).
    SiteHandoverPrepare { issi: u32, target_carrier: u16 },
    /// Multi-cell only (MLE ↔ site switch ↔ MLE, never forwarded to Brew/LST): announced type 1
    /// cell reselection with forward registration. The MM registration PDU the MS carried in its
    /// U-PREPARE, for the MM of the cell on `target_carrier`.
    SiteForwardRegistration { issi: u32, target_carrier: u16, sdu: BitBuffer },
    /// Multi-cell only: that MM's answer (None: the target isn't one of our cells, or no answer),
    /// for the serving cell to return inside D-NEW-CELL / D-PREPARE-FAIL.
    SiteForwardRegistrationResult { issi: u32, sdu: Option<BitBuffer> },
    /// Single-cell station with Brew (MLE → Brew): the MS asked in U-PREPARE to move to a
    /// neighbour cell that is not one of ours (announced type 1/2 cell reselection, TS 100 392-2
    /// clause 18.3.4.7.5). Brew asks the station serving that cell through the server; `sdu` is
    /// the forward registration (MM PDU) the U-PREPARE carried.
    HandoverRequest {
        issi: u32,
        target_carrier: u16,
        /// Absolute band plan of the target cell (main carrier number extension layout)
        target_carrier_ext: u16,
        target_mcc: Option<u16>,
        target_mnc: Option<u16>,
        target_location_area: Option<u16>,
        sdu: Option<BitBuffer>,
    },
    /// Brew → MLE: how the target station answered a `HandoverRequest`. `sdu` is the target MM's
    /// answer to the forward registration (D-LOCATION UPDATE ACCEPT or REJECT), if any.
    HandoverResult {
        issi: u32,
        outcome: HandoverOutcome,
        sdu: Option<BitBuffer>,
        /// The target's traffic channel for the MS's group call, when it offers one (type 1).
        channel: Option<HandoverChannel>,
    },
    /// Target station (Brew → CMCE): an MS of a running call to `gssi` is about to arrive here by
    /// announced type 1 cell reselection. Bring the call up here if it is only held as pending,
    /// count the MS as a listener, and answer with `HandoverGroupCallReady`.
    HandoverPrepareGroupCall { issi: u32, gssi: u32 },
    /// CMCE → Brew: the call to `gssi` runs here on this channel (call identifier, carrier,
    /// timeslot, usage marker), or `None` when it did not come up in time.
    HandoverGroupCallReady { issi: u32, gssi: u32, call: Option<(u16, u16, u8, u8)> },
    /// MLE → Brew: the MS was sent to the other station with that station's registration accept
    /// in D-NEW-CELL; it no longer belongs to this station.
    HandoverDone { issi: u32 },
    /// MLE → Brew: the reselection after a forwarded registration was refused with D-PREPARE-FAIL,
    /// so the MS stays registered here (clause 18.3.4.7.6) even if the target registered it.
    HandoverFailed { issi: u32 },
    /// MLE → Brew: the MS was answered with D-NEW-CELL and leaves by this announced cell
    /// reselection (for the server's cell change log).
    ReselectionAnnounced { issi: u32, reselection: CellReselection },
    /// Multi-cell only (site switch → target cell's CMCE): `issi` is reselecting into this cell
    /// while in an active individual (private/duplex) call with `peer_issi` (reachable via the
    /// switch). Pre-provision a restoring leg so the MS's U-CALL RESTORE on arrival is answered
    /// with D-CALL RESTORE (seamless), instead of being rejected. `brew_uuid` is the switch's
    /// site-wide id for the call; `simplex_duplex` and `priority` carry the call's parameters.
    SiteHandoverIndividualCall {
        brew_uuid: uuid::Uuid,
        issi: u32,
        peer_issi: u32,
        simplex_duplex: bool,
        priority: u8,
    },
    /// Separate station on a Brew network (CMCE → Brew): `issi` sent U-CALL RESTORE here for an
    /// individual call this station does not hold (it ran on the station the MS left). Ask the
    /// server for the call with `peer` (the other party's SSI from U-CALL RESTORE, 0 for the
    /// dummy address); answered with `IndividualCallRestoreAnswer`.
    IndividualCallRestoreRequest { issi: u32, peer: u32 },
    /// Brew → CMCE: the call `issi` restores here, or `None` when the network has none (or the
    /// server cannot move calls).
    IndividualCallRestoreAnswer {
        issi: u32,
        peer: u32,
        call: Option<NetworkRestoredCall>,
    },
    /// Brew → CMCE (separate stations, extension `call-handoff`): `issi` left this station during
    /// a local individual call with `peer`, which is still here, and restores the call on another
    /// station. Hand the call to the network: `peer`'s leg stays and is bridged to the network,
    /// `issi`'s leg ends here. Answered with `IndividualCallHandoffAnswer`.
    IndividualCallHandoffRequest { issi: u32, peer: u32 },
    /// CMCE → Brew: the call handed to the network, or `None` when there is no such call here.
    IndividualCallHandoffAnswer {
        issi: u32,
        peer: u32,
        call: Option<HandedOffCall>,
    },
    /// MM → Brew: `issi` registered here with a plain roaming location update. An MS with a
    /// circuit mode call registers with "service restoration roaming" instead (TS 100 392-2
    /// clause 16.4 a)), so it has no call to restore: the network may end an individual call that
    /// moved here with it at once.
    RegisteredWithoutCall { issi: u32 },
    /// Multi-cell only (site switch → CMCE): `issi`, registered on a sibling cell of the same
    /// location area, may be on this cell without registering here (TS 100 392-2 clause
    /// 18.3.4.7.1a: no registration within the registered area). `Some`: count it as a listener
    /// of these groups on this cell and accept individual calls for it (they are paged on every
    /// cell of the area), replacing what an earlier message said; `None`: not any more.
    SiteAreaMembers { issi: u32, groups: Option<Vec<u32>> },
    /// Multi-cell only (MM → site switch, never forwarded to Brew/LST): MM sent `issi` a presence
    /// check (D-LOCATION UPDATE COMMAND). The radio may be on a sibling cell of the same location
    /// area, so the switch has those cells ask too.
    SitePresenceProbe { issi: u32 },
}

/// An individual call the network moved to this station with its MS (see
/// `CallControl::IndividualCallRestoreAnswer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkRestoredCall {
    pub brew_uuid: uuid::Uuid,
    /// Duplex (true) or simplex
    pub simplex_duplex: bool,
    pub priority: u8,
    /// The party a simplex call's floor is with, if any
    pub floor_issi: Option<u32>,
}

/// A local individual call this station handed to the network (see
/// `CallControl::IndividualCallHandoffAnswer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandedOffCall {
    /// The network session the call runs under from now on
    pub brew_uuid: uuid::Uuid,
    /// The calling party of the call (the other one is the called party)
    pub calling_issi: u32,
    /// Duplex (true) or simplex
    pub simplex_duplex: bool,
    pub priority: u8,
    /// The party a simplex call's floor is with, if any
    pub floor_issi: Option<u32>,
}

/// The category of cell reselection an MS used to leave a cell (TS 100 392-2 clause 18.3.4.7.0),
/// as far as the cell it left can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellReselection {
    /// No circuit mode call here (clause 18.3.4.7.2).
    Undeclared,
    /// Left a circuit mode call here without announcing it (clause 18.3.4.7.3).
    Unannounced,
    /// D-NEW-CELL "follow MAC channel change" (clause 18.3.4.7.5: the "channel command valid" of
    /// D-NEW-CELL "defines the type of cell reselection that the SwMI is applying").
    AnnouncedType1,
    /// D-NEW-CELL "change channel immediately" after a U-PREPARE naming the cell (18.3.4.7.5).
    AnnouncedType2,
    /// D-NEW-CELL after a U-PREPARE without a cell (clause 18.3.4.7.4).
    AnnouncedType3,
}

/// A traffic channel on the target cell for announced type 1 cell reselection (TS 100 392-2
/// clause 18.3.4.7.5): the serving cell sends the MS the target's call identifier (D-INFO) and
/// then a channel allocation for another cell (clause 21.5.2, "cell change flag").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoverChannel {
    /// The serving cell's call identifier for the call, as the MS knows it
    pub call_id: u16,
    /// The target cell's call identifier for the same call
    pub new_call_id: u16,
    /// Carrier on the target cell (same band plan as the serving cell)
    pub carrier: u16,
    /// Timeslot 1 to 4 on the target cell
    pub timeslot: u8,
    /// Traffic usage marker on the target cell
    pub usage: u8,
    /// Carrier and timeslot of the call's traffic channel on the serving cell, where the MS
    /// listens and where D-INFO and D-NEW-CELL are stolen in
    pub serving_carrier: u16,
    pub serving_timeslot: u8,
}

/// Answer of the target station to a cross-station `HandoverRequest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoverOutcome {
    /// The target accepted the forward registration, or there was nothing to register.
    Accepted,
    /// The target's MM rejected the forward registration (its D-LOCATION UPDATE REJECT is in the SDU).
    Rejected,
    /// The target station went away while the request waited.
    TargetGone,
    /// Nobody prepared the target cell: unknown, not capable, no answer in time, or no server.
    NotPrepared,
}
