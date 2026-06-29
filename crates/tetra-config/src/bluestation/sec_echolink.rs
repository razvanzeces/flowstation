use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use toml::Value;

use crate::bluestation::SecretField;

/// EchoLink bridge configuration.
///
/// Disabled by default. This section provides the stable FlowStation-side integration surface:
/// credentials, local EchoLink ports, and deterministic TETRA routing. The voice QSO engine can
/// use the same config once the GSM-FR EchoLink audio bridge is enabled.
#[derive(Debug, Clone)]
pub struct CfgEcholink {
    pub enabled: bool,
    pub callsign: String,
    pub password: SecretField,
    pub location: String,
    pub status_text: String,
    pub directory_servers: Vec<String>,
    pub directory_port: u16,
    pub bind_addr: String,
    pub audio_port: u16,
    pub control_port: u16,
    pub inbound_enabled: bool,
    pub outbound_enabled: bool,
    pub outbound_prefix: String,
    pub strip_outbound_prefix: bool,
    pub service_numbers: Vec<String>,
    pub default_tetra_source_issi: u32,
    pub default_tetra_dest_issi: u32,
    pub default_tetra_dest_is_group: bool,
    pub routes: BTreeMap<String, String>,
    pub allowed_callsigns: Vec<String>,
    pub allowed_node_ids: Vec<u32>,
    pub auto_connect: String,
    pub reconnect_interval_secs: u64,
    pub max_session_secs: u64,
    pub telegram_session_alerts: bool,
    pub telegram_session_prefix: String,
}

impl Default for CfgEcholink {
    fn default() -> Self {
        apply_echolink_patch(CfgEcholinkDto::default()).expect("default echolink config must be valid")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgEcholinkDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub callsign: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_location")]
    pub location: String,
    #[serde(default = "default_status_text")]
    pub status_text: String,
    #[serde(default = "default_directory_servers")]
    pub directory_servers: Vec<String>,
    #[serde(default = "default_directory_port")]
    pub directory_port: u16,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_audio_port")]
    pub audio_port: u16,
    #[serde(default = "default_control_port")]
    pub control_port: u16,
    #[serde(default = "default_true")]
    pub inbound_enabled: bool,
    #[serde(default = "default_true")]
    pub outbound_enabled: bool,
    #[serde(default = "default_outbound_prefix")]
    pub outbound_prefix: String,
    #[serde(default = "default_strip_outbound_prefix")]
    pub strip_outbound_prefix: bool,
    #[serde(default)]
    pub service_numbers: Vec<String>,
    #[serde(default = "default_tetra_source_issi")]
    pub default_tetra_source_issi: u32,
    #[serde(default)]
    pub default_tetra_dest_issi: u32,
    #[serde(default)]
    pub default_tetra_dest_is_group: bool,
    #[serde(default)]
    pub routes: HashMap<String, String>,
    #[serde(default)]
    pub allowed_callsigns: Vec<String>,
    #[serde(default)]
    pub allowed_node_ids: Vec<u32>,
    #[serde(default)]
    pub auto_connect: String,
    #[serde(default = "default_reconnect_interval_secs")]
    pub reconnect_interval_secs: u64,
    #[serde(default = "default_max_session_secs")]
    pub max_session_secs: u64,
    #[serde(default)]
    pub telegram_session_alerts: bool,
    #[serde(default = "default_telegram_session_prefix")]
    pub telegram_session_prefix: String,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgEcholinkDto {
    fn default() -> Self {
        Self {
            enabled: false,
            callsign: String::new(),
            password: String::new(),
            location: default_location(),
            status_text: default_status_text(),
            directory_servers: default_directory_servers(),
            directory_port: default_directory_port(),
            bind_addr: default_bind_addr(),
            audio_port: default_audio_port(),
            control_port: default_control_port(),
            inbound_enabled: true,
            outbound_enabled: true,
            outbound_prefix: default_outbound_prefix(),
            strip_outbound_prefix: default_strip_outbound_prefix(),
            service_numbers: Vec::new(),
            default_tetra_source_issi: default_tetra_source_issi(),
            default_tetra_dest_issi: 0,
            default_tetra_dest_is_group: false,
            routes: HashMap::new(),
            allowed_callsigns: Vec::new(),
            allowed_node_ids: Vec::new(),
            auto_connect: String::new(),
            reconnect_interval_secs: default_reconnect_interval_secs(),
            max_session_secs: default_max_session_secs(),
            telegram_session_alerts: false,
            telegram_session_prefix: default_telegram_session_prefix(),
            extra: HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_location() -> String {
    "FlowStation".to_string()
}

fn default_status_text() -> String {
    "FlowStation EchoLink bridge".to_string()
}

fn default_directory_servers() -> Vec<String> {
    vec!["servers.echolink.org".to_string(), "backup.echolink.org".to_string()]
}

fn default_directory_port() -> u16 {
    5200
}

fn default_bind_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_audio_port() -> u16 {
    5198
}

fn default_control_port() -> u16 {
    5199
}

fn default_outbound_prefix() -> String {
    "92".to_string()
}

fn default_strip_outbound_prefix() -> bool {
    true
}

fn default_tetra_source_issi() -> u32 {
    9999
}

fn default_reconnect_interval_secs() -> u64 {
    30
}

fn default_max_session_secs() -> u64 {
    3600
}

fn default_telegram_session_prefix() -> String {
    "EchoLink".to_string()
}

pub fn apply_echolink_patch(src: CfgEcholinkDto) -> Result<CfgEcholink, String> {
    if src.enabled {
        if src.callsign.trim().is_empty() {
            return Err("echolink: callsign cannot be empty when enabled".to_string());
        }
        if src.password.trim().is_empty() {
            return Err("echolink: password cannot be empty when enabled".to_string());
        }
        if src.directory_port == 0 {
            return Err("echolink: directory_port cannot be 0".to_string());
        }
        if src.audio_port == 0 || src.control_port == 0 || src.audio_port == src.control_port {
            return Err("echolink: audio_port/control_port must be distinct non-zero ports".to_string());
        }
        if src.directory_servers.iter().all(|s| s.trim().is_empty()) {
            return Err("echolink: at least one directory server is required when enabled".to_string());
        }
    }

    let routes = normalize_routes(src.routes)?;
    let service_numbers = normalize_string_list(src.service_numbers, false);
    let allowed_callsigns = normalize_string_list(src.allowed_callsigns, true);
    let allowed_node_ids = src.allowed_node_ids.into_iter().filter(|id| *id > 0).collect();

    Ok(CfgEcholink {
        enabled: src.enabled,
        callsign: normalize_callsign(&src.callsign),
        password: SecretField::from(src.password),
        location: src.location,
        status_text: src.status_text,
        directory_servers: normalize_string_list(src.directory_servers, false),
        directory_port: src.directory_port,
        bind_addr: src.bind_addr,
        audio_port: src.audio_port,
        control_port: src.control_port,
        inbound_enabled: src.inbound_enabled,
        outbound_enabled: src.outbound_enabled,
        outbound_prefix: src.outbound_prefix,
        strip_outbound_prefix: src.strip_outbound_prefix,
        service_numbers,
        default_tetra_source_issi: src.default_tetra_source_issi.max(1),
        default_tetra_dest_issi: src.default_tetra_dest_issi,
        default_tetra_dest_is_group: src.default_tetra_dest_is_group,
        routes,
        allowed_callsigns,
        allowed_node_ids,
        auto_connect: normalize_echolink_target(&src.auto_connect),
        reconnect_interval_secs: src.reconnect_interval_secs.max(1),
        max_session_secs: src.max_session_secs.max(1),
        telegram_session_alerts: src.telegram_session_alerts,
        telegram_session_prefix: non_empty_or(src.telegram_session_prefix, "EchoLink"),
    })
}

pub fn normalize_echolink_target(target: &str) -> String {
    target.trim().to_ascii_uppercase()
}

fn normalize_callsign(callsign: &str) -> String {
    callsign.trim().to_ascii_uppercase()
}

fn normalize_string_list(values: Vec<String>, callsigns: bool) -> Vec<String> {
    values
        .into_iter()
        .map(|s| if callsigns { normalize_callsign(&s) } else { s.trim().to_string() })
        .filter(|s| !s.is_empty())
        .collect()
}

fn non_empty_or(value: String, fallback: &str) -> String {
    let value = value.trim().to_string();
    if value.is_empty() { fallback.to_string() } else { value }
}

fn normalize_routes(routes: HashMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (dial, target) in routes {
        let dial = dial.trim().to_string();
        let target = normalize_echolink_target(&target);
        if dial.is_empty() || target.is_empty() {
            return Err("echolink.routes: dial keys and targets cannot be empty".to_string());
        }
        out.insert(dial, target);
    }
    Ok(out)
}
