//! Re-export PHY types from tetra-core for backward compatibility
//!
//! These types are defined in tetra-core because they're used across multiple
//! layers (PHY, LMAC, UMAC) and in SAP primitives.

pub use tetra_core::{BurstType, PhyBlockNum, PhyBlockType, TrainingSequence};
