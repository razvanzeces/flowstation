//! Additional cells (`[[cells]]`) for multi-cell operation: one extra SDR per entry.
//!
//! The primary cell (id 0) is still described by the legacy `[cell_info]` + `[phy_io.soapysdr]`
//! sections, so single-cell configs are unchanged. Each `[[cells]]` entry adds one more cell:
//!
//! ```toml
//! [[cells]]
//! id = 1
//! enabled = true            # optional, default true
//!
//! [cells.cell_info]         # overrides merged on top of the primary [cell_info]
//! main_carrier = 1590
//! colour_code = 2
//!
//! [cells.soapysdr]          # full SDR section for this cell (same keys as [phy_io.soapysdr])
//! device = "driver=plutosdr,uri=ip:192.168.2.2"
//! tx_freq = 438150000
//! rx_freq = 433150000
//! ```

use serde::Deserialize;
use tetra_core::CellId;
use toml::Value;

use crate::bluestation::{CellInfoDto, CfgCellInfo, CfgSoapySdr, SoapySdrDto, cell_dto_to_cfg, soapy_dto_to_cfg, soapy_dto_unknown_keys};

/// `[cell_info]` keys that may not be overridden per cell. Neighbour lists are derived from the
/// sibling cells, and SDS remote control is a station-wide function owned by the primary cell.
const NON_OVERRIDABLE_CELL_KEYS: &[&str] = &["neighbor_cells_ca", "sds_command_control"];

/// One additional cell (SDR) of a multi-cell station.
#[derive(Debug, Clone)]
pub struct CfgExtraCell {
    pub id: CellId,
    /// Effective cell parameters: primary `[cell_info]` with this cell's overrides applied.
    pub cell: CfgCellInfo,
    /// SDR settings for this cell. Required when the PHY backend is SoapySdr.
    pub soapysdr: Option<CfgSoapySdr>,
    /// The `[networks]` entry this cell serves; `None` = the primary cell's network.
    pub network: Option<String>,
}

#[derive(Deserialize)]
struct ExtraCellDto {
    id: u8,
    enabled: Option<bool>,
    network: Option<String>,
    cell_info: Option<toml::Table>,
    soapysdr: Option<SoapySdrDto>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, Value>,
}

/// Parse the raw `cells` array. `primary_cell_info` is the primary `[cell_info]` table after the
/// separately-parsed sub-tables were removed; each cell's overrides are merged on top of it.
/// Disabled entries are dropped (their settings stay in the TOML for when they are re-enabled).
pub fn parse_extra_cells(raw_cells: Option<Value>, primary_cell_info: &toml::Table) -> Result<Vec<CfgExtraCell>, String> {
    let Some(raw_cells) = raw_cells else {
        return Ok(Vec::new());
    };
    let Value::Array(entries) = raw_cells else {
        return Err("cells must be an array of tables ([[cells]])".into());
    };

    let mut cells = Vec::with_capacity(entries.len());
    for (i, entry) in entries.into_iter().enumerate() {
        let dto: ExtraCellDto = entry.try_into().map_err(|e| format!("cells[{i}]: {e}"))?;
        if !dto.extra.is_empty() {
            let mut keys: Vec<&str> = dto.extra.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            return Err(format!("Unrecognized fields in cells[{i}]: {keys:?}"));
        }
        if !dto.enabled.unwrap_or(true) {
            continue;
        }

        let mut merged = primary_cell_info.clone();
        for (key, value) in dto.cell_info.unwrap_or_default() {
            if NON_OVERRIDABLE_CELL_KEYS.contains(&key.as_str()) {
                return Err(format!("cells[{i}].cell_info.{key} cannot be set per cell"));
            }
            merged.insert(key, value);
        }
        let cell_dto: CellInfoDto = Value::Table(merged).try_into().map_err(|e| format!("cells[{i}].cell_info: {e}"))?;
        if !cell_dto.extra.is_empty() {
            let mut keys: Vec<&str> = cell_dto.extra.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            return Err(format!("Unrecognized fields in cells[{i}].cell_info: {keys:?}"));
        }

        let soapysdr = match dto.soapysdr {
            Some(soapy) => {
                let unknown = soapy_dto_unknown_keys(&soapy);
                if !unknown.is_empty() {
                    return Err(format!("Unrecognized fields: cells[{i}].soapysdr::{unknown:?}"));
                }
                Some(soapy_dto_to_cfg(soapy))
            }
            None => None,
        };

        cells.push(CfgExtraCell {
            id: CellId(dto.id),
            cell: cell_dto_to_cfg(cell_dto),
            soapysdr,
            network: dto.network,
        });
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use crate::bluestation::parsing::from_toml_str;
    use tetra_core::CellId;

    const BASE: &str = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
colour_code = 1
"#;

    fn parse(extra: &str) -> Result<crate::bluestation::StackConfig, String> {
        from_toml_str(&format!("{BASE}{extra}")).map_err(|e| e.to_string())
    }

    #[test]
    fn single_cell_config_has_no_extra_cells() {
        let cfg = parse("").unwrap();
        assert!(cfg.extra_cells.is_empty());
        assert_eq!(cfg.cell_count(), 1);
        cfg.validate().unwrap();
    }

    #[test]
    fn extra_cell_inherits_and_overrides_primary() {
        let cfg = parse(
            r#"
[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
colour_code = 2
"#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.cell_count(), 2);
        let c = &cfg.extra_cells[0];
        assert_eq!(c.id, CellId(1));
        assert_eq!(c.cell.main_carrier, 1525);
        assert_eq!(c.cell.colour_code, 2);
        assert_eq!(c.cell.location_area, 1, "inherited from primary");
        let ids: Vec<_> = cfg.cells().iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, vec![CellId(0), CellId(1)]);
    }

    #[test]
    fn cells_without_network_link_are_not_site_linked() {
        let cfg = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(!cfg.is_site_linked());
        assert!(!cfg.for_extra_cell(CellId(1)).unwrap().is_site_linked());
        assert!(!parse("").unwrap().is_site_linked(), "single cell is never site-linked");
    }

    #[test]
    fn disabled_cell_is_dropped() {
        let cfg = parse("[[cells]]\nid = 1\nenabled = false\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(cfg.extra_cells.is_empty());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("[[cells]]\nid = 1\nbogus = 1\n").unwrap_err().contains("bogus"));
        assert!(
            parse("[[cells]]\nid = 1\n[cells.cell_info]\nbogus = 1\n")
                .unwrap_err()
                .contains("bogus")
        );
    }

    #[test]
    fn rejects_non_overridable_keys() {
        let err = parse("[[cells]]\nid = 1\n[cells.cell_info]\nsds_command_control = {}\n").unwrap_err();
        assert!(err.contains("cannot be set per cell"), "{err}");
    }

    #[test]
    fn validate_rejects_bad_cell_sets() {
        let dup_carrier = parse("[[cells]]\nid = 1\n").unwrap();
        assert!(dup_carrier.validate().unwrap_err().contains("carrier 1521"));

        let id_zero = parse("[[cells]]\nid = 0\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(id_zero.validate().unwrap_err().contains("reserved"));

        let dup_id =
            parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1530\n")
                .unwrap();
        assert!(dup_id.validate().unwrap_err().contains("duplicate id"));

        let out_of_range = parse("[[cells]]\nid = 8\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(out_of_range.validate().unwrap_err().contains("out of range"));

        let band = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\nfreq_band = 3\n").unwrap();
        assert!(band.validate().unwrap_err().contains("freq_band"));
    }

    #[test]
    fn for_extra_cell_isolates_radio_settings() {
        let cfg = parse(
            r#"
[brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"

[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
colour_code = 2
"#,
        )
        .unwrap();
        assert!(cfg.brew.is_some());

        let cell = cfg.for_extra_cell(CellId(1)).unwrap();
        assert_eq!(cell.cell.main_carrier, 1525);
        assert_eq!(cell.cell.colour_code, 2);
        assert!(cell.extra_cells.is_empty());
        assert!(cell.brew.is_some(), "kept so the cell's CMCE routes to the site switch");
        assert!(cfg.is_site_linked() && cell.is_site_linked(), "Brew + [[cells]] links the cells");
        assert!(cell.dashboard.is_none());
        assert!(!cell.recovery.enabled);
        assert_eq!(cell.cell_id, CellId(1));
        assert!(cell.is_multi_cell() && cfg.is_multi_cell());
        assert_eq!(cell.net.mcc, cfg.net.mcc);
        cell.validate().unwrap();

        assert!(cfg.for_extra_cell(CellId(2)).is_none());
    }

    #[test]
    fn sibling_cells_become_neighbours() {
        let mut cfg = parse(
            r#"
[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 0
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1525

[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
[[cells]]
id = 2
[cells.cell_info]
main_carrier = 1529
location_area = 2
"#,
        )
        .unwrap();
        cfg.add_sibling_neighbours();
        cfg.validate().unwrap();

        let carriers = |c: &crate::bluestation::CfgCellInfo| c.neighbor_cells_ca.iter().map(|n| n.main_carrier_number).collect::<Vec<_>>();
        assert_eq!(carriers(&cfg.cell), vec![1525, 1529], "configured 1525 kept, 1529 added");
        assert_eq!(cfg.cell.neighbor_cells_ca[1].cell_identifier_ca, 1, "first free id");
        assert_eq!(cfg.cell.neighbor_cells_ca[1].location_area, Some(2), "differing LA is advertised");
        assert_eq!(carriers(&cfg.extra_cells[0].cell), vec![1521, 1529]);
        assert_eq!(carriers(&cfg.extra_cells[1].cell), vec![1521, 1525]);
        assert!(cfg.cells().iter().all(|(_, c, _)| c.neighbor_cell_broadcast & 0b10 != 0));
        let added = &cfg.cell.neighbor_cells_ca[1];
        assert_eq!(added.cell_reselection_types_supported, 3, "announced and unannounced");
        assert_eq!(added.main_carrier_number_extension, None, "same band plan");
        assert_eq!(added.maximum_ms_transmit_power, None, "same max power");
        assert_eq!(cfg.cell.neighbor_cells_ca[0].cell_reselection_types_supported, 1, "configured entry untouched");
        assert_eq!(cfg.for_extra_cell(CellId(2)).unwrap().cell.neighbor_cells_ca.len(), 2);
    }

    #[test]
    fn single_cell_neighbours_are_untouched() {
        let mut cfg = parse("").unwrap();
        cfg.add_sibling_neighbours();
        assert!(cfg.cell.neighbor_cells_ca.is_empty());
        assert_eq!(cfg.cell.neighbor_cell_broadcast, 0);
    }

    #[test]
    fn sibling_with_another_band_plan_or_power_is_described_to_radios() {
        let mut cfg = parse(
            "[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\nfreq_offset = 12500\nms_txpwr_max_cell = 6\n",
        )
        .unwrap();
        cfg.add_sibling_neighbours();
        let n = &cfg.cell.neighbor_cells_ca[0];
        // band 4, offset +12.5 kHz (id 3), duplex spacing 4, not reversed.
        assert_eq!(n.main_carrier_number_extension, Some((4 << 6) | (3 << 4) | (4 << 1)));
        assert_eq!(n.maximum_ms_transmit_power, Some(6));
        let back = &cfg.extra_cells[0].cell.neighbor_cells_ca[0];
        assert_eq!(back.main_carrier_number, 1521);
        assert_eq!(back.main_carrier_number_extension, Some((4 << 6) | (0 << 4) | (4 << 1)), "primary's plan, offset 0");
        cfg.validate().unwrap();
    }

    #[test]
    fn cells_must_share_a_custom_duplex_spacing() {
        let cfg = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\ncustom_duplex_spacing = 10000000\n").unwrap();
        assert!(cfg.validate().unwrap_err().contains("custom_duplex_spacing"));
    }
}
