//! Local echo service — ISSI 999.
//!
//! When a terminal calls ISSI 999 (full-duplex P2P), FlowStation answers
//! immediately and loops UL audio back as DL with a small delay so the
//! caller hears their own voice.  No Brew / TetraPack involved.

use std::collections::VecDeque;
use tetra_core::{Sap, tetra_entities::TetraEntity};
use tetra_saps::{SapMsg, SapMsgInner, tmd::TmdCircuitDataReq};

/// Timeslot owned by an active echo call.
#[derive(Debug)]
pub struct EchoSession {
    pub ts: u8,
    pub call_id: u16,
    /// Ring buffer: frames received UL, replayed as DL after DELAY_FRAMES.
    buffer: VecDeque<Vec<u8>>,
}

/// Frames of delay before echo playback (~20 ms each → 200 ms default).
const DELAY_FRAMES: usize = 10;

impl EchoSession {
    pub fn new(ts: u8, call_id: u16) -> Self {
        Self { ts, call_id, buffer: VecDeque::with_capacity(DELAY_FRAMES + 4) }
    }

    /// Feed an UL ACELP frame. Returns a DL frame to play back (if ready).
    pub fn push_ul_frame(&mut self, data: Vec<u8>) -> Option<Vec<u8>> {
        self.buffer.push_back(data);
        if self.buffer.len() > DELAY_FRAMES {
            self.buffer.pop_front()
        } else {
            None
        }
    }

    /// Build the TmdCircuitDataReq SAP message to send DL audio to UMAC.
    pub fn make_dl_msg(ts: u8, data: Vec<u8>) -> SapMsg {
        SapMsg {
            sap: Sap::TmdSap,
            src: TetraEntity::Brew,   // UMAC listens for Brew→Umac on TmdSap
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq { ts, data }),
        }
    }
}

/// Constant: the echo service ISSI.
pub const ECHO_ISSI: u32 = 999;
