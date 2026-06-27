//! GitHub release update-check for the dashboard.
//!
//! Compares the locally built version (`tetra_core::STACK_VERSION`, e.g. "v0.2.5-gabc123")
//! against the latest GitHub release tag and reports whether a newer version exists. This
//! is purely informational — the actual update is performed by the existing git-based OTA
//! path (`run_update`). We only surface an "update available" badge so the operator knows
//! to click it.
//!
//! The check is best-effort: any network/parse failure yields `UpdateCheck::unknown()`
//! rather than an error, so a flaky connection never breaks the dashboard.

use std::time::Duration;

const GITHUB_API_LATEST: &str = "https://api.github.com/repos/razvanzeces/flowstation/releases/latest";
// GitHub requires a User-Agent on all API requests.
const USER_AGENT: &str = "FlowStation-Dashboard";

/// A parsed semantic version (major.minor.patch). Pre-release/build metadata is ignored
/// for comparison purposes — we only care about the release triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer {
    major: u32,
    minor: u32,
    patch: u32,
}

impl SemVer {
    /// Parse a version from a string like "v0.2.5", "0.2.5", or "v0.2.5-gabc123".
    /// Leading 'v'/'V' is optional; anything after the patch (a '-' or '+' suffix) is
    /// ignored. Returns None if the major.minor.patch core can't be parsed.
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
        // Cut at the first '-' or '+' (pre-release / build / git suffix).
        let core = s.split(['-', '+']).next().unwrap_or(s);
        let mut it = core.split('.');
        let major = it.next()?.trim().parse().ok()?;
        let minor = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        let patch = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        Some(SemVer { major, minor, patch })
    }
}

/// Result of an update check, serialised to JSON for the dashboard.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    /// Locally built version string (as-is, e.g. "v0.2.5-gabc123").
    pub current: String,
    /// Latest release tag from GitHub, if the check succeeded (e.g. "v0.2.6").
    pub latest: Option<String>,
    /// True when `latest` parses to a strictly higher SemVer than `current`.
    pub update_available: bool,
    /// URL of the latest release page, if available (for a "view release" link).
    pub release_url: Option<String>,
    /// True when the check itself failed (network/parse). The badge should stay hidden.
    pub check_failed: bool,
}

impl UpdateCheck {
    fn unknown(current: &str) -> Self {
        UpdateCheck {
            current: current.to_string(),
            latest: None,
            update_available: false,
            release_url: None,
            check_failed: true,
        }
    }

    /// Render as a JSON object for `GET /api/update/check`.
    pub fn to_json(&self) -> String {
        let latest = self
            .latest
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        let url = self
            .release_url
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        format!(
            "{{\"current\":\"{}\",\"latest\":{},\"update_available\":{},\"release_url\":{},\"check_failed\":{}}}",
            json_escape(&self.current),
            latest,
            self.update_available,
            url,
            self.check_failed
        )
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Query GitHub for the latest release and compare against `current_version`
/// (typically `tetra_core::STACK_VERSION`). Blocking; call from a worker thread.
pub fn check_for_update(current_version: &str) -> UpdateCheck {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return UpdateCheck::unknown(current_version),
    };

    let resp = match client
        .get(GITHUB_API_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(_) => return UpdateCheck::unknown(current_version),
    };

    let json: serde_json::Value = match resp.json() {
        Ok(j) => j,
        Err(_) => return UpdateCheck::unknown(current_version),
    };

    let tag = json.get("tag_name").and_then(|v| v.as_str());
    let html_url = json.get("html_url").and_then(|v| v.as_str()).map(|s| s.to_string());

    let Some(tag) = tag else {
        return UpdateCheck::unknown(current_version);
    };

    let update_available = match (SemVer::parse(current_version), SemVer::parse(tag)) {
        (Some(cur), Some(latest)) => latest > cur,
        // If we can't parse one side, don't claim an update is available.
        _ => false,
    };

    UpdateCheck {
        current: current_version.to_string(),
        latest: Some(tag.to_string()),
        update_available,
        release_url: html_url,
        check_failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain() {
        assert_eq!(
            SemVer::parse("0.2.5"),
            Some(SemVer {
                major: 0,
                minor: 2,
                patch: 5
            })
        );
    }

    #[test]
    fn parse_v_prefix() {
        assert_eq!(
            SemVer::parse("v1.4.0"),
            Some(SemVer {
                major: 1,
                minor: 4,
                patch: 0
            })
        );
    }

    #[test]
    fn parse_git_suffix() {
        assert_eq!(
            SemVer::parse("v0.2.5-gabc123"),
            Some(SemVer {
                major: 0,
                minor: 2,
                patch: 5
            })
        );
    }

    #[test]
    fn parse_partial() {
        assert_eq!(
            SemVer::parse("v2.1"),
            Some(SemVer {
                major: 2,
                minor: 1,
                patch: 0
            })
        );
        assert_eq!(
            SemVer::parse("3"),
            Some(SemVer {
                major: 3,
                minor: 0,
                patch: 0
            })
        );
    }

    #[test]
    fn compare_versions() {
        let a = SemVer::parse("v0.2.5").unwrap();
        let b = SemVer::parse("v0.2.6").unwrap();
        let c = SemVer::parse("v0.3.0").unwrap();
        let d = SemVer::parse("v1.0.0").unwrap();
        assert!(b > a);
        assert!(c > b);
        assert!(d > c);
        assert!(a == SemVer::parse("0.2.5-gdeadbeef").unwrap());
    }

    #[test]
    fn newer_release_detected() {
        // Simulate the comparison check_for_update does.
        let cur = SemVer::parse("v0.2.5-gabc").unwrap();
        let latest = SemVer::parse("v0.2.6").unwrap();
        assert!(latest > cur);
    }

    #[test]
    fn same_version_no_update() {
        let cur = SemVer::parse("v0.2.5-gabc").unwrap();
        let latest = SemVer::parse("v0.2.5").unwrap();
        assert!(!(latest > cur));
    }

    #[test]
    fn unparseable_tag_no_update() {
        assert_eq!(SemVer::parse("nightly"), None);
    }

    #[test]
    fn json_output() {
        let uc = UpdateCheck {
            current: "v0.2.5-gabc".to_string(),
            latest: Some("v0.2.6".to_string()),
            update_available: true,
            release_url: Some("https://github.com/razvanzeces/flowstation/releases/tag/v0.2.6".to_string()),
            check_failed: false,
        };
        let j = uc.to_json();
        assert!(j.contains("\"update_available\":true"));
        assert!(j.contains("\"latest\":\"v0.2.6\""));
        assert!(j.contains("\"check_failed\":false"));
    }
}
