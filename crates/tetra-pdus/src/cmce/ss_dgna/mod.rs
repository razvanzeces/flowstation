//! SS-DGNA (Dynamic Group Number Assignment) supplementary service.
//!
//! Implements the SS-DGNA PDUs and information elements from
//! TS 100 392-12-22 V1.5.1, carried over the CMCE U/D-FACILITY mechanism
//! (EN 300 392-9 V1.7.1 transport, EN 300 392-2 V2.4.1 CMCE framing).
//!
//! Scope today: ASSIGN / ASSIGN ACK / DEASSIGN / DEASSIGN ACK. The
//! DEFINE / DELETE / MODIFY / INTERROGATE families are not yet implemented;
//! the module layout mirrors `cmce/pdus` + `cmce/fields` so they slot in later.

pub mod enums;
pub mod fields;
pub mod pdus;
pub mod ss_dgna_pdu;

use tetra_core::typed_pdu_fields::delimiters;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

/// Write the terminating O-bit that closes an SS-DGNA PDU.
///
/// All four SS-DGNA PDUs we encode (ASSIGN / ASSIGN ACK / DEASSIGN /
/// DEASSIGN ACK) are made up only of type-1 elements, but EN 300 392-2 annex E
/// still requires a final O-bit = 0 ("no type 2/3/4 elements follow") as the
/// last bit of any such PDU. See the worked example in EN 300 392-2 annex E
/// table E.4 (D-FACILITY with SS-DGNA ASSIGN), which terminates the ASSIGN PDU
/// with this O-bit before the D-FACILITY's own trailing O-bit.
fn write_terminating_obit(buf: &mut BitBuffer) {
    delimiters::write_obit(buf, 0);
}

/// Read and require the terminating O-bit of an SS-DGNA PDU (see
/// [`write_terminating_obit`]). We never define type-2/3/4 elements, so the
/// only valid value is 0; a 1 would announce optional elements we do not parse.
fn read_terminating_obit(buf: &mut BitBuffer) -> Result<(), PduParseErr> {
    if delimiters::read_obit(buf)? {
        return Err(PduParseErr::InvalidTrailingMbitValue);
    }
    Ok(())
}
