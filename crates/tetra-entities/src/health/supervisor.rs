//! The health sampler thread.
//!
//! Wakes every `snapshot_interval`, rolls the registry into a [`HealthSnapshot`], and pushes it
//! down the telemetry channel (→ dashboard + Telegram). Optionally — only when
//! `restart_on_core_stall` is enabled — it also acts as a software watchdog: if the core loop
//! stops ticking for long enough it requests a service restart (debounced + rate-limited). It
//! reads atomics only and never touches RF/CMCE/UMAC, so it cannot stall the stack.
//! FlowStation-original work.

use std::thread;
use std::time::{Duration, Instant};

use crate::net_telemetry::TelemetryEvent;
use crate::net_telemetry::channel::TelemetrySink;
use crate::service_control::{self, ServiceAction};

use super::registry::{HealthThresholds, registry};

#[derive(Debug, Clone, Copy)]
pub struct HealthMonitorConfig {
    pub snapshot_interval: Duration,
    pub thresholds: HealthThresholds,
    /// Software watchdog: restart the service if the core loop stalls. Default off.
    pub restart_on_core_stall: bool,
    /// How long the core must stay stalled before a restart is requested.
    pub restart_after_critical: Duration,
    /// Minimum spacing between restart requests (anti-reboot-loop).
    pub restart_cooldown: Duration,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            snapshot_interval: Duration::from_secs(5),
            thresholds: HealthThresholds::default(),
            restart_on_core_stall: false,
            restart_after_critical: Duration::from_secs(30),
            restart_cooldown: Duration::from_secs(600),
        }
    }
}

/// Spawn the background health sampler. `sink` is a clone of the telemetry sink.
pub fn spawn_health_monitor(sink: TelemetrySink, cfg: HealthMonitorConfig) {
    let interval = cfg.snapshot_interval.max(Duration::from_secs(1));
    let stall_critical_ms = cfg.thresholds.core_stall_critical_ms.max(1_000);
    let restart_after = cfg.restart_after_critical.max(Duration::from_secs(1));
    let cooldown = cfg.restart_cooldown.max(Duration::from_secs(1));

    thread::Builder::new()
        .name("health-monitor".into())
        .spawn(move || {
            tracing::info!(
                "Health monitor started (interval {}s, watchdog restart {})",
                interval.as_secs(),
                if cfg.restart_on_core_stall { "ON" } else { "off" }
            );
            let mut stall_since: Option<Instant> = None;
            let mut last_restart: Option<Instant> = None;
            loop {
                thread::sleep(interval);

                let snapshot = registry().snapshot(&cfg.thresholds);
                sink.send(TelemetryEvent::HealthSnapshot(snapshot));

                if !cfg.restart_on_core_stall {
                    continue;
                }

                // Software watchdog. Only the core-loop liveness drives a restart — a Degraded
                // backhaul or congestion never reboots the station.
                let age_ms = registry().tick_age_ms();
                if age_ms < stall_critical_ms {
                    stall_since = None;
                    continue;
                }
                let now = Instant::now();
                let since = *stall_since.get_or_insert(now);
                if now.duration_since(since) < restart_after {
                    continue; // stalled, but not long enough yet
                }
                if last_restart.is_some_and(|t| now.duration_since(t) < cooldown) {
                    continue; // still in cooldown from a previous request
                }

                let reason = format!("core loop stalled {}s", age_ms / 1000);
                tracing::error!("HEALTH: {} — requesting service restart (watchdog)", reason);
                registry().record_action(format!("restart_service ({})", reason));
                service_control::schedule_service_action(ServiceAction::Restart, Duration::ZERO);
                last_restart = Some(now);
            }
        })
        .expect("failed to spawn health-monitor thread");
}
