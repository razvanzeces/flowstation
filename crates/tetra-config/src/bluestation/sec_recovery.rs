use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// Restart-recovery configuration.
///
/// After a BS process restart (update / crash / service restart) the in-RAM MM registry is
/// empty, yet radios are still RF-camped on the cell — so the first group PTT fails ("no
/// listeners") until each radio happens to re-register on its own (often only on power-cycle).
///
/// When `enabled`, the BS persists a small JSON cache of known terminals (ISSI + their
/// persistent groups + energy-saving mode) and, on startup, proactively sends
/// D-LOCATION-UPDATE-COMMAND (ETSI EN 300 392-2 §16.4.4) to each cached terminal, TDMA-paced,
/// forcing them to re-register with a group identity report. The existing coverage-return
/// re-affiliation path then restores CMCE/Brew group state — PTT works again within seconds of
/// boot, without human intervention.
///
/// Default OFF: proactively keying COMMANDs at a batch of ISSIs right after boot is RF-affecting
/// (MCCH load), so it must be a deliberate operator choice, consistent with the other opt-in
/// capabilities in this stack.
///
/// A second, lighter mechanism — `reactive_enabled` (default ON) — needs no cache at all: it
/// watches the uplink and, the moment an *unknown* but network-permitted ISSI transmits, keys it
/// a single rate-limited D-LOCATION-UPDATE-COMMAND. Because the COMMAND goes out only in response
/// to a radio that is demonstrably RF-present and active (never a boot-time batch), it is RF-cheap
/// and safe to leave on, and it heals the post-restart "ghost radio" case the proactive cache can
/// miss: a radio still camped and believing it is registered, whose first PTT would otherwise be
/// silently rejected ("no listeners") until its own periodic T351 — or a manual DMO/TMO toggle.
#[derive(Debug, Clone)]
pub struct CfgRecovery {
    /// Master on/off for the *proactive* boot-time cache replay (does not gate `reactive_enabled`).
    pub enabled: bool,
    /// Optional scope filter. Empty = recover every ISSI in the persisted cache (mirrors the
    /// empty-whitelist = "all" semantics of [`super::sec_security::CfgSecurity`]). Non-empty =
    /// only replay COMMANDs to these ISSIs.
    pub issi_allowlist: Vec<u32>,
    /// Optional explicit path to the recovery cache JSON. `None` = the binary derives
    /// `<config-dir>/recovery_cache.json` (the radioid_cache.json convention).
    pub cache_path: Option<String>,
    /// Per-ISSI D-LOCATION-UPDATE-COMMAND re-send attempts at startup before giving up on a
    /// terminal that never answers (e.g. powered off mid-outage). Clamped 1..=500.
    pub max_replay_attempts: u32,
    /// Number of COMMANDs emitted per TDMA frame during the startup sweep, to bound MCCH load.
    /// Clamped 1..=18.
    pub replay_per_frame: u32,
    /// Debounce window (seconds) for coalescing a burst of registry changes into one atomic
    /// cache write, to spare SD-card wear. Clamped 1..=300.
    pub debounce_secs: u64,
    /// Hard cap on cached ISSIs (bounds disk size + boot replay time). Clamped 1..=65535.
    pub max_cached_issis: u32,
    /// On/off for *reactive* recovery: command an unknown-but-permitted ISSI to re-register the
    /// moment it is seen transmitting on the uplink. Independent of `enabled` (needs no cache) and
    /// default ON — it is RF-cheap (one COMMAND per ghost, rate-limited) and fixes the common
    /// post-restart "I pressed PTT and got nothing" case without operator intervention.
    pub reactive_enabled: bool,
    /// Minimum gap (seconds) between reactive D-LOCATION-UPDATE-COMMANDs to the *same* ISSI, so a
    /// burst of uplink activity (a single PTT yields several RSSI samples) keys it only once while
    /// it re-registers. Clamped 2..=120.
    pub reactive_cooldown_secs: u64,
}

impl Default for CfgRecovery {
    fn default() -> Self {
        CfgRecovery {
            enabled: false,
            issi_allowlist: Vec::new(),
            cache_path: None,
            max_replay_attempts: 150,
            replay_per_frame: 1,
            debounce_secs: 5,
            max_cached_issis: 1024,
            reactive_enabled: true,
            reactive_cooldown_secs: 10,
        }
    }
}

// Deliberately NOT `derive(Default)`: a derived default would zero every field (e.g.
// reactive_enabled=false, max_replay_attempts=0), diverging from the serde field defaults used
// when a `[recovery]` table is present. parsing.rs resolves an *absent* section via
// `unwrap_or_default()`, so the manual impl below makes "no section" behave identically to an
// empty `[recovery]` table — in particular keeping reactive recovery ON by default.
#[derive(Debug, Clone, Deserialize)]
pub struct CfgRecoveryDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub issi_allowlist: Vec<u32>,
    #[serde(default)]
    pub cache_path: Option<String>,
    #[serde(default = "default_max_replay_attempts")]
    pub max_replay_attempts: u32,
    #[serde(default = "default_replay_per_frame")]
    pub replay_per_frame: u32,
    #[serde(default = "default_debounce_secs")]
    pub debounce_secs: u64,
    #[serde(default = "default_max_cached_issis")]
    pub max_cached_issis: u32,
    #[serde(default = "default_reactive_enabled")]
    pub reactive_enabled: bool,
    #[serde(default = "default_reactive_cooldown_secs")]
    pub reactive_cooldown_secs: u64,

    /// Captures any unrecognised key so parsing.rs can reject typos (e.g. `enable`,
    /// `max_replay_attempt`) rather than silently leaving the feature dormant.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgRecoveryDto {
    fn default() -> Self {
        CfgRecoveryDto {
            enabled: false,
            issi_allowlist: Vec::new(),
            cache_path: None,
            max_replay_attempts: default_max_replay_attempts(),
            replay_per_frame: default_replay_per_frame(),
            debounce_secs: default_debounce_secs(),
            max_cached_issis: default_max_cached_issis(),
            reactive_enabled: default_reactive_enabled(),
            reactive_cooldown_secs: default_reactive_cooldown_secs(),
            extra: HashMap::new(),
        }
    }
}

fn default_max_replay_attempts() -> u32 {
    150
}
fn default_replay_per_frame() -> u32 {
    1
}
fn default_debounce_secs() -> u64 {
    5
}
fn default_max_cached_issis() -> u32 {
    1024
}
fn default_reactive_enabled() -> bool {
    true
}
fn default_reactive_cooldown_secs() -> u64 {
    10
}

pub fn apply_recovery_patch(dto: CfgRecoveryDto) -> CfgRecovery {
    CfgRecovery {
        enabled: dto.enabled,
        issi_allowlist: dto.issi_allowlist,
        // An empty/whitespace string means "use the default path" (matching the documented
        // `cache_path = ""` example), not a literal empty path that would disable persistence.
        cache_path: dto.cache_path.filter(|s| !s.trim().is_empty()),
        // Clamp to sane ranges so a bad TOML value can't wedge boot (house style — same as
        // hangtime_secs / periodic_registration_secs in sec_cell.rs).
        max_replay_attempts: dto.max_replay_attempts.clamp(1, 500),
        replay_per_frame: dto.replay_per_frame.clamp(1, 18),
        debounce_secs: dto.debounce_secs.clamp(1, 300),
        max_cached_issis: dto.max_cached_issis.clamp(1, 65535),
        reactive_enabled: dto.reactive_enabled,
        reactive_cooldown_secs: dto.reactive_cooldown_secs.clamp(2, 120),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_defaults_apply_when_only_enabled_set() {
        // A minimal `[recovery]` table with just `enabled = true` must pick up the serde
        // field defaults (these are applied during deserialization, NOT via derive(Default)).
        let dto: CfgRecoveryDto = toml::from_str("enabled = true").unwrap();
        let c = apply_recovery_patch(dto);
        assert!(c.enabled);
        assert!(c.issi_allowlist.is_empty());
        assert_eq!(c.max_replay_attempts, 150);
        assert_eq!(c.replay_per_frame, 1);
        assert_eq!(c.debounce_secs, 5);
        assert_eq!(c.max_cached_issis, 1024);
        // Reactive recovery is independent of `enabled` and defaults ON.
        assert!(c.reactive_enabled);
        assert_eq!(c.reactive_cooldown_secs, 10);
    }

    #[test]
    fn reactive_defaults_on_when_section_absent() {
        // parsing.rs resolves a missing `[recovery]` table via `CfgRecoveryDto::default()`, so the
        // manual Default must keep reactive recovery ON (and the proactive cache OFF), matching an
        // empty `[recovery]` block rather than a zeroed derive-default.
        let c = apply_recovery_patch(CfgRecoveryDto::default());
        assert!(!c.enabled, "proactive cache replay stays opt-in");
        assert!(c.reactive_enabled, "reactive recovery is on by default");
        assert_eq!(c.reactive_cooldown_secs, 10);
        assert_eq!(c.max_replay_attempts, 150, "absent section mirrors serde defaults, not 0→1 clamp");
    }

    #[test]
    fn reactive_can_be_disabled_and_cooldown_clamps() {
        let dto: CfgRecoveryDto = toml::from_str("reactive_enabled = false\nreactive_cooldown_secs = 1").unwrap();
        let c = apply_recovery_patch(dto);
        assert!(!c.reactive_enabled);
        assert_eq!(c.reactive_cooldown_secs, 2, "clamped up to the 2s floor");
    }

    #[test]
    fn clamps_out_of_range() {
        let dto = CfgRecoveryDto {
            enabled: true,
            max_replay_attempts: 0,
            replay_per_frame: 0,
            debounce_secs: 0,
            max_cached_issis: 0,
            ..Default::default()
        };
        let c = apply_recovery_patch(dto);
        assert_eq!(c.max_replay_attempts, 1);
        assert_eq!(c.replay_per_frame, 1);
        assert_eq!(c.debounce_secs, 1);
        assert_eq!(c.max_cached_issis, 1);

        let dto = CfgRecoveryDto {
            replay_per_frame: 999,
            max_cached_issis: 9_999_999,
            ..Default::default()
        };
        let c = apply_recovery_patch(dto);
        assert_eq!(c.replay_per_frame, 18);
        assert_eq!(c.max_cached_issis, 65535);
    }

    #[test]
    fn empty_cache_path_becomes_none() {
        // The documented `cache_path = ""` must mean "use the default", not a literal empty path.
        let dto = CfgRecoveryDto {
            cache_path: Some("   ".to_string()),
            ..Default::default()
        };
        assert_eq!(apply_recovery_patch(dto).cache_path, None);
        let dto = CfgRecoveryDto {
            cache_path: Some("/etc/fs/cache.json".to_string()),
            ..Default::default()
        };
        assert_eq!(apply_recovery_patch(dto).cache_path.as_deref(), Some("/etc/fs/cache.json"));
    }
}
