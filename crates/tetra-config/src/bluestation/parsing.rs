use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::sec_cell::{CfgNeighborCellCa, SdsCommandControlDto};
use crate::bluestation::{CellInfoDto, CfgControlDto, NetInfoDto, apply_control_patch, cell_dto_to_cfg, net_dto_to_cfg};

use super::config::{StackConfig, StackMode};
use super::sec_asterisk::{CfgAsteriskDto, apply_asterisk_patch};
use super::sec_brew::{CfgBrewDto, apply_brew_patch};
use super::sec_dapnet::{CfgDapnetDto, apply_dapnet_patch};
use super::sec_dashboard::{CfgDashboardDto, apply_dashboard_patch};
use super::sec_emergency::{CfgEmergencyDto, apply_emergency_patch};
use super::sec_geoalarm::{CfgGeoalarmDto, apply_geoalarm_patch};
use super::sec_health::{CfgHealthDto, apply_health_patch};
use super::sec_packet_data::{CfgPacketDataDto, apply_packet_data_patch};
use super::sec_recovery::{CfgRecoveryDto, apply_recovery_patch};
use super::sec_security::{CfgSecurityDto, apply_security_patch};
use super::sec_snom_notify::{CfgSnomNotifyDto, apply_snom_notify_patch};
use super::sec_telegram::{CfgTelegramDto, apply_telegram_patch};
use super::sec_telemetry::{CfgTelemetryDto, apply_telemetry_patch};
use super::sec_tpg2200_action::{CfgTpg2200ActionDto, apply_tpg2200_action_patch};
use super::sec_wap::{CfgWapDto, apply_wap_patch};
use super::sec_wx::{CfgWxServiceDto, apply_wx_service_patch};
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
    let sds_command_control_raw = raw.get_mut("cell_info").and_then(|ci| {
        if let Value::Table(t) = ci {
            t.remove("sds_command_control")
        } else {
            None
        }
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

    // Optional wap section and its [wap.wtp] / [wap.browse] subsections.
    if let Some(ref wap) = root.wap {
        if !wap.extra.is_empty() {
            return Err(format!("Unrecognized fields in wap config: {:?}", sorted_keys(&wap.extra)).into());
        }
        if let Some(ref wtp) = wap.wtp
            && !wtp.extra.is_empty()
        {
            return Err(format!("Unrecognized fields in wap.wtp config: {:?}", sorted_keys(&wtp.extra)).into());
        }
        if let Some(ref browse) = wap.browse
            && !browse.extra.is_empty()
        {
            return Err(format!("Unrecognized fields in wap.browse config: {:?}", sorted_keys(&browse.extra)).into());
        }
    }

    // Optional packet_data section.
    if let Some(ref packet_data) = root.packet_data
        && !packet_data.extra.is_empty()
    {
        return Err(format!("Unrecognized fields in packet_data config: {:?}", sorted_keys(&packet_data.extra)).into());
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
        brew: None,
        asterisk: apply_asterisk_patch(root.asterisk.unwrap_or_default())?,
        dapnet: apply_dapnet_patch(root.dapnet.unwrap_or_default())?,
        geoalarm: apply_geoalarm_patch(root.geoalarm.unwrap_or_default())?,
        tpg2200_action: apply_tpg2200_action_patch(root.tpg2200_action.unwrap_or_default())?,
        snom_notify: apply_snom_notify_patch(root.snom_notify.unwrap_or_default())?,
        dashboard: None,
        telemetry: None,
        control: None,
        security: apply_security_patch(root.security.unwrap_or_default()),
        wx_service: apply_wx_service_patch(root.wx_service.unwrap_or_default()),
        recovery: apply_recovery_patch(root.recovery.unwrap_or_default()),
        telegram: None,
        health: apply_health_patch(root.health.unwrap_or_default()),
        emergency: apply_emergency_patch(root.emergency.unwrap_or_default()),
        wap: match root.wap {
            Some(wap) => apply_wap_patch(wap)?,
            None => Default::default(),
        },
        packet_data: match root.packet_data {
            Some(packet_data) => apply_packet_data_patch(packet_data)?,
            None => Default::default(),
        },
    };

    // The radios reach the gateway at [wap] gateway_ipv4: it cannot be handed out as a radio's
    // address.
    if cfg.packet_data.enabled && cfg.packet_data.in_pool(cfg.wap.gateway_ipv4) {
        return Err(format!(
            "packet_data: the pool {}..{} contains [wap] gateway_ipv4 {}",
            cfg.packet_data.pool_first, cfg.packet_data.pool_last, cfg.wap.gateway_ipv4
        )
        .into());
    }

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

    if let Some(telegram) = root.telegram_alerts {
        cfg.telegram = Some(apply_telegram_patch(telegram));
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
    #[serde(default)]
    service_name: Option<String>,

    phy_io: PhyIoDto,
    net_info: NetInfoDto,
    cell_info: CellInfoDto,

    brew: Option<CfgBrewDto>,
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
    recovery: Option<CfgRecoveryDto>,
    #[serde(rename = "telegram_alerts")]
    telegram_alerts: Option<CfgTelegramDto>,
    health: Option<CfgHealthDto>,
    emergency: Option<CfgEmergencyDto>,
    wap: Option<CfgWapDto>,
    packet_data: Option<CfgPacketDataDto>,

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
late_entry_supported = false
system_code = 3
sharing_mode = 0
ts_reserved_frames = 0
u_plane_dtx = false
frame_18_ext = false
ms_txpwr_max_cell = 4
subscriber_class = 0xFFFF

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

[wap]
enabled = true
gateway_ipv4 = "10.0.0.1"
mtu = 576
max_message_bytes = 8192
max_request_bytes = 1024
content_type = "short"
debug_udp_listen = "127.0.0.1:9200"
debug_udp_allowed_sources = ["127.0.0.1/32"]
debug_issi = 0

[wap.wtp]
sar = "auto"
group_size = 3
retry_base_ms = 4000
air_rate_bytes_per_sec = 450
max_retries = 4
retransmit_rid = true
hold_on_ms = 2000

[wap.browse]
enabled = true
allowed_issis = [2260618]
search_url = "http://lite.duckduckgo.com/lite/?q="
bookmarks = ["http://68k.news/", "http://wiby.me/", "http://text.npr.org/"]

[packet_data]
enabled = true
pool_first = "10.0.0.2"
pool_last = "10.0.0.254"
ready_timer_code = 10
bearer = "pdch"
pdch_timeslots = [4, 3, 2]
pdch_idle_release_secs = 10
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
        assert!(cfg.wap.browse_allowed(2260618));
        assert!(cfg.packet_data.enabled);
        assert_eq!(cfg.packet_data.ready_timer_code, 10);
        assert_eq!(cfg.packet_data.bearer, crate::bluestation::PacketDataBearer::Pdch);
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
    fn dgna_attachment_mode_defaults_to_permanent_and_clamps() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert_eq!(cfg.cell.dgna_attachment_mode, 0, "absent mode => Attached permanently");

        let cfg = from_toml_str(&minimal_toml("dgna_attachment_mode = 4")).expect("parse");
        assert_eq!(cfg.cell.dgna_attachment_mode, 4);

        let cfg = from_toml_str(&minimal_toml("dgna_attachment_mode = 99")).expect("parse");
        assert_eq!(cfg.cell.dgna_attachment_mode, 5, "mode is clamped to the valid Table 16.51 range");
    }

    #[test]
    fn parrot_defaults_off_on_99999_and_validates_values() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert!(!cfg.cell.parrot_enabled, "absent flag => parrot off");
        assert_eq!(cfg.cell.parrot_issi, 99_999);
        assert_eq!(cfg.cell.parrot_max_secs, 20);

        let cfg = from_toml_str(&minimal_toml("parrot_enabled = true\nparrot_issi = 12345\nparrot_max_secs = 10")).expect("parse");
        assert!(cfg.cell.parrot_enabled);
        assert_eq!(cfg.cell.parrot_issi, 12345);
        assert_eq!(cfg.cell.parrot_max_secs, 10);

        let cfg = from_toml_str(&minimal_toml("parrot_issi = 0\nparrot_max_secs = 0")).expect("parse");
        assert_eq!(cfg.cell.parrot_issi, 99_999, "ISSI 0 is not a valid parrot ISSI");
        assert_eq!(cfg.cell.parrot_max_secs, 1, "recording limit is clamped to 1..=60 s");

        let cfg = from_toml_str(&minimal_toml("parrot_issi = 16777215\nparrot_max_secs = 600")).expect("parse");
        assert_eq!(cfg.cell.parrot_issi, 99_999, "the all-ones SSI is not a valid parrot ISSI");
        assert_eq!(cfg.cell.parrot_max_secs, 60);
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

    #[test]
    fn wap_default_is_disabled() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert!(!cfg.wap.enabled);
        assert!(!cfg.wap.browse.enabled);
        assert!(cfg.wap.debug_udp_listen.is_none());
        assert_eq!(cfg.wap, crate::bluestation::CfgWap::default());
    }

    #[test]
    fn packet_data_default_is_disabled() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse");
        assert_eq!(cfg.packet_data, crate::bluestation::CfgPacketData::default());
        assert!(!cfg.packet_data.enabled);
    }

    #[test]
    fn packet_data_section_parses_and_validates() {
        let ok = minimal_toml("")
            + r#"
[packet_data]
enabled = true
pool_first = "10.0.0.10"
pool_last = "10.0.0.20"
ready_timer_code = 9
"#;
        let cfg = from_toml_str(&ok).expect("packet_data section must parse");
        assert!(cfg.packet_data.enabled);
        assert_eq!(cfg.packet_data.pool_first, std::net::Ipv4Addr::new(10, 0, 0, 10));
        assert_eq!(cfg.packet_data.ready_timer_code, 9);
        for bad in [
            "bogus = 1",
            "pool_first = \"10.0.0.9\"\npool_last = \"10.0.0.8\"",
            "pool_first = \"10.0.0.0\"\npool_last = \"10.0.8.0\"",
            "ready_timer_code = 0",
            "ready_timer_code = 15",
        ] {
            let toml = minimal_toml("") + "\n[packet_data]\n" + bad + "\n";
            assert!(from_toml_str(&toml).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn packet_data_pool_must_not_hold_the_gateway() {
        let clash = minimal_toml("") + "\n[packet_data]\nenabled = true\npool_first = \"10.0.0.1\"\n";
        assert!(from_toml_str(&clash).is_err(), "gateway 10.0.0.1 inside the pool");
        let moved = minimal_toml("") + "\n[wap]\ngateway_ipv4 = \"10.0.1.1\"\n[packet_data]\nenabled = true\npool_first = \"10.0.0.1\"\n";
        assert!(from_toml_str(&moved).is_ok());
        let off = minimal_toml("") + "\n[packet_data]\npool_first = \"10.0.0.1\"\n";
        assert!(from_toml_str(&off).is_ok(), "only checked with the bearer on");
    }

    fn pdch_carrier_toml(cell: &str, packet_data: &str) -> String {
        minimal_toml(cell) + "\n[wap]\ngateway_ipv4 = \"10.0.1.1\"\n[packet_data]\n" + packet_data + "\n"
    }

    /// The main carrier as `pdch_carrier` is ignored, never an error: the station starts.
    #[test]
    fn pdch_carrier_equal_to_the_main_carrier_is_ignored() {
        let cfg = from_toml_str(&pdch_carrier_toml(
            "secondary_carrier = 1585",
            "enabled = true\nbearer = \"pdch\"\npdch_carrier = 1584",
        ))
        .expect("parse");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.pdch_carrier(), None);
        let shared = crate::bluestation::SharedConfig::from_parts(cfg, None);
        assert_eq!(shared.config().pdch_carrier(), None);
    }

    /// Only the secondary carrier in use, with packet data on a PDCH, is a packet-data carrier.
    #[test]
    fn pdch_carrier_is_in_use_only_as_the_secondary() {
        let on = "enabled = true\nbearer = \"pdch\"\npdch_carrier = 1585";
        let carrier = |cell: &str, pd: &str| from_toml_str(&pdch_carrier_toml(cell, pd)).expect("parse").pdch_carrier();
        assert_eq!(
            carrier("secondary_carrier = 1585", on),
            Some(crate::bluestation::PdchCarrierCfg {
                carrier: 1585,
                timeslots: vec![4, 3, 2, 1],
                exclusive: false
            })
        );
        assert_eq!(
            carrier("secondary_carrier = 1585", "bearer = \"pdch\"\npdch_carrier = 1585"),
            None,
            "packet data off"
        );
        assert_eq!(
            carrier("secondary_carrier = 1585", "enabled = true\npdch_carrier = 1585"),
            None,
            "bearer mcch"
        );
        assert_eq!(carrier("", on), None, "no secondary");
        assert_eq!(
            carrier("secondary_carrier = 1585\ndual_carrier_enabled = false", on),
            None,
            "dual carrier off"
        );
        assert_eq!(carrier("secondary_carrier = 1586", on), None, "another secondary");
        let exclusive = carrier(
            "secondary_carrier = 1585",
            "enabled = true\nbearer = \"pdch\"\npdch_carrier = 1585\npdch_carrier_timeslots = [4, 3]\npdch_carrier_exclusive = true",
        )
        .expect("in use");
        assert_eq!((exclusive.timeslots, exclusive.exclusive), (vec![4, 3], true));
    }

    #[test]
    fn pdch_slots_per_radio_counts_the_carrier() {
        let slots = |cell: &str, pd: &str| from_toml_str(&pdch_carrier_toml(cell, pd)).expect("parse").pdch_slots_per_radio();
        let pd = "enabled = true\nbearer = \"pdch\"\npdch_timeslots = [3]\npdch_carrier = 1585\npdch_max_slots = 4";
        assert_eq!(slots("secondary_carrier = 1585", pd), 4);
        assert_eq!(slots("", pd), 1, "carrier not in use: main [3] only");
        assert_eq!(
            slots("secondary_carrier = 1585", &pd.replace("bearer = \"pdch\"", "bearer = \"mcch\"")),
            1
        );
        assert_eq!(
            slots("secondary_carrier = 1585", &pd.replace("pdch_max_slots = 4", "pdch_max_slots = 2")),
            2
        );
        assert_eq!(
            slots("secondary_carrier = 1585", "enabled = true\nbearer = \"pdch\"\npdch_max_slots = 3"),
            3,
            "without pdch_carrier: phase 1"
        );
    }

    /// Without `extra_carriers` the carriers, the packet-data carrier and the copy rules are what
    /// they were: one carrier, two, two with a packet-data carrier.
    #[test]
    fn extra_carriers_absent_keeps_today_s_carriers() {
        let pd = "enabled = true\nbearer = \"pdch\"\npdch_carrier = 1585";
        for (toml, carriers, pdch) in [
            (minimal_toml(""), vec![1584], None),
            (minimal_toml("secondary_carrier = 1585"), vec![1584, 1585], None),
            (pdch_carrier_toml("secondary_carrier = 1585", pd), vec![1584, 1585], Some(1585)),
        ] {
            let cfg = from_toml_str(&toml).expect("parse");
            assert!(cfg.validate().is_ok());
            assert!(cfg.cell.extra_carriers.is_empty());
            assert_eq!(cfg.cell.carriers(), carriers);
            let phase_mod: Vec<u16> = cfg.bs_phase_mod_carriers().unwrap().iter().map(|c| c.0).collect();
            assert_eq!(phase_mod, carriers);
            assert_eq!(cfg.pdch_carrier().map(|p| p.carrier), pdch);
            assert_eq!(cfg.uplink_copy_rules(), pdch.is_some());
            assert!(cfg.carrier_plan_warnings().is_empty());
        }
    }

    #[test]
    fn extra_carriers_come_after_the_secondary() {
        let cfg = from_toml_str(&minimal_toml("secondary_carrier = 1586\nextra_carriers = [1589]")).expect("parse");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.cell.carriers(), vec![1584, 1586, 1589]);
        assert_eq!(cfg.cell.other_carriers().collect::<Vec<_>>(), vec![1586, 1589]);
        let phase_mod: Vec<u16> = cfg.bs_phase_mod_carriers().unwrap().iter().map(|c| c.0).collect();
        assert_eq!(phase_mod, vec![1584, 1586, 1589]);
        assert!(cfg.uplink_copy_rules(), "always with three carriers");
        let cfg = from_toml_str(&minimal_toml("secondary_carrier = 1586\nextra_carriers = [1589, 1581]")).expect("parse");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.cell.carriers(), vec![1584, 1586, 1589, 1581], "config order");
    }

    /// Without the on-demand keys every carrier is always on the air, as today.
    #[test]
    fn on_demand_keys_absent_are_today_s() {
        let cfg = from_toml_str(&minimal_toml("secondary_carrier = 1586\nextra_carriers = [1589]")).expect("parse");
        assert!(cfg.validate().is_ok());
        assert!(!cfg.cell.secondary_carrier_on_demand);
        assert_eq!(cfg.cell.secondary_carrier_warmup_frames, 4);
        assert_eq!(cfg.cell.secondary_carrier_inactivity_secs, 180);
    }

    /// The `[cell_info]` lines of Rafa EA7KEN's notes (his build of 30-09) are read as written.
    #[test]
    fn on_demand_keys_as_rafa_wrote_them() {
        let cell = "\
secondary_carrier               = 1586
dual_carrier_enabled            = true
secondary_carrier_on_demand     = true   # por defecto false
secondary_carrier_warmup_frames = 4      # ~220 ms antes de mandar al terminal
secondary_carrier_inactivity_secs = 180  # gracia tras la ultima llamada";
        let cfg = from_toml_str(&minimal_toml(cell)).expect("parse");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.cell.carriers(), vec![1584, 1586]);
        assert!(cfg.cell.secondary_carrier_on_demand);
        assert_eq!(cfg.cell.secondary_carrier_warmup_frames, 4);
        assert_eq!(cfg.cell.secondary_carrier_inactivity_secs, 180);
    }

    /// Out-of-range values are brought into range, never an error: the station starts.
    #[test]
    fn on_demand_values_are_clamped() {
        let cell = |warmup: u32, secs: u32| {
            let cell = format!(
                "secondary_carrier = 1586\nsecondary_carrier_on_demand = true\nsecondary_carrier_warmup_frames = {warmup}\nsecondary_carrier_inactivity_secs = {secs}"
            );
            let cfg = from_toml_str(&minimal_toml(&cell)).expect("parse");
            assert!(cfg.validate().is_ok());
            (cfg.cell.secondary_carrier_warmup_frames, cfg.cell.secondary_carrier_inactivity_secs)
        };
        assert_eq!(cell(0, 0), (0, 1));
        assert_eq!(cell(200, 99_999), (18, 3600));
        assert_eq!(cell(18, 3600), (18, 3600));
        assert_eq!(cell(2, 10), (2, 10));
    }

    #[test]
    fn dual_carrier_off_hides_the_extra_carriers() {
        let cell = "secondary_carrier = 1586\nextra_carriers = [1589]\ndual_carrier_enabled = false";
        let cfg = from_toml_str(&minimal_toml(cell)).expect("parse");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.cell.carriers(), vec![1584]);
        assert!(cfg.cell.extra_carriers.is_empty());
        let pd = "enabled = true\nbearer = \"pdch\"\npdch_carrier = 1589";
        assert_eq!(from_toml_str(&pdch_carrier_toml(cell, pd)).expect("parse").pdch_carrier(), None);
    }

    #[test]
    fn extra_carriers_are_validated() {
        let err = |cell: &str| from_toml_str(&minimal_toml(cell)).expect("parse").validate().err().map(str::to_string);
        let differ = Some("cell.extra_carriers must differ from each other, from main_carrier and from secondary_carrier".to_string());
        assert_eq!(
            err("extra_carriers = [1589]"),
            Some("cell.extra_carriers needs cell.secondary_carrier".to_string())
        );
        assert_eq!(err("secondary_carrier = 1586\nextra_carriers = [1584]"), differ, "the main carrier");
        assert_eq!(err("secondary_carrier = 1586\nextra_carriers = [1586]"), differ, "the secondary");
        assert_eq!(err("secondary_carrier = 1586\nextra_carriers = [1589, 1589]"), differ, "twice");
        assert_eq!(
            err("secondary_carrier = 1586\nextra_carriers = [1588, 1589, 1590]"),
            Some("cell.extra_carriers: at most 4 carriers in the cell (main, secondary and two extra)".to_string())
        );
        assert_eq!(
            err("secondary_carrier = 1584\nextra_carriers = [1589]"),
            Some("cell.secondary_carrier must differ from cell.main_carrier".to_string()),
            "today's error first"
        );
        assert_eq!(err("secondary_carrier = 1586\nextra_carriers = [1589, 1581]"), None);
    }

    /// `cell` on the SoapySDR backend at 600 kHz, the TX centre `tx_off` Hz from the main downlink
    /// and the RX centre `rx_off` Hz from the main uplink.
    fn soapy_cell(cell: &str, tx_off: f64, rx_off: f64) -> StackConfig {
        let mut cfg = from_toml_str(&minimal_toml(cell)).expect("parse");
        let (_, dl, ul) = cfg.bs_phase_mod_carriers().unwrap()[0];
        cfg.phy_io.backend = crate::bluestation::PhyBackend::SoapySdr;
        cfg.phy_io.soapysdr = Some(crate::bluestation::CfgSoapySdr {
            ul_freq: f64::from(ul),
            dl_freq: f64::from(dl),
            rx_center_freq: Some(f64::from(ul) + rx_off),
            tx_center_freq: Some(f64::from(dl) + tx_off),
            ppm_err: 0.0,
            device: None,
            rx_ant: None,
            tx_ant: None,
            rx_gains: HashMap::new(),
            tx_gains: HashMap::new(),
            fs: Some(600_000.0),
            rx_ch: None,
            tx_ch: None,
        });
        cfg
    }

    /// From three carriers on every channel (+/-36 kHz) must fit the passband, the uplink measured
    /// from the RX LO 20 kHz below `rx_center_freq`; up to two carriers today's check, no margin.
    #[test]
    fn three_carriers_must_fit_the_passband_with_their_channel() {
        let three = "secondary_carrier = 1586\nextra_carriers = [1588]"; // main, +50 kHz, +100 kHz
        let downlink = "with three or more carriers every downlink channel (+/-36 kHz) must fit the TX passband: move tx_center_freq or raise sample_rate";
        let uplink = "with three or more carriers every uplink channel (+/-36 kHz) must fit the RX passband, centred 20 kHz below rx_center_freq";
        assert_eq!(soapy_cell(three, -180_000.0, 50_000.0).validate(), Err(downlink), "the third 280 kHz off");
        assert_eq!(soapy_cell(three, -150_000.0, 50_000.0).validate(), Ok(()), "250 kHz off");
        assert_eq!(
            soapy_cell("secondary_carrier = 1586", -230_000.0, 25_000.0).validate(),
            Ok(()),
            "two carriers: the secondary 280 kHz off still passes"
        );
        assert_eq!(
            soapy_cell(three, 50_000.0, -150_000.0).validate(),
            Err(uplink),
            "250 kHz from rx_center_freq, 270 kHz from the LO"
        );
        assert_eq!(soapy_cell(three, 50_000.0, -120_000.0).validate(), Ok(()));
    }

    #[test]
    fn pdch_carrier_may_be_an_extra_carrier() {
        let cell = "secondary_carrier = 1586\nextra_carriers = [1589]";
        let carrier = |pd: &str| from_toml_str(&pdch_carrier_toml(cell, pd)).expect("parse").pdch_carrier();
        assert_eq!(
            carrier("enabled = true\nbearer = \"pdch\"\npdch_carrier = 1589").map(|p| p.carrier),
            Some(1589)
        );
        assert_eq!(
            carrier("enabled = true\nbearer = \"pdch\"\npdch_carrier = 1586").map(|p| p.carrier),
            Some(1586)
        );
        assert_eq!(
            carrier("enabled = true\nbearer = \"pdch\"\npdch_carrier = 1590"),
            None,
            "not a carrier of the cell"
        );
    }

    /// With three or more carriers voice fills the packet-data carrier last: the allocator's
    /// carrier order. Up to two carriers, and with the packet-data carrier already last, config
    /// order.
    #[test]
    fn voice_fills_the_packet_data_carrier_last_from_three_carriers() {
        let order = |cell: &str, pdch: u16| {
            let pd = format!("enabled = true\nbearer = \"pdch\"\npdch_carrier = {pdch}");
            let cfg = from_toml_str(&pdch_carrier_toml(cell, &pd)).expect("parse");
            let shared = crate::bluestation::SharedConfig::from_parts(cfg, None);
            let carriers = shared.state_read().timeslot_alloc.carriers().to_vec();
            carriers
        };
        let three = "secondary_carrier = 1586\nextra_carriers = [1589]";
        assert_eq!(order(three, 1586), vec![1584, 1589, 1586]);
        assert_eq!(order(three, 1589), vec![1584, 1586, 1589]);
        assert_eq!(order(three, 1590), vec![1584, 1586, 1589], "not a carrier of the cell");
        assert_eq!(
            order("secondary_carrier = 1586\nextra_carriers = [1589, 1581]", 1586),
            vec![1584, 1589, 1581, 1586]
        );
        assert_eq!(order("secondary_carrier = 1586", 1586), vec![1584, 1586], "two carriers");
    }

    #[test]
    fn carrier_plan_warnings_from_three_carriers() {
        let warnings = |cell: &str| from_toml_str(&minimal_toml(cell)).expect("parse").carrier_plan_warnings();
        let im3 = warnings("secondary_carrier = 1586\nextra_carriers = [1588]");
        assert!(
            im3.iter().any(|w| w.contains("2 x 1586 - 1584") && w.contains("carrier 1588")),
            "{im3:?}"
        );
        let four = warnings("secondary_carrier = 1586\nextra_carriers = [1589, 1591]");
        assert!(
            four.iter().any(|w| w.contains("1584 + 1591 - 1586") && w.contains("carrier 1589")),
            "{four:?}"
        );
        let close = warnings("secondary_carrier = 1585\nextra_carriers = [1589]");
        assert_eq!(close.len(), 1, "{close:?}");
        assert!(close[0].contains("1584 and 1585 are 25 kHz apart"));
        assert!(warnings("secondary_carrier = 1586\nextra_carriers = [1590]").is_empty());

        // The RX LO: 30 kHz above the main uplink, clear of every channel; 5 kHz above it, inside.
        let plan = "secondary_carrier = 1586\nextra_carriers = [1590]";
        assert!(soapy_cell(plan, 75_000.0, 50_000.0).carrier_plan_warnings().is_empty());
        let lo = soapy_cell(plan, 75_000.0, 25_000.0).carrier_plan_warnings();
        assert_eq!(lo.len(), 1, "{lo:?}");
        assert!(lo[0].contains("5.0 kHz from the uplink of carrier 1584"), "{lo:?}");
        assert!(
            soapy_cell("secondary_carrier = 1586", 25_000.0, 5_000.0).carrier_plan_warnings().is_empty(),
            "nothing with two carriers"
        );
    }

    #[test]
    fn wap_section_parses_with_subsections() {
        let toml = minimal_toml("")
            + r#"
[wap]
enabled = true
gateway_ipv4 = "10.0.0.1"
mtu = 1500
debug_udp_listen = "0.0.0.0:9200"
debug_udp_allowed_sources = ["10.33.1.0/24"]
debug_issi = 9990

[wap.wtp]
sar = "off"
group_size = 4

[wap.browse]
enabled = true
allowed_issis = [2260618]
"#;
        let cfg = from_toml_str(&toml).expect("wap section must parse");
        assert!(cfg.wap.enabled);
        assert_eq!(cfg.wap.mtu, 1500);
        assert_eq!(cfg.wap.debug_udp_listen, Some("0.0.0.0:9200".parse().unwrap()));
        assert_eq!(cfg.wap.wtp.sar, crate::bluestation::WapSarMode::Off);
        assert_eq!(cfg.wap.wtp.group_size, 4);
        assert!(cfg.wap.browse_allowed(2260618));
        assert!(!cfg.wap.browse_allowed(9990));
    }

    #[test]
    fn wap_browse_without_wap_rejected() {
        let toml = minimal_toml("") + "\n[wap]\n[wap.browse]\nenabled = true\n";
        assert!(from_toml_str(&toml).is_err(), "browse needs [wap] enabled");
    }

    #[test]
    fn wap_bad_mtu_rejected() {
        let toml = minimal_toml("") + "\n[wap]\nmtu = 1000\n";
        assert!(from_toml_str(&toml).is_err(), "mtu must be one of the SNDCP values");
    }

    #[test]
    fn wap_unknown_keys_rejected() {
        for block in [
            "\n[wap]\nbogus = 1\n",
            "\n[wap]\n[wap.wtp]\nbogus = 1\n",
            "\n[wap]\n[wap.browse]\nbogus = 1\n",
        ] {
            let toml = minimal_toml("") + block;
            assert!(from_toml_str(&toml).is_err(), "unknown key must be rejected: {block}");
        }
    }

    #[test]
    fn wap_lan_debug_udp_without_sources_rejected() {
        let lan = minimal_toml("") + "\n[wap]\ndebug_udp_listen = \"0.0.0.0:9200\"\ndebug_udp_allowed_sources = []\n";
        assert!(from_toml_str(&lan).is_err(), "LAN debug bearer needs a source list");
        let lo = minimal_toml("") + "\n[wap]\ndebug_udp_listen = \"127.0.0.1:9200\"\ndebug_udp_allowed_sources = []\n";
        assert!(from_toml_str(&lo).is_ok(), "loopback debug bearer may accept any local source");
    }
}
