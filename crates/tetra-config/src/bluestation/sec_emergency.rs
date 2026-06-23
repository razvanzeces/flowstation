use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// Emergency-state handling configuration (FlowStation, LOCAL BTS).
///
/// A radio signals emergency by sending a U-STATUS PDU with `pre_coded_status = Emergency`
/// (status value 0, ETSI EN 300 392-2 table 14.72) to a codeplug-configured ISSI; it re-sends
/// this periodically while emergency is active on the radio, and sends nothing on exit. An
/// emergency CALL (U-SETUP with `call_priority` 15) raises the same state.
///
/// By design emergency is **LOCAL-only**: it raises a persistent dashboard banner and
/// (optionally) a Telegram alert, but is NOT forwarded to Brew unless `forward_to_brew` is set.
/// Because the radio is silent on exit, a session auto-clears `clear_timeout_secs` after the last
/// emergency status (or when the ISSI sends a non-Emergency status, or an operator clears it).
#[derive(Debug, Clone)]
pub struct CfgEmergency {
    /// Also forward the emergency U-STATUS to Brew. Default false (LOCAL-only).
    pub forward_to_brew: bool,
    /// Send a Telegram alert when an emergency ENTERS (not on every re-send). Default true.
    pub telegram_alert: bool,
    /// Auto-clear an ISSI's emergency session this many seconds after its last emergency status,
    /// detected in the periodic tick (the radio sends nothing on exit). Clamped 5..=600.
    ///
    /// Set this COMFORTABLY LARGER than the radio's emergency-status re-send interval (codeplug
    /// dependent): if the timeout is shorter than the re-send period, a still-active emergency is
    /// cleared between re-sends and then re-raised on the next one, flapping the banner and
    /// re-firing the Telegram alert. The default (30s) suits radios that re-send every few seconds.
    pub clear_timeout_secs: u64,
}

impl Default for CfgEmergency {
    fn default() -> Self {
        CfgEmergency {
            forward_to_brew: false,
            telegram_alert: true,
            clear_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CfgEmergencyDto {
    #[serde(default)]
    pub forward_to_brew: bool,
    #[serde(default = "default_telegram_alert")]
    pub telegram_alert: bool,
    #[serde(default = "default_clear_timeout_secs")]
    pub clear_timeout_secs: u64,

    /// Captures any unrecognised key so parsing.rs can reject typos (e.g. `forward_brew`)
    /// rather than silently leaving the setting at its default.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_telegram_alert() -> bool {
    true
}
fn default_clear_timeout_secs() -> u64 {
    30
}

pub fn apply_emergency_patch(dto: CfgEmergencyDto) -> CfgEmergency {
    CfgEmergency {
        forward_to_brew: dto.forward_to_brew,
        telegram_alert: dto.telegram_alert,
        // Clamp to a sane range so a bad TOML value can't wedge the timeout sweep (house style,
        // same as the recovery/cell timers).
        clear_timeout_secs: dto.clear_timeout_secs.clamp(5, 600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_defaults_apply_when_only_one_set() {
        // A minimal `[emergency]` table with just `forward_to_brew = true` must pick up the serde
        // field defaults (applied during deserialization, NOT via derive(Default)).
        let dto: CfgEmergencyDto = toml::from_str("forward_to_brew = true").unwrap();
        let c = apply_emergency_patch(dto);
        assert!(c.forward_to_brew);
        assert!(c.telegram_alert); // default true
        assert_eq!(c.clear_timeout_secs, 30);
    }

    #[test]
    fn defaults_are_local_only() {
        let c = CfgEmergency::default();
        assert!(!c.forward_to_brew);
        assert!(c.telegram_alert);
        assert_eq!(c.clear_timeout_secs, 30);
    }

    #[test]
    fn clamps_clear_timeout() {
        let dto = CfgEmergencyDto {
            clear_timeout_secs: 0,
            ..Default::default()
        };
        assert_eq!(apply_emergency_patch(dto).clear_timeout_secs, 5);
        let dto = CfgEmergencyDto {
            clear_timeout_secs: 99_999,
            ..Default::default()
        };
        assert_eq!(apply_emergency_patch(dto).clear_timeout_secs, 600);
    }
}
