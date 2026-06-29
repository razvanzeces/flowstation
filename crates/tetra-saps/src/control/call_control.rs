use tetra_core::Direction;

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
    /// Notify CMCE that network media is still arriving for an active group call.
    /// Used to refresh the BS-side call timeout while the backhaul is still sending voice.
    NetworkCallMediaActivity {
        brew_uuid: uuid::Uuid, // Identifies the active network group call
    },
    /// UL inactivity detected on a traffic timeslot: no voice frames received
    /// for the timeout period. Sent by UMAC to CMCE.
    UlInactivityTimeout { carrier_num: u16, ts: u8 },
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
}
