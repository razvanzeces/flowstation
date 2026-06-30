use std::{collections::HashMap, time::Duration};

use serde::Deserialize;
use toml::Value;

use crate::bluestation::SecretField;

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
    /// Local TETRA ISSIs allowed to register and originate traffic over this Brew server.
    /// None keeps legacy single-Brew behaviour; with two Brew servers it must be set.
    pub local_issi_allowlist: Option<Vec<u32>>,
    /// Local TETRA ISSIs that must never register or originate traffic over this Brew server.
    pub local_issi_blocklist: Vec<u32>,
    /// Subscriber message type used by this Brew server for deregistration.
    pub subscriber_type_deregister: u8,
    /// Subscriber message type used by this Brew server for first registration.
    pub subscriber_type_register: u8,
    /// Subscriber message type used by this Brew server for re-registration.
    pub subscriber_type_reregister: u8,
    /// Subscriber message type used by this Brew server for group affiliation.
    pub subscriber_type_affiliate: u8,
    /// Subscriber message type used by this Brew server for group de-affiliation.
    pub subscriber_type_deaffiliate: u8,
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

    /// Optional PBX gateway ISSIs that should be routable over Brew even if they don't match
    /// normal Tetrapack subscriber ISSI constraints.
    #[serde(alias = "pbx_gateway_issi")]
    pub pbx_gateway_issis: Option<Vec<u32>>,

    /// Local TETRA ISSIs allowed to register and originate traffic over this Brew server.
    #[serde(default, alias = "local_issi_whitelist", alias = "issi_allowlist", alias = "issi_whitelist")]
    pub local_issi_allowlist: Option<Vec<u32>>,

    /// Local TETRA ISSIs that must never register or originate traffic over this Brew server.
    #[serde(default, alias = "local_issi_blacklist", alias = "issi_blocklist", alias = "issi_blacklist")]
    pub local_issi_blocklist: Vec<u32>,

    /// Subscriber message type mapping. Defaults are the classic Brew/TetraPack values:
    /// deregister=0, register=1, reregister=2, affiliate=8, deaffiliate=9.
    #[serde(default = "default_subscriber_type_deregister")]
    pub subscriber_type_deregister: u8,
    #[serde(default = "default_subscriber_type_register")]
    pub subscriber_type_register: u8,
    #[serde(default = "default_subscriber_type_reregister")]
    pub subscriber_type_reregister: u8,
    #[serde(default = "default_subscriber_type_affiliate")]
    pub subscriber_type_affiliate: u8,
    #[serde(default = "default_subscriber_type_deaffiliate")]
    pub subscriber_type_deaffiliate: u8,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl CfgBrew {
    pub fn has_local_issi_allowlist(&self) -> bool {
        self.local_issi_allowlist.as_ref().is_some_and(|issis| !issis.is_empty())
    }

    pub fn local_issi_allowed(&self, issi: u32) -> bool {
        if self.local_issi_blocklist.contains(&issi) {
            return false;
        }

        self.local_issi_allowlist
            .as_ref()
            .map_or(true, |allowlist| allowlist.contains(&issi))
    }

    pub fn effective_local_issi_allowlist(&self) -> Option<Vec<u32>> {
        self.local_issi_allowlist.as_ref().map(|allowlist| {
            allowlist
                .iter()
                .copied()
                .filter(|issi| !self.local_issi_blocklist.contains(issi))
                .collect()
        })
    }
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

fn default_subscriber_type_deregister() -> u8 {
    0
}

fn default_subscriber_type_register() -> u8 {
    1
}

fn default_subscriber_type_reregister() -> u8 {
    2
}

fn default_subscriber_type_affiliate() -> u8 {
    8
}

fn default_subscriber_type_deaffiliate() -> u8 {
    9
}

/// Convert a CfgBrewDto (from TOML) into a CfgBrew (used in the stack config)
pub fn apply_brew_patch(src: CfgBrewDto) -> CfgBrew {
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
        local_issi_allowlist: src.local_issi_allowlist,
        local_issi_blocklist: src.local_issi_blocklist,
        subscriber_type_deregister: src.subscriber_type_deregister,
        subscriber_type_register: src.subscriber_type_register,
        subscriber_type_reregister: src.subscriber_type_reregister,
        subscriber_type_affiliate: src.subscriber_type_affiliate,
        subscriber_type_deaffiliate: src.subscriber_type_deaffiliate,
    }
}
