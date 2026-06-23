use std::collections::{BTreeSet, HashMap};

use serde::Deserialize;
use toml::Value;

use crate::bluestation::{SecretField, parse_ric_route_key};

/// Snom XML minibrowser notification bridge (`[snom_notify]`).
///
/// Sends FlowStation message events to one or more Asterisk PJSIP endpoints via AMI
/// `PJSIPNotify`. The generated SIP NOTIFY uses Snom's XML minibrowser format:
/// `Event: xml`, `Content-Type: application/snomxml`, body `SnomIPPhoneText`.
#[derive(Debug, Clone)]
pub struct CfgSnomNotify {
    pub enabled: bool,
    pub ami_host: String,
    pub ami_port: u16,
    pub ami_username: String,
    pub ami_password: SecretField,
    pub endpoints: Vec<String>,
    pub notify_sds: bool,
    pub notify_dapnet: bool,
    pub notify_telegram: bool,
    pub sds_directions: Vec<String>,
    /// Optional DAPNET RIC allowlist for Snom notifications. Empty means all RICs.
    pub dapnet_allowed_rics: BTreeSet<u32>,
    /// Optional SDS ISSI allowlist for Snom notifications. Empty means all SDS.
    pub sds_allowed_issis: BTreeSet<u32>,
    pub title_prefix: String,
    pub notify_event: String,
    pub content_type: String,
    pub subscription_state: String,
    pub max_text_chars: usize,
    pub connect_timeout_secs: u64,
}

impl Default for CfgSnomNotify {
    fn default() -> Self {
        apply_snom_notify_patch(CfgSnomNotifyDto::default()).expect("default snom_notify config must be valid")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgSnomNotifyDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ami_host")]
    pub ami_host: String,
    #[serde(default = "default_ami_port")]
    pub ami_port: u16,
    #[serde(default)]
    pub ami_username: String,
    #[serde(default)]
    pub ami_password: String,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default = "default_true")]
    pub notify_sds: bool,
    #[serde(default = "default_true")]
    pub notify_dapnet: bool,
    #[serde(default = "default_true")]
    pub notify_telegram: bool,
    #[serde(default = "default_sds_directions")]
    pub sds_directions: Vec<String>,
    #[serde(default)]
    pub dapnet_allowed_rics: Vec<Value>,
    #[serde(default)]
    pub sds_allowed_issis: Vec<u32>,
    #[serde(default = "default_title_prefix")]
    pub title_prefix: String,
    #[serde(default = "default_notify_event")]
    pub notify_event: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    #[serde(default = "default_subscription_state")]
    pub subscription_state: String,
    #[serde(default = "default_max_text_chars")]
    pub max_text_chars: usize,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgSnomNotifyDto {
    fn default() -> Self {
        Self {
            enabled: false,
            ami_host: default_ami_host(),
            ami_port: default_ami_port(),
            ami_username: String::new(),
            ami_password: String::new(),
            endpoints: Vec::new(),
            notify_sds: true,
            notify_dapnet: true,
            notify_telegram: true,
            sds_directions: default_sds_directions(),
            dapnet_allowed_rics: Vec::new(),
            sds_allowed_issis: Vec::new(),
            title_prefix: default_title_prefix(),
            notify_event: default_notify_event(),
            content_type: default_content_type(),
            subscription_state: default_subscription_state(),
            max_text_chars: default_max_text_chars(),
            connect_timeout_secs: default_connect_timeout_secs(),
            extra: HashMap::new(),
        }
    }
}

fn default_ami_host() -> String {
    "127.0.0.1".to_string()
}
fn default_ami_port() -> u16 {
    5038
}
fn default_sds_directions() -> Vec<String> {
    vec!["rx".to_string(), "net".to_string(), "tx".to_string()]
}
fn default_true() -> bool {
    true
}
fn default_title_prefix() -> String {
    "FlowStation".to_string()
}
fn default_notify_event() -> String {
    "xml".to_string()
}
fn default_content_type() -> String {
    "application/snomxml".to_string()
}
fn default_subscription_state() -> String {
    "active;expires=30000".to_string()
}
fn default_max_text_chars() -> usize {
    240
}
fn default_connect_timeout_secs() -> u64 {
    3
}

pub fn apply_snom_notify_patch(src: CfgSnomNotifyDto) -> Result<CfgSnomNotify, String> {
    if src.ami_port == 0 {
        return Err("snom_notify: ami_port cannot be 0".to_string());
    }
    let ami_host = src.ami_host.trim().to_string();
    if src.enabled && ami_host.is_empty() {
        return Err("snom_notify: ami_host cannot be empty when enabled=true".to_string());
    }

    let endpoints: Vec<String> = src
        .endpoints
        .into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();

    let sds_directions: Vec<String> = src
        .sds_directions
        .into_iter()
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    let dapnet_allowed_rics = normalize_ric_value_list(src.dapnet_allowed_rics)?;
    let sds_allowed_issis = normalize_issi_list(src.sds_allowed_issis)?;

    Ok(CfgSnomNotify {
        enabled: src.enabled,
        ami_host,
        ami_port: src.ami_port,
        ami_username: src.ami_username.trim().to_string(),
        ami_password: SecretField::from(src.ami_password),
        endpoints,
        notify_sds: src.notify_sds,
        notify_dapnet: src.notify_dapnet,
        notify_telegram: src.notify_telegram,
        sds_directions,
        dapnet_allowed_rics,
        sds_allowed_issis,
        title_prefix: non_empty_or(src.title_prefix, default_title_prefix()),
        notify_event: non_empty_or(src.notify_event, default_notify_event()),
        content_type: non_empty_or(src.content_type, default_content_type()),
        subscription_state: non_empty_or(src.subscription_state, default_subscription_state()),
        max_text_chars: src.max_text_chars.clamp(40, 2000),
        connect_timeout_secs: src.connect_timeout_secs.clamp(1, 30),
    })
}

fn normalize_ric_value_list(values: Vec<Value>) -> Result<BTreeSet<u32>, String> {
    let mut out = BTreeSet::new();
    for value in values {
        let ric = match value {
            Value::String(s) => parse_ric_route_key(&s)?,
            Value::Integer(n) if n >= 0 => parse_ric_route_key(&n.to_string())?,
            other => return Err(format!("snom_notify: invalid RIC value {other:?}")),
        };
        out.insert(ric);
    }
    Ok(out)
}

fn normalize_issi_list(values: Vec<u32>) -> Result<BTreeSet<u32>, String> {
    let mut out = BTreeSet::new();
    for issi in values {
        if issi > 16_777_215 {
            return Err(format!("snom_notify: SDS ISSI {} out of range", issi));
        }
        out.insert(issi);
    }
    Ok(out)
}

fn non_empty_or(value: String, fallback: String) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() { fallback } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_snom_xml_notify() {
        let cfg = CfgSnomNotify::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.ami_host, "127.0.0.1");
        assert_eq!(cfg.ami_port, 5038);
        assert_eq!(cfg.notify_event, "xml");
        assert_eq!(cfg.content_type, "application/snomxml");
    }

    #[test]
    fn trims_endpoint_and_direction_lists() {
        let dto = CfgSnomNotifyDto {
            endpoints: vec![" 385 ".to_string(), "".to_string()],
            sds_directions: vec![" RX ".to_string(), "net".to_string()],
            dapnet_allowed_rics: vec![Value::String("0632585".to_string())],
            sds_allowed_issis: vec![2632585, 9999],
            ..Default::default()
        };
        let cfg = apply_snom_notify_patch(dto).unwrap();
        assert_eq!(cfg.endpoints, vec!["385"]);
        assert_eq!(cfg.sds_directions, vec!["rx", "net"]);
        assert!(cfg.dapnet_allowed_rics.contains(&632585));
        assert!(cfg.sds_allowed_issis.contains(&2632585));
    }
}
