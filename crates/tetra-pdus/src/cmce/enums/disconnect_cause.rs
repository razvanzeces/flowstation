/// 14.8.18 Disconnect cause
/// Bits: 3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DisconnectCause {
    CauseNotDefinedOrUnknown = 0,
    UserRequestedDisconnection = 1,
    CalledPartyBusy = 2,
    CalledPartyNotReachable = 3,
    CalledPartyDoesNotSupportEncryption = 4,
    CongestionInInfrastructure = 5,
    NotAllowedTrafficCase = 6,
    IncompatibleTrafficCase = 7,
    RequestedServiceNotAvailable = 8,
    PreEmptiveUseOfResource = 9,
    InvalidCallIdentifier = 10,
    CallRejectedByTheCalledParty = 11,
    NoIdleCcEntity = 12,
    ExpiryOfTimer = 13,
    SwmiRequestedDisconnection = 14,
    AcknowledgedServiceNotComplete = 15,
    UnknownTetraIdentity = 16,
    SsSpecificDisconnection = 17,
    UnknownExternalSubscriberIdentity = 18,
    CallRestorationOfTheOtherUserFailed = 19,
    CalledPartyRequiresEncryption = 20,
    ConcurrentSetUpNotSupported = 21,
    CalledPartyIsUnderTheSameDmGateOfTheCallingParty = 22,
    NonCallOwnerRequestedDisconnection = 23,
}

impl std::convert::TryFrom<u64> for DisconnectCause {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(DisconnectCause::CauseNotDefinedOrUnknown),
            1 => Ok(DisconnectCause::UserRequestedDisconnection),
            2 => Ok(DisconnectCause::CalledPartyBusy),
            3 => Ok(DisconnectCause::CalledPartyNotReachable),
            4 => Ok(DisconnectCause::CalledPartyDoesNotSupportEncryption),
            5 => Ok(DisconnectCause::CongestionInInfrastructure),
            6 => Ok(DisconnectCause::NotAllowedTrafficCase),
            7 => Ok(DisconnectCause::IncompatibleTrafficCase),
            8 => Ok(DisconnectCause::RequestedServiceNotAvailable),
            9 => Ok(DisconnectCause::PreEmptiveUseOfResource),
            10 => Ok(DisconnectCause::InvalidCallIdentifier),
            11 => Ok(DisconnectCause::CallRejectedByTheCalledParty),
            12 => Ok(DisconnectCause::NoIdleCcEntity),
            13 => Ok(DisconnectCause::ExpiryOfTimer),
            14 => Ok(DisconnectCause::SwmiRequestedDisconnection),
            15 => Ok(DisconnectCause::AcknowledgedServiceNotComplete),
            16 => Ok(DisconnectCause::UnknownTetraIdentity),
            17 => Ok(DisconnectCause::SsSpecificDisconnection),
            18 => Ok(DisconnectCause::UnknownExternalSubscriberIdentity),
            19 => Ok(DisconnectCause::CallRestorationOfTheOtherUserFailed),
            20 => Ok(DisconnectCause::CalledPartyRequiresEncryption),
            21 => Ok(DisconnectCause::ConcurrentSetUpNotSupported),
            22 => Ok(DisconnectCause::CalledPartyIsUnderTheSameDmGateOfTheCallingParty),
            23 => Ok(DisconnectCause::NonCallOwnerRequestedDisconnection),
            _ => Err(()),
        }
    }
}

impl DisconnectCause {
    /// Convert this enum back into the raw integer value
    pub fn into_raw(self) -> u64 {
        match self {
            DisconnectCause::CauseNotDefinedOrUnknown => 0,
            DisconnectCause::UserRequestedDisconnection => 1,
            DisconnectCause::CalledPartyBusy => 2,
            DisconnectCause::CalledPartyNotReachable => 3,
            DisconnectCause::CalledPartyDoesNotSupportEncryption => 4,
            DisconnectCause::CongestionInInfrastructure => 5,
            DisconnectCause::NotAllowedTrafficCase => 6,
            DisconnectCause::IncompatibleTrafficCase => 7,
            DisconnectCause::RequestedServiceNotAvailable => 8,
            DisconnectCause::PreEmptiveUseOfResource => 9,
            DisconnectCause::InvalidCallIdentifier => 10,
            DisconnectCause::CallRejectedByTheCalledParty => 11,
            DisconnectCause::NoIdleCcEntity => 12,
            DisconnectCause::ExpiryOfTimer => 13,
            DisconnectCause::SwmiRequestedDisconnection => 14,
            DisconnectCause::AcknowledgedServiceNotComplete => 15,
            DisconnectCause::UnknownTetraIdentity => 16,
            DisconnectCause::SsSpecificDisconnection => 17,
            DisconnectCause::UnknownExternalSubscriberIdentity => 18,
            DisconnectCause::CallRestorationOfTheOtherUserFailed => 19,
            DisconnectCause::CalledPartyRequiresEncryption => 20,
            DisconnectCause::ConcurrentSetUpNotSupported => 21,
            DisconnectCause::CalledPartyIsUnderTheSameDmGateOfTheCallingParty => 22,
            DisconnectCause::NonCallOwnerRequestedDisconnection => 23,
        }
    }
}

impl From<DisconnectCause> for u64 {
    fn from(e: DisconnectCause) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for DisconnectCause {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DisconnectCause::CauseNotDefinedOrUnknown => write!(f, "CauseNotDefinedOrUnknown"),
            DisconnectCause::UserRequestedDisconnection => write!(f, "UserRequestedDisconnection"),
            DisconnectCause::CalledPartyBusy => write!(f, "CalledPartyBusy"),
            DisconnectCause::CalledPartyNotReachable => write!(f, "CalledPartyNotReachable"),
            DisconnectCause::CalledPartyDoesNotSupportEncryption => write!(f, "CalledPartyDoesNotSupportEncryption"),
            DisconnectCause::CongestionInInfrastructure => write!(f, "CongestionInInfrastructure"),
            DisconnectCause::NotAllowedTrafficCase => write!(f, "NotAllowedTrafficCase"),
            DisconnectCause::IncompatibleTrafficCase => write!(f, "IncompatibleTrafficCase"),
            DisconnectCause::RequestedServiceNotAvailable => write!(f, "RequestedServiceNotAvailable"),
            DisconnectCause::PreEmptiveUseOfResource => write!(f, "PreEmptiveUseOfResource"),
            DisconnectCause::InvalidCallIdentifier => write!(f, "InvalidCallIdentifier"),
            DisconnectCause::CallRejectedByTheCalledParty => write!(f, "CallRejectedByTheCalledParty"),
            DisconnectCause::NoIdleCcEntity => write!(f, "NoIdleCcEntity"),
            DisconnectCause::ExpiryOfTimer => write!(f, "ExpiryOfTimer"),
            DisconnectCause::SwmiRequestedDisconnection => write!(f, "SwmiRequestedDisconnection"),
            DisconnectCause::AcknowledgedServiceNotComplete => write!(f, "AcknowledgedServiceNotComplete"),
            DisconnectCause::UnknownTetraIdentity => write!(f, "UnknownTetraIdentity"),
            DisconnectCause::SsSpecificDisconnection => write!(f, "SsSpecificDisconnection"),
            DisconnectCause::UnknownExternalSubscriberIdentity => write!(f, "UnknownExternalSubscriberIdentity"),
            DisconnectCause::CallRestorationOfTheOtherUserFailed => write!(f, "CallRestorationOfTheOtherUserFailed"),
            DisconnectCause::CalledPartyRequiresEncryption => write!(f, "CalledPartyRequiresEncryption"),
            DisconnectCause::ConcurrentSetUpNotSupported => write!(f, "ConcurrentSetUpNotSupported"),
            DisconnectCause::CalledPartyIsUnderTheSameDmGateOfTheCallingParty => {
                write!(f, "CalledPartyIsUnderTheSameDmGateOfTheCallingParty")
            }
            DisconnectCause::NonCallOwnerRequestedDisconnection => write!(f, "NonCallOwnerRequestedDisconnection"),
        }
    }
}
