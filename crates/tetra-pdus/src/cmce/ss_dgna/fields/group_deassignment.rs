use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

/// Group deassignment IE, TS 100 392-12-22 V1.5.1 Table 47.
///
/// Carried (repeated) in the DEASSIGN PDU (Table 20), one entry per group to
/// remove/detach. It names the group only; the action (detach vs. remove) is
/// decided by the MS and reported back in the DEASSIGN ACK.
///
/// Layout:
/// ```text
///   Group SSI               24b  M
///   Group extension present  1b  M
///   Group extension         24b  C  (only if present = 1)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDeassignment {
    /// Group SSI (GSSI), 24 bits.
    pub group_ssi: u32,
    /// Group extension, 24 bits. `Some` sets the present bit.
    pub group_extension: Option<u32>,
}

impl GroupDeassignment {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let group_ssi = buf.read_field(24, "group_ssi")? as u32;
        let ext_present = buf.read_field(1, "group_extension_present")? == 1;
        let group_extension = if ext_present {
            Some(buf.read_field(24, "group_extension")? as u32)
        } else {
            None
        };
        Ok(GroupDeassignment { group_ssi, group_extension })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        buf.write_bits(self.group_ssi as u64, 24);
        buf.write_bits(self.group_extension.is_some() as u64, 1);
        if let Some(ext) = self.group_extension {
            buf.write_bits(ext as u64, 24);
        }
        Ok(())
    }
}

impl fmt::Display for GroupDeassignment {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupDeassignment {{ group_ssi: {} group_extension: {:?} }}",
            self.group_ssi, self.group_extension
        )
    }
}
