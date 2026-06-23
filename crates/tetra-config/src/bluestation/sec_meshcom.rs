use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use toml::Value;

/// MeshCom external UDP bridge configuration.
///
/// Disabled by default. When enabled, FlowStation listens for MeshCom JSON packets on the
/// configured UDP socket and can send JSON text messages back to a node using the documented
/// external-client format.
#[derive(Debug, Clone)]
pub struct CfgMeshcom {
    pub enabled: bool,
    pub bind_addr: String,
    pub bind_port: u16,
    pub tx_host: String,
    pub tx_port: u16,
    pub allow_broadcast: bool,
    pub max_messages: usize,
    pub max_nodes: usize,
    pub forward_sds: bool,
    pub forward_sip: bool,
    pub forward_telegram: bool,
    pub sds_source_issi: u32,
    pub sds_dest_issi: u32,
    pub sds_dest_is_group: bool,
    pub sds_allowed_sources: BTreeSet<String>,
    pub sip_title_prefix: String,
    pub sip_allowed_sources: BTreeSet<String>,
    pub telegram_prefix: String,
    pub telegram_allowed_sources: BTreeSet<String>,
}

impl Default for CfgMeshcom {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: default_bind_addr(),
            bind_port: default_udp_port(),
            tx_host: default_tx_host(),
            tx_port: default_udp_port(),
            allow_broadcast: true,
            max_messages: default_max_messages(),
            max_nodes: default_max_nodes(),
            forward_sds: false,
            forward_sip: false,
            forward_telegram: false,
            sds_source_issi: default_source_issi(),
            sds_dest_issi: 0,
            sds_dest_is_group: false,
            sds_allowed_sources: BTreeSet::new(),
            sip_title_prefix: default_meshcom_prefix(),
            sip_allowed_sources: BTreeSet::new(),
            telegram_prefix: default_meshcom_prefix(),
            telegram_allowed_sources: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgMeshcomDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_udp_port")]
    pub bind_port: u16,
    #[serde(default = "default_tx_host")]
    pub tx_host: String,
    #[serde(default = "default_udp_port")]
    pub tx_port: u16,
    #[serde(default = "default_true")]
    pub allow_broadcast: bool,
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
    #[serde(default)]
    pub forward_sds: bool,
    #[serde(default)]
    pub forward_sip: bool,
    #[serde(default)]
    pub forward_telegram: bool,
    #[serde(default = "default_source_issi")]
    pub sds_source_issi: u32,
    #[serde(default)]
    pub sds_dest_issi: u32,
    #[serde(default)]
    pub sds_dest_is_group: bool,
    #[serde(default)]
    pub sds_allowed_sources: Vec<String>,
    #[serde(default = "default_meshcom_prefix")]
    pub sip_title_prefix: String,
    #[serde(default)]
    pub sip_allowed_sources: Vec<String>,
    #[serde(default = "default_meshcom_prefix")]
    pub telegram_prefix: String,
    #[serde(default)]
    pub telegram_allowed_sources: Vec<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgMeshcomDto {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: default_bind_addr(),
            bind_port: default_udp_port(),
            tx_host: default_tx_host(),
            tx_port: default_udp_port(),
            allow_broadcast: true,
            max_messages: default_max_messages(),
            max_nodes: default_max_nodes(),
            forward_sds: false,
            forward_sip: false,
            forward_telegram: false,
            sds_source_issi: default_source_issi(),
            sds_dest_issi: 0,
            sds_dest_is_group: false,
            sds_allowed_sources: Vec::new(),
            sip_title_prefix: default_meshcom_prefix(),
            sip_allowed_sources: Vec::new(),
            telegram_prefix: default_meshcom_prefix(),
            telegram_allowed_sources: Vec::new(),
            extra: HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_bind_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_tx_host() -> String {
    "255.255.255.255".to_string()
}

fn default_udp_port() -> u16 {
    1799
}

fn default_max_messages() -> usize {
    500
}

fn default_max_nodes() -> usize {
    1000
}

fn default_source_issi() -> u32 {
    9999
}

fn default_meshcom_prefix() -> String {
    "MeshCom".to_string()
}

pub fn apply_meshcom_patch(src: CfgMeshcomDto) -> Result<CfgMeshcom, String> {
    if src.enabled {
        if src.bind_addr.trim().is_empty() {
            return Err("meshcom: bind_addr cannot be empty when enabled".to_string());
        }
        if src.tx_host.trim().is_empty() {
            return Err("meshcom: tx_host cannot be empty when enabled".to_string());
        }
        if src.bind_port == 0 || src.tx_port == 0 {
            return Err("meshcom: bind_port/tx_port must be non-zero".to_string());
        }
    }
    if src.forward_sds {
        if src.sds_source_issi == 0 || src.sds_source_issi > 16_777_215 {
            return Err("meshcom: sds_source_issi must be 1..=16777215".to_string());
        }
        if src.sds_dest_issi > 16_777_215 {
            return Err("meshcom: sds_dest_issi must be 0..=16777215".to_string());
        }
    }

    Ok(CfgMeshcom {
        enabled: src.enabled,
        bind_addr: non_empty_or(src.bind_addr, "0.0.0.0"),
        bind_port: nonzero_port_or(src.bind_port, default_udp_port()),
        tx_host: non_empty_or(src.tx_host, "255.255.255.255"),
        tx_port: nonzero_port_or(src.tx_port, default_udp_port()),
        allow_broadcast: src.allow_broadcast,
        max_messages: src.max_messages.clamp(10, 10_000),
        max_nodes: src.max_nodes.clamp(10, 65_535),
        forward_sds: src.forward_sds,
        forward_sip: src.forward_sip,
        forward_telegram: src.forward_telegram,
        sds_source_issi: src.sds_source_issi.max(1),
        sds_dest_issi: src.sds_dest_issi,
        sds_dest_is_group: src.sds_dest_is_group,
        sds_allowed_sources: normalize_source_list(src.sds_allowed_sources),
        sip_title_prefix: non_empty_or(src.sip_title_prefix, "MeshCom"),
        sip_allowed_sources: normalize_source_list(src.sip_allowed_sources),
        telegram_prefix: non_empty_or(src.telegram_prefix, "MeshCom"),
        telegram_allowed_sources: normalize_source_list(src.telegram_allowed_sources),
    })
}

fn non_empty_or(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn nonzero_port_or(value: u16, fallback: u16) -> u16 {
    if value == 0 { fallback } else { value }
}

fn normalize_source_list(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .flat_map(|value| {
            value
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(|part| part.to_ascii_uppercase())
                .collect::<Vec<_>>()
        })
        .collect()
}
