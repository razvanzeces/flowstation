use std::collections::HashMap;
use std::net::Ipv4Addr;

use serde::Deserialize;
use toml::Value;

/// Largest dynamic address pool (one PDP context per address).
pub const PACKET_DATA_MAX_POOL: u32 = 1024;

/// Where the radios' packet data goes after an SN-DATA TRANSMIT RESPONSE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDataBearer {
    /// On the common control channel, no channel assigned (clause 28.3.4.2 NOTE 2).
    Mcch,
    /// On a packet-data channel of the main carrier or of the packet-data carrier
    /// (`pdch_max_slots` timeslots at most per radio), which voice takes back.
    Pdch,
}

/// `[packet_data]`: the SNDCP packet-data bearer (PDP contexts, SN-UNITDATA) that carries the
/// radios' IPv4 datagrams to the `[wap]` gateway. The gateway address and the MTU announced in
/// the SN-ACTIVATE PDP CONTEXT ACCEPT come from `[wap]` (`gateway_ipv4`, `mtu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfgPacketData {
    pub enabled: bool,
    /// First address of the dynamic IPv4 pool.
    pub pool_first: Ipv4Addr,
    /// Last address of the dynamic IPv4 pool.
    pub pool_last: Ipv4Addr,
    /// READY timer announced in the ACCEPT (EN 300 392-2 table 28.112): 8 = 10 s, 9 = 20 s,
    /// 10 = 30 s, 11 = 60 s. The station's own READY timer runs 2 s shorter.
    pub ready_timer_code: u8,
    pub bearer: PacketDataBearer,
    /// Main-carrier timeslots a PDCH may take, in order of preference (2..=4).
    pub pdch_timeslots: Vec<u8>,
    /// A PDCH without data for this long goes back to the pool (voice takes it earlier if needed).
    pub pdch_idle_release_secs: u32,
    /// Timeslots one radio's PDCH may have (1..=4), within the full capability the radio declares
    /// in its resource request: at most the distinct slots of `pdch_timeslots` (the main carrier's
    /// ts1 is the MCCH), or of `pdch_carrier_timeslots` when a packet-data carrier is in use, one
    /// traffic slot is left for voice when no other carrier has a free one, and 1 with the bearer
    /// on the MCCH. See `StackConfig::pdch_slots_per_radio`.
    pub pdch_max_slots: u8,
    /// The packet-data carrier: packet-data channels on this carrier too (all four slots), which
    /// must be a carrier of the cell in use other than the main one (`cell_info.secondary_carrier`
    /// or one of `cell_info.extra_carriers`); otherwise ignored.
    pub pdch_carrier: Option<u16>,
    /// Timeslots of the packet-data carrier a PDCH may take, in order of preference (1..=4, one of
    /// 2..=4 at least: a PDCH never transmits uplink on ts1 of that carrier).
    pub pdch_carrier_timeslots: Vec<u8>,
    /// Voice never goes to the packet-data carrier (a carrier for data only).
    pub pdch_carrier_exclusive: bool,
}

impl Default for CfgPacketData {
    fn default() -> Self {
        Self {
            enabled: false,
            pool_first: Ipv4Addr::new(10, 0, 0, 2),
            pool_last: Ipv4Addr::new(10, 0, 0, 254),
            ready_timer_code: 10,
            bearer: PacketDataBearer::Mcch,
            pdch_timeslots: vec![4, 3, 2],
            pdch_idle_release_secs: 10,
            pdch_max_slots: 1,
            pdch_carrier: None,
            pdch_carrier_timeslots: vec![4, 3, 2, 1],
            pdch_carrier_exclusive: false,
        }
    }
}

impl CfgPacketData {
    /// Whether `ip` is in the dynamic pool.
    pub fn in_pool(&self, ip: Ipv4Addr) -> bool {
        (u32::from(self.pool_first)..=u32::from(self.pool_last)).contains(&u32::from(ip))
    }

    /// Distinct main-carrier timeslots (2..=4) of `pdch_timeslots`.
    pub fn pdch_timeslot_count(&self) -> u8 {
        (2..=4u8).filter(|ts| self.pdch_timeslots.contains(ts)).count() as u8
    }

    /// Timeslots a radio's packet-data channel of the main carrier may have: `pdch_max_slots`
    /// within the distinct slots of `pdch_timeslots`, 1 with the bearer on the MCCH. With a
    /// packet-data carrier in use, `StackConfig::pdch_slots_per_radio` counts its slots too.
    pub fn pdch_slots_per_radio(&self) -> u8 {
        match self.bearer {
            PacketDataBearer::Pdch => self.pdch_max_slots.min(self.pdch_timeslot_count()).max(1),
            PacketDataBearer::Mcch => 1,
        }
    }
}

/// Duration of a READY timer code (table 28.112), in milliseconds. None for the reserved codes.
pub fn ready_timer_ms(code: u8) -> Option<u64> {
    Some(match code {
        1 => 200,
        2 => 500,
        3 => 700,
        4 => 1_000,
        5 => 2_000,
        6 => 3_000,
        7 => 5_000,
        8 => 10_000,
        9 => 20_000,
        10 => 30_000,
        11 => 60_000,
        12 => 120_000,
        13 => 180_000,
        14 => 300_000,
        _ => return None,
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgPacketDataDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_pool_first")]
    pub pool_first: String,
    #[serde(default = "default_pool_last")]
    pub pool_last: String,
    #[serde(default = "default_ready_timer_code")]
    pub ready_timer_code: u8,
    #[serde(default = "default_bearer")]
    pub bearer: String,
    #[serde(default = "default_pdch_timeslots")]
    pub pdch_timeslots: Vec<u8>,
    #[serde(default = "default_pdch_idle_release_secs")]
    pub pdch_idle_release_secs: u32,
    #[serde(default = "default_pdch_max_slots")]
    pub pdch_max_slots: u8,
    #[serde(default)]
    pub pdch_carrier: Option<u16>,
    #[serde(default = "default_pdch_carrier_timeslots")]
    pub pdch_carrier_timeslots: Vec<u8>,
    #[serde(default)]
    pub pdch_carrier_exclusive: bool,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_pool_first() -> String {
    CfgPacketData::default().pool_first.to_string()
}
fn default_pool_last() -> String {
    CfgPacketData::default().pool_last.to_string()
}
fn default_ready_timer_code() -> u8 {
    CfgPacketData::default().ready_timer_code
}
fn default_bearer() -> String {
    "mcch".to_string()
}
fn default_pdch_timeslots() -> Vec<u8> {
    CfgPacketData::default().pdch_timeslots
}
fn default_pdch_idle_release_secs() -> u32 {
    CfgPacketData::default().pdch_idle_release_secs
}
fn default_pdch_max_slots() -> u8 {
    CfgPacketData::default().pdch_max_slots
}
fn default_pdch_carrier_timeslots() -> Vec<u8> {
    CfgPacketData::default().pdch_carrier_timeslots
}

fn parse_ipv4(key: &str, s: &str) -> Result<Ipv4Addr, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("packet_data: {key} {s:?} is not an IPv4 address"))
}

pub fn apply_packet_data_patch(dto: CfgPacketDataDto) -> Result<CfgPacketData, String> {
    let pool_first = parse_ipv4("pool_first", &dto.pool_first)?;
    let pool_last = parse_ipv4("pool_last", &dto.pool_last)?;
    let (first, last) = (u32::from(pool_first), u32::from(pool_last));
    if first > last {
        return Err("packet_data: pool_first must not be above pool_last".to_string());
    }
    if last - first >= PACKET_DATA_MAX_POOL {
        return Err(format!("packet_data: the pool may hold at most {PACKET_DATA_MAX_POOL} addresses"));
    }
    if ready_timer_ms(dto.ready_timer_code).is_none() {
        return Err("packet_data: ready_timer_code must be within 1..=14".to_string());
    }
    let bearer = match dto.bearer.trim().to_ascii_lowercase().as_str() {
        "mcch" => PacketDataBearer::Mcch,
        "pdch" => PacketDataBearer::Pdch,
        other => return Err(format!("packet_data: bearer {other:?} is not \"mcch\" or \"pdch\"")),
    };
    let ts = &dto.pdch_timeslots;
    if ts.is_empty() || ts.iter().any(|t| !(2..=4).contains(t)) || (1..ts.len()).any(|i| ts[..i].contains(&ts[i])) {
        return Err("packet_data: pdch_timeslots must list main-carrier timeslots 2, 3 or 4, each once".to_string());
    }
    if !(1..=300).contains(&dto.pdch_idle_release_secs) {
        return Err("packet_data: pdch_idle_release_secs must be within 1..=300".to_string());
    }
    if !(1..=4).contains(&dto.pdch_max_slots) {
        return Err("packet_data: pdch_max_slots must be within 1..=4".to_string());
    }
    // Carrier number: 12 bits (EN 300 392-2 table 21.87).
    if dto.pdch_carrier.is_some_and(|c| c > 4095) {
        return Err("packet_data: pdch_carrier must be a carrier number 0..=4095".to_string());
    }
    let cts = &dto.pdch_carrier_timeslots;
    if cts.is_empty() || cts.iter().any(|t| !(1..=4).contains(t)) || (1..cts.len()).any(|i| cts[..i].contains(&cts[i])) {
        return Err(
            "packet_data: pdch_carrier_timeslots must list timeslots 1, 2, 3 or 4 of the packet-data carrier, each once".to_string(),
        );
    }
    if !cts.iter().any(|t| (2..=4).contains(t)) {
        return Err(
            "packet_data: pdch_carrier_timeslots must include timeslot 2, 3 or 4 (a packet-data channel never transmits uplink on ts1 of that carrier)"
                .to_string(),
        );
    }
    Ok(CfgPacketData {
        enabled: dto.enabled,
        pool_first,
        pool_last,
        ready_timer_code: dto.ready_timer_code,
        bearer,
        pdch_timeslots: dto.pdch_timeslots,
        pdch_idle_release_secs: dto.pdch_idle_release_secs,
        pdch_max_slots: dto.pdch_max_slots,
        pdch_carrier: dto.pdch_carrier,
        pdch_carrier_timeslots: dto.pdch_carrier_timeslots,
        pdch_carrier_exclusive: dto.pdch_carrier_exclusive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(toml_src: &str) -> CfgPacketDataDto {
        toml::from_str(toml_src).expect("packet_data dto parses")
    }

    #[test]
    fn empty_section_matches_default() {
        let cfg = apply_packet_data_patch(dto("")).unwrap();
        assert_eq!(cfg, CfgPacketData::default());
        assert!(!cfg.enabled);
        assert!(cfg.in_pool(Ipv4Addr::new(10, 0, 0, 2)) && cfg.in_pool(Ipv4Addr::new(10, 0, 0, 254)));
        assert!(!cfg.in_pool(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn bad_pools_rejected() {
        assert!(apply_packet_data_patch(dto("pool_first = \"10.0.0.9\"\npool_last = \"10.0.0.8\"")).is_err());
        assert!(apply_packet_data_patch(dto("pool_first = \"10.0.0.0\"\npool_last = \"10.0.4.0\"")).is_err());
        assert!(apply_packet_data_patch(dto("pool_first = \"10.0.0\"")).is_err());
        let one = apply_packet_data_patch(dto("pool_first = \"10.0.0.7\"\npool_last = \"10.0.0.7\"")).unwrap();
        assert!(one.in_pool(Ipv4Addr::new(10, 0, 0, 7)));
        assert!(apply_packet_data_patch(dto("pool_first = \"10.0.0.0\"\npool_last = \"10.0.3.255\"")).is_ok());
    }

    #[test]
    fn ready_timer_code_range() {
        assert!(apply_packet_data_patch(dto("ready_timer_code = 0")).is_err());
        assert!(apply_packet_data_patch(dto("ready_timer_code = 15")).is_err());
        assert_eq!(apply_packet_data_patch(dto("ready_timer_code = 8")).unwrap().ready_timer_code, 8);
        assert_eq!(ready_timer_ms(10), Some(30_000));
    }

    #[test]
    fn pdch_keys() {
        let default = apply_packet_data_patch(dto("")).unwrap();
        assert_eq!(
            (default.bearer, default.pdch_timeslots, default.pdch_idle_release_secs),
            (PacketDataBearer::Mcch, vec![4, 3, 2], 10)
        );
        let pdch = apply_packet_data_patch(dto("bearer = \"PDCH\"
pdch_timeslots = [3]
pdch_idle_release_secs = 300"))
        .unwrap();
        assert_eq!((pdch.bearer, pdch.pdch_timeslots), (PacketDataBearer::Pdch, vec![3]));
        for bad in [
            "bearer = \"tch\"",
            "pdch_timeslots = []",
            "pdch_timeslots = [1]",
            "pdch_timeslots = [5]",
            "pdch_timeslots = [2, 3, 2]",
            "pdch_idle_release_secs = 0",
            "pdch_idle_release_secs = 301",
        ] {
            assert!(apply_packet_data_patch(dto(bad)).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn pdch_max_slots_range_and_default() {
        assert_eq!(apply_packet_data_patch(dto("")).unwrap().pdch_max_slots, 1);
        for n in 1..=4u8 {
            assert_eq!(
                apply_packet_data_patch(dto(&format!("pdch_max_slots = {n}")))
                    .unwrap()
                    .pdch_max_slots,
                n
            );
        }
        for bad in ["pdch_max_slots = 0", "pdch_max_slots = 5"] {
            assert_eq!(
                apply_packet_data_patch(dto(bad)),
                Err("packet_data: pdch_max_slots must be within 1..=4".to_string())
            );
        }
    }

    #[test]
    fn pdch_carrier_keys() {
        let default = apply_packet_data_patch(dto("")).unwrap();
        assert_eq!(
            (default.pdch_carrier, default.pdch_carrier_timeslots, default.pdch_carrier_exclusive),
            (None, vec![4, 3, 2, 1], false)
        );
        let on = apply_packet_data_patch(dto("pdch_carrier = 1598
pdch_carrier_timeslots = [4, 3, 2, 1]
pdch_carrier_exclusive = true"))
        .unwrap();
        assert_eq!(
            (on.pdch_carrier, on.pdch_carrier_timeslots, on.pdch_carrier_exclusive),
            (Some(1598), vec![4, 3, 2, 1], true)
        );
        assert_eq!(
            apply_packet_data_patch(dto("pdch_carrier = 4096")),
            Err("packet_data: pdch_carrier must be a carrier number 0..=4095".to_string())
        );
        for bad in ["[]", "[0]", "[5]", "[1, 1]"] {
            assert_eq!(
                apply_packet_data_patch(dto(&format!("pdch_carrier_timeslots = {bad}"))),
                Err(
                    "packet_data: pdch_carrier_timeslots must list timeslots 1, 2, 3 or 4 of the packet-data carrier, each once"
                        .to_string()
                ),
                "{bad}"
            );
        }
        assert!(
            apply_packet_data_patch(dto("pdch_carrier_timeslots = [1]"))
                .unwrap_err()
                .contains("must include timeslot 2, 3 or 4"),
            "never ts1 alone"
        );
        for good in ["[2]", "[1, 4]", "[4, 3, 2, 1]"] {
            assert!(
                apply_packet_data_patch(dto(&format!("pdch_carrier_timeslots = {good}"))).is_ok(),
                "{good}"
            );
        }
    }

    /// At most the distinct main-carrier slots of `pdch_timeslots`, and 1 on the MCCH.
    #[test]
    fn pdch_slots_per_radio_is_clamped() {
        let cfg = |bearer: PacketDataBearer, slots: &[u8], max: u8| CfgPacketData {
            bearer,
            pdch_timeslots: slots.to_vec(),
            pdch_max_slots: max,
            ..CfgPacketData::default()
        };
        assert_eq!(cfg(PacketDataBearer::Mcch, &[4, 3, 2], 3).pdch_slots_per_radio(), 1);
        assert_eq!(cfg(PacketDataBearer::Pdch, &[4, 3, 2], 4).pdch_slots_per_radio(), 3);
        assert_eq!(cfg(PacketDataBearer::Pdch, &[4, 3, 2], 2).pdch_slots_per_radio(), 2);
        assert_eq!(cfg(PacketDataBearer::Pdch, &[3], 3).pdch_slots_per_radio(), 1);
        assert_eq!(
            cfg(PacketDataBearer::Pdch, &[4, 4, 3], 3).pdch_slots_per_radio(),
            2,
            "distinct slots"
        );
        assert_eq!(CfgPacketData::default().pdch_slots_per_radio(), 1);
    }
}
