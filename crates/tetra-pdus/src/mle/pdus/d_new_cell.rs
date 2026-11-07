use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use super::sdu;
use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;

/// Representation of the D-NEW-CELL PDU (Clause 18.4.1.4.2).
/// Upon receipt from the SwMI the message shall inform the MS-MLE that it can select a new cell as previously indicated in the U-PREPARE or U-PREPARE-DA PDU.
/// Response expected: -
/// Response to: U-PREPARE/U-PREPARE-DA

// note 1: The SDU may carry an MM registration PDU which is used to forward register to a new cell during announced type 1 cell reselection or a D-OTAR CCK PROVIDE PDU which is used to identify the current CCK; it may also provide the future CCK for the LA which the MS has indicated in the U-OTAR CCK DEMAND PDU and whether the CCK provided is in use in other LAs or is used throughout the SwMI. The SDU is coded according to the MM protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug, Clone)]
pub struct DNewCell {
    /// Type1, 2 bits, Channel command valid
    pub channel_command_valid: u8,
    /// Conditional SDU, see note
    pub sdu: Option<BitBuffer>,
}

impl DNewCell {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DNewCell)?;
        let channel_command_valid = buffer.read_field(2, "channel_command_valid")? as u8;
        // No type 2 elements are defined, so the O-bit carries no information; the SDU follows
        // whatever its value (Annex E.2.1 note 2).
        let _obit = delimiters::read_obit(buffer)?;
        let sdu = sdu::read_rest(buffer);

        Ok(DNewCell { channel_command_valid, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MlePduTypeDl::DNewCell.into_raw(), 3);
        buffer.write_bits(self.channel_command_valid as u64, 2);
        // O-bit 0: no type 2 elements; the SDU is not announced by it (Annex E.2.1, E.20/E.22/E.23).
        delimiters::write_obit(buffer, 0);
        if let Some(ref s) = self.sdu {
            sdu::write(buffer, s);
        }
        Ok(())
    }
}

impl fmt::Display for DNewCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DNewCell {{ channel_command_valid: {:?} sdu: {} bits }}", self.channel_command_valid, self.sdu.as_ref().map_or(0, |s| s.get_len()))
    }
}
