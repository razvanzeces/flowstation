//! Result and mode code points used in the SS-DGNA ACK PDUs and the Group
//! assignment IE, all from TS 100 392-12-22 V1.5.1.
//!
//! Each follows the same `TryFrom<u64>` / `into_raw` / `From` shape as the
//! other CMCE enums so they slot into `write_bits` / `read_field` directly.

/// Result of assignment, TS 100 392-12-22 V1.5.1 Table 65.
///
/// Reported per group in the ASSIGN ACK (one entry per Group assignment Ack IE).
///
/// Bits: 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResultOfAssignment {
    /// Assignment rejected (unspecified reason).
    Rejected = 0,
    /// Group successfully assigned.
    Accepted = 1,
    /// Assignment rejected for a security reason.
    SecurityRejected = 2,
    /// Assignment rejected, no capacity for further groups.
    CapacityRejected = 3,
}

impl std::convert::TryFrom<u64> for ResultOfAssignment {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(ResultOfAssignment::Rejected),
            1 => Ok(ResultOfAssignment::Accepted),
            2 => Ok(ResultOfAssignment::SecurityRejected),
            3 => Ok(ResultOfAssignment::CapacityRejected),
            _ => Err(()),
        }
    }
}

impl ResultOfAssignment {
    pub fn into_raw(self) -> u64 {
        match self {
            ResultOfAssignment::Rejected => 0,
            ResultOfAssignment::Accepted => 1,
            ResultOfAssignment::SecurityRejected => 2,
            ResultOfAssignment::CapacityRejected => 3,
        }
    }
}

impl From<ResultOfAssignment> for u64 {
    fn from(e: ResultOfAssignment) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for ResultOfAssignment {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ResultOfAssignment::Rejected => write!(f, "Rejected"),
            ResultOfAssignment::Accepted => write!(f, "Accepted"),
            ResultOfAssignment::SecurityRejected => write!(f, "SecurityRejected"),
            ResultOfAssignment::CapacityRejected => write!(f, "CapacityRejected"),
        }
    }
}

/// Result of attachment, TS 100 392-12-22 V1.5.1 Table 66.
///
/// Reported per group in the ASSIGN ACK alongside the result of assignment.
///
/// Bits: 1
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResultOfAttachment {
    /// Group was not attached.
    NotAttached = 0,
    /// Group was attached.
    Attached = 1,
}

impl std::convert::TryFrom<u64> for ResultOfAttachment {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(ResultOfAttachment::NotAttached),
            1 => Ok(ResultOfAttachment::Attached),
            _ => Err(()),
        }
    }
}

impl ResultOfAttachment {
    pub fn into_raw(self) -> u64 {
        match self {
            ResultOfAttachment::NotAttached => 0,
            ResultOfAttachment::Attached => 1,
        }
    }
}

impl From<ResultOfAttachment> for u64 {
    fn from(e: ResultOfAttachment) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for ResultOfAttachment {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ResultOfAttachment::NotAttached => write!(f, "NotAttached"),
            ResultOfAttachment::Attached => write!(f, "Attached"),
        }
    }
}

/// Result of deassignment, TS 100 392-12-22 V1.5.1 Table 67.
///
/// Reported per group in the DEASSIGN ACK. Code points 2 and 3 are reserved.
///
/// Bits: 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResultOfDeassignment {
    /// Group definition kept in the MS, but the group was detached.
    DefinitionKeptDetached = 0,
    /// Group definition removed from the MS.
    DefinitionRemoved = 1,
}

impl std::convert::TryFrom<u64> for ResultOfDeassignment {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(ResultOfDeassignment::DefinitionKeptDetached),
            1 => Ok(ResultOfDeassignment::DefinitionRemoved),
            _ => Err(()),
        }
    }
}

impl ResultOfDeassignment {
    pub fn into_raw(self) -> u64 {
        match self {
            ResultOfDeassignment::DefinitionKeptDetached => 0,
            ResultOfDeassignment::DefinitionRemoved => 1,
        }
    }
}

impl From<ResultOfDeassignment> for u64 {
    fn from(e: ResultOfDeassignment) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for ResultOfDeassignment {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ResultOfDeassignment::DefinitionKeptDetached => write!(f, "DefinitionKeptDetached"),
            ResultOfDeassignment::DefinitionRemoved => write!(f, "DefinitionRemoved"),
        }
    }
}

/// Group identity attachment mode, TS 100 392-12-22 V1.5.1 Table 51.
///
/// Carries the same attachment-lifetime semantics as the MM group identity
/// attachment lifetime (EN 300 392-2 V2.4.1 cl.16.10.16). `AttachedPermanently`
/// is the mode used for the SwMI-initiated regroup push: the group stays
/// attached without a re-attach at the next ITSI attach or location update.
/// Codes 6 and 7 are reserved.
///
/// Bits: 3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GroupIdentityAttachmentMode {
    /// Attached permanently (no re-attach required).
    AttachedPermanently = 0,
    /// Attachment requested at next ITSI attach.
    AttachRequestedNextItsiAttach = 1,
    /// Attachment not allowed at next ITSI attach.
    AttachNotAllowedNextItsiAttach = 2,
    /// Attachment requested at next location update.
    AttachRequestedNextLocationUpdate = 3,
    /// Not attached; the MS may request attachment.
    NotAttachedMayRequest = 4,
    /// Not attached; the MS may not request attachment.
    NotAttachedMayNotRequest = 5,
}

impl std::convert::TryFrom<u64> for GroupIdentityAttachmentMode {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(GroupIdentityAttachmentMode::AttachedPermanently),
            1 => Ok(GroupIdentityAttachmentMode::AttachRequestedNextItsiAttach),
            2 => Ok(GroupIdentityAttachmentMode::AttachNotAllowedNextItsiAttach),
            3 => Ok(GroupIdentityAttachmentMode::AttachRequestedNextLocationUpdate),
            4 => Ok(GroupIdentityAttachmentMode::NotAttachedMayRequest),
            5 => Ok(GroupIdentityAttachmentMode::NotAttachedMayNotRequest),
            _ => Err(()),
        }
    }
}

impl GroupIdentityAttachmentMode {
    pub fn into_raw(self) -> u64 {
        match self {
            GroupIdentityAttachmentMode::AttachedPermanently => 0,
            GroupIdentityAttachmentMode::AttachRequestedNextItsiAttach => 1,
            GroupIdentityAttachmentMode::AttachNotAllowedNextItsiAttach => 2,
            GroupIdentityAttachmentMode::AttachRequestedNextLocationUpdate => 3,
            GroupIdentityAttachmentMode::NotAttachedMayRequest => 4,
            GroupIdentityAttachmentMode::NotAttachedMayNotRequest => 5,
        }
    }
}

impl From<GroupIdentityAttachmentMode> for u64 {
    fn from(e: GroupIdentityAttachmentMode) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for GroupIdentityAttachmentMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GroupIdentityAttachmentMode::AttachedPermanently => write!(f, "AttachedPermanently"),
            GroupIdentityAttachmentMode::AttachRequestedNextItsiAttach => write!(f, "AttachRequestedNextItsiAttach"),
            GroupIdentityAttachmentMode::AttachNotAllowedNextItsiAttach => write!(f, "AttachNotAllowedNextItsiAttach"),
            GroupIdentityAttachmentMode::AttachRequestedNextLocationUpdate => write!(f, "AttachRequestedNextLocationUpdate"),
            GroupIdentityAttachmentMode::NotAttachedMayRequest => write!(f, "NotAttachedMayRequest"),
            GroupIdentityAttachmentMode::NotAttachedMayNotRequest => write!(f, "NotAttachedMayNotRequest"),
        }
    }
}
