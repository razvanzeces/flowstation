/// Clause 14.8.16 Call time-out
/// Bits: 4
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CallTimeout {
    Infinite = 0,
    T30s = 1,
    T45s = 2,
    T60s = 3,
    T2m = 4,
    T3m = 5,
    T4m = 6,
    T5m = 7,
    T6m = 8,
    T8m = 9,
    T10m = 10,
    T12m = 11,
    T15m = 12,
    T20m = 13,
    T30m = 14,
    Reserved = 15,
}

impl std::convert::TryFrom<u64> for CallTimeout {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(CallTimeout::Infinite),
            1 => Ok(CallTimeout::T30s),
            2 => Ok(CallTimeout::T45s),
            3 => Ok(CallTimeout::T60s),
            4 => Ok(CallTimeout::T2m),
            5 => Ok(CallTimeout::T3m),
            6 => Ok(CallTimeout::T4m),
            7 => Ok(CallTimeout::T5m),
            8 => Ok(CallTimeout::T6m),
            9 => Ok(CallTimeout::T8m),
            10 => Ok(CallTimeout::T10m),
            11 => Ok(CallTimeout::T12m),
            12 => Ok(CallTimeout::T15m),
            13 => Ok(CallTimeout::T20m),
            14 => Ok(CallTimeout::T30m),
            15 => Ok(CallTimeout::Reserved),
            _ => Err(()),
        }
    }
}

impl CallTimeout {
    /// Convert this enum back into the raw integer value
    pub fn into_raw(self) -> u64 {
        match self {
            CallTimeout::Infinite => 0,
            CallTimeout::T30s => 1,
            CallTimeout::T45s => 2,
            CallTimeout::T60s => 3,
            CallTimeout::T2m => 4,
            CallTimeout::T3m => 5,
            CallTimeout::T4m => 6,
            CallTimeout::T5m => 7,
            CallTimeout::T6m => 8,
            CallTimeout::T8m => 9,
            CallTimeout::T10m => 10,
            CallTimeout::T12m => 11,
            CallTimeout::T15m => 12,
            CallTimeout::T20m => 13,
            CallTimeout::T30m => 14,
            CallTimeout::Reserved => 15,
        }
    }
}

impl From<CallTimeout> for u64 {
    fn from(e: CallTimeout) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for CallTimeout {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CallTimeout::Infinite => write!(f, "Infinite"),
            CallTimeout::T30s => write!(f, "T30s"),
            CallTimeout::T45s => write!(f, "T45s"),
            CallTimeout::T60s => write!(f, "T60s"),
            CallTimeout::T2m => write!(f, "T2m"),
            CallTimeout::T3m => write!(f, "T3m"),
            CallTimeout::T4m => write!(f, "T4m"),
            CallTimeout::T5m => write!(f, "T5m"),
            CallTimeout::T6m => write!(f, "T6m"),
            CallTimeout::T8m => write!(f, "T8m"),
            CallTimeout::T10m => write!(f, "T10m"),
            CallTimeout::T12m => write!(f, "T12m"),
            CallTimeout::T15m => write!(f, "T15m"),
            CallTimeout::T20m => write!(f, "T20m"),
            CallTimeout::T30m => write!(f, "T30m"),
            CallTimeout::Reserved => write!(f, "Reserved"),
        }
    }
}
