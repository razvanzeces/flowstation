use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::ss_dgna_pdu_type::SsDgnaPduType;
use crate::cmce::ss_dgna::enums::ss_type::SsType;
use crate::cmce::ss_dgna::fields::group_deassignment_ack::GroupDeassignmentAck;

/// DEASSIGN ACK PDU, TS 100 392-12-22 V1.5.1 Table 21 (uplink, carried in
/// U-FACILITY).
///
/// Sent by the affected MS in response to a DEASSIGN. Reports the deassignment
/// result per group and a final "Acknowledgement complete" bit.
///
/// Wire layout:
/// ```text
///   SS type                       6b  = 22 (DGNA)
///   SS-DGNA PDU type               5b  = 01010 (DEASSIGN ACK)   [TS Table 74]
///   Number of groups in deassign ack 5b = acks.len()
///   Group deassignment Ack IE     var  repeated, that many times  [Table 48]
///   Acknowledgement complete       1b  once, at the end
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeassignAck {
    /// One Group deassignment Ack IE per group reported on.
    pub acks: Vec<GroupDeassignmentAck>,
    /// "Acknowledgement complete" — the MS has finished processing the request.
    pub ack_complete: bool,
}

impl DeassignAck {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ss_type = buf.read_field(6, "ss_type")?;
        expect_pdu_type!(ss_type, SsType::Dgna)?;
        let pdu_type = buf.read_field(5, "ss_dgna_pdu_type")?;
        expect_pdu_type!(pdu_type, SsDgnaPduType::DeassignAck)?;

        let number_of_groups = buf.read_field(5, "number_of_groups")? as usize;
        let mut acks = Vec::with_capacity(number_of_groups);
        for _ in 0..number_of_groups {
            acks.push(GroupDeassignmentAck::from_bitbuf(buf)?);
        }

        let ack_complete = buf.read_field(1, "ack_complete")? == 1;

        Ok(DeassignAck { acks, ack_complete })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.acks.len() > 31 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_groups",
                value: self.acks.len() as u64,
            });
        }

        buf.write_bits(SsType::Dgna.into_raw(), 6);
        buf.write_bits(SsDgnaPduType::DeassignAck.into_raw(), 5);
        buf.write_bits(self.acks.len() as u64, 5);
        for ack in &self.acks {
            ack.to_bitbuf(buf)?;
        }
        buf.write_bits(self.ack_complete as u64, 1);
        Ok(())
    }
}

impl fmt::Display for DeassignAck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "DeassignAck {{ acks: {:?} ack_complete: {} }}", self.acks, self.ack_complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmce::ss_dgna::enums::results::ResultOfDeassignment;

    #[test]
    fn deassign_ack_round_trips() {
        let pdu = DeassignAck {
            acks: vec![GroupDeassignmentAck {
                group_ssi: 7654321,
                group_extension: None,
                result_of_deassignment: ResultOfDeassignment::DefinitionRemoved,
            }],
            ack_complete: true,
        };

        let mut buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = DeassignAck::from_bitbuf(&mut buf).expect("parse");
        assert_eq!(parsed, pdu);
    }
}
