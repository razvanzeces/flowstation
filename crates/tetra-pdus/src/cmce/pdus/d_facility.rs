use core::fmt;

use crate::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
use crate::cmce::ss_dgna::ss_dgna_pdu::SsDgnaPdu;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

/// Representation of the D-FACILITY PDU (EN 300 392-2 V2.4.1 cl.14.7.1.7).
/// Used to send call-unrelated supplementary-service information.
///
/// CMCE owns only the 5-bit PDU type (= 16); everything after it is the
/// SS-protocol-defined body. For SS-DGNA we carry the EN 300 392-9 V1.7.1
/// SS-PDU container (Table 4) directly in the body:
///
/// ```text
///   PDU type           5b  = 10000 (16)          [EN 300 392-2 Table 114]
///   --- SS body (EN 300 392-9 Table 4) ---
///   Routeing           2b  = 00 (same SwMI; v1 fixed)
///   Number of SS PDUs  4b  = 0001 (v1)
///   Length indicator  11b  = bit-length of the SS PDU
///   SS PDU contents    Nb  = the SS-DGNA PDU
/// ```
///
/// Empty-body back-compat: a non-DGNA / legacy D-FACILITY carries no SS PDU.
/// EN 300 392-9 does not mandate an O-bit/M-bit trailer on the FACILITY SS
/// body, so we keep the original stub convention — a single trailing O-bit = 0
/// — only for the empty case (`facility = None`). When a real SS PDU is present
/// we follow Table 4 exactly (no trailing M-bit). On parse the two are
/// distinguished by the Number-of-SS-PDUs field: a populated container has
/// Number of SS PDUs >= 1, whereas the empty body has at most the single O-bit.
///
/// Note: EN 300 392-9 Table 4 also defines a 24-bit MNI that is only present
/// when Routeing = 11. v1 always uses Routeing = 00, so the MNI is never
/// emitted; non-zero Routeing on the wire is rejected rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DFacility {
    /// The SS-DGNA SS-PDU container, or `None` for a legacy empty body.
    pub facility: Option<DFacilitySsBody>,
}

/// The EN 300 392-9 V1.7.1 Table 4 SS-PDU container as carried in D-FACILITY.
/// v1 carries exactly one SS PDU and uses Routeing = 00 (same SwMI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DFacilitySsBody {
    /// Routeing, 2 bits. 00 = same SwMI (the only value supported in v1).
    pub routeing: u8,
    /// The single SS-DGNA PDU (Number of SS PDUs = 1).
    pub ss_pdu: SsDgnaPdu,
}

impl DFacility {
    /// Parse from BitBuffer.
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DFacility)?;

        // Distinguish a populated SS-PDU container from a legacy empty body.
        // The container starts with Routeing (2b) + Number of SS PDUs (4b);
        // a real container always carries at least one SS PDU. An empty body
        // has at most the single trailing O-bit, so it cannot supply 6 header
        // bits with a non-zero Number of SS PDUs.
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
            return Ok(DFacility { facility: None });
        }

        let routeing = buffer.read_field(2, "routeing")? as u8;
        if routeing != 0 {
            // Routeing 11 would introduce a 24-bit MNI; not handled in v1.
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

        Ok(DFacility {
            facility: Some(DFacilitySsBody { routeing, ss_pdu }),
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type.
        buffer.write_bits(CmcePduTypeDl::DFacility.into_raw(), 5);

        let Some(body) = &self.facility else {
            // Legacy empty body: keep the original single trailing O-bit = 0.
            delimiters::write_mbit(buffer, 0);
            return Ok(());
        };

        // SS body (EN 300 392-9 Table 4).
        buffer.write_bits(body.routeing as u64, 2);
        buffer.write_bits(1, 4); // Number of SS PDUs = 1.

        // Serialize the SS PDU into a scratch buffer first so we can write its
        // exact bit length as the 11-bit Length indicator.
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

        Ok(())
    }
}

impl fmt::Display for DFacility {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.facility {
            None => write!(f, "DFacility {{ }}"),
            Some(body) => write!(f, "DFacility {{ routeing: {} ss_pdu: {} }}", body.routeing, body.ss_pdu),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmce::ss_dgna::enums::results::GroupIdentityAttachmentMode;
    use crate::cmce::ss_dgna::fields::group_assignment::GroupAssignment;
    use crate::cmce::ss_dgna::pdus::assign::Assign;

    /// Legacy empty body round-trips and stays `None` (back-compat).
    #[test]
    fn d_facility_empty_round_trips() {
        let pdu = DFacility { facility: None };
        let mut buf = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = DFacility::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
        assert!(parsed.facility.is_none());
    }

    /// A D-FACILITY wrapping an ASSIGN: the container parses, carries exactly
    /// one SS PDU, and the 11-bit Length indicator equals the inner SS PDU's
    /// bit length.
    #[test]
    fn d_facility_wraps_assign() {
        let assign = Assign {
            groups: vec![GroupAssignment {
                group_ssi: 1234567,
                group_extension: None,
                attachment_mode: GroupIdentityAttachmentMode::AttachedPermanently,
                class_of_usage: Some(4),
                mnemonic: None,
                security_related_information: None,
                additional_group_information: None,
                vgssi: None,
            }],
            ack_requested: true,
        };

        // Independently compute the inner SS PDU bit length.
        let mut inner = BitBuffer::new_autoexpand(32);
        assign.to_bitbuf(&mut inner).expect("serialize inner");
        let inner_bits = inner.get_pos();

        let pdu = DFacility {
            facility: Some(DFacilitySsBody {
                routeing: 0,
                ss_pdu: SsDgnaPdu::Assign(assign.clone()),
            }),
        };

        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).expect("serialize");

        // Decode the framing fields by hand to assert their exact values.
        buf.seek(0);
        assert_eq!(buf.read_bits(5).unwrap(), 16, "PDU type = D-FACILITY (16)");
        assert_eq!(buf.read_bits(2).unwrap(), 0, "Routeing = 00");
        assert_eq!(buf.read_bits(4).unwrap(), 1, "Number of SS PDUs = 1");
        assert_eq!(
            buf.read_bits(11).unwrap() as usize,
            inner_bits,
            "Length indicator == inner SS PDU bit length"
        );

        // Full round-trip.
        buf.seek(0);
        let parsed = DFacility::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
        let body = parsed.facility.expect("container present");
        match body.ss_pdu {
            SsDgnaPdu::Assign(a) => assert_eq!(a, assign),
            other => panic!("expected Assign, got {other}"),
        }
    }
}
