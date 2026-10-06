//! Texts of the SDS command replies `ip` and `info` (a U-STATUS to ISSI 9999): what the station
//! knows of itself and of its host, gathered at once on the stack's thread (cheap reads, no
//! network, no waiting) and laid out for the radio.
//!
//! The replies are SDS-TL simple text in ISO-8859-1 with printable ASCII only (see
//! `SdsBsSubentity::send_text_sds`). One SDS type 4 carries at most 2047 bits: 251 characters
//! after the 4 octets of SDS-TL header. Each reply stays within [`SDS_TEXT_MAX`], and `info` is
//! sent as numbered parts ("1/3 ...") that each fit.

use std::collections::HashSet;
use std::net::Ipv4Addr;

use tetra_config::bluestation::SharedConfig;

use crate::sndcp::wap_status::compact_uptime;

/// Longest text of one reply SDS. The PDU takes 251 characters; 160 keeps every part readable
/// on a terminal that shows less of a message.
pub const SDS_TEXT_MAX: usize = 160;

/// Whether a carrier is on the air (`[cell_info] secondary_carrier_on_demand`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Air {
    On,
    WarmingUp,
    Off,
}

impl Air {
    fn as_str(self) -> &'static str {
        match self {
            Air::On => "on",
            Air::WarmingUp => "warming",
            Air::Off => "off",
        }
    }
}

/// What the `info` reply tells; `None` for what this host cannot say.
#[derive(Debug, Clone, PartialEq)]
pub struct StationFacts {
    pub version: String,
    /// Overall health, as on the dashboard: "OK", "DEGRADED" or "CRITICAL".
    pub health: String,
    pub uptime_secs: u64,
    pub host_uptime_secs: Option<u64>,
    pub mcc: u16,
    pub mnc: u16,
    pub location_area: u16,
    pub colour_code: u8,
    pub radios: usize,
    pub calls: usize,
    /// Main carrier first.
    pub carriers: Vec<(u16, Air)>,
    /// `None` without a `[brew]` section, else whether the backhaul is connected.
    pub brew: Option<bool>,
    pub lan_ip: Option<String>,
    pub public_ip: Option<Ipv4Addr>,
    pub cpu_temp_c: Option<f32>,
    pub load: Option<[f32; 3]>,
    /// Available and total, MiB.
    pub memory_mib: Option<(u64, u64)>,
    /// Free and total, GB.
    pub disk_gb: Option<(f64, f64)>,
}

impl StationFacts {
    /// The station now, from its config, its shared state, the health registry and the host.
    pub fn gather(config: &SharedConfig) -> Self {
        use crate::sys_telemetry as host;
        let cfg = config.config();
        let reg = crate::health::registry();
        let health = reg
            .snapshot(&crate::sndcp::wapgw::snapshot::thresholds(&cfg))
            .overall
            .as_str()
            .to_ascii_uppercase();
        let on_demand = cfg.cell.secondary_carrier_on_demand && cfg.cell.secondary_carrier.is_some();
        let carriers = cfg
            .cell
            .carriers()
            .into_iter()
            .enumerate()
            .map(|(i, carrier)| {
                let air = if i == 0 || !on_demand {
                    Air::On
                } else {
                    match reg.carrier_air(carrier) {
                        Some(2) => Air::On,
                        Some(1) => Air::WarmingUp,
                        _ => Air::Off,
                    }
                };
                (carrier, air)
            })
            .collect();
        let (radios, calls, brew_up) = {
            let state = config.state_read();
            // One entry per group and per party of a call: count the traffic channels.
            let calls = state.active_call_ts.values().collect::<HashSet<_>>().len();
            (state.subscribers.all_registered_issis().count(), calls, state.network_connected)
        };
        Self {
            version: tetra_core::STACK_VERSION.to_string(),
            health,
            uptime_secs: reg.uptime_secs(),
            host_uptime_secs: host::host_uptime_secs(),
            mcc: cfg.net.mcc,
            mnc: cfg.net.mnc,
            location_area: cfg.cell.location_area,
            colour_code: cfg.cell.colour_code,
            radios,
            calls,
            carriers,
            brew: cfg.brew.as_ref().map(|_| brew_up),
            lan_ip: host::primary_ip(),
            public_ip: crate::public_ip::get(),
            cpu_temp_c: host::cpu_temp_c(),
            load: host::load_average(),
            memory_mib: host::memory_mib(),
            disk_gb: host::disk_gb("/"),
        }
    }
}

fn public_text(ip: Option<Ipv4Addr>) -> String {
    ip.map_or_else(|| "unknown".to_string(), |ip| ip.to_string())
}

/// The `ip` reply: the LAN address and the public one.
pub fn ip_text(lan_ip: Option<&str>, public_ip: Option<Ipv4Addr>) -> String {
    format!("LAN IP {} | Public IP {}", lan_ip.unwrap_or("n/a"), public_text(public_ip))
}

/// The `info` reply, one text per SDS: the station, the radio side, the host.
pub fn info_texts(f: &StationFacts) -> Vec<String> {
    let mut station = vec![
        format!("FlowStation {}", f.version),
        format!("Health {}", f.health),
        format!("Up {}", compact_uptime(f.uptime_secs)),
    ];
    station.extend(f.host_uptime_secs.map(|s| format!("Host up {}", compact_uptime(s))));
    station.push(format!("MCC {} MNC {} LA {} CC {}", f.mcc, f.mnc, f.location_area, f.colour_code));

    let carriers: Vec<String> = f.carriers.iter().map(|(c, air)| format!("{c} {}", air.as_str())).collect();
    let radio = vec![
        format!("Radios {}", f.radios),
        format!("Calls {}", f.calls),
        format!("Carriers {}", carriers.join(", ")),
        match f.brew {
            None => "Brew off".to_string(),
            Some(true) => "Brew up".to_string(),
            Some(false) => "Brew down".to_string(),
        },
    ];

    let mut host = vec![
        format!("LAN {}", f.lan_ip.as_deref().unwrap_or("n/a")),
        format!("Public {}", public_text(f.public_ip)),
    ];
    host.extend(f.cpu_temp_c.map(|t| format!("CPU {t:.1}C")));
    host.extend(f.load.map(|[a, b, c]| format!("Load {a:.2} {b:.2} {c:.2}")));
    host.extend(f.memory_mib.map(|(free, total)| format!("RAM free {free}/{total}MB")));
    host.extend(f.disk_gb.map(|(free, total)| format!("Disk free {free:.1}/{total:.1}GB")));

    let parts = [station, radio, host];
    let n = parts.len();
    parts
        .iter()
        .enumerate()
        .map(|(i, fields)| {
            let mut text = format!("{}/{n} {}", i + 1, fields.join(" | "));
            // ASCII only, so cutting by bytes is cutting by characters.
            text.truncate(SDS_TEXT_MAX);
            text
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The .75 on 06-10, with what its host reported (see the sys_telemetry tests).
    fn pi75() -> StationFacts {
        StationFacts {
            version: "v0.4.0-838b5e8".to_string(),
            health: "OK".to_string(),
            uptime_secs: 3_725,
            host_uptime_secs: Some(4_043),
            mcc: 214,
            mnc: 9999,
            location_area: 1,
            colour_code: 1,
            radios: 3,
            calls: 1,
            carriers: vec![(1528, Air::On), (1530, Air::On), (1533, Air::Off)],
            brew: Some(true),
            lan_ip: Some("10.33.1.75".to_string()),
            public_ip: Some(Ipv4Addr::new(84, 127, 123, 185)),
            cpu_temp_c: Some(47.2),
            load: Some([0.20, 0.15, 0.16]),
            memory_mib: Some((1662, 2010)),
            disk_gb: Some((1.09, 11.76)),
        }
    }

    #[test]
    fn ip_reply_gives_lan_and_public() {
        assert_eq!(
            ip_text(Some("10.33.1.75"), Some(Ipv4Addr::new(84, 127, 123, 185))),
            "LAN IP 10.33.1.75 | Public IP 84.127.123.185"
        );
        // Not known yet (first lookup still running, or no Internet): said so.
        assert_eq!(ip_text(Some("10.33.1.75"), None), "LAN IP 10.33.1.75 | Public IP unknown");
        assert_eq!(ip_text(None, None), "LAN IP n/a | Public IP unknown");
    }

    #[test]
    fn info_reply_in_three_numbered_parts() {
        assert_eq!(
            info_texts(&pi75()),
            [
                "1/3 FlowStation v0.4.0-838b5e8 | Health OK | Up 1h02m | Host up 1h07m | MCC 214 MNC 9999 LA 1 CC 1",
                "2/3 Radios 3 | Calls 1 | Carriers 1528 on, 1530 on, 1533 off | Brew up",
                "3/3 LAN 10.33.1.75 | Public 84.127.123.185 | CPU 47.2C | Load 0.20 0.15 0.16 | RAM free 1662/2010MB | Disk free 1.1/11.8GB",
            ]
        );
    }

    #[test]
    fn info_reply_without_host_facts() {
        // A host without /proc, thermal zones or a route (a development PC), no [brew].
        let facts = StationFacts {
            host_uptime_secs: None,
            carriers: vec![(1528, Air::On)],
            brew: None,
            lan_ip: None,
            public_ip: None,
            cpu_temp_c: None,
            load: None,
            memory_mib: None,
            disk_gb: None,
            ..pi75()
        };
        assert_eq!(
            info_texts(&facts),
            [
                "1/3 FlowStation v0.4.0-838b5e8 | Health OK | Up 1h02m | MCC 214 MNC 9999 LA 1 CC 1",
                "2/3 Radios 3 | Calls 1 | Carriers 1528 on | Brew off",
                "3/3 LAN n/a | Public unknown",
            ]
        );
    }

    #[test]
    fn every_part_fits_one_sds_at_its_longest() {
        let worst = StationFacts {
            version: "v0.4.10-0123456789ab-modified".to_string(),
            health: "CRITICAL".to_string(),
            uptime_secs: 999 * 86_400 + 23 * 3_600,
            host_uptime_secs: Some(9_999 * 86_400),
            mcc: 1023,
            mnc: 16383,
            location_area: 16383,
            colour_code: 63,
            radios: 9_999,
            calls: 99,
            carriers: (0..6).map(|i| (3000 + i, Air::WarmingUp)).collect(),
            brew: Some(false),
            lan_ip: Some("192.168.100.200".to_string()),
            public_ip: Some(Ipv4Addr::new(255, 255, 255, 255)),
            cpu_temp_c: Some(-12.5),
            load: Some([123.45, 100.0, 99.99]),
            memory_mib: Some((65_536, 65_536)),
            disk_gb: Some((1_999.9, 2_000.0)),
        };
        for text in info_texts(&worst)
            .iter()
            .chain([&ip_text(worst.lan_ip.as_deref(), worst.public_ip)])
        {
            assert!(text.len() <= SDS_TEXT_MAX, "{} chars: {text}", text.len());
            assert!(text.bytes().all(|b| (0x20..=0x7e).contains(&b)), "{text}");
        }
        assert!(info_texts(&worst)[1].contains("3005 warming"), "all six carriers");
    }
}
