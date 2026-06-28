use core::fmt;

use crate::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use crate::cmce::ss_dgna::ss_dgna_pdu::SsDgnaPdu;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

/// Representation of the U-FACILITY PDU (EN 300 392-2 V2.4.1 cl.14.7.2.5).
/// Used to send call-unrelated supplementary-service information.
///
/// As with D-FACILITY, CMCE owns only the 5-bit PDU type (= 16); the body is
/// the EN 300 392-9 V1.7.1 SS-PDU container (Table 4):
///
/// ```text
///   PDU type           5b  = 10000 (16)          [EN 300 392-2 Table 114]
///   --- SS body (EN 300 392-9 Table 4) ---
///   Routeing           2b  = 00 (same SwMI; v1 fixed)
///   Number of SS PDUs  4b  = 0001 (v1)
///   Length indicator  11b  = bit-length of the SS PDU
///   SS PDU contents    Nb  = the SS-DGNA PDU
///   O-bit              1b  = 0  (terminates the U-FACILITY PDU)
/// ```
///
/// Empty-body back-compat works exactly as in D-FACILITY: a legacy / non-DGNA
/// U-FACILITY carries no SS PDU and keeps the original single trailing
/// O-bit = 0 convention; a populated container follows Table 4. The two are
/// distinguished on parse by the Number-of-SS-PDUs field.
///
/// The container is terminated by an O-bit = 0, symmetric with D-FACILITY (Annex
/// E Table E.4). We always emit it. On parse we stay tolerant: a real MS's
/// ASSIGN-ACK / DEASSIGN-ACK framing of this trailing bit is not yet confirmed
/// on-air, so the bit is consumed only when present and a peer that omits it is
/// still accepted.
///
/// The empty-vs-container split is a value heuristic, not an explicit
/// presence/length discriminator — EN 300 392-9 V1.7.1 Table 4 carries none. We
/// support exactly two body shapes: an empty body (a single O-bit = 0) and a
/// Table-4 container holding one SS-DGNA PDU. Any other SS body is rejected
/// downstream by `SsType` (only 22 = DGNA is accepted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UFacility {
    /// The SS-DGNA SS-PDU container, or `None` for a legacy empty body.
    pub facility: Option<UFacilitySsBody>,
}

/// The EN 300 392-9 V1.7.1 Table 4 SS-PDU container as carried in U-FACILITY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UFacilitySsBody {
    /// Routeing, 2 bits. 00 = same SwMI (the only value supported in v1).
    pub routeing: u8,
    /// The single SS-DGNA PDU (Number of SS PDUs = 1).
    pub ss_pdu: SsDgnaPdu,
}

impl UFacility {
    /// Parse from BitBuffer.
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeUl::UFacility)?;

        // Distinguish a populated container from a legacy empty body via the
        // Number-of-SS-PDUs field (a real container always carries >= 1).
        let has_container = match buffer.peek_bits(6) {
            Some(header) => (header & 0b1111) >= 1,
            None => false,
        };

        if !has_container {
            // Legacy empty-body convention: trailing O-bit must be 0.
            let obit = delimiters::read_obit(buffer)?;
            if obit {
                return Err(PduParseErr::InvalidTrailingMbitValue);
            }
            return Ok(UFacility { facility: None });
        }

        let routeing = buffer.read_field(2, "routeing")? as u8;
        if routeing != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "routeing",
                value: routeing as u64,
            });
        }
        let number_of_ss_pdus = buffer.read_field(4, "number_of_ss_pdus")?;
        if number_of_ss_pdus != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_ss_pdus",
                value: number_of_ss_pdus,
            });
        }

        let length_indicator = buffer.read_field(11, "length_indicator")? as usize;
        let start_pos = buffer.get_pos();
        let ss_pdu = SsDgnaPdu::from_bitbuf(buffer)?;
        let parsed_bits = buffer.get_pos() - start_pos;
        if parsed_bits != length_indicator {
            return Err(PduParseErr::InconsistentLength {
                expected: length_indicator,
                found: parsed_bits,
            });
        }

        // O-bit terminating the U-FACILITY PDU (symmetric with D-FACILITY; EN 300 392-2 V2.4.1 Annex E
        // Table E.4). We emit it on the write side, but stay tolerant on parse: a real MS's ASSIGN-ACK
        // / DEASSIGN-ACK framing of this trailing bit is not yet confirmed on-air, so consume it only
        // if a bit remains and do NOT reject a peer that omits it. A spurious set bit is ignored.
        let _ = buffer.peek_bits(1).map(|_| delimiters::read_obit(buffer));

        Ok(UFacility {
            facility: Some(UFacilitySsBody { routeing, ss_pdu }),
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type.
        buffer.write_bits(CmcePduTypeUl::UFacility.into_raw(), 5);

        let Some(body) = &self.facility else {
            // Legacy empty body: keep the original single trailing O-bit = 0.
            delimiters::write_mbit(buffer, 0);
            return Ok(());
        };

        // SS body (EN 300 392-9 Table 4).
        buffer.write_bits(body.routeing as u64, 2);
        buffer.write_bits(1, 4); // Number of SS PDUs = 1.

        // Serialize the SS PDU into a scratch buffer to obtain its exact bit
        // length for the 11-bit Length indicator.
        let mut scratch = BitBuffer::new_autoexpand(32);
        body.ss_pdu.to_bitbuf(&mut scratch)?;
        let ss_pdu_bits = scratch.get_pos();
        if ss_pdu_bits > 0x7FF {
            return Err(PduParseErr::InvalidValue {
                field: "length_indicator",
                value: ss_pdu_bits as u64,
            });
        }
        buffer.write_bits(ss_pdu_bits as u64, 11);
        scratch.seek(0);
        buffer.copy_bits(&mut scratch, ss_pdu_bits);

        // O-bit terminating the U-FACILITY PDU itself, symmetric with D-FACILITY (EN 300 392-2 V2.4.1
        // Annex E Table E.4). The Length indicator above counts only the inner SS PDU, so this bit
        // sits after the copied body.
        delimiters::write_obit(buffer, 0);

        Ok(())
    }
}

impl fmt::Display for UFacility {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.facility {
            None => write!(f, "UFacility {{ }}"),
            Some(body) => write!(f, "UFacility {{ routeing: {} ss_pdu: {} }}", body.routeing, body.ss_pdu),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmce::ss_dgna::enums::results::{ResultOfAssignment, ResultOfAttachment};
    use crate::cmce::ss_dgna::fields::group_assignment_ack::GroupAssignmentAck;
    use crate::cmce::ss_dgna::pdus::assign_ack::AssignAck;

    /// Legacy empty body round-trips and stays `None`.
    #[test]
    fn u_facility_empty_round_trips() {
        let pdu = UFacility { facility: None };
        let mut buf = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = UFacility::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
        assert!(parsed.facility.is_none());
    }

    /// A U-FACILITY wrapping an ASSIGN ACK round-trips, with the Length
    /// indicator matching the inner SS PDU's bit length.
    #[test]
    fn u_facility_wraps_assign_ack() {
        let ack = AssignAck {
            acks: vec![GroupAssignmentAck {
                group_ssi: 1234567,
                group_extension: None,
                result_of_assignment: ResultOfAssignment::Accepted,
                result_of_attachment: ResultOfAttachment::Attached,
            }],
        };

        let mut inner = BitBuffer::new_autoexpand(32);
        ack.to_bitbuf(&mut inner).expect("serialize inner");
        let inner_bits = inner.get_pos();

        let pdu = UFacility {
            facility: Some(UFacilitySsBody {
                routeing: 0,
                ss_pdu: SsDgnaPdu::AssignAck(ack.clone()),
            }),
        };

        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).expect("serialize");

        buf.seek(0);
        assert_eq!(buf.read_bits(5).unwrap(), 16, "PDU type = U-FACILITY (16)");
        assert_eq!(buf.read_bits(2).unwrap(), 0, "Routeing = 00");
        assert_eq!(buf.read_bits(4).unwrap(), 1, "Number of SS PDUs = 1");
        assert_eq!(buf.read_bits(11).unwrap() as usize, inner_bits, "Length indicator");

        buf.seek(0);
        let parsed = UFacility::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
    }

    /// The container is terminated by an O-bit = 0, symmetric with D-FACILITY (Annex E Table E.4):
    /// the serialized form is exactly one bit longer than the framing + inner SS PDU, and that bit is
    /// 0. The PDU still round-trips.
    #[test]
    fn u_facility_emits_terminating_obit() {
        let ack = AssignAck {
            acks: vec![GroupAssignmentAck {
                group_ssi: 91,
                group_extension: None,
                result_of_assignment: ResultOfAssignment::Accepted,
                result_of_attachment: ResultOfAttachment::Attached,
            }],
        };

        let mut inner = BitBuffer::new_autoexpand(32);
        ack.to_bitbuf(&mut inner).expect("serialize inner");
        let inner_bits = inner.get_pos();

        let pdu = UFacility {
            facility: Some(UFacilitySsBody {
                routeing: 0,
                ss_pdu: SsDgnaPdu::AssignAck(ack),
            }),
        };

        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        // 5 (PDU type) + 2 (routeing) + 4 (num SS PDUs) + 11 (length) + inner + 1 (terminating O-bit).
        let framed = 5 + 2 + 4 + 11 + inner_bits;
        assert_eq!(buf.get_pos(), framed + 1, "one trailing O-bit beyond the framed body");
        let bits = buf.to_bitstr();
        assert_eq!(&bits[framed..], "0", "terminating O-bit is 0");

        buf.seek(0);
        assert_eq!(UFacility::from_bitbuf(&mut buf).expect("parse"), pdu);
    }

    /// Parse is tolerant of a peer that omits the trailing O-bit (the legacy 67-bit short form): the
    /// container without the terminating bit still parses to the same value. This keeps the BS able
    /// to read a real radio's ASSIGN-ACK whichever form it sends.
    #[test]
    fn u_facility_parses_without_trailing_obit() {
        let ack = AssignAck {
            acks: vec![GroupAssignmentAck {
                group_ssi: 91,
                group_extension: None,
                result_of_assignment: ResultOfAssignment::Accepted,
                result_of_attachment: ResultOfAttachment::Attached,
            }],
        };
        let pdu = UFacility {
            facility: Some(UFacilitySsBody {
                routeing: 0,
                ss_pdu: SsDgnaPdu::AssignAck(ack),
            }),
        };

        // Serialize, then drop the final O-bit to reconstruct the short form.
        let mut full = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut full).expect("serialize");
        let bits = full.to_bitstr();
        let short = &bits[..bits.len() - 1];

        let mut buf = BitBuffer::new_autoexpand(64);
        for ch in short.chars() {
            buf.write_bit(if ch == '1' { 1 } else { 0 });
        }
        buf.seek(0);
        assert_eq!(UFacility::from_bitbuf(&mut buf).expect("parse"), pdu);
    }
}
