use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

use crate::bluestation::SecretField;

/// Token-protected HTTP ActionURL endpoint for triggering a Motorola TPG2200 Call-Out.
///
/// Intended for desk phones such as Snom: a function key calls
/// `/api/action/tpg2200?token=...`, FlowStation sends a configured Type-4 SDS to the configured
/// TPG2200 ISSI, and the incident sequence advances in memory after each successful trigger.
#[derive(Debug, Clone)]
pub struct CfgTpg2200Action {
    pub enabled: bool,
    pub token: SecretField,
    pub source_issi: u32,
    pub dest_issi: u32,
    pub tpg_ric: u32,
    /// TPG incident number to start from. FlowStation converts this to the TPG selector byte
    /// sequence used by the existing ActionURL implementation.
    pub incident_base: u16,
    pub priority: u8,
    pub tpg_issi_priorities: BTreeMap<u32, u8>,
    pub tpg_ric_priorities: BTreeMap<u32, u8>,
    pub default_text: String,
    pub max_text_chars: usize,
}

impl Default for CfgTpg2200Action {
    fn default() -> Self {
        Self {
            enabled: false,
            token: SecretField::from(String::new()),
            source_issi: 9999,
            dest_issi: 0,
            tpg_ric: default_tpg2200_ric(),
            incident_base: default_incident_base(),
            priority: default_priority(),
            tpg_issi_priorities: BTreeMap::new(),
            tpg_ric_priorities: BTreeMap::new(),
            default_text: "ALARM".to_string(),
            max_text_chars: 80,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgTpg2200ActionDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_source_issi")]
    pub source_issi: u32,
    #[serde(default)]
    pub dest_issi: u32,
    #[serde(default)]
    pub tpg_ric: Option<u32>,
    #[serde(default)]
    pub ric: Option<u32>,
    #[serde(default)]
    pub callout_id_base: Option<u16>,
    #[serde(default)]
    pub incident_base: Option<u16>,
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub tpg_issi_priorities: HashMap<String, u8>,
    #[serde(default)]
    pub tpg_ric_priorities: HashMap<String, u8>,
    #[serde(default)]
    pub issi_priorities: HashMap<String, u8>,
    #[serde(default)]
    pub ric_priorities: HashMap<String, u8>,
    #[serde(default = "default_text")]
    pub default_text: String,
    #[serde(default = "default_max_text_chars")]
    pub max_text_chars: usize,

    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, toml::Value>,
}

impl Default for CfgTpg2200ActionDto {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            source_issi: default_source_issi(),
            dest_issi: 0,
            tpg_ric: None,
            ric: None,
            callout_id_base: None,
            incident_base: None,
            priority: default_priority(),
            tpg_issi_priorities: HashMap::new(),
            tpg_ric_priorities: HashMap::new(),
            issi_priorities: HashMap::new(),
            ric_priorities: HashMap::new(),
            default_text: default_text(),
            max_text_chars: default_max_text_chars(),
            extra: std::collections::HashMap::new(),
        }
    }
}

fn default_source_issi() -> u32 {
    9999
}

fn default_incident_base() -> u16 {
    1
}

fn default_tpg2200_ric() -> u32 {
    0x0009_0D10
}

fn default_priority() -> u8 {
    15
}

fn incident_from_selector_byte(selector: u16) -> u16 {
    let selector = selector.min(255) as u8;
    let major = (selector >> 4) as u16;
    let minor = (selector & 0x0F) as u16;
    let slot = if major == 0 { 16 } else { major };
    let block = if minor == 0 { 16 } else { minor };
    ((block - 1) * 16) + slot
}

fn select_incident_base(dto: &CfgTpg2200ActionDto) -> u16 {
    dto.incident_base
        .map(|incident| incident.clamp(1, 256))
        .or_else(|| dto.callout_id_base.map(incident_from_selector_byte))
        .unwrap_or_else(default_incident_base)
}

fn merge_priority_maps(legacy: HashMap<String, u8>, preferred: HashMap<String, u8>) -> HashMap<String, u8> {
    let mut merged = legacy;
    merged.extend(preferred);
    merged
}

fn default_text() -> String {
    "ALARM".to_string()
}

fn default_max_text_chars() -> usize {
    80
}

pub fn apply_tpg2200_action_patch(dto: CfgTpg2200ActionDto) -> Result<CfgTpg2200Action, String> {
    let incident_base = select_incident_base(&dto);
    if dto.enabled {
        if dto.token.trim().is_empty() {
            return Err("tpg2200_action: token cannot be empty when enabled".to_string());
        }
        if dto.source_issi == 0 {
            return Err("tpg2200_action: source_issi cannot be 0 when enabled".to_string());
        }
        if dto.dest_issi == 0 {
            return Err("tpg2200_action: dest_issi cannot be 0 when enabled".to_string());
        }
    }
    if dto.token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("tpg2200_action: token must not contain spaces or control characters".to_string());
    }
    let tpg_issi_priorities = normalize_issi_priorities(merge_priority_maps(dto.issi_priorities, dto.tpg_issi_priorities))?;
    let tpg_ric_priorities = normalize_ric_priorities(merge_priority_maps(dto.ric_priorities, dto.tpg_ric_priorities))?;

    Ok(CfgTpg2200Action {
        enabled: dto.enabled,
        token: SecretField::from(dto.token),
        source_issi: dto.source_issi,
        dest_issi: dto.dest_issi,
        tpg_ric: dto.tpg_ric.or(dto.ric).unwrap_or_else(default_tpg2200_ric),
        incident_base,
        priority: dto.priority.min(15),
        tpg_issi_priorities,
        tpg_ric_priorities,
        default_text: dto.default_text.trim().to_string(),
        max_text_chars: dto.max_text_chars.clamp(1, 240),
    })
}

fn normalize_issi_priorities(values: HashMap<String, u8>) -> Result<BTreeMap<u32, u8>, String> {
    let mut out = BTreeMap::new();
    for (raw_issi, priority) in values {
        let issi = raw_issi
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("tpg2200_action: invalid ISSI priority key '{raw_issi}'"))?;
        if issi == 0 || issi > 16_777_215 {
            return Err(format!("tpg2200_action: ISSI priority key {raw_issi} must be 1..=16777215"));
        }
        if priority > 15 {
            return Err(format!("tpg2200_action: priority for ISSI {issi} must be 0..=15"));
        }
        out.insert(issi, priority);
    }
    Ok(out)
}

fn normalize_ric_priorities(values: HashMap<String, u8>) -> Result<BTreeMap<u32, u8>, String> {
    let mut out = BTreeMap::new();
    for (raw_ric, priority) in values {
        let ric = parse_tpg_ric_key(&raw_ric)?;
        if priority > 15 {
            return Err(format!("tpg2200_action: priority for TPG RIC {raw_ric} must be 0..=15"));
        }
        out.insert(ric, priority);
    }
    Ok(out)
}

fn parse_tpg_ric_key(raw: &str) -> Result<u32, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Err("empty TPG RIC key".to_string());
    }
    if let Some(hex) = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("invalid hex TPG RIC key '{raw}'"));
    }
    key.parse::<u32>().map_err(|_| format!("invalid decimal TPG RIC key '{raw}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_aliases_and_preferred_priority_maps_are_normalized() {
        let mut dto = CfgTpg2200ActionDto::default();
        dto.ric = Some(0x0009_0D10);
        dto.callout_id_base = Some(0x21);
        dto.issi_priorities.insert("2632585".to_string(), 10);
        dto.tpg_issi_priorities.insert("2632585".to_string(), 14);
        dto.ric_priorities.insert("0x00090D10".to_string(), 11);

        let cfg = apply_tpg2200_action_patch(dto).unwrap();

        assert_eq!(cfg.tpg_ric, 0x0009_0D10);
        assert_eq!(cfg.incident_base, 2);
        assert_eq!(cfg.tpg_issi_priorities.get(&2632585), Some(&14));
        assert_eq!(cfg.tpg_ric_priorities.get(&0x0009_0D10), Some(&11));
    }

    #[test]
    fn invalid_priority_map_is_rejected() {
        let mut dto = CfgTpg2200ActionDto::default();
        dto.tpg_issi_priorities.insert("2632585".to_string(), 16);
        assert!(apply_tpg2200_action_patch(dto).is_err());
    }
}
