use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::ss_dgna_pdu_type::SsDgnaPduType;
use crate::cmce::ss_dgna::enums::ss_type::SsType;
use crate::cmce::ss_dgna::fields::group_deassignment::GroupDeassignment;
use crate::cmce::ss_dgna::{read_terminating_obit, write_terminating_obit};

/// DEASSIGN PDU, TS 100 392-12-22 V1.5.1 Table 20 (downlink, carried in
/// D-FACILITY).
///
/// Removes/detaches groups from the affected user(s). A "Number of groups in
/// deassign request" of 00000 means "deassign all groups" (Table 64): in that
/// case `groups` is empty and no Group deassignment IE follows.
///
/// Wire layout:
/// ```text
///   SS type                                6b  = 22 (DGNA)
///   SS-DGNA PDU type                        5b  = 01001 (DEASSIGN)    [TS Table 74]
///   Number of groups in deassign request    5b  = groups.len() (00000 = ALL)
///   Group deassignment IE                  var  repeated, that many times  [Table 47]
///   Acknowledgement requested               1b  once, at the end
///   O-bit                                   1b  = 0, terminates the SS PDU
///                                                (EN 300 392-2 annex E; see table E.4)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deassign {
    /// One Group deassignment IE per group; empty means "deassign all" (the
    /// Number-of-groups field is encoded as 00000).
    pub groups: Vec<GroupDeassignment>,
    /// "Acknowledgement requested from affected user(s)" — request a DEASSIGN ACK.
    pub ack_requested: bool,
}

impl Deassign {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ss_type = buf.read_field(6, "ss_type")?;
        expect_pdu_type!(ss_type, SsType::Dgna)?;
        let pdu_type = buf.read_field(5, "ss_dgna_pdu_type")?;
        expect_pdu_type!(pdu_type, SsDgnaPduType::Deassign)?;

        let number_of_groups = buf.read_field(5, "number_of_groups")? as usize;
        let mut groups = Vec::with_capacity(number_of_groups);
        for _ in 0..number_of_groups {
            groups.push(GroupDeassignment::from_bitbuf(buf)?);
        }

        let ack_requested = buf.read_field(1, "ack_requested")? == 1;
        read_terminating_obit(buf)?;

        Ok(Deassign { groups, ack_requested })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.groups.len() > 31 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_groups",
                value: self.groups.len() as u64,
            });
        }

        buf.write_bits(SsType::Dgna.into_raw(), 6);
        buf.write_bits(SsDgnaPduType::Deassign.into_raw(), 5);
        buf.write_bits(self.groups.len() as u64, 5);
        for group in &self.groups {
            group.to_bitbuf(buf)?;
        }
        buf.write_bits(self.ack_requested as u64, 1);
        write_terminating_obit(buf);
        Ok(())
    }
}

impl fmt::Display for Deassign {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Deassign {{ groups: {:?} ack_requested: {} }}", self.groups, self.ack_requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deassign_round_trips() {
        let pdu = Deassign {
            groups: vec![GroupDeassignment {
                group_ssi: 7654321,
                group_extension: None,
            }],
            ack_requested: true,
        };

        let mut buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = Deassign::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
    }

    /// "Deassign all" — Number of groups = 00000, no IE follows, then the ack bit.
    #[test]
    fn deassign_all_round_trips() {
        let pdu = Deassign {
            groups: vec![],
            ack_requested: false,
        };

        let mut buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        // SS type 6 + PDU type 5 + number of groups 5 + ack 1 + terminating O-bit 1 = 18 bits.
        assert_eq!(buf.get_pos(), 18);
        buf.seek(0);
        let parsed = Deassign::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
    }
}
