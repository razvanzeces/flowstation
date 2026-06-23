use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::bluestation::SecretField;

/// DAPNET receiver/forwarder configuration.
///
/// Disabled by default. When enabled, the worker receives DAPNET calls from the RWTH core
/// transmitter TCP feed and can forward each normalized message to local SDS, TPG2200 Call-Out,
/// and/or the existing Telegram alerter.
#[derive(Debug, Clone)]
pub struct CfgDapnet {
    pub enabled: bool,
    pub api_url: String,
    pub username: String,
    pub password: SecretField,
    pub poll_interval_secs: u64,

    pub forward_sds: bool,
    pub forward_callout: bool,
    pub forward_telegram: bool,

    pub sds_source_issi: u32,
    pub sds_dest_issi: u32,
    pub sds_dest_is_group: bool,
    /// Optional incoming DAPNET RIC/CAP-code to TETRA ISSI routes.
    ///
    /// Config keys may use decimal RICs with leading zeros (e.g. "0632585") or hex with a
    /// `0x` prefix. Internally the key is normalized to the numeric RIC received from the core.
    pub ric_issi_routes: BTreeMap<u32, u32>,
    /// Optional incoming DAPNET RIC/CAP-code to TETRA group GSSI routes.
    pub ric_gssi_routes: BTreeMap<u32, u32>,
    /// Optional per-path RIC allowlists. Empty means "allow all RICs" for that path.
    pub sds_allowed_rics: BTreeSet<u32>,
    pub callout_allowed_rics: BTreeSet<u32>,
    pub telegram_allowed_rics: BTreeSet<u32>,

    pub callout_source_issi: u32,
    pub callout_dest_issi: u32,
    pub callout_incident_base: u16,
    pub callout_text_prefix: String,

    pub telegram_prefix: String,

    pub rwth_core_enabled: bool,
    pub rwth_core_host: String,
    pub rwth_core_port: u16,
    pub rwth_core_device: String,
    pub rwth_core_version: String,
    pub rwth_core_callsign: String,
    pub rwth_core_authkey: SecretField,
    pub rwth_messages_limit: usize,
}

impl Default for CfgDapnet {
    fn default() -> Self {
        CfgDapnet {
            enabled: false,
            api_url: default_api_url(),
            username: String::new(),
            password: SecretField::from(String::new()),
            poll_interval_secs: 30,

            forward_sds: false,
            forward_callout: false,
            forward_telegram: false,

            sds_source_issi: 9999,
            sds_dest_issi: 0,
            sds_dest_is_group: false,
            ric_issi_routes: BTreeMap::new(),
            ric_gssi_routes: BTreeMap::new(),
            sds_allowed_rics: BTreeSet::new(),
            callout_allowed_rics: BTreeSet::new(),
            telegram_allowed_rics: BTreeSet::new(),

            callout_source_issi: 9999,
            callout_dest_issi: 0,
            callout_incident_base: 2,
            callout_text_prefix: "DAPNET".to_string(),

            telegram_prefix: "DAPNET".to_string(),

            rwth_core_enabled: true,
            rwth_core_host: "dapnet.afu.rwth-aachen.de".to_string(),
            rwth_core_port: 43434,
            rwth_core_device: "FlowStation".to_string(),
            rwth_core_version: "1.0".to_string(),
            rwth_core_callsign: String::new(),
            rwth_core_authkey: SecretField::from(String::new()),
            rwth_messages_limit: 100,
        }
    }
}

impl CfgDapnet {
    pub fn effective_poll_interval_secs(&self) -> u64 {
        self.poll_interval_secs.max(1)
    }

    pub fn effective_messages_limit(&self) -> usize {
        self.rwth_messages_limit.max(1)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgDapnetDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_api_url")]
    pub api_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,

    #[serde(default)]
    pub forward_sds: bool,
    #[serde(default)]
    pub forward_callout: bool,
    #[serde(default)]
    pub forward_telegram: bool,

    #[serde(default = "default_source_issi")]
    pub sds_source_issi: u32,
    #[serde(default)]
    pub sds_dest_issi: u32,
    #[serde(default)]
    pub sds_dest_is_group: bool,
    #[serde(default)]
    pub ric_issi_routes: HashMap<String, u32>,
    #[serde(default)]
    pub ric_gssi_routes: HashMap<String, u32>,
    #[serde(default)]
    pub sds_allowed_rics: Vec<toml::Value>,
    #[serde(default)]
    pub callout_allowed_rics: Vec<toml::Value>,
    #[serde(default)]
    pub telegram_allowed_rics: Vec<toml::Value>,

    #[serde(default = "default_source_issi")]
    pub callout_source_issi: u32,
    #[serde(default)]
    pub callout_dest_issi: u32,
    #[serde(default = "default_callout_incident_base")]
    pub callout_incident_base: u16,
    #[serde(default = "default_dapnet_prefix")]
    pub callout_text_prefix: String,

    #[serde(default = "default_dapnet_prefix")]
    pub telegram_prefix: String,

    #[serde(default = "default_true")]
    pub rwth_core_enabled: bool,
    #[serde(default = "default_rwth_core_host")]
    pub rwth_core_host: String,
    #[serde(default = "default_rwth_core_port")]
    pub rwth_core_port: u16,
    #[serde(default = "default_rwth_core_device")]
    pub rwth_core_device: String,
    #[serde(default = "default_rwth_core_version")]
    pub rwth_core_version: String,
    #[serde(default)]
    pub rwth_core_callsign: String,
    #[serde(default)]
    pub rwth_core_authkey: String,
    #[serde(default = "default_rwth_messages_limit")]
    pub rwth_messages_limit: usize,

    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, toml::Value>,
}

impl Default for CfgDapnetDto {
    fn default() -> Self {
        CfgDapnetDto {
            enabled: false,
            api_url: default_api_url(),
            username: String::new(),
            password: String::new(),
            poll_interval_secs: default_poll_interval_secs(),
            forward_sds: false,
            forward_callout: false,
            forward_telegram: false,
            sds_source_issi: default_source_issi(),
            sds_dest_issi: 0,
            sds_dest_is_group: false,
            ric_issi_routes: HashMap::new(),
            ric_gssi_routes: HashMap::new(),
            sds_allowed_rics: Vec::new(),
            callout_allowed_rics: Vec::new(),
            telegram_allowed_rics: Vec::new(),
            callout_source_issi: default_source_issi(),
            callout_dest_issi: 0,
            callout_incident_base: default_callout_incident_base(),
            callout_text_prefix: default_dapnet_prefix(),
            telegram_prefix: default_dapnet_prefix(),
            rwth_core_enabled: true,
            rwth_core_host: default_rwth_core_host(),
            rwth_core_port: default_rwth_core_port(),
            rwth_core_device: default_rwth_core_device(),
            rwth_core_version: default_rwth_core_version(),
            rwth_core_callsign: String::new(),
            rwth_core_authkey: String::new(),
            rwth_messages_limit: default_rwth_messages_limit(),
            extra: std::collections::HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_api_url() -> String {
    "https://hampager.de/api/calls".to_string()
}

fn default_poll_interval_secs() -> u64 {
    30
}

fn default_source_issi() -> u32 {
    9999
}

fn default_callout_incident_base() -> u16 {
    2
}

fn default_dapnet_prefix() -> String {
    "DAPNET".to_string()
}

fn default_rwth_core_host() -> String {
    "dapnet.afu.rwth-aachen.de".to_string()
}

fn default_rwth_core_port() -> u16 {
    43434
}

fn default_rwth_core_device() -> String {
    "FlowStation".to_string()
}

fn default_rwth_core_version() -> String {
    "1.0".to_string()
}

fn default_rwth_messages_limit() -> usize {
    100
}

pub fn apply_dapnet_patch(dto: CfgDapnetDto) -> Result<CfgDapnet, String> {
    let ric_issi_routes = normalize_ric_ssi_routes("dapnet.ric_issi_routes", dto.ric_issi_routes)?;
    let ric_gssi_routes = normalize_ric_ssi_routes("dapnet.ric_gssi_routes", dto.ric_gssi_routes)?;
    ensure_no_route_conflicts(&ric_issi_routes, &ric_gssi_routes)?;
    Ok(CfgDapnet {
        enabled: dto.enabled,
        api_url: dto.api_url,
        username: dto.username,
        password: SecretField::from(dto.password),
        poll_interval_secs: dto.poll_interval_secs.max(1),
        forward_sds: dto.forward_sds,
        forward_callout: dto.forward_callout,
        forward_telegram: dto.forward_telegram,
        sds_source_issi: dto.sds_source_issi,
        sds_dest_issi: dto.sds_dest_issi,
        sds_dest_is_group: dto.sds_dest_is_group,
        ric_issi_routes,
        ric_gssi_routes,
        sds_allowed_rics: normalize_ric_value_list("dapnet.sds_allowed_rics", dto.sds_allowed_rics)?,
        callout_allowed_rics: normalize_ric_value_list("dapnet.callout_allowed_rics", dto.callout_allowed_rics)?,
        telegram_allowed_rics: normalize_ric_value_list("dapnet.telegram_allowed_rics", dto.telegram_allowed_rics)?,
        callout_source_issi: dto.callout_source_issi,
        callout_dest_issi: dto.callout_dest_issi,
        callout_incident_base: dto.callout_incident_base.clamp(1, 256),
        callout_text_prefix: dto.callout_text_prefix,
        telegram_prefix: dto.telegram_prefix,
        rwth_core_enabled: dto.rwth_core_enabled,
        rwth_core_host: dto.rwth_core_host,
        rwth_core_port: dto.rwth_core_port,
        rwth_core_device: dto.rwth_core_device,
        rwth_core_version: dto.rwth_core_version,
        rwth_core_callsign: dto.rwth_core_callsign,
        rwth_core_authkey: SecretField::from(dto.rwth_core_authkey),
        rwth_messages_limit: dto.rwth_messages_limit.max(1),
    })
}

pub fn parse_ric_route_key(raw: &str) -> Result<u32, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Err("empty RIC route key".to_string());
    }
    if let Some(hex) = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("invalid hex RIC route key '{raw}'"));
    }
    if key.chars().all(|c| c.is_ascii_digit()) {
        return key.parse::<u32>().map_err(|_| format!("invalid decimal RIC route key '{raw}'"));
    }
    Err(format!("invalid RIC route key '{raw}'"))
}

pub fn format_ric_route_key(ric: u32) -> String {
    format!("{ric:07}")
}

fn normalize_ric_ssi_routes(field: &str, routes: HashMap<String, u32>) -> Result<BTreeMap<u32, u32>, String> {
    let mut out = BTreeMap::new();
    for (raw_ric, issi) in routes {
        let ric = parse_ric_route_key(&raw_ric)?;
        if issi == 0 {
            return Err(format!("{field}: SSI for RIC {raw_ric} cannot be 0"));
        }
        if issi > 16_777_215 {
            return Err(format!("{field}: SSI for RIC {raw_ric} exceeds 16777215"));
        }
        out.insert(ric, issi);
    }
    Ok(out)
}

fn normalize_ric_value_list(field: &str, values: Vec<toml::Value>) -> Result<BTreeSet<u32>, String> {
    let mut out = BTreeSet::new();
    for value in values {
        let ric = match value {
            toml::Value::String(s) => parse_ric_route_key(&s)?,
            toml::Value::Integer(n) if n >= 0 && n <= u32::MAX as i64 => n as u32,
            _ => return Err(format!("{field}: RIC values must be strings or positive integers")),
        };
        out.insert(ric);
    }
    Ok(out)
}

fn ensure_no_route_conflicts(issi_routes: &BTreeMap<u32, u32>, gssi_routes: &BTreeMap<u32, u32>) -> Result<(), String> {
    for ric in issi_routes.keys() {
        if gssi_routes.contains_key(ric) {
            return Err(format!(
                "dapnet RIC {} is configured as both ISSI and GSSI route",
                format_ric_route_key(*ric)
            ));
        }
    }
    Ok(())
}
