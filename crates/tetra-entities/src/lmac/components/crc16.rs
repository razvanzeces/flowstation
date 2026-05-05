/// CRC-16 (ITU-T / X.25) over raw bits or byte streams.
pub const GEN_POLY: u16 = 0x1021;
pub const TETRA_CRC_OK: u16 = 0x1d0f;

#[inline]
pub fn get_nth_bit(input: &[u8], bit: usize) -> u16 {
    let byte = bit / 8;
    let bit_in_byte = 7 - (bit % 8);
    ((input[byte] >> bit_in_byte) & 1) as u16
}

/// CRC-16 ITU-T over a byte stream, processing `number_bits` bits (MSB first).
/// `crc` is the initial CRC value.  
/// Returns the updated CRC.
pub fn crc16_itut_bytes(mut crc: u16, input: &[u8], number_bits: usize) -> u16 {
    for i in 0..number_bits {
        let bit = get_nth_bit(input, i);
        crc ^= bit << 15;
        if (crc & 0x8000) != 0 {
            crc = (crc << 1).wrapping_add(0) ^ GEN_POLY;
        } else {
            crc <<= 1;
        }
    }
    crc
}

/// CRC-16 ITU-T over a bit-per-byte slice: each `input[i] & 1` is one bit.
/// `crc` is the initial CRC value.  
/// Processes the first `number_bits` entries of `input`.
pub fn crc16_itut_bits(mut crc: u16, input: &[u8], number_bits: usize) -> u16 {
    for &b in input.iter().take(number_bits) {
        let bit = (b & 1) as u16;
        crc ^= bit << 15;
        if (crc & 0x8000) != 0 {
            crc = (crc << 1).wrapping_add(0) ^ GEN_POLY;
        } else {
            crc <<= 1;
        }
    }
    crc
}

/// Standard CRC-ITU-T (initial 0xffff) over a bit-per-byte slice, as it is used in TETRA.
pub fn crc16_ccitt_bits(input: &[u8], len: usize) -> u16 {
    crc16_itut_bits(0xffff, input, len)
}
