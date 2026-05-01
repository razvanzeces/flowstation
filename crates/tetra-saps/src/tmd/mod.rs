/// Pass TMD circuit data to UMAC for TX scheduling
#[derive(Debug, Clone)]
pub struct TmdCircuitDataReq {
    // call_id: CallId,
    pub ts: u8,
    pub data: Vec<u8>,
}

/// Rx'ed traffic
#[derive(Debug, Clone)]
pub struct TmdCircuitDataInd {
    // call_id: CallId,
    pub ts: u8,
    pub data: Vec<u8>,
}
