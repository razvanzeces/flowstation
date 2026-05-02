pub mod removal {
    use tetra_core::bitbuffer::BitBuffer;

    /// Returns the number of fill bits at the end of the PDU in bitbuf, given the total pdu_len_bits.
    pub fn get_num_fill_bits(bitbuf: &BitBuffer, pdu_len_bits: usize, suppress_warning: bool) -> usize {
        let mut index = pdu_len_bits as isize - 1;
        // TODO FIXME improve efficiency by fetching larger chunks than 1 bit
        while index >= bitbuf.get_pos() as isize {
            let bit = bitbuf.peek_bits_startoffset(index as usize, 1).unwrap();
            if bit == 0 {
                index -= 1;
            } else {
                return (pdu_len_bits as isize - index) as usize;
            }
        }

        if !suppress_warning {
            tracing::warn!("No fill bits found");
        }

        0
    }
}

pub mod addition {
    use tetra_core::bitbuffer::BitBuffer;

    /// Compute how many fill bits need to be added in order to reach the next byte boundary.
    /// Returns 0-7
    #[inline(always)]
    pub fn compute_required_bytealigned(total_pdu_sdu_len_bits: usize) -> usize {
        (8 - total_pdu_sdu_len_bits % 8) % 8
    }

    /// Compute how many fill bits need to be added in order to:
    /// - Reach the next byte boundary, if this would not overflow the slot, or:
    /// - Reach the end of the slot if that occurs after sdu end but before next byte boundary
    /// - If total_pdu_sdu_len_bits already exceeds slot capacity, returns 0 (no fill bits can be added)
    pub fn compute_required(total_pdu_sdu_len_bits: usize, dest_cap_left: usize) -> usize {
        if total_pdu_sdu_len_bits >= dest_cap_left {
            // No fill bits can be added, slot is already full or overflowing
            return 0;
        }
        let bytealigned_fill_bits = compute_required_bytealigned(total_pdu_sdu_len_bits);
        if total_pdu_sdu_len_bits + bytealigned_fill_bits <= dest_cap_left {
            // Simple case; we align to next byte boundary without overflowing the slot
            return bytealigned_fill_bits;
        } else {
            // Adding bytealigned_fill_bits would overflow the slot, so we fill to the end of the slot instead
            return dest_cap_left - total_pdu_sdu_len_bits;
        }
    }

    /// Zeroes all bits behind the current position to end of window, preceded by a 1
    pub fn write(bitbuf: &mut BitBuffer, num_fill_bits: Option<usize>) {
        if let Some(num_fill_bits) = num_fill_bits {
            if num_fill_bits > 0 {
                // Write fill bits
                bitbuf.write_bit(1);
                bitbuf.write_zeroes(num_fill_bits - 1);
            } else {
                // Zero fill bits
            }
        } else if bitbuf.get_len_remaining() > 0 {
            // Fill  buf
            bitbuf.write_bit(1);
            bitbuf.write_zeroes(bitbuf.get_len_remaining());
        } else {
            // No space left
        }
    }
}
