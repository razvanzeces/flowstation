// ---------------------------------------------------------------------------
// TelemetryEvent — concrete enum sent through the channel
//
// Small, hot-path variants are inline (no heap allocation).
// Rare / large variants use heap-allocated payload so the enum stays small.
// ---------------------------------------------------------------------------

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

/// TelemetryEvent enum sent by a TetraEntity through the TelemetrySink
/// then, serializable by any codec for transmission over the network,
/// using any Transport.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum TelemetryEvent {
    /// MS registered on BS
    MsRegistration { issi: u32 },
    /// MS deregistered. Also counts as deregistration for all groups.
    MsDeregistration { issi: u32 },
    /// MS affiliated to groups
    MsGroupAttach { issi: u32, gssis: Vec<u32> },
    /// Full snapshot of all currently attached groups — emitted after any attach/detach
    MsGroupsSnapshot { issi: u32, gssis: Vec<u32> },
    /// MS detached from groups
    MsGroupDetach { issi: u32, gssis: Vec<u32> },
    /// RSSI measurement for a known MS (dBFS)
    MsRssi { issi: u32, rssi_dbfs: f32 },
    /// Group call started
    GroupCallStarted { call_id: u16, gssi: u32, caller_issi: u32 },
    /// Group call ended
    GroupCallEnded { call_id: u16, gssi: u32 },
    /// Speaker changed on active group call
    GroupCallSpeakerChanged { call_id: u16, gssi: u32, speaker_issi: u32 },
    /// Individual (P2P) call started
    IndividualCallStarted { call_id: u16, calling_issi: u32, called_issi: u32, simplex: bool },
    /// Individual call ended
    IndividualCallEnded { call_id: u16 },
    /// Energy saving mode updated for MS (0=StayAlive, 1=Eg1..7=Eg7)
    MsEnergySaving { issi: u32, mode: u8 },
}
