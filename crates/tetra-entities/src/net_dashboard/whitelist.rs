//! Dashboard-editable ISSI whitelist.
//!
//! The whitelist lives in two places at runtime:
//!   1. `StackState.issi_whitelist_override` — the live value the MM layer consults on
//!      every registration. Editing it here takes effect immediately, no restart.
//!   2. The `[security] issi_whitelist` line in the TOML config — so the change survives
//!      a restart. We rewrite this line surgically rather than re-serialising the whole
//!      file, to preserve comments and formatting the operator put in by hand.
//!
//! An empty list means "open network" (any ISSI may register), matching the semantics of
//! an empty/absent whitelist in the config.

/// Parse a whitelist POST body. Accepts either a bare JSON array `[1,2,3]` or an object
/// `{"issi_whitelist":[1,2,3]}`. Returns the parsed ISSIs (deduplicated, sorted) or an
/// error string suitable for an HTTP 400 body.
pub fn parse_whitelist_body(body: &str) -> Result<Vec<u32>, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // Try object form first: {"issi_whitelist":[...]}
    let arr_value = if trimmed.starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON object: {e}"))?;
        v.get("issi_whitelist")
            .cloned()
            .ok_or_else(|| "missing 'issi_whitelist' field".to_string())?
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON array: {e}"))?
    };

    let arr = arr_value
        .as_array()
        .ok_or_else(|| "'issi_whitelist' must be an array".to_string())?;

    let mut out: Vec<u32> = Vec::with_capacity(arr.len());
    for item in arr {
        // Accept numbers and numeric strings ("2260571").
        let n = if let Some(u) = item.as_u64() {
            u
        } else if let Some(s) = item.as_str() {
            s.trim().parse::<u64>().map_err(|_| format!("'{s}' is not a valid ISSI"))?
        } else {
            return Err(format!("invalid ISSI entry: {item}"));
        };
        if n == 0 || n > 0xFF_FFFF {
            return Err(format!("ISSI {n} out of range (1..=16777215)"));
        }
        out.push(n as u32);
    }

    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Serialise a whitelist as a TOML array literal, e.g. `[1001, 1002, 1003]`.
///
/// The entries are sorted and deduplicated so the written config is deterministic regardless
/// of the caller's ordering, matching the "deduplicated, sorted" contract of
/// [`parse_whitelist_body`].
fn format_array(list: &[u32]) -> String {
    let mut sorted: Vec<u32> = list.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let items: Vec<String> = sorted.iter().map(|n| n.to_string()).collect();
    format!("[{}]", items.join(", "))
}

/// Rewrite (or insert) the `[security] issi_whitelist = [...]` line in the TOML file at
/// `config_path`, preserving everything else. Returns Ok(()) on success.
///
/// Strategy:
///   - If a `[security]` section exists, replace its `issi_whitelist = ...` line (or add
///     one right after the header if absent).
///   - If no `[security]` section exists, append one at the end of the file.
pub fn write_whitelist_to_toml(config_path: &str, list: &[u32]) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let array_lit = format_array(list);
    let new_line = format!("issi_whitelist = {array_lit}");

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);

    let mut in_security = false;
    let mut wrote_line = false;
    let mut security_seen = false;

    for &line in &lines {
        let trimmed = line.trim_start();

        // Detect section headers.
        if trimmed.starts_with('[') && trimmed.contains(']') {
            // Leaving a previous [security] section without having written the line yet?
            if in_security && !wrote_line {
                out.push(new_line.clone());
                wrote_line = true;
            }
            in_security = trimmed.starts_with("[security]");
            if in_security {
                security_seen = true;
            }
            out.push(line.to_string());
            continue;
        }

        // Within [security], replace an existing issi_whitelist line (skip comments).
        if in_security && !wrote_line {
            let is_whitelist_line = trimmed.trim_start_matches('#').trim_start().starts_with("issi_whitelist");
            // Only replace an *active* (uncommented) assignment. A commented example is
            // left in place and we add the active line just after it.
            if is_whitelist_line && !trimmed.starts_with('#') {
                out.push(new_line.clone());
                wrote_line = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    // File ended while still inside [security] without writing the line.
    if in_security && !wrote_line {
        out.push(new_line.clone());
        wrote_line = true;
    }

    // No [security] section at all — append one.
    if !security_seen {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[security]".to_string());
        out.push(new_line.clone());
        wrote_line = true;
    }

    let _ = wrote_line; // invariant: always true by here

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }

    // Back up then write.
    let backup = format!("{config_path}.whitelist.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_array_form() {
        assert_eq!(parse_whitelist_body("[3, 1, 2]").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn parse_object_form() {
        assert_eq!(
            parse_whitelist_body("{\"issi_whitelist\":[2260571, 2260570]}").unwrap(),
            vec![2260570, 2260571]
        );
    }

    #[test]
    fn parse_string_entries() {
        assert_eq!(parse_whitelist_body("[\"1001\", \"1002\"]").unwrap(), vec![1001, 1002]);
    }

    #[test]
    fn parse_empty_is_open() {
        assert_eq!(parse_whitelist_body("[]").unwrap(), Vec::<u32>::new());
        assert_eq!(parse_whitelist_body("").unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn parse_dedup() {
        assert_eq!(parse_whitelist_body("[5, 5, 1, 5]").unwrap(), vec![1, 5]);
    }

    #[test]
    fn parse_rejects_out_of_range() {
        assert!(parse_whitelist_body("[0]").is_err());
        assert!(parse_whitelist_body("[16777216]").is_err());
    }

    #[test]
    fn replace_existing_line() {
        let cfg = "[cell]\nfoo = 1\n\n[security]\nissi_whitelist = [1, 2]\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_wl_test_replace.toml");
        std::fs::write(&path, cfg).unwrap();
        write_whitelist_to_toml(path.to_str().unwrap(), &[9, 8]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("issi_whitelist = [8, 9]"));
        assert!(out.contains("[cell]"));
        assert!(!out.contains("[1, 2]"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn append_section_when_missing() {
        let cfg = "[cell]\nfoo = 1\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_wl_test_append.toml");
        std::fs::write(&path, cfg).unwrap();
        write_whitelist_to_toml(path.to_str().unwrap(), &[7]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[security]"));
        assert!(out.contains("issi_whitelist = [7]"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preserves_commented_example() {
        let cfg = "[security]\n# issi_whitelist = [2260571, 2260572]\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_wl_test_comment.toml");
        std::fs::write(&path, cfg).unwrap();
        write_whitelist_to_toml(path.to_str().unwrap(), &[1]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        // The comment stays, and an active line is added.
        assert!(out.contains("# issi_whitelist = [2260571, 2260572]"));
        assert!(out.contains("issi_whitelist = [1]"));
        let _ = std::fs::remove_file(&path);
    }
}
