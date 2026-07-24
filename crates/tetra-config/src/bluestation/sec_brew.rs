use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::HashMap, time::Duration};

use serde::Deserialize;
use toml::Value;

use crate::bluestation::SecretField;

/// FH-BUG-079 runtime opt-in: has the operator enabled accepting the ISSI registration whitelist
/// pushed by the core over the Brew link (`[brew] feature_issi_whitelist_sync`)? Seeded once from
/// the config in [`apply_brew_patch`]; the Brew worker snapshots it at startup to gate the 0x30
/// Service handler. Held here rather than on [`CfgBrew`] so adding the knob does not force a new
/// field onto a struct that downstream integration tests build with exhaustive literals.
static ACCEPT_ISSI_WHITELIST_OVER_BREW: AtomicBool = AtomicBool::new(false);

/// Whether accepting the ISSI registration whitelist over Brew is enabled (default `false` —
/// opt-in). See [`ACCEPT_ISSI_WHITELIST_OVER_BREW`].
pub fn accept_issi_whitelist_over_brew() -> bool {
    ACCEPT_ISSI_WHITELIST_OVER_BREW.load(Ordering::Relaxed)
}

/// Brew protocol (TetraPack/BrandMeister) configuration
#[derive(Debug, Clone)]
pub struct CfgBrew {
    /// TetraPack server hostname or IP
    pub host: String,
    /// TetraPack server port
    pub port: u16,
    /// Use TLS (wss:// / https://)
    pub tls: bool,
    /// Optional username for HTTP Digest auth
    pub username: Option<String>,
    /// Optional password for HTTP Digest auth
    pub password: Option<SecretField>,
    /// Reconnection delay
    pub reconnect_delay: Duration,
    /// Extra initial jitter playout delay in frames (added on top of adaptive baseline)
    pub jitter_initial_latency_frames: u8,

    /// Set to true when SDS between local and Brew clients is enabled
    pub feature_sds_enabled: bool,
    /// If true, RSSI measurements are exported to the Brew server as Service (0xf4) JSON messages.
    /// Disabled by default. Enable only if the Brew server supports and expects RSSI data.
    pub feature_rssi_export: bool,
    /// If present, restrict Brew call to these remote SSIs
    pub whitelisted_ssis: Option<Vec<u32>>,
    /// Optional PBX gateway ISSIs that should be routable over Brew even if they don't match
    /// normal Tetrapack subscriber ISSI constraints.
    pub pbx_gateway_issis: Option<Vec<u32>>,
}

#[derive(Default, Deserialize)]
pub struct CfgBrewDto {
    /// TetraPack server hostname or IP
    pub host: String,
    /// TetraPack server port
    #[serde(default = "default_brew_port")]
    pub port: u16,
    /// Use TLS (wss:// / https://)
    pub tls: bool,
    /// Optional username for HTTP Digest auth
    pub username: u32,
    /// Optional password for HTTP Digest auth
    pub password: String,
    /// Reconnection delay in seconds
    #[serde(default = "default_brew_reconnect_delay")]
    pub reconnect_delay_secs: u64,
    /// Extra initial jitter playout delay in frames (added on top of adaptive baseline)
    #[serde(default)]
    pub jitter_initial_latency_frames: u8,

    /// If present, restrict Brew call to these remote SSIs
    pub whitelisted_ssis: Option<Vec<u32>>,

    /// Set to true when SDS between local and Brew clients is enabled
    #[serde(default = "default_brew_feature_sds_enabled")]
    pub feature_sds_enabled: bool,

    /// Export RSSI measurements to the Brew server as Service JSON messages. Default: false.
    #[serde(default)]
    pub feature_rssi_export: bool,

    /// FH-BUG-079: accept the ISSI registration whitelist pushed by the core over the Brew link
    /// (Service 0xf4 type 0x30). Off by default — a remote core must not silently take over local
    /// access control unless the operator opted in. `#[serde(default)]` keeps existing configs
    /// parsing unchanged. Consumed via [`accept_issi_whitelist_over_brew`], not stored on CfgBrew.
    #[serde(default)]
    pub feature_issi_whitelist_sync: bool,

    /// Optional PBX gateway ISSIs that should be routable over Brew even if they don't match
    /// normal Tetrapack subscriber ISSI constraints.
    #[serde(alias = "pbx_gateway_issi")]
    pub pbx_gateway_issis: Option<Vec<u32>>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_brew_port() -> u16 {
    443
}

fn default_brew_reconnect_delay() -> u64 {
    15
}

fn default_brew_feature_sds_enabled() -> bool {
    true
}

/// Convert a CfgBrewDto (from TOML) into a CfgBrew (used in the stack config)
pub fn apply_brew_patch(src: CfgBrewDto) -> CfgBrew {
    // FH-BUG-079: capture the whitelist-sync opt-in so the Brew worker can gate the 0x30 handler.
    // Load-once, like the other Brew feature flags; kept off CfgBrew to avoid a breaking literal.
    ACCEPT_ISSI_WHITELIST_OVER_BREW.store(src.feature_issi_whitelist_sync, Ordering::Relaxed);
    CfgBrew {
        host: src.host,
        port: src.port,
        tls: src.tls,
        username: Some(src.username.to_string()),
        password: Some(SecretField::from(src.password)),
        reconnect_delay: Duration::from_secs(src.reconnect_delay_secs),
        jitter_initial_latency_frames: src.jitter_initial_latency_frames,
        feature_sds_enabled: src.feature_sds_enabled,
        feature_rssi_export: src.feature_rssi_export,
        whitelisted_ssis: src.whitelisted_ssis,
        pbx_gateway_issis: src.pbx_gateway_issis,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `[brew]` section with the required fields but no whitelist knob.
    fn brew_toml(extra: &str) -> String {
        format!("host = \"core.example\"\nport = 443\ntls = true\nusername = 123456700\npassword = \"012345\"\n{extra}")
    }

    #[test]
    fn issi_whitelist_sync_defaults_off() {
        // FH-BUG-079: the opt-in must default OFF so an existing config never starts honouring a
        // core-pushed whitelist just because it upgraded.
        let dto: CfgBrewDto = toml::from_str(&brew_toml("")).expect("brew config without the knob must parse");
        assert!(!dto.feature_issi_whitelist_sync, "feature_issi_whitelist_sync must default to false");
    }

    #[test]
    fn issi_whitelist_sync_parses_when_enabled() {
        let dto: CfgBrewDto =
            toml::from_str(&brew_toml("feature_issi_whitelist_sync = true")).expect("explicit knob must parse");
        assert!(dto.feature_issi_whitelist_sync);
    }
}
