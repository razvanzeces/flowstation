use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use toml::Value;

/// Geo-fence alarm configuration.
///
/// Disabled by default. When enabled, FlowStation watches decoded TETRA LIP positions and
/// MeshCom position packets. Devices entering the configured radius around the station can be
/// forwarded to the existing TPG2200, SDS, Snom/SIP and Telegram paths.
#[derive(Debug, Clone)]
pub struct CfgGeoalarm {
    pub enabled: bool,
    pub flowstation_lat: f64,
    pub flowstation_lon: f64,
    pub radius_m: f64,
    pub cooldown_secs: u64,
    pub trigger_tetra: bool,
    pub trigger_meshcom: bool,
    pub forward_tpg2200: bool,
    pub forward_sds: bool,
    pub forward_sip: bool,
    pub forward_telegram: bool,
    pub tetra_issi_whitelist: BTreeSet<u32>,
    pub tetra_issi_blacklist: BTreeSet<u32>,
    pub meshcom_source_whitelist: BTreeSet<String>,
    pub meshcom_source_blacklist: BTreeSet<String>,
    pub sds_source_issi: u32,
    pub sds_dest_issi: u32,
    pub sds_dest_is_group: bool,
    pub tpg2200_source_issi: u32,
    pub tpg2200_dest_issi: u32,
    pub tpg2200_ric: u32,
    pub tpg2200_incident_base: u16,
    pub tpg2200_priority: u8,
    pub tpg2200_issi_priorities: BTreeMap<u32, u8>,
    pub tpg2200_ric_priorities: BTreeMap<u32, u8>,
    pub tpg2200_text_prefix: String,
    pub tpg2200_max_text_chars: usize,
    pub sip_title_prefix: String,
    pub telegram_prefix: String,
}

impl Default for CfgGeoalarm {
    fn default() -> Self {
        Self {
            enabled: false,
            flowstation_lat: 0.0,
            flowstation_lon: 0.0,
            radius_m: 500.0,
            cooldown_secs: 300,
            trigger_tetra: true,
            trigger_meshcom: true,
            forward_tpg2200: false,
            forward_sds: false,
            forward_sip: false,
            forward_telegram: false,
            tetra_issi_whitelist: BTreeSet::new(),
            tetra_issi_blacklist: BTreeSet::new(),
            meshcom_source_whitelist: BTreeSet::new(),
            meshcom_source_blacklist: BTreeSet::new(),
            sds_source_issi: default_source_issi(),
            sds_dest_issi: 0,
            sds_dest_is_group: false,
            tpg2200_source_issi: default_source_issi(),
            tpg2200_dest_issi: 0,
            tpg2200_ric: default_tpg2200_ric(),
            tpg2200_incident_base: default_tpg2200_callout_id_base(),
            tpg2200_priority: default_tpg2200_priority(),
            tpg2200_issi_priorities: BTreeMap::new(),
            tpg2200_ric_priorities: BTreeMap::new(),
            tpg2200_text_prefix: default_geoalarm_prefix(),
            tpg2200_max_text_chars: 80,
            sip_title_prefix: default_geoalarm_prefix(),
            telegram_prefix: default_geoalarm_prefix(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgGeoalarmDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub flowstation_lat: f64,
    #[serde(default)]
    pub flowstation_lon: f64,
    #[serde(default = "default_radius_m")]
    pub radius_m: f64,
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    #[serde(default = "default_true")]
    pub trigger_tetra: bool,
    #[serde(default = "default_true")]
    pub trigger_meshcom: bool,
    #[serde(default)]
    pub forward_tpg2200: bool,
    #[serde(default)]
    pub forward_sds: bool,
    #[serde(default)]
    pub forward_sip: bool,
    #[serde(default)]
    pub forward_telegram: bool,
    #[serde(default)]
    pub tetra_issi_whitelist: Vec<u32>,
    #[serde(default)]
    pub tetra_issi_blacklist: Vec<u32>,
    #[serde(default)]
    pub meshcom_source_whitelist: Vec<String>,
    #[serde(default)]
    pub meshcom_source_blacklist: Vec<String>,
    #[serde(default = "default_source_issi")]
    pub sds_source_issi: u32,
    #[serde(default)]
    pub sds_dest_issi: u32,
    #[serde(default)]
    pub sds_dest_is_group: bool,
    #[serde(default = "default_source_issi")]
    pub tpg2200_source_issi: u32,
    #[serde(default)]
    pub tpg2200_dest_issi: u32,
    #[serde(default = "default_tpg2200_ric")]
    pub tpg2200_ric: u32,
    #[serde(default)]
    pub tpg2200_callout_id_base: Option<u16>,
    #[serde(default)]
    pub tpg2200_incident_base: Option<u16>,
    #[serde(default = "default_tpg2200_priority")]
    pub tpg2200_priority: u8,
    #[serde(default)]
    pub tpg2200_issi_priorities: HashMap<String, u8>,
    #[serde(default)]
    pub tpg2200_ric_priorities: HashMap<String, u8>,
    #[serde(default = "default_geoalarm_prefix")]
    pub tpg2200_text_prefix: String,
    #[serde(default = "default_tpg2200_max_text_chars")]
    pub tpg2200_max_text_chars: usize,
    #[serde(default = "default_geoalarm_prefix")]
    pub sip_title_prefix: String,
    #[serde(default = "default_geoalarm_prefix")]
    pub telegram_prefix: String,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgGeoalarmDto {
    fn default() -> Self {
        Self {
            enabled: false,
            flowstation_lat: 0.0,
            flowstation_lon: 0.0,
            radius_m: default_radius_m(),
            cooldown_secs: default_cooldown_secs(),
            trigger_tetra: true,
            trigger_meshcom: true,
            forward_tpg2200: false,
            forward_sds: false,
            forward_sip: false,
            forward_telegram: false,
            tetra_issi_whitelist: Vec::new(),
            tetra_issi_blacklist: Vec::new(),
            meshcom_source_whitelist: Vec::new(),
            meshcom_source_blacklist: Vec::new(),
            sds_source_issi: default_source_issi(),
            sds_dest_issi: 0,
            sds_dest_is_group: false,
            tpg2200_source_issi: default_source_issi(),
            tpg2200_dest_issi: 0,
            tpg2200_ric: default_tpg2200_ric(),
            tpg2200_callout_id_base: None,
            tpg2200_incident_base: None,
            tpg2200_priority: default_tpg2200_priority(),
            tpg2200_issi_priorities: HashMap::new(),
            tpg2200_ric_priorities: HashMap::new(),
            tpg2200_text_prefix: default_geoalarm_prefix(),
            tpg2200_max_text_chars: default_tpg2200_max_text_chars(),
            sip_title_prefix: default_geoalarm_prefix(),
            telegram_prefix: default_geoalarm_prefix(),
            extra: HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_radius_m() -> f64 {
    500.0
}

fn default_cooldown_secs() -> u64 {
    300
}

fn default_source_issi() -> u32 {
    9999
}

fn default_tpg2200_callout_id_base() -> u16 {
    0x21
}

fn default_tpg2200_ric() -> u32 {
    0x0009_0D10
}

fn default_tpg2200_priority() -> u8 {
    15
}

fn legacy_incident_selector(incident: u16) -> u16 {
    let incident = incident.clamp(1, 256);
    let zero_based = incident - 1;
    let major = ((zero_based + 1) & 0x0F) as u16;
    let minor = (((zero_based / 16) + 1) & 0x0F) as u16;
    (major << 4) | minor
}

fn select_tpg2200_callout_id_base(src: &CfgGeoalarmDto) -> u16 {
    src.tpg2200_callout_id_base
        .map(|id| id.min(255))
        .or_else(|| src.tpg2200_incident_base.map(legacy_incident_selector))
        .unwrap_or_else(default_tpg2200_callout_id_base)
}

fn default_tpg2200_max_text_chars() -> usize {
    80
}

fn default_geoalarm_prefix() -> String {
    "GeoAlarm".to_string()
}

pub fn apply_geoalarm_patch(src: CfgGeoalarmDto) -> Result<CfgGeoalarm, String> {
    let tpg2200_callout_id_base = select_tpg2200_callout_id_base(&src);
    if src.enabled {
        validate_lat_lon(src.flowstation_lat, src.flowstation_lon)?;
        if !src.radius_m.is_finite() || src.radius_m <= 0.0 {
            return Err("geoalarm: radius_m must be greater than 0".to_string());
        }
        if src.forward_sds {
            validate_issi("geoalarm: sds_source_issi", src.sds_source_issi, true)?;
            validate_issi("geoalarm: sds_dest_issi", src.sds_dest_issi, false)?;
        }
        if src.forward_tpg2200 {
            validate_issi("geoalarm: tpg2200_source_issi", src.tpg2200_source_issi, true)?;
            validate_issi("geoalarm: tpg2200_dest_issi", src.tpg2200_dest_issi, false)?;
        }
    }

    let tetra_issi_whitelist = normalize_issi_set(src.tetra_issi_whitelist)?;
    let tetra_issi_blacklist = normalize_issi_set(src.tetra_issi_blacklist)?;
    let tpg2200_issi_priorities = normalize_issi_priority_map("geoalarm.tpg2200_issi_priorities", src.tpg2200_issi_priorities)?;
    let tpg2200_ric_priorities = normalize_ric_priority_map("geoalarm.tpg2200_ric_priorities", src.tpg2200_ric_priorities)?;

    Ok(CfgGeoalarm {
        enabled: src.enabled,
        flowstation_lat: src.flowstation_lat,
        flowstation_lon: src.flowstation_lon,
        radius_m: if src.radius_m.is_finite() && src.radius_m > 0.0 {
            src.radius_m
        } else {
            default_radius_m()
        },
        cooldown_secs: src.cooldown_secs.clamp(1, 86_400),
        trigger_tetra: src.trigger_tetra,
        trigger_meshcom: src.trigger_meshcom,
        forward_tpg2200: src.forward_tpg2200,
        forward_sds: src.forward_sds,
        forward_sip: src.forward_sip,
        forward_telegram: src.forward_telegram,
        tetra_issi_whitelist,
        tetra_issi_blacklist,
        meshcom_source_whitelist: normalize_source_list(src.meshcom_source_whitelist),
        meshcom_source_blacklist: normalize_source_list(src.meshcom_source_blacklist),
        sds_source_issi: src.sds_source_issi.max(1),
        sds_dest_issi: src.sds_dest_issi,
        sds_dest_is_group: src.sds_dest_is_group,
        tpg2200_source_issi: src.tpg2200_source_issi.max(1),
        tpg2200_dest_issi: src.tpg2200_dest_issi,
        tpg2200_ric: src.tpg2200_ric,
        tpg2200_incident_base: tpg2200_callout_id_base,
        tpg2200_priority: src.tpg2200_priority.min(15),
        tpg2200_issi_priorities,
        tpg2200_ric_priorities,
        tpg2200_text_prefix: non_empty_or(src.tpg2200_text_prefix, "GeoAlarm"),
        tpg2200_max_text_chars: src.tpg2200_max_text_chars.clamp(8, 160),
        sip_title_prefix: non_empty_or(src.sip_title_prefix, "GeoAlarm"),
        telegram_prefix: non_empty_or(src.telegram_prefix, "GeoAlarm"),
    })
}

fn validate_lat_lon(lat: f64, lon: f64) -> Result<(), String> {
    if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return Err("geoalarm: flowstation_lat must be -90..=90".to_string());
    }
    if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
        return Err("geoalarm: flowstation_lon must be -180..=180".to_string());
    }
    Ok(())
}

fn validate_issi(label: &str, value: u32, nonzero: bool) -> Result<(), String> {
    if nonzero && value == 0 {
        return Err(format!("{label} must be non-zero"));
    }
    if value > 16_777_215 {
        return Err(format!("{label} must be <= 16777215"));
    }
    Ok(())
}

fn normalize_issi_set(values: Vec<u32>) -> Result<BTreeSet<u32>, String> {
    let mut out = BTreeSet::new();
    for value in values {
        validate_issi("geoalarm: ISSI list entry", value, false)?;
        out.insert(value);
    }
    Ok(out)
}

fn normalize_issi_priority_map(field: &str, values: HashMap<String, u8>) -> Result<BTreeMap<u32, u8>, String> {
    let mut out = BTreeMap::new();
    for (raw_issi, priority) in values {
        let issi = raw_issi
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("{field}: invalid ISSI '{raw_issi}'"))?;
        validate_issi(field, issi, false)?;
        if priority > 15 {
            return Err(format!("{field}: priority for ISSI {issi} must be 0..=15"));
        }
        out.insert(issi, priority);
    }
    Ok(out)
}

fn normalize_ric_priority_map(field: &str, values: HashMap<String, u8>) -> Result<BTreeMap<u32, u8>, String> {
    let mut out = BTreeMap::new();
    for (raw_ric, priority) in values {
        let ric = parse_ric_key(&raw_ric)?;
        if priority > 15 {
            return Err(format!("{field}: priority for TPG RIC {raw_ric} must be 0..=15"));
        }
        out.insert(ric, priority);
    }
    Ok(out)
}

fn parse_ric_key(raw: &str) -> Result<u32, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Err("empty TPG RIC key".to_string());
    }
    if let Some(hex) = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("invalid hex TPG RIC key '{raw}'"));
    }
    key.parse::<u32>().map_err(|_| format!("invalid decimal TPG RIC key '{raw}'"))
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

fn non_empty_or(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_incident_and_direct_id_are_both_supported() {
        let mut legacy = CfgGeoalarmDto::default();
        legacy.tpg2200_incident_base = Some(2);
        assert_eq!(apply_geoalarm_patch(legacy).unwrap().tpg2200_incident_base, 0x21);

        let mut direct = CfgGeoalarmDto::default();
        direct.tpg2200_callout_id_base = Some(0);
        assert_eq!(apply_geoalarm_patch(direct).unwrap().tpg2200_incident_base, 0);
    }
}
