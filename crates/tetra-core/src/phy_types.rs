//! PHY-layer types that are used across multiple layers
//!
//! These types originate from the PHY layer but are referenced by LMAC, UMAC,
//! and SAP primitives, so they live in tetra-core to avoid circular dependencies.

/// Identifies which block(s) within a timeslot
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PhyBlockNum {
    /// Both half-slots combined (full slot)
    Both,
    /// First half-slot only
    Block1,
    /// Second half-slot only
    Block2,
    /// Block number not determined
    Undefined,
}

/// Physical block types
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PhyBlockType {
    BBK,
    /// TODO FIXME Merge SB1 and SB2 into SDB
    SB1,
    SB2,
    NDB,
    NUB,
    SSN1,
    SSN2,
}

/// Burst types (Clause 9.4.4.1)
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BurstType {
    /// Control Uplink Burst
    CUB,
    /// Normal Uplink Burst
    NUB,
    /// Normal Downlink Burst (continuous and discontinuous)
    NDB,
    /// Synchronization Downlink Burst (continuous and discontinuous)
    SDB,
}

/// Training sequences
#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub enum TrainingSequence {
    /// 22 n bits
    NormalTrainSeq1 = 1,
    /// 22 p bits
    NormalTrainSeq2 = 2,
    /// 22 q bits
    NormalTrainSeq3 = 3,
    /// 30 x bits
    ExtendedTrainSeq = 4,
    /// 38 y bits
    SyncTrainSeq = 5,
    /// Not found
    #[default]
    NotFound = 0,
}
