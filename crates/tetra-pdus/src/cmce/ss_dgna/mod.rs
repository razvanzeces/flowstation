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
