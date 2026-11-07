use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::sec_cell::{CfgNeighborCellCa, SdsCommandControlDto};
use crate::bluestation::{CellInfoDto, CfgControlDto, NetInfoDto, apply_control_patch, cell_dto_to_cfg, net_dto_to_cfg};

use super::config::{StackConfig, StackMode};
use super::sec_aprs_wx::{CfgAprsWxDto, apply_aprs_wx_patch};
use super::sec_asterisk::{CfgAsteriskDto, apply_asterisk_patch};
use super::sec_brew::{CfgBrewDto, apply_brew_patch};
use super::sec_cells::parse_extra_cells;
use super::sec_dapnet::{CfgDapnetDto, apply_dapnet_patch};
use super::sec_dashboard::{CfgDashboardDto, apply_dashboard_patch};
use super::sec_emergency::{CfgEmergencyDto, apply_emergency_patch};
use super::sec_geoalarm::{CfgGeoalarmDto, apply_geoalarm_patch};
use super::sec_health::{CfgHealthDto, apply_health_patch};
use super::sec_lst_dispatch::{CfgLstDispatchDto, apply_lst_dispatch_patch};
use super::sec_multi_network::{CfgMultiNetworkDto, apply_multi_network_patch};
use super::sec_networks::{CfgNetwork, DEFAULT_NETWORK, parse_networks};
use super::sec_recovery::{CfgRecoveryDto, apply_recovery_patch};
use super::sec_security::{CfgSecurityDto, apply_security_patch};
use super::sec_snom_notify::{CfgSnomNotifyDto, apply_snom_notify_patch};
use super::sec_telegram::{CfgTelegramDto, apply_telegram_patch};
use super::sec_telemetry::{CfgTelemetryDto, apply_telemetry_patch};
use super::sec_tpg2200_action::{CfgTpg2200ActionDto, apply_tpg2200_action_patch};
use super::sec_wx::{CfgWxServiceDto, apply_wx_service_patch};
use super::{PhyIoDto, phy_dto_to_cfg};

/// A `neighbor_cells_ca` array of tables (`what` names it in errors).
pub(crate) fn parse_neighbour_list(v: Value, what: &str) -> Result<Vec<CfgNeighborCellCa>, String> {
    // v is a Value::Array of Value::Table — deserialise via serde
    let tables = v.try_into::<Vec<toml::Table>>().map_err(|e| format!("{what}: {e}"))?;
    tables
        .into_iter()
        .enumerate()
        .map(|(i, t)| Value::Table(t).try_into::<CfgNeighborCellCa>().map_err(|e| format!("{what}[{i}]: {e}")))
        .collect()
}

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
        .map(|v| parse_neighbour_list(v, "cell_info.neighbor_cells_ca"))
        .transpose()?
        .unwrap_or_default();

    if neighbor_cells_ca.len() > 7 {
        return Err("cell_info.neighbor_cells_ca: at most 7 entries allowed".into());
    }

    // Extract sds_command_control from cell_info before typed deserialisation
    // (same reason as neighbor_cells_ca: serde #[flatten] would capture it as opaque Value)
    let sds_command_control_raw = raw.get_mut("cell_info").and_then(|ci| {
        if let Value::Table(t) = ci {
            t.remove("sds_command_control")
        } else {
            None
        }
    });

    // Extract [[cells]] (additional cells for multi-cell operation). Each entry's cell_info
    // overrides are merged on top of the primary [cell_info] as it stands now, i.e. without the
    // neighbour list and SDS command control that were removed above.
    let raw_cells = raw.remove("cells");
    let primary_cell_info_raw = match raw.get("cell_info") {
        Some(Value::Table(t)) => t.clone(),
        _ => toml::Table::new(),
    };
    let extra_cells = parse_extra_cells(raw_cells, &primary_cell_info_raw)?;
    // `[networks]` is a table of free-form names, parsed by hand like the cells.
    let raw_networks = raw.remove("networks");

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
    if let Some(ref brew) = root.brew
        && !brew.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in brew config: {:?}", sorted_keys(&brew.extra)).into());
    }

    // Optional lst_dispatch section
    if let Some(ref lst) = root.lst_dispatch
        && !lst.extra.is_empty()
    {
        return Err(format!(
            "Unrecognized fields in lst_dispatch config: {:?}",
            sorted_keys(&lst.extra)
        )
        .into());
    }

    // Optional asterisk section
    if let Some(ref asterisk) = root.asterisk
        && !asterisk.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in asterisk config: {:?}", sorted_keys(&asterisk.extra)).into());
    }

    // Optional dapnet section
    if let Some(ref dapnet) = root.dapnet
        && !dapnet.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in dapnet config: {:?}", sorted_keys(&dapnet.extra)).into());
    }

    // Optional geoalarm section
    if let Some(ref geoalarm) = root.geoalarm
        && !geoalarm.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in geoalarm config: {:?}", sorted_keys(&geoalarm.extra)).into());
    }

    // Optional tpg2200_action section
    if let Some(ref action) = root.tpg2200_action
        && !action.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in tpg2200_action config: {:?}", sorted_keys(&action.extra)).into());
    }

    // Optional snom_notify section
    if let Some(ref snom) = root.snom_notify
        && !snom.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in snom_notify config: {:?}", sorted_keys(&snom.extra)).into());
    }

    // Optional telemetry section
    if let Some(ref telemetry) = root.telemetry
        && !telemetry.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in telemetry config: {:?}", sorted_keys(&telemetry.extra)).into());
    }

    // Optional telegram_alerts section
    if let Some(ref telegram) = root.telegram_alerts
        && !telegram.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in telegram_alerts config: {:?}", sorted_keys(&telegram.extra)).into());
    }

    // Optional recovery section — reject typos so the RF-affecting feature can't be left dormant.
    if let Some(ref recovery) = root.recovery
        && !recovery.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in recovery config: {:?}", sorted_keys(&recovery.extra)).into());
    }

    if let Some(ref mn) = root.multi_network
        && !mn.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in multi_network config: {:?}", sorted_keys(&mn.extra)).into());
    }

    // Optional health section — reject typos so a mis-spelled watchdog/threshold key is caught.
    if let Some(ref health) = root.health
        && !health.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in health config: {:?}", sorted_keys(&health.extra)).into());
    }

    // Optional emergency section — reject typos so a mis-spelled toggle isn't silently ignored.
    if let Some(ref emergency) = root.emergency
        && !emergency.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in emergency config: {:?}", sorted_keys(&emergency.extra)).into());
    }

    // Build cell config, then inject the separately-parsed neighbor cells and sds_command_control
    let mut cell_cfg = cell_dto_to_cfg(root.cell_info);
    cell_cfg.neighbor_cells_ca = neighbor_cells_ca;
    if let Some(v) = sds_command_control_raw {
        let dto = v
            .try_into::<SdsCommandControlDto>()
            .map_err(|e| format!("cell_info.sds_command_control: {}", e))?;
        if !dto.extra.is_empty() {
            return Err(format!(
                "Unrecognized fields in cell_info.sds_command_control: {:?}",
                dto.extra.keys().collect::<Vec<_>>()
            )
            .into());
        }
        use crate::bluestation::sec_cell::{CfgSdsCommandControl, CfgSdsCommandEntry};
        cell_cfg.sds_command_control = Some(CfgSdsCommandControl {
            authorized_issis: dto.authorized_issis,
            commands: dto
                .commands
                .into_iter()
                .map(|e| CfgSdsCommandEntry {
                    status_code: e.status_code,
                    action: e.action,
                })
                .collect(),
        });
    }

    // Build config from required and optional values
    let mut cfg = StackConfig {
        stack_mode: root.stack_mode,
        debug_log: root.debug_log,
        service_name: root.service_name,
        phy_io: phy_dto_to_cfg(root.phy_io),
        net: net_dto_to_cfg(root.net_info),
        cell: cell_cfg,
        extra_cells,
        networks: Vec::new(),
        networks_explicit: false,
        network: DEFAULT_NETWORK.to_owned(),
        site_linked: false,
        station_carriers: Vec::new(),
        cell_id: tetra_core::CellId::PRIMARY,
        brew: None,
        lst_dispatch: None,
        asterisk: apply_asterisk_patch(root.asterisk.unwrap_or_default())?,
        dapnet: apply_dapnet_patch(root.dapnet.unwrap_or_default())?,
        geoalarm: apply_geoalarm_patch(root.geoalarm.unwrap_or_default())?,
        tpg2200_action: apply_tpg2200_action_patch(root.tpg2200_action.unwrap_or_default())?,
        snom_notify: apply_snom_notify_patch(root.snom_notify.unwrap_or_default())?,
        dashboard: None,
        telemetry: None,
        control: None,
        security: apply_security_patch(root.security.unwrap_or_default())?,
        wx_service: apply_wx_service_patch(root.wx_service.unwrap_or_default()),
        aprs_wx: apply_aprs_wx_patch(root.aprs_wx.unwrap_or_default()),
        recovery: apply_recovery_patch(root.recovery.unwrap_or_default()),
        telegram: None,
        health: apply_health_patch(root.health.unwrap_or_default()),
        emergency: apply_emergency_patch(root.emergency.unwrap_or_default()),
        multi_network: apply_multi_network_patch(root.multi_network.unwrap_or_default()),
    };

    if let Some(brew) = root.brew {
        cfg.brew = Some(apply_brew_patch(brew));
    }

    if let Some(lst) = root.lst_dispatch {
        cfg.lst_dispatch = Some(apply_lst_dispatch_patch(lst));
    }

    resolve_networks(&mut cfg, raw_networks, root.network)?;

    if let Some(dashboard) = root.dashboard {
        cfg.dashboard = Some(apply_dashboard_patch(dashboard)?);
    }

    if let Some(telemetry) = root.telemetry {
        cfg.telemetry = Some(apply_telemetry_patch(telemetry)?);
    }

    if let Some(command) = root.command {
        cfg.control = Some(apply_control_patch(command)?);
    }

    if let Some(telegram) = root.telegram_alerts {
        cfg.telegram = Some(apply_telegram_patch(telegram));
    }

    Ok(cfg)
}

/// Fill `networks` / `network` and bind every cell. With a `[networks]` section the primary
/// cell takes its network's identity and core; `[brew]` and `[lst_dispatch]` then belong in the
/// networks and are refused at the root. Without one, `[net_info]` + `[brew]` form the single
/// implicit network and cells may not name one.
fn resolve_networks(cfg: &mut StackConfig, raw_networks: Option<Value>, primary_network: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let networks = parse_networks(raw_networks, &cfg.net)?;
    if networks.is_empty() {
        if let Some(name) = primary_network {
            return Err(format!("network = {name:?} needs a [networks.{name}] section").into());
        }
        if let Some(c) = cfg.extra_cells.iter().find(|c| c.network.is_some()) {
            return Err(format!("{}: network = {:?} needs a [networks] section", c.id, c.network.as_deref().unwrap_or("")).into());
        }
        cfg.networks = vec![CfgNetwork {
            name: DEFAULT_NETWORK.to_owned(),
            net: cfg.net.clone(),
            brew: cfg.brew.clone(),
            via: None,
        }];
        cfg.network = DEFAULT_NETWORK.to_owned();
        return Ok(());
    }

    if cfg.brew.is_some() {
        return Err("[brew] cannot be used together with [networks]: put it under the network it serves ([networks.<name>.brew])".into());
    }
    if cfg.lst_dispatch.as_ref().is_some_and(|l| l.enabled) && networks.iter().any(|n| n.brew.is_some()) {
        return Err("lst_dispatch.enabled cannot be used together with a [networks.<name>.brew] link".into());
    }
    let Some(primary_name) = primary_network else {
        return Err("[networks] needs `network = \"<name>\"` at the top of the file: the primary cell's network".into());
    };
    let primary = networks
        .iter()
        .find(|n| n.name == primary_name)
        .ok_or_else(|| format!("network = {primary_name:?} is not a [networks] entry"))?;
    for c in &cfg.extra_cells {
        if let Some(name) = &c.network
            && !networks.iter().any(|n| &n.name == name)
        {
            return Err(format!("{}: network = {name:?} is not a [networks] entry", c.id).into());
        }
    }
    cfg.network = primary.name.clone();
    cfg.net = primary.net.clone();
    cfg.brew = primary.brew.clone();
    cfg.networks = networks;
    cfg.networks_explicit = true;
    for n in &cfg.networks {
        if cfg.network_cells(&n.name).is_empty() {
            return Err(format!("networks.{}: no cell serves this network (bind one with network = {:?})", n.name, n.name).into());
        }
    }
    Ok(())
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
    #[serde(default)]
    service_name: Option<String>,

    phy_io: PhyIoDto,
    net_info: NetInfoDto,
    cell_info: CellInfoDto,

    /// The primary cell's network; required with `[networks]`.
    network: Option<String>,
    brew: Option<CfgBrewDto>,
    lst_dispatch: Option<CfgLstDispatchDto>,
    asterisk: Option<CfgAsteriskDto>,
    dapnet: Option<CfgDapnetDto>,
    geoalarm: Option<CfgGeoalarmDto>,
    tpg2200_action: Option<CfgTpg2200ActionDto>,
    snom_notify: Option<CfgSnomNotifyDto>,
    dashboard: Option<CfgDashboardDto>,
    telemetry: Option<CfgTelemetryDto>,
    command: Option<CfgControlDto>,
    security: Option<CfgSecurityDto>,
    #[serde(rename = "wx_service")]
    wx_service: Option<CfgWxServiceDto>,
    #[serde(rename = "aprs_wx")]
    aprs_wx: Option<CfgAprsWxDto>,
    recovery: Option<CfgRecoveryDto>,
    #[serde(rename = "telegram_alerts")]
    telegram_alerts: Option<CfgTelegramDto>,
    health: Option<CfgHealthDto>,
    emergency: Option<CfgEmergencyDto>,
    multi_network: Option<CfgMultiNetworkDto>,

    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped example config must always parse cleanly through the real loader. This guards
    /// against config-documentation drift: every option documented (or set) in example_config must
    /// stay valid against the DTOs, and the strict `extra`/`deny`-style flatten maps must not reject
    /// any uncommented key.
    #[test]
    fn example_config_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../example_config/config.toml");
        from_file(path).unwrap_or_else(|e| panic!("example_config/config.toml must parse: {e}"));
    }

    /// Gold-standard guard: every optional block documented (commented) in example_config must
    /// parse against the DTOs when a user UNCOMMENTS it — field names and types must match.
    #[test]
    fn documented_optional_blocks_parse_when_uncommented() {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"
ul_input_file = "./ul.bin"
dl_input_file = "./dl.bin"

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
registration = true
deregistration = true
priority_cell = false
no_minimum_mode = false
migration = false
circuit_mode_data_service = false
aie_service = false
neighbor_cell_broadcast = 0
late_entry_supported = true
system_code = 3
sharing_mode = 0
ts_reserved_frames = 0
u_plane_dtx = false
frame_18_ext = false
ms_txpwr_max_cell = 4
subscriber_class = 0xFFFF
sndcp_service = true
advanced_link = true

[cell_info.wap_ip]
enabled = true
address = "10.0.0.1"
port = 9200
response_ttl = 32
max_request_payload_bytes = 1024

[cell_info.cell_reselect]
slow_threshold_db = 20
fast_threshold_db = 10
slow_hysteresis_db = 10
fast_hysteresis_db = 6

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1525

[cell_info.neighbor_cells_ca.bs_service_details]
registration = true
deregistration = true
priority_cell = false
no_minimum_mode = false
migration = false
system_wide_services = false
voice_service = true
circuit_mode_data_service = false
sndcp_service = false
aie_service = false
advanced_link = false

[telemetry]
host = "t.example.com"
port = 443
use_tls = true
ca_cert = "/tmp/ca.der"
username = "bts"
password = "x"

[command]
host = "c.example.com"
port = 443
use_tls = true
ca_cert = "/tmp/ca.der"
username = "station"
password = "x"

[asterisk]
enabled = true
outbound_prefix = "91"
strip_outbound_prefix = true
inbound_prefix = "T"
register = true
codec = "PCMU"
service_numbers = ["600", "601"]
rtp_port_min = 30000
rtp_port_max = 30100
bind_addr = "0.0.0.0"
bind_port = 5062
remote_host = "127.0.0.1"
remote_port = 5060
contact_host = "127.0.0.1"
from_domain = "127.0.0.1"
local_user = "flowstation"
auth_user = "flowstation"
password = ""
realm = "asterisk"

[dapnet]
enabled = true
api_url = "https://hampager.de/api/calls"
username = "dl1abc"
password = "example"
poll_interval_secs = 30
forward_sds = true
forward_callout = true
forward_telegram = true
sds_source_issi = 9999
sds_dest_issi = 1234567
sds_dest_is_group = false
ric_issi_routes = { "0632585" = 2632585, "0x9A70A" = 2632586 }
ric_gssi_routes = { "0004520" = 80 }
sds_allowed_rics = ["0632585", "0004520"]
callout_allowed_rics = ["0004520"]
telegram_allowed_rics = ["0000200", "0x1C40"]
callout_source_issi = 9999
callout_dest_issi = 1234567
callout_incident_base = 2
callout_text_prefix = "DAPNET"
telegram_prefix = "DAPNET"
rwth_core_enabled = true
rwth_core_host = "dapnet.afu.rwth-aachen.de"
rwth_core_port = 43434
rwth_core_device = "FlowStation"
rwth_core_version = "1.0"
rwth_core_callsign = "DL1ABC"
rwth_core_authkey = "example"
rwth_messages_limit = 100

[geoalarm]
enabled = true

[tpg2200_action]
enabled = true
token = "example-token"
source_issi = 9999
dest_issi = 1234567
incident_base = 1
default_text = "ALARM"
max_text_chars = 80

[snom_notify]
enabled = true
ami_host = "127.0.0.1"
ami_port = 5038
ami_username = "flowstation"
ami_password = "example"
endpoints = ["385"]
notify_sds = true
notify_dapnet = true
notify_telegram = true
sds_directions = ["rx", "net", "tx"]
dapnet_allowed_rics = ["0632585", "0000200"]
sds_allowed_issis = [2632585, 9999]
title_prefix = "FlowStation"
notify_event = "xml"
content_type = "application/snomxml"
subscription_state = "active;expires=30000"
max_text_chars = 240
connect_timeout_secs = 3

[recovery]
enabled = true
issi_allowlist = []
max_replay_attempts = 150
replay_per_frame = 1
debounce_secs = 5
max_cached_issis = 1024

[health]
enabled = true
snapshot_interval_secs = 5
restart_on_core_stall = false
core_stall_secs = 10
restart_after_critical_secs = 30
restart_cooldown_secs = 600
radios_silent_secs = 900
dl_queue_degraded = 64
dl_queue_critical = 192
sds_queue_degraded = 32
sds_queue_critical = 128
"#;
        let cfg = from_toml_str(toml).unwrap_or_else(|e| panic!("documented optional blocks must parse when uncommented: {e}"));
        assert!(cfg.recovery.enabled);
        assert!(cfg.tpg2200_action.enabled);
        assert_eq!(cfg.tpg2200_action.dest_issi, 1234567);
        assert!(cfg.snom_notify.enabled);
        assert_eq!(cfg.snom_notify.endpoints, vec!["385"]);
        assert!(cfg.snom_notify.notify_sds);
        assert!(cfg.snom_notify.sds_directions.iter().any(|d| d == "tx"));
        assert!(cfg.snom_notify.dapnet_allowed_rics.contains(&632585));
        assert!(cfg.snom_notify.sds_allowed_issis.contains(&2632585));
        assert_eq!(cfg.snom_notify.content_type, "application/snomxml");
        assert_eq!(cfg.recovery.max_replay_attempts, 150);
        assert!(cfg.health.enabled);
        assert_eq!(cfg.health.core_stall_secs, 10);
        assert!(cfg.asterisk.enabled);
        assert_eq!(cfg.asterisk.service_numbers, vec!["600".to_string(), "601".to_string()]);
        assert!(cfg.dapnet.enabled);
        let wap = cfg.cell.wap_ip.expect("[cell_info.wap_ip] must parse when uncommented");
        assert!(wap.enabled);
        assert_eq!(wap.address, [10, 0, 0, 1]);
        assert_eq!(wap.port, 9200);
        assert_eq!(wap.response_ttl, 32);
        assert_eq!(wap.max_request_payload_bytes, 1024);
        assert!(cfg.dapnet.rwth_core_enabled);
        assert_eq!(cfg.dapnet.callout_incident_base, 2);
        assert_eq!(cfg.dapnet.ric_issi_routes.get(&632585), Some(&2632585));
        assert_eq!(cfg.dapnet.ric_issi_routes.get(&632586), Some(&2632586));
        assert_eq!(cfg.dapnet.ric_gssi_routes.get(&4520), Some(&80));
        assert!(cfg.dapnet.sds_allowed_rics.contains(&632585));
        assert!(cfg.dapnet.sds_allowed_rics.contains(&4520));
        assert!(cfg.dapnet.callout_allowed_rics.contains(&4520));
        assert!(cfg.dapnet.telegram_allowed_rics.contains(&200));
        assert!(cfg.dapnet.telegram_allowed_rics.contains(&0x1C40));
        assert!(cfg.geoalarm.enabled);
    }

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
    fn dual_carrier_enabled_flag_gates_effective_secondary_carrier() {
        // Absent flag is backward compatible: a configured secondary carrier is on by default.
        let cfg = from_toml_str(&minimal_toml("secondary_carrier = 1585")).expect("parse");
        assert_eq!(cfg.cell.secondary_carrier, Some(1585), "absent flag => enabled");

        // Explicitly enabled.
        let cfg = from_toml_str(&minimal_toml("secondary_carrier = 1585\ndual_carrier_enabled = true")).expect("parse");
        assert_eq!(cfg.cell.secondary_carrier, Some(1585));

        // Disabled: the number stays in the file but the stack sees None (single carrier). This is
        // what the dashboard "Dual-Carrier OFF" toggle writes, and the whole stack reads this field.
        let cfg = from_toml_str(&minimal_toml("secondary_carrier = 1585\ndual_carrier_enabled = false")).expect("parse");
        assert_eq!(cfg.cell.secondary_carrier, None, "disabled switch hides the carrier from the stack");
    }

    #[test]
    fn dgna_use_ss_facility_flag_defaults_on() {
        // Absent flag => SS-DGNA D-FACILITY path (TS 100 392-12-22) is the default.
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert!(cfg.cell.dgna_use_ss_facility, "absent flag => SS-DGNA path on");

        // Explicitly rolled back to the legacy MM D-ATTACH path.
        let cfg = from_toml_str(&minimal_toml("dgna_use_ss_facility = false")).expect("parse");
        assert!(!cfg.cell.dgna_use_ss_facility, "false => legacy MM D-ATTACH path");
    }

    #[test]
    fn rxlev_access_min_defaults_to_minus_110_and_is_configurable() {
        // Absent: -110 dBm, SYSINFO code 3 (table 21.65), config still valid.
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert_eq!(cfg.cell.rxlev_access_min_dbm, -110);
        assert_eq!(cfg.cell.rxlev_access_min(), 3);
        cfg.validate().expect("default is valid");

        // Ends of table 21.65 and a value in between.
        for (dbm, code) in [(-125, 0), (-75, 10), (-50, 15)] {
            let cfg = from_toml_str(&minimal_toml(&format!("rxlev_access_min_dbm = {dbm}"))).expect("parse");
            cfg.validate().expect("coded level is valid");
            assert_eq!(cfg.cell.rxlev_access_min(), code, "{dbm} dBm");
        }

        // Not one of the 16 coded levels.
        for dbm in [-130, -45, -77, 0] {
            let cfg = from_toml_str(&minimal_toml(&format!("rxlev_access_min_dbm = {dbm}"))).expect("parse");
            let err = cfg.validate().unwrap_err();
            assert!(err.contains("rxlev_access_min_dbm"), "{dbm}: {err}");
        }
    }

    #[test]
    fn try_from_parts_errors_instead_of_panicking_on_bad_config() {
        use crate::bluestation::SharedConfig;
        // ms_txpwr_max_cell = 8 is out of the 0-7 range and fails validate().
        let bad = from_toml_str(&minimal_toml("ms_txpwr_max_cell = 8")).expect("parses");
        assert!(
            SharedConfig::try_from_parts(bad, None).is_err(),
            "an invalid config must return Err, not panic the station"
        );
        // A valid config still builds.
        let good = from_toml_str(&minimal_toml("")).expect("parses");
        assert!(SharedConfig::try_from_parts(good, None).is_ok());
    }

    #[test]
    fn wap_ip_profile_resolves_with_overrides_and_defaults() {
        let cfg = from_toml_str(&minimal_toml(
            "sndcp_service = true\nadvanced_link = true\n\n[cell_info.wap_ip]\nenabled = true\naddress = \"10.0.0.5\"\nport = 9201\nresponse_ttl = 16",
        ))
        .expect("a consistent WAP profile must parse");
        let w = cfg.cell.wap_ip.expect("wap_ip resolved");
        assert!(w.enabled);
        assert_eq!(w.address, [10, 0, 0, 5]);
        assert_eq!(w.port, 9201);
        assert_eq!(w.response_ttl, 16);
        assert_eq!(w.max_request_payload_bytes, 1024, "default request cap");

        // Omitted fields fall back to the spec defaults.
        let cfg = from_toml_str(&minimal_toml("sndcp_service = true\nadvanced_link = true\n\n[cell_info.wap_ip]\nenabled = true")).expect("parse");
        let w = cfg.cell.wap_ip.unwrap();
        assert_eq!((w.address, w.port, w.response_ttl), ([10, 0, 0, 1], 9200, 32));
    }

    #[test]
    fn wap_master_switch_never_fails_to_start() {
        let validated = |extra: &str| from_toml_str(&minimal_toml(extra)).expect("parses").validate();

        // Regression: turning the gateway off while a stale sndcp_service flag is still set must
        // NEVER fail validation — disabling a feature can't crash the station at startup.
        assert!(
            validated("sndcp_service = true\n\n[cell_info.wap_ip]\nenabled = false").is_ok(),
            "disabling WAP must never fail validation, even with sndcp_service still true"
        );
        // Every other combination is also accepted (the flags are advisory now).
        assert!(validated("").is_ok(), "both off (default)");
        assert!(validated("sndcp_service = true").is_ok(), "sndcp_service with no gateway: just off");
        assert!(validated("[cell_info.wap_ip]\nenabled = true").is_ok(), "enabling needs no other flags");

        // wap_enabled() is the single master switch, derived only from [cell_info.wap_ip].enabled.
        let on = from_toml_str(&minimal_toml("[cell_info.wap_ip]\nenabled = true")).unwrap();
        assert!(on.cell.wap_enabled());
        let off = from_toml_str(&minimal_toml("sndcp_service = true\n\n[cell_info.wap_ip]\nenabled = false")).unwrap();
        assert!(!off.cell.wap_enabled(), "enabled=false is off regardless of sndcp_service");
        assert!(!from_toml_str(&minimal_toml("sndcp_service = true")).unwrap().cell.wap_enabled(), "no wap_ip section = off");
    }

    #[test]
    fn dgna_attachment_mode_defaults_to_permanent_and_clamps() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert_eq!(cfg.cell.dgna_attachment_mode, 0, "absent mode => Attached permanently");

        let cfg = from_toml_str(&minimal_toml("dgna_attachment_mode = 4")).expect("parse");
        assert_eq!(cfg.cell.dgna_attachment_mode, 4);

        let cfg = from_toml_str(&minimal_toml("dgna_attachment_mode = 99")).expect("parse");
        assert_eq!(cfg.cell.dgna_attachment_mode, 5, "mode is clamped to the valid Table 16.51 range");
    }

    #[test]
    fn telegram_alerts_section_parses() {
        let toml = minimal_toml("")
            + r#"
[telegram_alerts]
enabled = true
bot_token = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
chat_ids = [987654321, -1001234567890]
alert_connect = true
alert_critical_logs = false
"#;
        let cfg = from_toml_str(&toml).expect("telegram_alerts must parse");
        let tg = cfg.telegram.expect("telegram section present");
        assert!(tg.enabled);
        assert_eq!(tg.chat_ids, vec![987654321, -1001234567890]);
        // Explicit override is respected …
        assert!(!tg.alert_critical_logs);
        // … while unspecified toggles default to on.
        assert!(tg.alert_disconnect);
        assert!(tg.is_deliverable());
    }

    #[test]
    fn aprs_wx_section_parses() {
        let toml = minimal_toml("")
            + r#"
[aprs_wx]
apikey = " 123456.abcdef "
default_call = "dn9cho-13"
periodic_enabled = true
periodic_dest = 1001
periodic_interval_secs = 60
"#;
        let cfg = from_toml_str(&toml).expect("aprs_wx must parse");
        let a = cfg.aprs_wx;
        assert!(a.has_apikey());
        assert_eq!(a.apikey.val, "123456.abcdef");
        assert_eq!(a.default_call, "DN9CHO-13");
        assert_eq!(a.effective_periodic_call(), "DN9CHO-13");
        assert_eq!(a.max_age_secs, 3600);
        assert_eq!(a.effective_interval_secs(), 300);
    }

    #[test]
    fn aprs_wx_absent_is_disabled() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert!(!cfg.aprs_wx.has_apikey());
        assert!(!cfg.aprs_wx.periodic_enabled);
    }

    #[test]
    fn aprs_wx_unknown_field_rejected() {
        let toml = minimal_toml("") + "\n[aprs_wx]\napi_key = \"x\"\n";
        assert!(from_toml_str(&toml).is_err(), "should reject unknown aprs_wx field");
    }

    #[test]
    fn telegram_alerts_unknown_field_rejected() {
        let toml = minimal_toml("")
            + r#"
[telegram_alerts]
enabled = true
bogus = 1
"#;
        assert!(from_toml_str(&toml).is_err(), "should reject unknown telegram_alerts field");
    }

    #[test]
    fn brew_pbx_gateway_issis_parse() {
        let toml = minimal_toml("")
            + r#"
[brew]
host = "core.tetrapack.online"
port = 443
tls = true
username = 123456700
password = "012345"
pbx_gateway_issis = [16777184, 16777186]
"#;
        let cfg = from_toml_str(&toml).expect("brew config with pbx_gateway_issis must parse");
        let brew = cfg.brew.expect("brew config should be present");
        assert_eq!(brew.pbx_gateway_issis, Some(vec![16_777_184, 16_777_186]));
    }

    #[test]
    fn brew_pbx_gateway_issi_alias_parses() {
        let toml = minimal_toml("")
            + r#"
[brew]
host = "core.tetrapack.online"
port = 443
tls = true
username = 123456700
password = "012345"
pbx_gateway_issi = [16777184, 16777186]
"#;
        let cfg = from_toml_str(&toml).expect("brew config with pbx_gateway_issi alias must parse");
        let brew = cfg.brew.expect("brew config should be present");
        assert_eq!(brew.pbx_gateway_issis, Some(vec![16_777_184, 16_777_186]));
    }
}
