use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::ss_dgna_pdu_type::SsDgnaPduType;
use crate::cmce::ss_dgna::enums::ss_type::SsType;
use crate::cmce::ss_dgna::fields::group_assignment::GroupAssignment;
use crate::cmce::ss_dgna::{read_terminating_obit, write_terminating_obit};

/// ASSIGN PDU, TS 100 392-12-22 V1.5.1 Table 18 (downlink, carried in
/// D-FACILITY).
///
/// Adds groups and their parameters to the affected user(s). Each group is a
/// Group assignment IE (Table 45); the per-PDU "Acknowledgement requested" bit
/// is written once at the end, after all the IEs, not per group.
///
/// Wire layout:
/// ```text
///   SS type                   6b  = 22 (DGNA)              [EN 300 392-9 Table 5]
///   SS-DGNA PDU type           5b  = 00111 (ASSIGN)        [TS Table 74]
///   Number of groups           5b  = groups.len() (1..31)
///   Group assignment IE       var  repeated, Number of groups times  [Table 45]
///   Acknowledgement requested  1b  once, at the end
///   O-bit                      1b  = 0, terminates the SS PDU (EN 300 392-2
///                                   annex E; see table E.4)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assign {
    /// One Group assignment IE per group being assigned. Number of groups is
    /// derived from `groups.len()`; must be 1..=31.
    pub groups: Vec<GroupAssignment>,
    /// "Acknowledgement requested from affected user(s)" — request an ASSIGN ACK.
    pub ack_requested: bool,
}

impl Assign {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ss_type = buf.read_field(6, "ss_type")?;
        expect_pdu_type!(ss_type, SsType::Dgna)?;
        let pdu_type = buf.read_field(5, "ss_dgna_pdu_type")?;
        expect_pdu_type!(pdu_type, SsDgnaPduType::Assign)?;

        let number_of_groups = buf.read_field(5, "number_of_groups")? as usize;
        // An ASSIGN must carry at least one group (Table 18). 0 has no meaning and the encoder rejects
        // it, so reject it on decode too — otherwise a parsed 0-group ASSIGN could never be
        // re-serialized (decode-then-forward would fail).
        if number_of_groups == 0 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_groups",
                value: 0,
            });
        }
        let mut groups = Vec::with_capacity(number_of_groups);
        for _ in 0..number_of_groups {
            groups.push(GroupAssignment::from_bitbuf(buf)?);
        }

        let ack_requested = buf.read_field(1, "ack_requested")? == 1;
        read_terminating_obit(buf)?;

        Ok(Assign { groups, ack_requested })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.groups.is_empty() || self.groups.len() > 31 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_groups",
                value: self.groups.len() as u64,
            });
        }

        buf.write_bits(SsType::Dgna.into_raw(), 6);
        buf.write_bits(SsDgnaPduType::Assign.into_raw(), 5);
        buf.write_bits(self.groups.len() as u64, 5);
        for group in &self.groups {
            group.to_bitbuf(buf)?;
        }
        buf.write_bits(self.ack_requested as u64, 1);
        write_terminating_obit(buf);
        Ok(())
    }
}

impl fmt::Display for Assign {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Assign {{ groups: {:?} ack_requested: {} }}", self.groups, self.ack_requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmce::ss_dgna::enums::results::GroupIdentityAttachmentMode;

    /// The SwMI-initiated regroup push: one GSSI, attachment mode 000 (attached
    /// permanently), class of usage 4, no mnemonic. Round-trips and matches the
    /// exact bit string of the header plus minimal IE.
    #[test]
    fn assign_round_trips() {
        for mnemonic in [None, Some("DISPATCH".to_string())] {
            let pdu = Assign {
                groups: vec![GroupAssignment {
                    group_ssi: 1234567,
                    group_extension: None,
                    attachment_mode: GroupIdentityAttachmentMode::AttachedPermanently,
                    class_of_usage: Some(4),
                    mnemonic: mnemonic.clone(),
                    security_related_information: None,
                    additional_group_information: None,
                    vgssi: None,
                }],
                ack_requested: true,
            };

            let mut buf = BitBuffer::new_autoexpand(32);
            pdu.to_bitbuf(&mut buf).expect("serialize");
            buf.seek(0);
            let parsed = Assign::from_bitbuf(&mut buf).expect("parse");
            assert_eq!(parsed, pdu);
        }
    }

    /// Exact bit-vector for a known minimal ASSIGN with no type-2 optionals:
    /// SS type = 22 (010110), PDU type = ASSIGN (00111), Number of groups = 1
    /// (00001), then the single Group assignment IE and the trailing ack bit.
    ///
    /// IE: Group SSI = 1 (24b), ext present = 0, attachment mode = 000, O-bit = 0.
    /// Trailing "Acknowledgement requested" = 1, then the SS PDU terminating
    /// O-bit = 0 (EN 300 392-2 annex E table E.4).
    #[test]
    fn assign_exact_bits() {
        let pdu = Assign {
            groups: vec![GroupAssignment {
                group_ssi: 1,
                group_extension: None,
                attachment_mode: GroupIdentityAttachmentMode::AttachedPermanently,
                class_of_usage: None,
                mnemonic: None,
                security_related_information: None,
                additional_group_information: None,
                vgssi: None,
            }],
            ack_requested: true,
        };

        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).expect("serialize");

        let expected = concat!(
            "010110", // SS type = 22
            "00111",  // SS-DGNA PDU type = ASSIGN (7)
            "00001",  // Number of groups = 1
            // Group assignment IE:
            "000000000000000000000001", // Group SSI = 1
            "0",                        // Group extension present = 0
            "000",                      // Group identity attachment mode = 000
            "0",                        // O-bit (no type-2 optionals)
            "1",                        // Acknowledgement requested = 1
            "0",                        // SS PDU terminating O-bit
        );
        assert_eq!(buf.to_bitstr(), expected);
    }

    /// A 0-group ASSIGN must be rejected on decode (the encoder already rejects it). Build the header
    /// by hand with Number of groups = 0 and assert the parse fails with InvalidValue.
    #[test]
    fn assign_rejects_zero_groups() {
        let mut buf = BitBuffer::new_autoexpand(8);
        buf.write_bits(SsType::Dgna.into_raw(), 6);
        buf.write_bits(SsDgnaPduType::Assign.into_raw(), 5);
        buf.write_bits(0, 5); // Number of groups = 0.
        buf.write_bits(0, 1); // ack_requested (unreached, but keep the buffer non-empty).
        buf.seek(0);
        match Assign::from_bitbuf(&mut buf) {
            Err(PduParseErr::InvalidValue { field, value }) => {
                assert_eq!(field, "number_of_groups");
                assert_eq!(value, 0);
            }
            other => panic!("expected InvalidValue for 0 groups, got {other:?}"),
        }
    }

    /// Type-2 framing: mnemonic present vs absent must produce the right O/P
    /// bits and round-trip identically. With only a present mnemonic the IE
    /// ends O-bit=1, P(class)=0, P(mnemonic)=1, 7-bit coding-scheme(0x01),
    /// length-in-bits, octets, P(sec)=0, P(add)=0, P(vgssi)=0.
    #[test]
    fn group_assignment_ie_optionals() {
        // Absent: O-bit clear, no P-bits.
        let absent = GroupAssignment {
            group_ssi: 42,
            group_extension: None,
            attachment_mode: GroupIdentityAttachmentMode::AttachedPermanently,
            class_of_usage: None,
            mnemonic: None,
            security_related_information: None,
            additional_group_information: None,
            vgssi: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        absent.to_bitbuf(&mut buf).expect("serialize");
        // 24 (ssi) + 1 (ext) + 3 (mode) + 1 (o-bit) = 29 bits, last bit = O-bit = 0.
        assert_eq!(buf.get_pos(), 29);
        assert!(buf.to_bitstr().ends_with('0'), "O-bit must be clear when no optionals");

        // Present mnemonic only: O-bit set, P(class)=0, P(mnemonic)=1.
        let present = GroupAssignment {
            mnemonic: Some("AB".to_string()),
            ..absent.clone()
        };
        let mut buf2 = BitBuffer::new_autoexpand(32);
        present.to_bitbuf(&mut buf2).expect("serialize");
        let s = buf2.to_bitstr();
        // Fixed region: 24 + 1 + 3 = 28 bits, then O-bit=1, P(class)=0, P(mnemonic)=1.
        assert_eq!(&s[28..29], "1", "O-bit set");
        assert_eq!(&s[29..30], "0", "P-bit for class of usage = 0 (absent)");
        assert_eq!(&s[30..31], "1", "P-bit for mnemonic = 1 (present)");
        assert_eq!(&s[31..38], "0000001", "mnemonic coding scheme = 0x01 (Latin-1)");
        assert_eq!(&s[38..46], "00010000", "mnemonic length = 16 bits");
        assert_eq!(&s[46..54], "01000001", "first octet 'A' (0x41)");
        assert_eq!(&s[54..62], "01000010", "second octet 'B' (0x42)");
        assert_eq!(&s[62..65], "000", "remaining type-2 P-bits absent");

        // Round-trip both.
        for ga in [absent, present] {
            let mut b = BitBuffer::new_autoexpand(32);
            ga.to_bitbuf(&mut b).expect("serialize");
            b.seek(0);
            let parsed = GroupAssignment::from_bitbuf(&mut b).expect("parse");
            assert_eq!(parsed, ga);
        }
    }
}
