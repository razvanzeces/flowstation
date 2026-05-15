use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

use super::SecretField;

const DEFAULT_CACHE_TTL_SECS: u64 = 86_400;
const DEFAULT_NEGATIVE_CACHE_TTL_SECS: u64 = 3_600;
const DEFAULT_CACHE_MAX_ENTRIES: usize = 10_000;
const DEFAULT_RADIOID_ENDPOINT: &str = "https://radioid.net/api/dmr/user/";
const DEFAULT_RADIOID_TIMEOUT_SECS: u64 = 3;
const DEFAULT_RADIOID_MIN_INTERVAL_MS: u64 = 1_000;
const DEFAULT_RADIOID_USER_AGENT: &str = "FlowStation/0.1 SS-TPI identity resolver";

#[derive(Debug, Clone)]
pub struct CfgIdentity {
    pub enabled: bool,
    pub emit_mnemonic_name: bool,
    pub subscription_allows_mnemonic: bool,
    pub cache_ttl_secs: u64,
    pub negative_cache_ttl_secs: u64,
    pub cache_max_entries: usize,
    pub manual: Vec<CfgManualIdentity>,
    pub radioid: CfgRadioId,
}

impl Default for CfgIdentity {
    fn default() -> Self {
        Self {
            enabled: false,
            emit_mnemonic_name: true,
            subscription_allows_mnemonic: true,
            cache_ttl_secs: DEFAULT_CACHE_TTL_SECS,
            negative_cache_ttl_secs: DEFAULT_NEGATIVE_CACHE_TTL_SECS,
            cache_max_entries: DEFAULT_CACHE_MAX_ENTRIES,
            manual: Vec::new(),
            radioid: CfgRadioId::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfgManualIdentity {
    pub ssi: u32,
    pub mnemonic: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CfgRadioId {
    pub enabled: bool,
    pub endpoint: String,
    pub timeout_secs: u64,
    pub min_lookup_interval_ms: u64,
    pub user_agent: String,
    pub api_token: Option<SecretField>,
}

impl Default for CfgRadioId {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: DEFAULT_RADIOID_ENDPOINT.to_string(),
            timeout_secs: DEFAULT_RADIOID_TIMEOUT_SECS,
            min_lookup_interval_ms: DEFAULT_RADIOID_MIN_INTERVAL_MS,
            user_agent: DEFAULT_RADIOID_USER_AGENT.to_string(),
            api_token: None,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct CfgIdentityDto {
    pub enabled: Option<bool>,
    pub emit_mnemonic_name: Option<bool>,
    pub subscription_allows_mnemonic: Option<bool>,
    pub cache_ttl_secs: Option<u64>,
    pub negative_cache_ttl_secs: Option<u64>,
    pub cache_max_entries: Option<usize>,
    #[serde(default)]
    pub manual: Vec<CfgManualIdentityDto>,
    pub radioid: Option<CfgRadioIdDto>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct CfgManualIdentityDto {
    pub ssi: u32,
    pub mnemonic: Option<String>,
    pub label: Option<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Deserialize, Default)]
pub struct CfgRadioIdDto {
    pub enabled: Option<bool>,
    pub endpoint: Option<String>,
    pub timeout_secs: Option<u64>,
    pub min_lookup_interval_ms: Option<u64>,
    pub user_agent: Option<String>,
    pub api_token: Option<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn apply_identity_patch(src: CfgIdentityDto) -> Result<CfgIdentity, String> {
    let mut cfg = CfgIdentity::default();

    if let Some(enabled) = src.enabled {
        cfg.enabled = enabled;
    }
    if let Some(emit) = src.emit_mnemonic_name {
        cfg.emit_mnemonic_name = emit;
    }
    if let Some(allows) = src.subscription_allows_mnemonic {
        cfg.subscription_allows_mnemonic = allows;
    }
    if let Some(ttl) = src.cache_ttl_secs {
        cfg.cache_ttl_secs = ttl;
    }
    if let Some(ttl) = src.negative_cache_ttl_secs {
        cfg.negative_cache_ttl_secs = ttl;
    }
    if let Some(max_entries) = src.cache_max_entries {
        cfg.cache_max_entries = max_entries;
    }
    if cfg.cache_max_entries == 0 {
        return Err("identity: cache_max_entries must be greater than zero".to_string());
    }

    cfg.manual = src
        .manual
        .into_iter()
        .map(|entry| CfgManualIdentity {
            ssi: entry.ssi,
            mnemonic: empty_to_none(entry.mnemonic),
            label: empty_to_none(entry.label),
        })
        .collect();

    if let Some(radioid) = src.radioid {
        cfg.radioid = apply_radioid_patch(radioid)?;
    }

    Ok(cfg)
}

fn apply_radioid_patch(src: CfgRadioIdDto) -> Result<CfgRadioId, String> {
    let mut cfg = CfgRadioId::default();

    if let Some(enabled) = src.enabled {
        cfg.enabled = enabled;
    }
    if let Some(endpoint) = src.endpoint {
        cfg.endpoint = endpoint;
    }
    if let Some(timeout_secs) = src.timeout_secs {
        cfg.timeout_secs = timeout_secs;
    }
    if let Some(interval_ms) = src.min_lookup_interval_ms {
        cfg.min_lookup_interval_ms = interval_ms;
    }
    if let Some(user_agent) = empty_to_none(src.user_agent) {
        cfg.user_agent = user_agent;
    }
    cfg.api_token = empty_to_none(src.api_token).map(SecretField::from);

    if cfg.enabled && cfg.endpoint.trim().is_empty() {
        return Err("identity.radioid: endpoint must not be empty when enabled".to_string());
    }
    if cfg.enabled && cfg.timeout_secs == 0 {
        return Err("identity.radioid: timeout_secs must be greater than zero when enabled".to_string());
    }
    if cfg.enabled && cfg.user_agent.trim().is_empty() {
        return Err("identity.radioid: user_agent must not be empty when enabled".to_string());
    }

    Ok(cfg)
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    })
}
