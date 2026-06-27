use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::results::{ResultOfAssignment, ResultOfAttachment};

/// Group assignment Ack IE, TS 100 392-12-22 V1.5.1 Table 46.
///
/// Carried (repeated) in the ASSIGN ACK PDU (Table 19), one entry per group the
/// affected MS reports on.
///
/// Layout:
/// ```text
///   Group SSI               24b  M
///   Group extension present  1b  M
///   Group extension         24b  C  (only if present = 1)
///   Result of assignment     2b  M  (Table 65)
///   Result of attachment     1b  M  (Table 66)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAssignmentAck {
    /// Group SSI (GSSI), 24 bits.
    pub group_ssi: u32,
    /// Group extension, 24 bits. `Some` sets the present bit.
    pub group_extension: Option<u32>,
    /// Result of assignment, 2 bits (Table 65).
    pub result_of_assignment: ResultOfAssignment,
    /// Result of attachment, 1 bit (Table 66).
    pub result_of_attachment: ResultOfAttachment,
}

impl GroupAssignmentAck {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let group_ssi = buf.read_field(24, "group_ssi")? as u32;
        let ext_present = buf.read_field(1, "group_extension_present")? == 1;
        let group_extension = if ext_present {
            Some(buf.read_field(24, "group_extension")? as u32)
        } else {
            None
        };

        let roa_raw = buf.read_field(2, "result_of_assignment")?;
        let result_of_assignment = ResultOfAssignment::try_from(roa_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "result_of_assignment",
            value: roa_raw,
        })?;

        let rot_raw = buf.read_field(1, "result_of_attachment")?;
        let result_of_attachment = ResultOfAttachment::try_from(rot_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "result_of_attachment",
            value: rot_raw,
        })?;

        Ok(GroupAssignmentAck {
            group_ssi,
            group_extension,
            result_of_assignment,
            result_of_attachment,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        buf.write_bits(self.group_ssi as u64, 24);
        buf.write_bits(self.group_extension.is_some() as u64, 1);
        if let Some(ext) = self.group_extension {
            buf.write_bits(ext as u64, 24);
        }
        buf.write_bits(self.result_of_assignment.into_raw(), 2);
        buf.write_bits(self.result_of_attachment.into_raw(), 1);
        Ok(())
    }
}

impl fmt::Display for GroupAssignmentAck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupAssignmentAck {{ group_ssi: {} group_extension: {:?} result_of_assignment: {} result_of_attachment: {} }}",
            self.group_ssi, self.group_extension, self.result_of_assignment, self.result_of_attachment
        )
    }
}
