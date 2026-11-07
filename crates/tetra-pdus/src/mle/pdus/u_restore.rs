use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use super::sdu;
use crate::mle::enums::mle_pdu_type_ul::MlePduTypeUl;

/// Representation of the U-RESTORE PDU (Clause 18.4.1.4.7).
/// The message shall be sent by the MS-MLE, when restoration of the C-Plane towards a new cell is in progress.
/// Response expected: D-RESTORE-ACK/D-RESTORE-FAIL
/// Response to: -

// note 1: The element is present in the PDU if its value on the new cell is different from that on the old cell.
// note 2: When included, this element gives the value for the old cell.
// note 3: This PDU shall carry a CMCE U-CALL RESTORE PDU which shall be used to restore a call after cell reselection. There shall be no P-bit in the PDU coding preceding the "SDU" information element.
#[derive(Debug, Clone)]
pub struct URestore {
    /// Type2, 10 bits, See notes 1 and 2,
    pub mcc: Option<u64>,
    /// Type2, 14 bits, See notes 1 and 2,
    pub mnc: Option<u64>,
    /// Type2, 14 bits, See notes 1 and 2,
    pub la: Option<u64>,
    /// Conditional: the CMCE U-CALL RESTORE PDU, see note 3
    pub sdu: Option<BitBuffer>,
}

impl URestore {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeUl::URestore)?;

        let obit = delimiters::read_obit(buffer)?;
        let mcc = typed::parse_type2_generic(obit, buffer, 10, "mcc")?;
        let mnc = typed::parse_type2_generic(obit, buffer, 14, "mnc")?;
        let la = typed::parse_type2_generic(obit, buffer, 14, "la")?;
        // The O-bit only announces MCC/MNC/LA; the U-CALL RESTORE follows either way, also when
        // the MS stays in the same LA (Annex E.2.1 note 2, table E.29).
        let sdu = sdu::read_rest(buffer);

        Ok(URestore { mcc, mnc, la, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MlePduTypeUl::URestore.into_raw(), 3);
        let obit = self.mcc.is_some() || self.mnc.is_some() || self.la.is_some();
        delimiters::write_obit(buffer, obit as u8);
        typed::write_type2_generic(obit, buffer, self.mcc, 10);
        typed::write_type2_generic(obit, buffer, self.mnc, 14);
        typed::write_type2_generic(obit, buffer, self.la, 14);
        if let Some(ref s) = self.sdu {
            sdu::write(buffer, s);
        }
        Ok(())
    }
}

impl fmt::Display for URestore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "URestore {{ mcc: {:?} mnc: {:?} la: {:?} sdu: {} bits }}",
            self.mcc,
            self.mnc,
            self.la,
            self.sdu.as_ref().map_or(0, |s| s.get_len()),
        )
    }
}
