/// Supplementary service type, EN 300 392-9 V1.7.1 Table 5 ("SS type").
///
/// Identifies which supplementary service a FACILITY-borne SS PDU belongs to.
/// Only DGNA (Dynamic Group Number Assignment) is carried today; the remaining
/// SS types fall through to `TryFrom` errors and are answered with the generic
/// "SS not supported" path by the receiver.
///
/// Bits: 6
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SsType {
    /// Dynamic Group Number Assignment, TS 100 392-12-22 V1.5.1.
    Dgna = 22,
}

impl std::convert::TryFrom<u64> for SsType {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            22 => Ok(SsType::Dgna),
            _ => Err(()),
        }
    }
}

impl SsType {
    /// Convert this enum back into the raw integer value.
    pub fn into_raw(self) -> u64 {
        match self {
            SsType::Dgna => 22,
        }
    }
}

impl From<SsType> for u64 {
    fn from(e: SsType) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for SsType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SsType::Dgna => write!(f, "Dgna"),
        }
    }
}
