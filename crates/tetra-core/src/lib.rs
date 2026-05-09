//! Core utilities for TETRA BlueStation
//!
//! This crate provides fundamental types and utilities used across the TETRA stack

/// Short git commit hash, set at compile time (e.g. "g2aad62c")
pub const GIT_HASH: &str = git_version::git_version!(
    args = ["--always", "--dirty=-modified", "--match=", "--abbrev=8"],
    fallback = "unknown"
);
/// Full stack version string, e.g. "v0.0.6-g2aad62c"
pub const STACK_VERSION: &str = const_format::formatcp!("v{}-{}", env!("CARGO_PKG_VERSION"), GIT_HASH);

pub mod address;
pub mod bitbuffer;
pub mod debug;
pub mod direction;
pub mod freqs;
pub mod pdu_parse_error;
pub mod phy_types;
pub mod ranges;
pub mod sap_fields;
pub mod tdma_time;
pub mod tetra_common;
pub mod tetra_entities;
pub mod timeslot_alloc;
pub mod tx_receipt;
pub mod typed_pdu_fields;

// Re-export commonly used items
pub use address::*;
pub use bitbuffer::BitBuffer;
pub use direction::Direction;
pub use pdu_parse_error::PduParseErr;
pub use phy_types::*;
pub use sap_fields::*;
pub use tdma_time::TdmaTime;
pub use tetra_common::*;
pub use timeslot_alloc::*;
pub use tx_receipt::*;
