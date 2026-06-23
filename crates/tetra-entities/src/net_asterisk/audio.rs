use std::ptr::NonNull;

pub(crate) const PCMU_PAYLOAD_TYPE: u8 = 0;

const TETRA_PCM_SAMPLES_PER_FRAME: usize = 240;
const TETRA_PCM_SAMPLES_PER_BLOCK: usize = TETRA_PCM_SAMPLES_PER_FRAME * 2;
const TETRA_CODED_BITS_PER_FRAME: usize = 137;
const TETRA_CODED_BYTES_PER_FRAME: usize = (TETRA_CODED_BITS_PER_FRAME + 7) / 8;
const TETRA_TMD_BITS_PER_BLOCK: usize = TETRA_CODED_BITS_PER_FRAME * 2;
const TETRA_TMD_PACKED_BYTES: usize = (TETRA_TMD_BITS_PER_BLOCK + 7) / 8;

#[repr(C)]
struct RawTetraCodec {
    _private: [u8; 0],
}

#[link(name = "tetra-codec")]
unsafe extern "C" {
    fn tetra_encoder_create() -> *mut RawTetraCodec;
    fn tetra_decoder_create() -> *mut RawTetraCodec;
    fn tetra_codec_destroy(st: *mut RawTetraCodec);
    fn tetra_encode(st: *mut RawTetraCodec, pcm: *const i16, coded: *mut u8);
    fn tetra_decode(st: *mut RawTetraCodec, coded: *const u8, pcm: *mut i16, bfi: i32);
}

struct CodecHandle {
    ptr: NonNull<RawTetraCodec>,
}

// The codec state is owned by one SIP dialog and accessed only through &mut self on the
// entity thread. Moving that owner between threads is safe; sharing it concurrently is not.
unsafe impl Send for CodecHandle {}

impl CodecHandle {
    fn from_raw(ptr: *mut RawTetraCodec) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr })
    }
}

impl Drop for CodecHandle {
    fn drop(&mut self) {
        unsafe {
            tetra_codec_destroy(self.ptr.as_ptr());
        }
    }
}

pub(crate) struct AsteriskAudioTranscoder {
    encoder: CodecHandle,
    decoder: CodecHandle,
    downlink_pcm: Vec<i16>,
}

impl AsteriskAudioTranscoder {
    pub(crate) fn new() -> Option<Self> {
        let encoder = CodecHandle::from_raw(unsafe { tetra_encoder_create() })?;
        let decoder = CodecHandle::from_raw(unsafe { tetra_decoder_create() })?;
        Some(Self {
            encoder,
            decoder,
            downlink_pcm: Vec::with_capacity(TETRA_PCM_SAMPLES_PER_BLOCK * 2),
        })
    }

    pub(crate) fn decode_tmd_to_pcmu(&mut self, acelp: &[u8]) -> Option<Vec<u8>> {
        let coded = split_tmd_block_to_codec_frames(acelp)?;
        let mut out = Vec::with_capacity(TETRA_PCM_SAMPLES_PER_BLOCK);

        for frame in &coded {
            let mut pcm = [0i16; TETRA_PCM_SAMPLES_PER_FRAME];
            unsafe {
                tetra_decode(self.decoder.ptr.as_ptr(), frame.as_ptr(), pcm.as_mut_ptr(), 0);
            }
            out.extend(pcm.into_iter().map(linear_to_ulaw));
        }

        Some(out)
    }

    pub(crate) fn encode_pcmu_to_tmd(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        self.downlink_pcm.extend(payload.iter().map(|&sample| ulaw_to_linear(sample)));

        let mut out = Vec::new();
        while self.downlink_pcm.len() >= TETRA_PCM_SAMPLES_PER_BLOCK {
            let mut pcm_a = [0i16; TETRA_PCM_SAMPLES_PER_FRAME];
            let mut pcm_b = [0i16; TETRA_PCM_SAMPLES_PER_FRAME];
            pcm_a.copy_from_slice(&self.downlink_pcm[..TETRA_PCM_SAMPLES_PER_FRAME]);
            pcm_b.copy_from_slice(&self.downlink_pcm[TETRA_PCM_SAMPLES_PER_FRAME..TETRA_PCM_SAMPLES_PER_BLOCK]);
            self.downlink_pcm.drain(..TETRA_PCM_SAMPLES_PER_BLOCK);

            let mut coded_a = [0u8; TETRA_CODED_BYTES_PER_FRAME];
            let mut coded_b = [0u8; TETRA_CODED_BYTES_PER_FRAME];
            unsafe {
                tetra_encode(self.encoder.ptr.as_ptr(), pcm_a.as_ptr(), coded_a.as_mut_ptr());
                tetra_encode(self.encoder.ptr.as_ptr(), pcm_b.as_ptr(), coded_b.as_mut_ptr());
            }
            out.push(join_codec_frames_to_tmd_block(&coded_a, &coded_b));
        }

        out
    }
}

pub(crate) fn rtp_payload(packet: &[u8]) -> Option<(u8, &[u8])> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }

    let has_padding = packet[0] & 0x20 != 0;
    let has_extension = packet[0] & 0x10 != 0;
    let csrc_count = (packet[0] & 0x0f) as usize;
    let payload_type = packet[1] & 0x7f;

    let mut end = packet.len();
    if has_padding {
        let padding = *packet.last()? as usize;
        if padding == 0 || padding > end {
            return None;
        }
        end -= padding;
    }

    let mut offset = 12 + csrc_count * 4;
    if offset > end {
        return None;
    }

    if has_extension {
        if offset + 4 > end {
            return None;
        }
        let extension_words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        offset += 4 + extension_words * 4;
        if offset > end {
            return None;
        }
    }

    Some((payload_type, &packet[offset..end]))
}

fn split_tmd_block_to_codec_frames(data: &[u8]) -> Option<[[u8; TETRA_CODED_BYTES_PER_FRAME]; 2]> {
    let packed = if data.len() == TETRA_TMD_PACKED_BYTES + 1 {
        Some(&data[1..])
    } else if data.len() == TETRA_TMD_PACKED_BYTES {
        Some(data)
    } else {
        None
    };

    let mut frames = [[0u8; TETRA_CODED_BYTES_PER_FRAME]; 2];
    if let Some(packed) = packed {
        for bit_idx in 0..TETRA_TMD_BITS_PER_BLOCK {
            let bit = get_packed_bit(packed, bit_idx);
            set_packed_bit(
                &mut frames[bit_idx / TETRA_CODED_BITS_PER_FRAME],
                bit_idx % TETRA_CODED_BITS_PER_FRAME,
                bit,
            );
        }
        return Some(frames);
    }

    if data.len() < TETRA_TMD_BITS_PER_BLOCK {
        return None;
    }
    for bit_idx in 0..TETRA_TMD_BITS_PER_BLOCK {
        set_packed_bit(
            &mut frames[bit_idx / TETRA_CODED_BITS_PER_FRAME],
            bit_idx % TETRA_CODED_BITS_PER_FRAME,
            data[bit_idx] & 1,
        );
    }
    Some(frames)
}

fn join_codec_frames_to_tmd_block(frame_a: &[u8; TETRA_CODED_BYTES_PER_FRAME], frame_b: &[u8; TETRA_CODED_BYTES_PER_FRAME]) -> Vec<u8> {
    let mut out = vec![0u8; TETRA_TMD_PACKED_BYTES];
    for bit_idx in 0..TETRA_TMD_BITS_PER_BLOCK {
        let frame = if bit_idx < TETRA_CODED_BITS_PER_FRAME { frame_a } else { frame_b };
        let frame_bit = bit_idx % TETRA_CODED_BITS_PER_FRAME;
        set_packed_bit(&mut out, bit_idx, get_packed_bit(frame, frame_bit));
    }
    out
}

fn get_packed_bit(data: &[u8], bit_idx: usize) -> u8 {
    (data[bit_idx / 8] >> (7 - (bit_idx % 8))) & 1
}

fn set_packed_bit(data: &mut [u8], bit_idx: usize, bit: u8) {
    if bit & 1 != 0 {
        data[bit_idx / 8] |= 1 << (7 - (bit_idx % 8));
    }
}

fn ulaw_to_linear(sample: u8) -> i16 {
    const BIAS: i16 = 0x84;

    let sample = !sample;
    let mantissa = (sample & 0x0f) as i16;
    let exponent = ((sample & 0x70) >> 4) as u32;
    let value = ((mantissa << 3) + BIAS) << exponent;

    if sample & 0x80 != 0 { BIAS - value } else { value - BIAS }
}

fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;
    const SEG_END: [i32; 8] = [0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff, 0x3fff, 0x7fff];

    let mut pcm = sample as i32;
    let mask = if pcm < 0 {
        pcm = -pcm;
        0x7f
    } else {
        0xff
    };
    pcm = pcm.min(CLIP) + BIAS;

    let segment = SEG_END.iter().position(|&end| pcm <= end).unwrap_or(SEG_END.len() - 1) as i32;
    let ulaw = ((segment << 4) | ((pcm >> (segment + 3)) & 0x0f)) as u8;

    ulaw ^ mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_tmd_round_trip_keeps_274_bits() {
        let mut bits = [0u8; TETRA_TMD_BITS_PER_BLOCK];
        for (idx, bit) in bits.iter_mut().enumerate() {
            *bit = (idx % 3 == 0) as u8;
        }

        let frames = split_tmd_block_to_codec_frames(&bits).unwrap();
        let packed = join_codec_frames_to_tmd_block(&frames[0], &frames[1]);
        assert_eq!(packed.len(), TETRA_TMD_PACKED_BYTES);

        let frames_again = split_tmd_block_to_codec_frames(&packed).unwrap();
        assert_eq!(frames, frames_again);
    }

    #[test]
    fn rtp_payload_skips_extension_and_padding() {
        let packet = [
            0b1011_0000,
            PCMU_PAYLOAD_TYPE,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            0xab,
            0xcd,
            0,
            1,
            0xaa,
            0xbb,
            0xcc,
            0xdd,
            0x11,
            0x22,
            2,
            2,
        ];
        let (pt, payload) = rtp_payload(&packet).unwrap();
        assert_eq!(pt, PCMU_PAYLOAD_TYPE);
        assert_eq!(payload, &[0x11, 0x22]);
    }
}
