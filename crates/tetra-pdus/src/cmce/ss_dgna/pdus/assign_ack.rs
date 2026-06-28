use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::ss_dgna_pdu_type::SsDgnaPduType;
use crate::cmce::ss_dgna::enums::ss_type::SsType;
use crate::cmce::ss_dgna::fields::group_assignment_ack::GroupAssignmentAck;
use crate::cmce::ss_dgna::{read_terminating_obit, write_terminating_obit};

/// ASSIGN ACK PDU, TS 100 392-12-22 V1.5.1 Table 19 (uplink, carried in
/// U-FACILITY).
///
/// Sent by the affected MS in response to an ASSIGN with the acknowledgement
/// bit set. Reports the assignment/attachment result per group.
///
/// Wire layout:
/// ```text
///   SS type                  6b  = 22 (DGNA)              [EN 300 392-9 Table 5]
///   SS-DGNA PDU type          5b  = 01000 (ASSIGN ACK)    [TS Table 74]
///   Number of groups          5b  = acks.len()
///   Group assignment Ack IE  var  repeated, Number of groups times  [Table 46]
///   O-bit                     1b  = 0, terminates the SS PDU (EN 300 392-2
///                                  annex E; see table E.4)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignAck {
    /// One Group assignment Ack IE per group reported on. Number of groups is
    /// derived from `acks.len()`.
    pub acks: Vec<GroupAssignmentAck>,
}

impl AssignAck {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ss_type = buf.read_field(6, "ss_type")?;
        expect_pdu_type!(ss_type, SsType::Dgna)?;
        let pdu_type = buf.read_field(5, "ss_dgna_pdu_type")?;
        expect_pdu_type!(pdu_type, SsDgnaPduType::AssignAck)?;

        let number_of_groups = buf.read_field(5, "number_of_groups")? as usize;
        let mut acks = Vec::with_capacity(number_of_groups);
        for _ in 0..number_of_groups {
            acks.push(GroupAssignmentAck::from_bitbuf(buf)?);
        }
        read_terminating_obit(buf)?;

        Ok(AssignAck { acks })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.acks.len() > 31 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_groups",
                value: self.acks.len() as u64,
            });
        }

        buf.write_bits(SsType::Dgna.into_raw(), 6);
        buf.write_bits(SsDgnaPduType::AssignAck.into_raw(), 5);
        buf.write_bits(self.acks.len() as u64, 5);
        for ack in &self.acks {
            ack.to_bitbuf(buf)?;
        }
        write_terminating_obit(buf);
        Ok(())
    }
}

impl fmt::Display for AssignAck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "AssignAck {{ acks: {:?} }}", self.acks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmce::ss_dgna::enums::results::{ResultOfAssignment, ResultOfAttachment};

    #[test]
    fn assign_ack_round_trips() {
        let pdu = AssignAck {
            acks: vec![
                GroupAssignmentAck {
                    group_ssi: 1234567,
                    group_extension: None,
                    result_of_assignment: ResultOfAssignment::Accepted,
                    result_of_attachment: ResultOfAttachment::Attached,
                },
                GroupAssignmentAck {
                    group_ssi: 7654321,
                    group_extension: Some(0xABCDE),
                    result_of_assignment: ResultOfAssignment::CapacityRejected,
                    result_of_attachment: ResultOfAttachment::NotAttached,
                },
            ],
        };

        let mut buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = AssignAck::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
    }
}
