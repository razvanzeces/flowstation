use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::results::ResultOfDeassignment;

/// Group deassignment Ack IE, TS 100 392-12-22 V1.5.1 Table 48.
///
/// Carried (repeated) in the DEASSIGN ACK PDU (Table 21), one entry per group.
///
/// Layout:
/// ```text
///   Group SSI               24b  M
///   Group extension present  1b  M
///   Group extension         24b  C  (only if present = 1)
///   Result of deassignment   2b  M  (Table 67)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDeassignmentAck {
    /// Group SSI (GSSI), 24 bits.
    pub group_ssi: u32,
    /// Group extension, 24 bits. `Some` sets the present bit.
    pub group_extension: Option<u32>,
    /// Result of deassignment, 2 bits (Table 67).
    pub result_of_deassignment: ResultOfDeassignment,
}

impl GroupDeassignmentAck {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let group_ssi = buf.read_field(24, "group_ssi")? as u32;
        let ext_present = buf.read_field(1, "group_extension_present")? == 1;
        let group_extension = if ext_present {
            Some(buf.read_field(24, "group_extension")? as u32)
        } else {
            None
        };

        let rod_raw = buf.read_field(2, "result_of_deassignment")?;
        let result_of_deassignment = ResultOfDeassignment::try_from(rod_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "result_of_deassignment",
            value: rod_raw,
        })?;

        Ok(GroupDeassignmentAck {
            group_ssi,
            group_extension,
            result_of_deassignment,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        buf.write_bits(self.group_ssi as u64, 24);
        buf.write_bits(self.group_extension.is_some() as u64, 1);
        if let Some(ext) = self.group_extension {
            buf.write_bits(ext as u64, 24);
        }
        buf.write_bits(self.result_of_deassignment.into_raw(), 2);
        Ok(())
    }
}

impl fmt::Display for GroupDeassignmentAck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupDeassignmentAck {{ group_ssi: {} group_extension: {:?} result_of_deassignment: {} }}",
            self.group_ssi, self.group_extension, self.result_of_deassignment
        )
    }
}
