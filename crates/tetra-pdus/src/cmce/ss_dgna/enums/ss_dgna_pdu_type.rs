/// SS-DGNA PDU type, TS 100 392-12-22 V1.5.1 Table 74.
///
/// Selects the DGNA operation carried in the SS PDU body. Codes 0..4 are
/// reserved by TS 100 392-12-22 for the generic EN 300 392-9 responses
/// (SS-not-supported / action-not-supported) and are not represented here.
///
/// v1 implements ASSIGN / ASSIGN ACK / DEASSIGN / DEASSIGN ACK; the
/// DEFINE / DELETE / MODIFY families are listed for completeness so the parser
/// can name them, but their bodies are out of scope for now.
///
/// Bits: 5
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SsDgnaPduType {
    Define = 5,
    DefineAck = 6,
    Assign = 7,
    AssignAck = 8,
    Deassign = 9,
    DeassignAck = 10,
    Delete = 13,
    DeleteAck = 14,
    Modify = 15,
    ModifyAck = 16,
}

impl std::convert::TryFrom<u64> for SsDgnaPduType {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            5 => Ok(SsDgnaPduType::Define),
            6 => Ok(SsDgnaPduType::DefineAck),
            7 => Ok(SsDgnaPduType::Assign),
            8 => Ok(SsDgnaPduType::AssignAck),
            9 => Ok(SsDgnaPduType::Deassign),
            10 => Ok(SsDgnaPduType::DeassignAck),
            13 => Ok(SsDgnaPduType::Delete),
            14 => Ok(SsDgnaPduType::DeleteAck),
            15 => Ok(SsDgnaPduType::Modify),
            16 => Ok(SsDgnaPduType::ModifyAck),
            _ => Err(()),
        }
    }
}

impl SsDgnaPduType {
    /// Convert this enum back into the raw integer value.
    pub fn into_raw(self) -> u64 {
        match self {
            SsDgnaPduType::Define => 5,
            SsDgnaPduType::DefineAck => 6,
            SsDgnaPduType::Assign => 7,
            SsDgnaPduType::AssignAck => 8,
            SsDgnaPduType::Deassign => 9,
            SsDgnaPduType::DeassignAck => 10,
            SsDgnaPduType::Delete => 13,
            SsDgnaPduType::DeleteAck => 14,
            SsDgnaPduType::Modify => 15,
            SsDgnaPduType::ModifyAck => 16,
        }
    }
}

impl From<SsDgnaPduType> for u64 {
    fn from(e: SsDgnaPduType) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for SsDgnaPduType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SsDgnaPduType::Define => write!(f, "Define"),
            SsDgnaPduType::DefineAck => write!(f, "DefineAck"),
            SsDgnaPduType::Assign => write!(f, "Assign"),
            SsDgnaPduType::AssignAck => write!(f, "AssignAck"),
            SsDgnaPduType::Deassign => write!(f, "Deassign"),
            SsDgnaPduType::DeassignAck => write!(f, "DeassignAck"),
            SsDgnaPduType::Delete => write!(f, "Delete"),
            SsDgnaPduType::DeleteAck => write!(f, "DeleteAck"),
            SsDgnaPduType::Modify => write!(f, "Modify"),
            SsDgnaPduType::ModifyAck => write!(f, "ModifyAck"),
        }
    }
}
