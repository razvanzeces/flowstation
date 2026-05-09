use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::{CellInfoDto, CfgControlDto, NetInfoDto, apply_control_patch, cell_dto_to_cfg, net_dto_to_cfg};
use crate::bluestation::sec_cell::CfgNeighborCellCa;

use super::config::{StackConfig, StackMode};
use super::sec_brew::{CfgBrewDto, apply_brew_patch};
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

    // Build cell config, then inject the separately-parsed neighbor cells
    let mut cell_cfg = cell_dto_to_cfg(root.cell_info);
    cell_cfg.neighbor_cells_ca = neighbor_cells_ca;

    // Build config from required and optional values
    let mut cfg = StackConfig {
        stack_mode: root.stack_mode,
        debug_log: root.debug_log,
        phy_io: phy_dto_to_cfg(root.phy_io),
        net: net_dto_to_cfg(root.net_info),
        cell: cell_cfg,
        brew: None,
        telemetry: None,
        control: None,
        security: apply_security_patch(root.security.unwrap_or_default()),
    };

    if let Some(brew) = root.brew {
        cfg.brew = Some(apply_brew_patch(brew));
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
    telemetry: Option<CfgTelemetryDto>,
    command: Option<CfgControlDto>,
    security: Option<CfgSecurityDto>,

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
        let toml = minimal_toml(r#"
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
"#);
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
}
