use serde::Deserialize;
use std::collections::HashMap;

use tetra_core::ranges::{SortedDisjointSsiRanges, SsiRange};
use toml::Value;

/// Service details for a neighbor cell — mirrors BsServiceDetails but for config parsing.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CfgBsServiceDetails {
    #[serde(default)]
    pub registration: bool,
    #[serde(default)]
    pub deregistration: bool,
    #[serde(default)]
    pub priority_cell: bool,
    #[serde(default)]
    pub no_minimum_mode: bool,
    #[serde(default)]
    pub migration: bool,
    #[serde(default)]
    pub system_wide_services: bool,
    #[serde(default)]
    pub voice_service: bool,
    #[serde(default)]
    pub circuit_mode_data_service: bool,
    #[serde(default)]
    pub sndcp_service: bool,
    #[serde(default)]
    pub aie_service: bool,
    #[serde(default)]
    pub advanced_link: bool,
}

/// Configuration for a single CA neighbor cell, included in D-NWRK-BROADCAST.
/// Per ETSI EN 300 392-2 clause 18.5.17 / Table 18.64.
#[derive(Debug, Clone, Deserialize)]
pub struct CfgNeighborCellCa {
    /// 5 bits — cell identifier within the CA cluster (0-31)
    pub cell_identifier_ca: u8,
    /// 2 bits — cell reselection types supported (0-3)
    pub cell_reselection_types_supported: u8,
    /// 1 bit — true if this neighbor is time-synchronized with us
    pub neighbor_cell_synchronized: bool,
    /// 2 bits — current load indicator (0=low, 3=high)
    pub cell_load_ca: u8,
    /// 12 bits — main carrier number of the neighbor cell (0-4095)
    pub main_carrier_number: u16,

    /// Optional: carrier number extension (10 bits, 0-1023)
    pub main_carrier_number_extension: Option<u16>,
    /// Optional: MCC of the neighbor (10 bits, 0-1023)
    pub mcc: Option<u16>,
    /// Optional: MNC of the neighbor (14 bits, 0-16383)
    pub mnc: Option<u16>,
    /// Optional: location area of the neighbor (14 bits, 0-16383)
    pub location_area: Option<u16>,
    /// Optional: max MS TX power allowed in neighbor cell (3 bits, 0-7)
    pub maximum_ms_transmit_power: Option<u8>,
    /// Optional: minimum RX level for access (4 bits, 0-15)
    pub minimum_rx_access_level: Option<u8>,
    /// Optional: subscriber class mask (16 bits)
    pub subscriber_class: Option<u16>,
    /// Optional: BS service details for the neighbor cell
    pub bs_service_details: Option<CfgBsServiceDetails>,
    /// Optional: timeshare/security parameters (5 bits, 0-31)
    pub timeshare_cell_information_or_security_parameters: Option<u8>,
    /// Optional: TDMA frame offset relative to this cell (6 bits, 0-63)
    pub tdma_frame_offset: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct CfgCellInfo {
    // 2 bits, from 18.4.2.1 D-MLE-SYNC
    pub neighbor_cell_broadcast: u8,
    // 2 bits, from 18.4.2.1 D-MLE-SYNC
    pub late_entry_supported: bool,

    /// 12 bits, from MAC SYSINFO
    pub main_carrier: u16,
    /// 4 bits, from MAC SYSINFO
    pub freq_band: u8,
    /// Offset in Hz from 25kHz aligned carrier. Options: 0, 6250, -6250, 12500 Hz
    /// Represented as 0-3 in SYSINFO
    pub freq_offset_hz: i16,
    /// Index in duplex setting table. Sent in SYSINFO. Maps to a specific duplex spacing in Hz.
    /// Custom spacing can be provided optionally by setting
    pub duplex_spacing_id: u8,
    /// Custom duplex spacing in Hz, for users that use a modified, non-standard duplex spacing table.
    pub custom_duplex_spacing: Option<u32>,
    /// 1 bits, from MAC SYSINFO
    pub reverse_operation: bool,

    // 14 bits, from 18.4.2.2 D-MLE-SYSINFO
    pub location_area: u16,
    // 16 bits, from 18.4.2.2 D-MLE-SYSINFO
    pub subscriber_class: u16,

    // 1-bit service flags
    pub registration: bool,
    pub deregistration: bool,
    pub priority_cell: bool,
    pub no_minimum_mode: bool,
    pub migration: bool,
    pub system_wide_services: bool,
    pub voice_service: bool,
    pub circuit_mode_data_service: bool,
    pub sndcp_service: bool,
    pub aie_service: bool,
    pub advanced_link: bool,

    // From SYNC
    pub system_code: u8,
    pub colour_code: u8,
    pub sharing_mode: u8,
    pub ts_reserved_frames: u8,
    pub u_plane_dtx: bool,
    pub frame_18_ext: bool,

    pub ms_txpwr_max_cell: u8,

    pub local_ssi_ranges: SortedDisjointSsiRanges,

    /// IANA timezone name (e.g. "Europe/Amsterdam"). When set, enables D-NWRK-BROADCAST
    /// time broadcasting so MSs can synchronize their clocks.
    pub timezone: Option<String>,

    /// Neighbor cells to include in D-NWRK-BROADCAST for cell reselection.
    /// Up to 7 entries. MSs use this list to find alternative cells when signal degrades.
    pub neighbor_cells_ca: Vec<CfgNeighborCellCa>,

    /// Group call hangtime in seconds: how long an idle group call circuit stays open
    /// after the last speaker releases the floor, before the call is torn down.
    /// During hangtime, any MS can retake the floor without a new D-SETUP/D-CONNECT cycle.
    /// Default: 5 seconds. Range: 0–300.
    pub hangtime_secs: u32,

    /// Maximum active call duration in seconds (ETSI T310 equivalent, EN 300 392-2 §14.9.1).
    /// After this time the BS sends D-RELEASE regardless of call activity.
    /// Shorter values free up timeslots faster when MS leaves coverage without disconnecting.
    /// Default: 120 seconds (2 minutes). Range: 30–300.
    pub call_timeout_secs: u32,

    /// UL inactivity timeout in seconds: if no voice frames are received from the transmitting
    /// MS for this duration, the BS forces TX-CEASED and enters hangtime.
    /// Must be above T.213 (1s) to tolerate DTX and brief RF fading.
    /// Default: 3 seconds. Range: 1–30.
    pub ul_inactivity_secs: u32,
}

#[derive(Default, Deserialize)]
pub struct CellInfoDto {
    pub main_carrier: u16,
    pub freq_band: u8,
    pub freq_offset: i16,
    pub duplex_spacing: u8,
    pub reverse_operation: bool,
    pub custom_duplex_spacing: Option<u32>,

    pub location_area: u16,

    pub neighbor_cell_broadcast: Option<u8>,
    pub late_entry_supported: Option<bool>,
    pub subscriber_class: Option<u16>,
    pub registration: Option<bool>,
    pub deregistration: Option<bool>,
    pub priority_cell: Option<bool>,
    pub no_minimum_mode: Option<bool>,
    pub migration: Option<bool>,
    pub system_wide_services: Option<bool>,
    pub voice_service: Option<bool>,
    pub circuit_mode_data_service: Option<bool>,
    pub sndcp_service: Option<bool>,
    pub aie_service: Option<bool>,
    pub advanced_link: Option<bool>,

    pub system_code: Option<u8>,
    pub colour_code: Option<u8>,
    pub sharing_mode: Option<u8>,
    pub ts_reserved_frames: Option<u8>,
    pub u_plane_dtx: Option<bool>,
    pub frame_18_ext: Option<bool>,

    pub ms_txpwr_max_cell: Option<u8>,

    pub local_ssi_ranges: Option<Vec<(u32, u32)>>,

    pub timezone: Option<String>,

    /// Neighbor cells for D-NWRK-BROADCAST. Up to 7 entries.
    /// Parsed separately in parsing.rs from toml::Value to avoid serde flatten conflict.
    #[serde(skip)]
    pub neighbor_cells_ca: Vec<CfgNeighborCellCa>,

    /// Group call hangtime in seconds. Default: 5.
    pub hangtime_secs: Option<u32>,

    /// Active call timeout (T310) in seconds. Default: 120.
    pub call_timeout_secs: Option<u32>,

    /// UL inactivity timeout in seconds. Default: 3.
    pub ul_inactivity_secs: Option<u32>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn cell_dto_to_cfg(ci: CellInfoDto) -> CfgCellInfo {
    CfgCellInfo {
        main_carrier: ci.main_carrier,
        freq_band: ci.freq_band,
        freq_offset_hz: ci.freq_offset,
        duplex_spacing_id: ci.duplex_spacing,
        reverse_operation: ci.reverse_operation,
        custom_duplex_spacing: ci.custom_duplex_spacing,
        location_area: ci.location_area,
        neighbor_cell_broadcast: ci.neighbor_cell_broadcast.unwrap_or(0),
        late_entry_supported: ci.late_entry_supported.unwrap_or(false),
        subscriber_class: ci.subscriber_class.unwrap_or(65535), // All subscriber classes allowed
        registration: ci.registration.unwrap_or(true),
        deregistration: ci.deregistration.unwrap_or(true),
        priority_cell: ci.priority_cell.unwrap_or(false),
        no_minimum_mode: ci.no_minimum_mode.unwrap_or(false),
        migration: ci.migration.unwrap_or(false),
        system_wide_services: ci.system_wide_services.unwrap_or(false),
        voice_service: ci.voice_service.unwrap_or(true),
        circuit_mode_data_service: ci.circuit_mode_data_service.unwrap_or(false),
        sndcp_service: ci.sndcp_service.unwrap_or(false),
        aie_service: ci.aie_service.unwrap_or(false),
        advanced_link: ci.advanced_link.unwrap_or(false),
        system_code: ci.system_code.unwrap_or(3), // 3 = ETSI EN 300 392-2 V3.1.1
        colour_code: ci.colour_code.unwrap_or(0),
        sharing_mode: ci.sharing_mode.unwrap_or(0),
        ts_reserved_frames: ci.ts_reserved_frames.unwrap_or(0),
        u_plane_dtx: ci.u_plane_dtx.unwrap_or(false),
        frame_18_ext: ci.frame_18_ext.unwrap_or(false),
        ms_txpwr_max_cell: ci.ms_txpwr_max_cell.unwrap_or(4), // 30 dBm (1W), Table 18.57
        local_ssi_ranges: ci
            .local_ssi_ranges
            .map(SortedDisjointSsiRanges::from_vec_tuple)
            .unwrap_or(default_tetrapack_local_ranges()),
        timezone: ci.timezone,
        neighbor_cells_ca: ci.neighbor_cells_ca,
        hangtime_secs: ci.hangtime_secs.unwrap_or(5).clamp(0, 300),
        call_timeout_secs: ci.call_timeout_secs.unwrap_or(120).clamp(30, 300),
        ul_inactivity_secs: ci.ul_inactivity_secs.unwrap_or(3).clamp(1, 30),
    }
}

/// Default local SSI ranges are defined as 0-90 (inclusive), which fits the TetraPack configuration.
/// This helps prevent excessive flows of unroutable traffic to TetraPack, and can be overridden
/// by users if needed.
fn default_tetrapack_local_ranges() -> SortedDisjointSsiRanges {
    SortedDisjointSsiRanges::from_vec_ssirange(vec![SsiRange::new(0, 90)])
}
