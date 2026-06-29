use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::ss_dgna_pdu_type::SsDgnaPduType;
use crate::cmce::ss_dgna::enums::ss_type::SsType;
use crate::cmce::ss_dgna::pdus::assign::Assign;
use crate::cmce::ss_dgna::pdus::assign_ack::AssignAck;
use crate::cmce::ss_dgna::pdus::deassign::Deassign;
use crate::cmce::ss_dgna::pdus::deassign_ack::DeassignAck;

/// One SS-DGNA PDU as carried inside a FACILITY SS-PDU container.
///
/// The variant is chosen from the SS header (SS type 6b + SS-DGNA PDU type 5b,
/// §3.3): on parse we peek those 11 bits before dispatching to the right
/// `from_bitbuf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsDgnaPdu {
    Assign(Assign),
    AssignAck(AssignAck),
    Deassign(Deassign),
    DeassignAck(DeassignAck),
}

impl SsDgnaPdu {
    /// Serialize the inner SS-DGNA PDU (SS header + body, no FACILITY framing).
    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        match self {
            SsDgnaPdu::Assign(p) => p.to_bitbuf(buf),
            SsDgnaPdu::AssignAck(p) => p.to_bitbuf(buf),
            SsDgnaPdu::Deassign(p) => p.to_bitbuf(buf),
            SsDgnaPdu::DeassignAck(p) => p.to_bitbuf(buf),
        }
    }

    /// Parse one SS-DGNA PDU. Peeks the SS type (6b) and SS-DGNA PDU type (5b)
    /// to select the variant, then runs the matching `from_bitbuf` (which
    /// re-reads the header and validates it).
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ss_type_raw = buf.peek_bits(6).ok_or(PduParseErr::BufferEnded { field: Some("ss_type") })?;
        SsType::try_from(ss_type_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "ss_type",
            value: ss_type_raw,
        })?;

        let pdu_type_raw = buf.peek_bits_posoffset(6, 5).ok_or(PduParseErr::BufferEnded {
            field: Some("ss_dgna_pdu_type"),
        })?;
        let pdu_type = SsDgnaPduType::try_from(pdu_type_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "ss_dgna_pdu_type",
            value: pdu_type_raw,
        })?;

        match pdu_type {
            SsDgnaPduType::Assign => Ok(SsDgnaPdu::Assign(Assign::from_bitbuf(buf)?)),
            SsDgnaPduType::AssignAck => Ok(SsDgnaPdu::AssignAck(AssignAck::from_bitbuf(buf)?)),
            SsDgnaPduType::Deassign => Ok(SsDgnaPdu::Deassign(Deassign::from_bitbuf(buf)?)),
            SsDgnaPduType::DeassignAck => Ok(SsDgnaPdu::DeassignAck(DeassignAck::from_bitbuf(buf)?)),
            // DEFINE / DELETE / MODIFY families are recognised but not yet decoded.
            other => Err(PduParseErr::NotImplemented {
                field: Some(match other {
                    SsDgnaPduType::Define => "SS-DGNA DEFINE",
                    SsDgnaPduType::DefineAck => "SS-DGNA DEFINE ACK",
                    SsDgnaPduType::Delete => "SS-DGNA DELETE",
                    SsDgnaPduType::DeleteAck => "SS-DGNA DELETE ACK",
                    SsDgnaPduType::Modify => "SS-DGNA MODIFY",
                    SsDgnaPduType::ModifyAck => "SS-DGNA MODIFY ACK",
                    _ => "SS-DGNA PDU",
                }),
            }),
        }
    }
}

impl fmt::Display for SsDgnaPdu {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SsDgnaPdu::Assign(p) => write!(f, "{}", p),
            SsDgnaPdu::AssignAck(p) => write!(f, "{}", p),
            SsDgnaPdu::Deassign(p) => write!(f, "{}", p),
            SsDgnaPdu::DeassignAck(p) => write!(f, "{}", p),
        }
    }
}
