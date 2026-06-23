use serde::Deserialize;

use crate::bluestation::SecretField;

/// Token-protected HTTP ActionURL endpoint for triggering a Motorola TPG2200 Call-Out.
///
/// Intended for desk phones such as Snom: a function key calls
/// `/api/action/tpg2200?token=...`, FlowStation sends a configured Type-4 SDS to the configured
/// TPG2200 ISSI, and the incident number advances in memory after each successful trigger.
#[derive(Debug, Clone)]
pub struct CfgTpg2200Action {
    pub enabled: bool,
    pub token: SecretField,
    pub source_issi: u32,
    pub dest_issi: u32,
    pub incident_base: u16,
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
            incident_base: 1,
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
    #[serde(default = "default_incident_base")]
    pub incident_base: u16,
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
            incident_base: default_incident_base(),
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

fn default_text() -> String {
    "ALARM".to_string()
}

fn default_max_text_chars() -> usize {
    80
}

pub fn apply_tpg2200_action_patch(dto: CfgTpg2200ActionDto) -> Result<CfgTpg2200Action, String> {
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

    Ok(CfgTpg2200Action {
        enabled: dto.enabled,
        token: SecretField::from(dto.token),
        source_issi: dto.source_issi,
        dest_issi: dto.dest_issi,
        incident_base: dto.incident_base.clamp(1, 256),
        default_text: dto.default_text.trim().to_string(),
        max_text_chars: dto.max_text_chars.clamp(1, 240),
    })
}
