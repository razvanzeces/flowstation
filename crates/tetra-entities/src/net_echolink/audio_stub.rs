pub(crate) const ECHOLINK_GSM_FRAME_BYTES: usize = 33;
pub(crate) const ECHOLINK_GSM_PACKET_BYTES: usize = 132;

/// Build-time fallback used when the native EchoLink codecs are not enabled.
pub(crate) struct EcholinkAudioTranscoder;

impl EcholinkAudioTranscoder {
    pub(crate) fn new() -> Option<Self> {
        None
    }

    pub(crate) fn decode_tmd_to_gsm_packets(&mut self, _acelp: &[u8]) -> Option<Vec<Vec<u8>>> {
        None
    }

    pub(crate) fn decode_gsm_payload_to_tmd(&mut self, _payload: &[u8]) -> Vec<Vec<u8>> {
        Vec::new()
    }
}
