use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::sec_cell::{CfgNeighborCellCa, SdsCommandControlDto};
use crate::bluestation::{
    CellInfoDto, CfgControlDto, CfgIdentityDto, NetInfoDto, apply_control_patch, apply_identity_patch, cell_dto_to_cfg, net_dto_to_cfg,
};

use super::config::{StackConfig, StackMode};
use super::sec_brew::{CfgBrewDto, apply_brew_patch};
use super::sec_dashboard::{CfgDashboardDto, apply_dashboard_patch};
use super::sec_security::{CfgSecurityDto, apply_security_patch};
use super::sec_telemetry::{CfgTelemetryDto, apply_telemetry_patch};
use super::{PhyIoDto, phy_dto_to_cfg};

/// Build `StackConfig` from a TOML configuration file
pub fn from_toml_str(toml_str: &str) -> Result<StackConfig, Box<dyn std::error::Error>> {
    // Parse once as raw Value so we can extract neighbor_cells_ca before
    // deserializing into typed DTOs. This avoids a conflict between serde's
    // #[flatten] HashMap (used for unrecognised-field detection) and an array-of-
    // tables field: the flatten map would capture neighbor_cells_ca as an opaque
    // Value, causing the "unrecognised field" check to fire.
    let mut raw: toml::Table = toml::from_str(toml_str)?;

    // Extract neighbor_cells_ca from cell_info before typed deserialisation.
    let neighbor_cells_ca: Vec<CfgNeighborCellCa> = raw
        .get_mut("cell_info")
        .and_then(|ci| {
            if let Value::Table(t) = ci {
                t.remove("neighbor_cells_ca")
            } else {
                None
            }
        })
        .map(|v| {
            // v is a Value::Array of Value::Table — deserialise via serde
            v.try_into::<Vec<toml::Table>>()
                .map_err(|e| format!("cell_info.neighbor_cells_ca: {}", e))
                .and_then(|tables| {
                    tables
                        .into_iter()
                        .enumerate()
                        .map(|(i, t)| {
                            Value::Table(t)
                                .try_into::<CfgNeighborCellCa>()
                                .map_err(|e| format!("cell_info.neighbor_cells_ca[{}]: {}", i, e))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
        })
        .transpose()?
        .unwrap_or_default();

    if neighbor_cells_ca.len() > 7 {
        return Err("cell_info.neighbor_cells_ca: at most 7 entries allowed".into());
    }

    // Extract sds_command_control from cell_info before typed deserialisation
    // (same reason as neighbor_cells_ca: serde #[flatten] would capture it as opaque Value)
    let sds_command_control_raw = raw
        .get_mut("cell_info")
        .and_then(|ci| {
            if let Value::Table(t) = ci { t.remove("sds_command_control") } else { None }
        });

    // Now deserialise the (mutated) Value into the typed root — neighbor_cells_ca
    // has been removed so it will not appear in the flatten HashMap.
    let root: TomlConfigRoot = Value::Table(raw).try_into()?;

    // Various sanity checks
    let expected_config_version = "0.6";
    if !root.config_version.eq(expected_config_version) {
        return Err(format!(
            "Unrecognized config_version: {}, expect {}",
            root.config_version, expected_config_version
        )
        .into());
    }
    if !root.extra.is_empty() {
        return Err(format!("Unrecognized top-level fields: {:?}", sorted_keys(&root.extra)).into());
    }

    if !root.phy_io.extra.is_empty() {
        return Err(format!("Unrecognized fields: phy_io::{:?}", sorted_keys(&root.phy_io.extra)).into());
    }
    if let Some(ref soapy) = root.phy_io.soapysdr {
        let extra_keys = sorted_keys(&soapy.extra);
        let extra_keys_filtered = extra_keys
            .iter()
            .filter(|key| !(key.starts_with("rx_gain_") || key.starts_with("tx_gain_")))
            .collect::<Vec<&&str>>();
        if !extra_keys_filtered.is_empty() {
            return Err(format!("Unrecognized fields: phy_io.soapysdr::{:?}", extra_keys_filtered).into());
        }
        if let Some(ref autocal) = soapy.sx1255_autocal {
            if !autocal.extra.is_empty() {
                return Err(format!(
                    "Unrecognized fields: phy_io.soapysdr.sx1255_autocal::{:?}",
                    sorted_keys(&autocal.extra)
                )
                .into());
            }
        }
    }
    if !root.net_info.extra.is_empty() {
        return Err(format!("Unrecognized fields in net_info: {:?}", sorted_keys(&root.net_info.extra)).into());
    }
    if !root.cell_info.extra.is_empty() {
        return Err(format!("Unrecognized fields in cell_info: {:?}", sorted_keys(&root.cell_info.extra)).into());
    }

    // Optional brew section
    if let Some(ref brew) = root.brew {
        if !brew.extra.is_empty() {
            return Err(format!("Unrecognized fields in brew config: {:?}", sorted_keys(&brew.extra)).into());
        }
    }

    // Optional telemetry section
    if let Some(ref telemetry) = root.telemetry {
        if !telemetry.extra.is_empty() {
            return Err(format!("Unrecognized fields in telemetry config: {:?}", sorted_keys(&telemetry.extra)).into());
        }
    }

    // Optional identity section
    if let Some(ref identity) = root.identity {
        if !identity.extra.is_empty() {
            return Err(format!("Unrecognized fields in identity config: {:?}", sorted_keys(&identity.extra)).into());
        }
        for (idx, manual) in identity.manual.iter().enumerate() {
            if !manual.extra.is_empty() {
                return Err(format!("Unrecognized fields in identity.manual[{}]: {:?}", idx, sorted_keys(&manual.extra)).into());
            }
        }
        if let Some(ref radioid) = identity.radioid {
            if !radioid.extra.is_empty() {
                return Err(format!("Unrecognized fields in identity.radioid: {:?}", sorted_keys(&radioid.extra)).into());
            }
        }
    }

    // Build cell config, then inject separately-parsed nested sections.
    let mut cell_cfg = cell_dto_to_cfg(root.cell_info);
    cell_cfg.neighbor_cells_ca = neighbor_cells_ca;
    if let Some(v) = sds_command_control_raw {
        let dto = v.try_into::<SdsCommandControlDto>()
            .map_err(|e| format!("cell_info.sds_command_control: {}", e))?;
        if !dto.extra.is_empty() {
            return Err(format!("Unrecognized fields in cell_info.sds_command_control: {:?}",
                dto.extra.keys().collect::<Vec<_>>()).into());
        }
        use crate::bluestation::sec_cell::{CfgSdsCommandControl, CfgSdsCommandEntry};
        cell_cfg.sds_command_control = Some(CfgSdsCommandControl {
            authorized_issis: dto.authorized_issis,
            commands: dto.commands.into_iter().map(|e| CfgSdsCommandEntry {
                status_code: e.status_code,
                action: e.action,
            }).collect(),
        });
    }

    // Build config from required and optional values
    let mut cfg = StackConfig {
        stack_mode: root.stack_mode,
        debug_log: root.debug_log,
        phy_io: phy_dto_to_cfg(root.phy_io),
        net: net_dto_to_cfg(root.net_info),
        cell: cell_cfg,
        brew: None,
        dashboard: None,
        telemetry: None,
        control: None,
        security: apply_security_patch(root.security.unwrap_or_default()),
        identity: apply_identity_patch(root.identity.unwrap_or_default())?,
    };

    if let Some(brew) = root.brew {
        cfg.brew = Some(apply_brew_patch(brew));
    }

    if let Some(dashboard) = root.dashboard {
        cfg.dashboard = Some(apply_dashboard_patch(dashboard)?);
    }

    if let Some(telemetry) = root.telemetry {
        cfg.telemetry = Some(apply_telemetry_patch(telemetry)?);
    }

    if let Some(command) = root.command {
        cfg.control = Some(apply_control_patch(command)?);
    }

    Ok(cfg)
}

/// Build `SharedConfig` from any reader.
pub fn from_reader<R: Read>(reader: R) -> Result<StackConfig, Box<dyn std::error::Error>> {
    let mut contents = String::new();
    let mut reader = BufReader::new(reader);
    reader.read_to_string(&mut contents)?;
    from_toml_str(&contents)
}

/// Build `SharedConfig` from a file path.
pub fn from_file<P: AsRef<Path>>(path: P) -> Result<StackConfig, Box<dyn std::error::Error>> {
    let f = File::open(path)?;
    let r = BufReader::new(f);
    let cfg = from_reader(r)?;
    Ok(cfg)
}

fn sorted_keys(map: &HashMap<String, Value>) -> Vec<&str> {
    let mut v: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
    v.sort_unstable();
    v
}

/// ----------------------- DTOs for input shape -----------------------

#[derive(Deserialize)]
struct TomlConfigRoot {
    config_version: String,
    stack_mode: StackMode,
    debug_log: Option<String>,

    phy_io: PhyIoDto,
    net_info: NetInfoDto,
    cell_info: CellInfoDto,

    brew: Option<CfgBrewDto>,
    dashboard: Option<CfgDashboardDto>,
    telemetry: Option<CfgTelemetryDto>,
    command: Option<CfgControlDto>,
    security: Option<CfgSecurityDto>,
    identity: Option<CfgIdentityDto>,

    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml(extra_cell: &str) -> String {
        format!(
            r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
{}
"#,
            extra_cell
        )
    }

    #[test]
    fn test_no_neighbor_cells() {
        let toml = minimal_toml("");
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.neighbor_cells_ca.len(), 0);
    }

    #[test]
    fn test_two_neighbor_cells() {
        let toml = minimal_toml(
            r#"
neighbor_cell_broadcast = 2

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 0
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1585
mcc = 901
mnc = 9999
location_area = 1

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 2
cell_reselection_types_supported = 0
neighbor_cell_synchronized = false
cell_load_ca = 1
main_carrier_number = 1586
"#,
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.neighbor_cells_ca.len(), 2);
        assert_eq!(cfg.cell.neighbor_cells_ca[0].cell_identifier_ca, 1);
        assert_eq!(cfg.cell.neighbor_cells_ca[0].main_carrier_number, 1585);
        assert_eq!(cfg.cell.neighbor_cells_ca[1].cell_identifier_ca, 2);
        assert_eq!(cfg.cell.neighbor_cells_ca[1].cell_load_ca, 1);
        assert_eq!(cfg.cell.neighbor_cell_broadcast, 2);
    }

    #[test]
    fn test_too_many_neighbor_cells_rejected() {
        // 8 entries — should fail validation
        let entries: String = (1u8..=8)
            .map(|i| format!(
                "\n[[cell_info.neighbor_cells_ca]]\ncell_identifier_ca = {}\ncell_reselection_types_supported = 0\nneighbor_cell_synchronized = false\ncell_load_ca = 0\nmain_carrier_number = {}\n",
                i, 1584 + i as u16
            ))
            .collect();
        let toml = minimal_toml(&entries);
        assert!(from_toml_str(&toml).is_err(), "should reject >7 neighbours");
    }

    #[test]
    fn test_unrecognized_cell_info_field_still_rejected() {
        let toml = minimal_toml("bogus_field = 42");
        assert!(from_toml_str(&toml).is_err(), "should reject unknown field");
    }

    #[test]
    fn test_example_config_file_parses() {
        let toml = include_str!("../../../../example_config/config.toml");
        from_toml_str(toml).expect("example_config/config.toml should parse");
    }

    #[test]
    fn test_identity_config_parses_manual_and_radioid() {
        let toml = format!(
            "{}\n{}",
            minimal_toml(""),
            r#"
[identity]
enabled = true
cache_ttl_secs = 60
cache_max_entries = 128

[[identity.manual]]
ssi = 2260571
mnemonic = "YO6RZV"
label = "Razvan"

[identity.radioid]
enabled = true
timeout_secs = 2
min_lookup_interval_ms = 500
user_agent = "FlowStation test"
api_token = "secret"
"#
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert!(cfg.identity.enabled);
        assert_eq!(cfg.identity.cache_ttl_secs, 60);
        assert_eq!(cfg.identity.cache_max_entries, 128);
        assert_eq!(cfg.identity.manual[0].ssi, 2260571);
        assert_eq!(cfg.identity.manual[0].mnemonic.as_deref(), Some("YO6RZV"));
        assert!(cfg.identity.radioid.enabled);
        assert_eq!(cfg.identity.radioid.min_lookup_interval_ms, 500);
        assert!(cfg.identity.radioid.api_token.is_some());
    }

    #[test]
    fn test_identity_rejects_unknown_nested_fields() {
        let toml = format!(
            "{}\n{}",
            minimal_toml(""),
            r#"
[identity]
enabled = true

[identity.radioid]
enabled = false
bogus = "nope"
"#
        );
        assert!(from_toml_str(&toml).is_err(), "should reject unknown identity.radioid field");
    }

    #[test]
    fn test_sx1255_autocal_config_parses() {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "SoapySdr"

[phy_io.soapysdr]
rx_freq = 431362500
tx_freq = 438362500

[phy_io.soapysdr.sx1255_autocal]
enabled = true
interval_secs = 1800
allow_periodic_temperature_read = true
temperature_sensor = "temperature"
min_temperature_delta_c = 3.5
reference_temperature_c = 25.0
temp_ppm_per_c = 0.12
allow_periodic_retune = true
rf_loopback_startup_check = false
rf_filter_profile = "tetra_clean"
rf_loopback_startup_calibration = true
rf_loopback_tone_hz = 25000.0
rf_loopback_tone_amplitude = 0.75
rf_loopback_settle_blocks = 12
rf_loopback_capture_blocks = 16
rf_loopback_min_snr_db = 24.0
rf_loopback_max_image_coeff = 0.25
rf_loopback_max_dc = 0.2
rf_loopback_apply_dc = true
rf_loopback_apply_iq = false

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1534
freq_band = 4
freq_offset = 12500
duplex_spacing = 1
reverse_operation = false
location_area = 1
"#;
        let cfg = from_toml_str(toml).expect("parse failed");
        let autocal = &cfg.phy_io.soapysdr.as_ref().expect("soapy config").sx1255_autocal;
        assert!(autocal.enabled);
        assert_eq!(autocal.interval_secs, 1800);
        assert!(autocal.allow_periodic_temperature_read);
        assert_eq!(autocal.temperature_sensor.as_deref(), Some("temperature"));
        assert_eq!(autocal.min_temperature_delta_c, 3.5);
        assert_eq!(autocal.reference_temperature_c, Some(25.0));
        assert_eq!(autocal.temp_ppm_per_c, 0.12);
        assert!(autocal.allow_periodic_retune);
        assert!(!autocal.rf_loopback_startup_check);
        assert_eq!(autocal.rf_filter_profile, "TETRA_CLEAN");
        assert!(autocal.rf_loopback_startup_calibration);
        assert_eq!(autocal.rf_loopback_tone_hz, 25000.0);
        assert_eq!(autocal.rf_loopback_tone_amplitude, 0.75);
        assert_eq!(autocal.rf_loopback_settle_blocks, 12);
        assert_eq!(autocal.rf_loopback_capture_blocks, 16);
        assert_eq!(autocal.rf_loopback_min_snr_db, 24.0);
        assert_eq!(autocal.rf_loopback_max_image_coeff, 0.25);
        assert_eq!(autocal.rf_loopback_max_dc, 0.2);
        assert!(autocal.rf_loopback_apply_dc);
        assert!(!autocal.rf_loopback_apply_iq);
    }

    #[test]
    fn test_sx1255_autocal_rejects_unknown_nested_fields() {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "SoapySdr"

[phy_io.soapysdr]
rx_freq = 431362500
tx_freq = 438362500

[phy_io.soapysdr.sx1255_autocal]
enabled = true
bogus = "nope"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1534
freq_band = 4
freq_offset = 12500
duplex_spacing = 1
reverse_operation = false
location_area = 1
"#;
        assert!(from_toml_str(toml).is_err(), "should reject unknown sx1255_autocal field");
    }
}
