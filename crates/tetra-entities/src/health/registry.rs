//! Process-global health registry.
//!
//! Hot paths (the core loop, MM, UMAC, SDS, the Brew fan-out) poke cheap atomics here; the
//! sampler thread reads them every few seconds and rolls them into a [`HealthSnapshot`]. The
//! registry never calls back into RF/CMCE/UMAC, so observing health can never stall the stack.
//! FlowStation-original work.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use super::types::{DomainHealth, HealthDomain, HealthLevel, HealthSnapshot};

/// Tunables for turning raw counters into levels. Built from `[health]` config by the caller.
#[derive(Debug, Clone, Copy)]
pub struct HealthThresholds {
    /// Service is Critical if no core tick for this long (Degraded at half this).
    pub core_stall_critical_ms: u64,
    /// Floor for the "attached but silent" Degraded signal. The EFFECTIVE threshold is
    /// `max(this, 1.5 * periodic_registration_secs)` so a quiet radio is never flagged inside
    /// its T351 re-registration window. Set to 0 to disable the silent-radio signal entirely.
    pub radios_silent_degraded_secs: u64,
    /// Configured periodic-registration interval (ETSI T351 equivalent), in seconds; 0 = the
    /// terminals do no periodic registration. The radios-silent threshold is derived from this
    /// so a long T351 (e.g. 24 h) cannot produce a false "silent" Degraded between registrations.
    pub periodic_registration_secs: u64,
    /// Downlink queue depth at/above which Congestion is Degraded / Critical.
    pub dl_queue_degraded: usize,
    pub dl_queue_critical: usize,
    /// Live SDS queue depth at/above which Congestion is Degraded / Critical.
    pub sds_queue_degraded: usize,
    pub sds_queue_critical: usize,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            core_stall_critical_ms: 10_000,
            radios_silent_degraded_secs: 900, // floor; effective threshold respects T351 (below)
            periodic_registration_secs: 3600, // matches cell.periodic_registration_secs default
            dl_queue_degraded: 64,
            dl_queue_critical: 192,
            sds_queue_degraded: 32,
            sds_queue_critical: 128,
        }
    }
}

/// Effective "attached but silent" threshold in seconds. A radio re-registers every T351
/// (`periodic_registration_secs`) and is legitimately quiet in between, so the configured floor
/// is raised to at least 1.5 × T351 — a radio is only "overdue" once it has clearly missed its
/// scheduled registration (with grace for a single transient miss). T351 == 0 (no periodic
/// registration) keeps the plain floor.
fn effective_radios_silent_secs(floor_secs: u64, periodic_registration_secs: u64) -> u64 {
    if periodic_registration_secs > 0 {
        floor_secs.max(periodic_registration_secs + periodic_registration_secs / 2)
    } else {
        floor_secs
    }
}

/// Startup grace: don't flag Service Critical for the first few seconds while the stack boots.
const SERVICE_STARTUP_GRACE_MS: u64 = 5_000;

pub struct HealthRegistry {
    start: Instant,
    /// Millis-since-`start` at the last core tick; 0 = no tick observed yet.
    last_tick_ms: AtomicU64,
    brew_configured: AtomicBool,
    brew_up: AtomicBool,
    registered_radios: AtomicUsize,
    /// Millis-since-`start` of the last uplink heard from any radio; 0 = none yet.
    last_radio_activity_ms: AtomicU64,
    dl_queue_depth: AtomicUsize,
    sds_queue_depth: AtomicUsize,
    /// Most recent remediation action, human-readable. Written rarely (only when an action fires).
    last_action: Mutex<Option<String>>,
}

static REGISTRY: OnceLock<HealthRegistry> = OnceLock::new();

/// The process-global health registry (created on first use).
pub fn registry() -> &'static HealthRegistry {
    REGISTRY.get_or_init(HealthRegistry::new)
}

impl HealthRegistry {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            last_tick_ms: AtomicU64::new(0),
            brew_configured: AtomicBool::new(false),
            brew_up: AtomicBool::new(false),
            registered_radios: AtomicUsize::new(0),
            last_radio_activity_ms: AtomicU64::new(0),
            dl_queue_depth: AtomicUsize::new(0),
            sds_queue_depth: AtomicUsize::new(0),
            last_action: Mutex::new(None),
        }
    }

    #[inline]
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }

    // ── hot-path setters (cheap, never block) ──────────────────────────────────────────────
    /// Stamp a core-loop tick (called from the message router each TDMA tick).
    pub fn note_tick(&self) {
        self.last_tick_ms.store(self.now_ms().max(1), Ordering::Relaxed);
    }
    pub fn set_brew_configured(&self, on: bool) {
        self.brew_configured.store(on, Ordering::Relaxed);
    }
    pub fn set_brew_up(&self, up: bool) {
        self.brew_up.store(up, Ordering::Relaxed);
    }
    pub fn set_registered_radios(&self, n: usize) {
        self.registered_radios.store(n, Ordering::Relaxed);
    }
    /// Note that some radio was just heard on the air (any uplink).
    pub fn note_radio_activity(&self) {
        self.last_radio_activity_ms.store(self.now_ms().max(1), Ordering::Relaxed);
    }
    pub fn set_dl_queue_depth(&self, n: usize) {
        self.dl_queue_depth.store(n, Ordering::Relaxed);
    }
    pub fn set_sds_queue_depth(&self, n: usize) {
        self.sds_queue_depth.store(n, Ordering::Relaxed);
    }
    /// Record a remediation action (shown in the snapshot / dashboard / Telegram).
    pub fn record_action(&self, what: String) {
        if let Ok(mut g) = self.last_action.lock() {
            *g = Some(what);
        }
    }

    /// Millis since the last core tick. `u64::MAX`-ish handling is avoided: if no tick has been
    /// seen yet we report the time since registry start (so a stalled-from-boot loop still ages).
    pub fn tick_age_ms(&self) -> u64 {
        let last = self.last_tick_ms.load(Ordering::Relaxed);
        if last == 0 {
            self.now_ms()
        } else {
            self.now_ms().saturating_sub(last)
        }
    }

    // ── snapshot (read by the sampler) ─────────────────────────────────────────────────────
    pub fn snapshot(&self, t: &HealthThresholds) -> HealthSnapshot {
        let mut domains = Vec::with_capacity(4);

        // Service liveness (watchdog).
        let age = self.tick_age_ms();
        let booting = self.last_tick_ms.load(Ordering::Relaxed) == 0 && self.now_ms() < SERVICE_STARTUP_GRACE_MS;
        let (svc, svc_detail) = if booting {
            (HealthLevel::Ok, "starting".to_string())
        } else if age >= t.core_stall_critical_ms {
            (HealthLevel::Critical, format!("core loop stalled {}s", age / 1000))
        } else if age >= t.core_stall_critical_ms / 2 {
            (HealthLevel::Degraded, format!("core loop slow ({}ms since last tick)", age))
        } else {
            (HealthLevel::Ok, "ticking".to_string())
        };
        domains.push(DomainHealth {
            domain: HealthDomain::Service,
            level: svc,
            detail: svc_detail,
        });

        // Backhaul (Brew). Down is Degraded, not Critical: local service keeps working.
        let (bh, bh_detail) = if !self.brew_configured.load(Ordering::Relaxed) {
            (HealthLevel::Ok, "not configured".to_string())
        } else if self.brew_up.load(Ordering::Relaxed) {
            (HealthLevel::Ok, "connected".to_string())
        } else {
            (HealthLevel::Degraded, "disconnected (local-only)".to_string())
        };
        domains.push(DomainHealth {
            domain: HealthDomain::Backhaul,
            level: bh,
            detail: bh_detail,
        });

        // Radios. Informational, with a Degraded signal for "attached but silent".
        let radios = self.registered_radios.load(Ordering::Relaxed);
        let last_act = self.last_radio_activity_ms.load(Ordering::Relaxed);
        let silent_ms = if last_act == 0 {
            self.now_ms()
        } else {
            self.now_ms().saturating_sub(last_act)
        };
        // Respect the periodic-registration window (ETSI T351): between two periodic
        // registrations a radio is legitimately quiet, so only flag Degraded once the cell has
        // been silent for clearly longer than that window (1.5 × T351, floored by the config).
        let floor = t.radios_silent_degraded_secs;
        let silent_check = floor > 0; // 0 disables the silent-radio signal
        let effective_secs = effective_radios_silent_secs(floor, t.periodic_registration_secs);
        let (rad, rad_detail) = if radios > 0 && silent_check && silent_ms >= effective_secs * 1000 {
            (
                HealthLevel::Degraded,
                format!(
                    "{} attached, silent {}s (exceeds {}s T351 window)",
                    radios,
                    silent_ms / 1000,
                    effective_secs
                ),
            )
        } else {
            (HealthLevel::Ok, format!("{} attached", radios))
        };
        domains.push(DomainHealth {
            domain: HealthDomain::Radios,
            level: rad,
            detail: rad_detail,
        });

        // Congestion (downlink + live-SDS queues).
        let dl = self.dl_queue_depth.load(Ordering::Relaxed);
        let sds = self.sds_queue_depth.load(Ordering::Relaxed);
        let level = if dl >= t.dl_queue_critical || sds >= t.sds_queue_critical {
            HealthLevel::Critical
        } else if dl >= t.dl_queue_degraded || sds >= t.sds_queue_degraded {
            HealthLevel::Degraded
        } else {
            HealthLevel::Ok
        };
        domains.push(DomainHealth {
            domain: HealthDomain::Congestion,
            level,
            detail: format!("dl={} sds={}", dl, sds),
        });

        let overall = domains.iter().fold(HealthLevel::Ok, |acc, d| acc.worst(d.level));
        HealthSnapshot {
            overall,
            domains,
            last_action: self.last_action.lock().ok().and_then(|g| g.clone()),
            uptime_secs: self.uptime_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_rolls_up_worst_level() {
        // A fresh registry on a separate instance (not the global) for deterministic assertions.
        let reg = HealthRegistry::new();
        let t = HealthThresholds::default();

        // Before any tick, within the startup grace → Service Ok.
        let snap = reg.snapshot(&t);
        assert_eq!(snap.domains[0].level, HealthLevel::Ok); // service "starting"
        assert_eq!(snap.overall, HealthLevel::Ok);

        // Brew configured but down → Backhaul Degraded → overall Degraded.
        reg.set_brew_configured(true);
        reg.note_tick(); // service Ok
        let snap = reg.snapshot(&t);
        assert_eq!(snap.overall, HealthLevel::Degraded);
        assert_eq!(snap.domains[1].level, HealthLevel::Degraded);

        // Brew up → back to Ok.
        reg.set_brew_up(true);
        assert_eq!(reg.snapshot(&t).overall, HealthLevel::Ok);

        // Congestion over the critical mark → Critical overall.
        reg.set_dl_queue_depth(t.dl_queue_critical + 1);
        let snap = reg.snapshot(&t);
        assert_eq!(snap.overall, HealthLevel::Critical);
        assert_eq!(snap.domains[3].level, HealthLevel::Critical);
    }

    #[test]
    fn radios_silent_is_degraded_only_when_attached() {
        let reg = HealthRegistry::new();
        let mut t = HealthThresholds::default();
        t.radios_silent_degraded_secs = 1; // 1s window for a fast, deterministic test
        t.periodic_registration_secs = 0; // no T351 derivation here — exercise the raw floor
        reg.note_tick();
        reg.note_radio_activity();

        // Let the activity age past the 1s window.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        // No radios attached → Ok regardless of silence.
        reg.set_registered_radios(0);
        assert_eq!(reg.snapshot(&t).domains[2].level, HealthLevel::Ok);

        // Attached but silent ≥ 1s → Degraded.
        reg.set_registered_radios(3);
        assert_eq!(reg.snapshot(&t).domains[2].level, HealthLevel::Degraded);

        // Fresh uplink activity → back to Ok.
        reg.note_radio_activity();
        assert_eq!(reg.snapshot(&t).domains[2].level, HealthLevel::Ok);

        // Silent-radio signal disabled (0) → never Degraded even when attached + silent.
        t.radios_silent_degraded_secs = 0;
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(reg.snapshot(&t).domains[2].level, HealthLevel::Ok);
    }

    #[test]
    fn radios_silent_threshold_respects_t351() {
        // The effective threshold must never fall inside the T351 re-registration window, so a
        // radio that is simply quiet between periodic registrations is not flagged. (This is the
        // FH fix for false "Radios DEGRADED" when, e.g., T351 = 24 h but a radio has been quiet
        // for only ~1700 s.)
        assert_eq!(effective_radios_silent_secs(900, 86_400), 129_600); // 1.5 × 24 h, not 900
        assert_eq!(effective_radios_silent_secs(900, 3_600), 5_400); // 1.5 × 1 h (> 900 floor)
        assert_eq!(effective_radios_silent_secs(900, 0), 900); // no periodic registration → floor
        assert_eq!(effective_radios_silent_secs(7_200, 3_600), 7_200); // explicit floor wins when larger
        // 1700 s of silence with a 24 h T351 is far below the effective window → not overdue.
        assert!(1_700 < effective_radios_silent_secs(900, 86_400));
    }
}
