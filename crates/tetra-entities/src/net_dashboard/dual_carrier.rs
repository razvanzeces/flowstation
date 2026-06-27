//! Dashboard "Dual-Carrier ON/OFF" support.
//!
//! Toggling dual carrier is a config-file operation applied via a controlled restart: the secondary
//! carrier is fixed at startup (PHY/SDR tuning, UMAC schedulers and the timeslot allocator all read
//! it once at construction), so it cannot be reconfigured live. The toggle therefore edits
//! `[cell_info]` in the TOML and the service restarts to pick up the new carrier set.
//!
//! Representation: `secondary_carrier = N` is the *configured* carrier number (kept across OFF so it
//! is remembered), and `dual_carrier_enabled = true|false` is the operational switch. The config
//! loader (`cell_dto_to_cfg`) collapses these into the effective `CfgCellInfo::secondary_carrier`
//! (`None` when disabled), so the rest of the stack is unchanged.

/// Current dual-carrier configuration as read straight from the TOML file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DualCarrierState {
    /// The `dual_carrier_enabled` switch (absent = true for backward compatibility).
    pub enabled: bool,
    /// The configured `secondary_carrier` number, if any (preserved even while disabled).
    pub secondary_carrier: Option<u16>,
}

impl DualCarrierState {
    /// Dual carrier is operationally active only when switched on AND a carrier is configured.
    pub fn active(&self) -> bool {
        self.enabled && self.secondary_carrier.is_some()
    }
}

/// Read the dual-carrier switch + configured secondary carrier from the TOML file.
/// Tolerant of a missing/garbled file: defaults to enabled=true, no secondary carrier. Uses a
/// line scan of the active `[cell_info]` keys (no extra TOML dependency, mirrors `compute_toml`).
pub fn read_dual_carrier(config_path: &str) -> DualCarrierState {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut in_cell = false;
    let mut enabled = true;
    let mut secondary_carrier = None;

    for line in txt.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            in_cell = trimmed.starts_with("[cell_info]");
            continue;
        }
        if !in_cell || trimmed.starts_with('#') {
            continue;
        }
        if let Some(v) = active_value(trimmed, "secondary_carrier") {
            secondary_carrier = value_token(v).parse::<u16>().ok();
        } else if let Some(v) = active_value(trimmed, "dual_carrier_enabled") {
            enabled = value_token(v) == "true";
        }
    }
    DualCarrierState { enabled, secondary_carrier }
}

/// For an active (uncommented) `key = <value>` line, return the trimmed value part; else None.
fn active_value<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    if !trimmed.starts_with(key) {
        return None;
    }
    trimmed[key.len()..].trim_start().strip_prefix('=').map(str::trim)
}

/// Strip a trailing `# inline comment` and surrounding whitespace from a TOML scalar value.
fn value_token(v: &str) -> &str {
    v.split('#').next().unwrap_or(v).trim()
}

/// Produce a new TOML body with `dual_carrier_enabled` (and, when `secondary_carrier` is `Some`,
/// the active `secondary_carrier` key) set inside `[cell_info]`, preserving everything else
/// including comments. When `secondary_carrier` is `None`, any existing `secondary_carrier` line is
/// left untouched (so the configured number is remembered while the switch is off).
pub fn compute_toml(original: &str, enabled: bool, secondary_carrier: Option<u16>) -> String {
    let enabled_line = format!("dual_carrier_enabled = {enabled}");
    let secondary_line = secondary_carrier.map(|c| format!("secondary_carrier = {c}"));

    let mut out: Vec<String> = Vec::new();
    let mut in_cell = false;
    let mut cell_seen = false;
    let mut wrote_enabled = false;
    // Nothing to write for secondary if the caller passed None.
    let mut wrote_secondary = secondary_line.is_none();

    // True if `trimmed` is an active (uncommented) `key = ...` assignment.
    let is_active_key = |trimmed: &str, key: &str| {
        !trimmed.starts_with('#')
            && trimmed.starts_with(key)
            && trimmed[key.len()..].trim_start().starts_with('=')
    };

    let flush_missing = |out: &mut Vec<String>, wrote_enabled: &mut bool, wrote_secondary: &mut bool| {
        if !*wrote_enabled {
            out.push(enabled_line.clone());
            *wrote_enabled = true;
        }
        if !*wrote_secondary {
            if let Some(ref s) = secondary_line {
                out.push(s.clone());
            }
            *wrote_secondary = true;
        }
    };

    for line in original.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with('[') && trimmed.contains(']') {
            // Leaving [cell_info] without having written every key: append them at section end.
            if in_cell {
                flush_missing(&mut out, &mut wrote_enabled, &mut wrote_secondary);
            }
            in_cell = trimmed.starts_with("[cell_info]");
            if in_cell {
                cell_seen = true;
            }
            out.push(line.to_string());
            continue;
        }

        if in_cell {
            if !wrote_enabled && is_active_key(trimmed, "dual_carrier_enabled") {
                out.push(enabled_line.clone());
                wrote_enabled = true;
                continue;
            }
            if !wrote_secondary && is_active_key(trimmed, "secondary_carrier") {
                if let Some(ref s) = secondary_line {
                    out.push(s.clone());
                }
                wrote_secondary = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    // File ended while still inside [cell_info].
    if in_cell {
        flush_missing(&mut out, &mut wrote_enabled, &mut wrote_secondary);
    }

    // No [cell_info] section at all — append one.
    if !cell_seen {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[cell_info]".to_string());
        out.push(enabled_line.clone());
        if let Some(ref s) = secondary_line {
            out.push(s.clone());
        }
    }

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }
    new_content
}

/// Apply the toggle to the config file (backup, then write). Pair with `compute_toml`'s rules.
pub fn write_dual_carrier(config_path: &str, enabled: bool, secondary_carrier: Option<u16>) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let new_content = compute_toml(&original, enabled, secondary_carrier);
    let backup = format!("{config_path}.dualcarrier.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[net]
mcc = 1

[cell_info]
main_carrier = 1521                 # comment kept
# secondary_carrier = 1522          # optional, commented by default
duplex_spacing = 4

[security]
issi_whitelist = []
";

    #[test]
    fn enable_inserts_both_keys_and_keeps_comments() {
        let out = compute_toml(SAMPLE, true, Some(1522));
        assert!(out.contains("dual_carrier_enabled = true"));
        assert!(out.contains("secondary_carrier = 1522"));
        // The documenting comment line is preserved.
        assert!(out.contains("# secondary_carrier = 1522          # optional, commented by default"));
        // Untouched sections survive.
        assert!(out.contains("[security]"));
        assert!(out.contains("main_carrier = 1521"));
        // The inserted keys land inside [cell_info], before the next section.
        let cell_idx = out.find("[cell_info]").unwrap();
        let sec_idx = out.find("[security]").unwrap();
        let enabled_idx = out.find("dual_carrier_enabled = true").unwrap();
        assert!(cell_idx < enabled_idx && enabled_idx < sec_idx);
    }

    #[test]
    fn disable_sets_flag_and_keeps_existing_secondary() {
        // First enable to get an active secondary_carrier line.
        let enabled = compute_toml(SAMPLE, true, Some(1522));
        // Now disable without passing a number: flag flips, the number is remembered.
        let disabled = compute_toml(&enabled, false, None);
        assert!(disabled.contains("dual_carrier_enabled = false"));
        assert!(!disabled.contains("dual_carrier_enabled = true"));
        assert!(disabled.contains("secondary_carrier = 1522"));
    }

    #[test]
    fn toggling_is_idempotent_no_duplicate_keys() {
        let once = compute_toml(SAMPLE, true, Some(1522));
        let twice = compute_toml(&once, true, Some(1530));
        assert_eq!(twice.matches("dual_carrier_enabled =").count(), 1);
        // Exactly one ACTIVE secondary_carrier line (the commented example does not count).
        assert_eq!(
            twice.lines().filter(|l| l.trim_start().starts_with("secondary_carrier =")).count(),
            1
        );
        assert!(twice.contains("secondary_carrier = 1530"));
    }

    #[test]
    fn read_back_round_trips() {
        let dir = std::env::temp_dir();
        let path = dir.join("dc_test_roundtrip.toml");
        let path_str = path.to_str().unwrap();
        std::fs::write(&path, compute_toml(SAMPLE, true, Some(1522))).unwrap();
        let st = read_dual_carrier(path_str);
        assert_eq!(st, DualCarrierState { enabled: true, secondary_carrier: Some(1522) });
        assert!(st.active());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_cell_section_is_appended() {
        let out = compute_toml("[net]\nmcc = 1\n", true, Some(1522));
        assert!(out.contains("[cell_info]"));
        assert!(out.contains("dual_carrier_enabled = true"));
        assert!(out.contains("secondary_carrier = 1522"));
    }
}
