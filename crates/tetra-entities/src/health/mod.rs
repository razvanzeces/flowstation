//! Lite stack-health for FlowStation.
//!
//! A process-global atomics [`registry`] is poked by hot paths (the message router, MM, UMAC,
//! SDS, the Brew fan-out); a background [`supervisor`] sampler rolls it into a [`HealthSnapshot`]
//! every few seconds and emits it on the telemetry channel, where the dashboard renders a tile
//! and the Telegram alerter notifies on level transitions. Optionally the sampler also acts as a
//! software watchdog that restarts the service if the core loop stalls (off by default).
//!
//! Design: observe-only by default, lock-free hot paths, and zero calls back into RF/CMCE/UMAC —
//! monitoring health can never block the TETRA core. FlowStation-original work (not derived from
//! any noncommercially-licensed source).

pub mod registry;
pub mod supervisor;
pub mod types;

pub use registry::{HealthRegistry, HealthThresholds, registry};
pub use supervisor::{HealthMonitorConfig, spawn_health_monitor};
pub use types::{DomainHealth, HealthDomain, HealthLevel, HealthSnapshot};
