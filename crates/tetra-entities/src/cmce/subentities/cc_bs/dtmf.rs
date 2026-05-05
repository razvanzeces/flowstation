use super::*;

// TODO: This should probably be in U/D-Info
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DtmfKind {
    /// ETSI EN 300 392-2 V3.x: DTMF type = 000 (digits present)
    ToneStart,
    /// ETSI EN 300 392-2 V3.x: DTMF type = 001
    ToneEnd,
    /// ETSI EN 300 392-2 V3.x: DTMF type = 010
    NotSupported,
    /// ETSI EN 300 392-2 V3.x: DTMF type = 011
    NotSubscribed,
    /// ETSI EN 300 392-2 V3.x: reserved values 100..111
    Reserved(u8),
    /// Legacy edition-1 style payload (length divisible by 4): digits only, no 3-bit type.
    LegacyDigits,
    /// Payload could not be interpreted according to either format.
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DtmfDecoded {
    pub(super) kind: DtmfKind,
    pub(super) digits: String,
    pub(super) parsed_bits: usize,
    pub(super) full_len_bits: usize,
    pub(super) malformed: bool,
}

#[inline]
fn decode_dtmf_digit(nibble: u8) -> Option<char> {
    match nibble {
        0..=9 => Some(char::from(b'0' + nibble)),
        0x0a => Some('*'),
        0x0b => Some('#'),
        0x0c => Some('A'),
        0x0d => Some('B'),
        0x0e => Some('C'),
        0x0f => Some('D'),
        _ => None,
    }
}

#[inline]
fn type3_read_bit(field: &Type3FieldGeneric, bit_idx: usize) -> Option<u8> {
    // Type3FieldGeneric.data is a u64 holding up to 64 bits (MSB-first)
    if bit_idx >= field.len || bit_idx >= 64 {
        return None;
    }
    let shift = 63 - bit_idx;
    Some(((field.data >> shift) & 0x01) as u8)
}

#[inline]
fn type3_read_bits(field: &Type3FieldGeneric, start_bit: usize, num_bits: usize) -> Option<u64> {
    if num_bits > 64 || start_bit + num_bits > field.len {
        return None;
    }

    let mut value = 0u64;
    for i in 0..num_bits {
        value = (value << 1) | type3_read_bit(field, start_bit + i)? as u64;
    }
    Some(value)
}

pub(super) fn decode_dtmf(field: &Type3FieldGeneric) -> DtmfDecoded {
    let full_len_bits = field.len;
    let len_bits = full_len_bits.min(64usize); // data is u64, max 64 bits
    if len_bits == 0 {
        return DtmfDecoded {
            kind: DtmfKind::Invalid,
            digits: String::new(),
            parsed_bits: 0,
            full_len_bits,
            malformed: true,
        };
    }

    // Legacy mechanism (edition-1): payload is 4-bit digit nibbles only.
    // ETSI EN 300 392-2 V3.x note: new mechanism length is not divisible by 4.
    if len_bits % 4 == 0 {
        let nibble_count = len_bits / 4;
        let mut digits = String::with_capacity(nibble_count);
        for i in 0..nibble_count {
            let nibble = type3_read_bits(field, i * 4, 4).unwrap_or(0) as u8;
            if let Some(c) = decode_dtmf_digit(nibble) {
                digits.push(c);
            }
        }
        return DtmfDecoded {
            kind: DtmfKind::LegacyDigits,
            digits,
            parsed_bits: len_bits,
            full_len_bits,
            malformed: len_bits != full_len_bits,
        };
    }

    if len_bits < 3 {
        return DtmfDecoded {
            kind: DtmfKind::Invalid,
            digits: String::new(),
            parsed_bits: len_bits,
            full_len_bits,
            malformed: true,
        };
    }

    let dtmf_type = type3_read_bits(field, 0, 3).unwrap_or(0) as u8;
    let tail_bits = len_bits - 3;

    let mut digits = String::new();
    let mut malformed = len_bits != full_len_bits;
    let kind = match dtmf_type {
        0 => {
            if tail_bits == 0 || tail_bits % 4 != 0 {
                malformed = true;
            } else {
                let nibble_count = tail_bits / 4;
                digits.reserve(nibble_count);
                for i in 0..nibble_count {
                    let nibble = type3_read_bits(field, 3 + i * 4, 4).unwrap_or(0) as u8;
                    if let Some(c) = decode_dtmf_digit(nibble) {
                        digits.push(c);
                    }
                }
            }
            DtmfKind::ToneStart
        }
        1 => {
            if tail_bits != 0 {
                malformed = true;
            }
            DtmfKind::ToneEnd
        }
        2 => {
            if tail_bits != 0 {
                malformed = true;
            }
            DtmfKind::NotSupported
        }
        3 => {
            if tail_bits != 0 {
                malformed = true;
            }
            DtmfKind::NotSubscribed
        }
        4..=7 => {
            if tail_bits != 0 {
                malformed = true;
            }
            DtmfKind::Reserved(dtmf_type)
        }
        _ => DtmfKind::Invalid,
    };

    DtmfDecoded {
        kind,
        digits,
        parsed_bits: len_bits,
        full_len_bits,
        malformed,
    }
}

pub(super) fn pack_type3_bits_to_bytes(field: &Type3FieldGeneric) -> (u16, Vec<u8>) {
    let len_bits = field.len.min(64usize); // data is u64
    if len_bits == 0 {
        return (0, Vec::new());
    }

    let num_bytes = len_bits.div_ceil(8);
    // Extract MSB-first bytes from the u64
    let mut out = Vec::with_capacity(num_bytes);
    for byte_idx in 0..num_bytes {
        let shift = 56usize.saturating_sub(byte_idx * 8);
        out.push((field.data >> shift) as u8);
    }
    // Mask last byte if len_bits isn't byte-aligned
    if len_bits % 8 != 0 {
        let last = out.last_mut().unwrap();
        *last &= 0xffu8 << (8 - (len_bits % 8));
    }
    (len_bits as u16, out)
}
