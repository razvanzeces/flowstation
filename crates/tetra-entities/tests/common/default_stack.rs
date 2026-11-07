use tetra_config::bluestation::{
    CfgAprsWx, CfgAsterisk, CfgCellInfo, CfgDapnet, CfgEmergency, CfgGeoalarm, CfgHealth, CfgMultiNetwork, CfgNetInfo, CfgPhyIo, CfgRecovery, CfgSecurity,
    CfgSnomNotify, CfgTpg2200Action, CfgWxService, PhyBackend, StackConfig, StackMode,
};
use tetra_core::{freqs::FreqInfo, ranges::SortedDisjointSsiRanges};

/// Creates a default config for testing. It can still be modified as needed
/// before passing it to the ComponentTest constructor
pub fn default_test_config_bs() -> StackConfig {
    let phy_io = default_phy_io();
    let net_info = default_net_info();
    let freq_info = FreqInfo::from_components(4, 1521, 0, false, 4, None).unwrap();
    let cell_info = default_cell_info(freq_info);

    // Put together components and return this proto config
    StackConfig {
        stack_mode: StackMode::Bs,
        debug_log: None,
        service_name: None,
        phy_io,
        net: net_info,
        cell: cell_info,
        extra_cells: Vec::new(),
        networks: Vec::new(),
        networks_explicit: false,
        network: tetra_config::bluestation::DEFAULT_NETWORK.to_owned(),
        site_linked: false,
        station_carriers: Vec::new(),
        cell_id: tetra_core::CellId::PRIMARY,
        brew: None,
        lst_dispatch: None,
        asterisk: CfgAsterisk::default(),
        dapnet: CfgDapnet::default(),
        geoalarm: CfgGeoalarm::default(),
        tpg2200_action: CfgTpg2200Action::default(),
        snom_notify: CfgSnomNotify::default(),
        dashboard: None,
        telemetry: None,
        control: None,
        security: CfgSecurity::default(),
        wx_service: CfgWxService::default(),
        aprs_wx: CfgAprsWx::default(),
        recovery: CfgRecovery::default(),
        telegram: None,
        health: CfgHealth::default(),
        emergency: CfgEmergency::default(),
        multi_network: CfgMultiNetwork::default(),
    }
}

pub fn default_phy_io() -> CfgPhyIo {
    CfgPhyIo {
        backend: PhyBackend::None,
        dl_tx_file: None,
        ul_rx_file: None,
        ul_input_file: None,
        dl_input_file: None,
        soapysdr: None,
    }
}

pub fn default_net_info() -> CfgNetInfo {
    CfgNetInfo { mcc: 204, mnc: 1337 }
}

pub fn default_cell_info(freq_info: FreqInfo) -> CfgCellInfo {
    CfgCellInfo {
        colour_code: 1,
        location_area: 2,
        dgna_attachment_mode: 0,
        main_carrier: freq_info.carrier,
        secondary_carrier: None,
        freq_band: freq_info.band,
        freq_offset_hz: freq_info.freq_offset_hz,
        duplex_spacing_id: freq_info.duplex_spacing_id,
        custom_duplex_spacing: None,
        reverse_operation: freq_info.reverse_operation,
        neighbor_cell_broadcast: 0,
        late_entry_supported: true,
        subscriber_class: 65535, // All subscriber classes allowed
        registration: true,
        deregistration: true,
        priority_cell: false,
        no_minimum_mode: false,
        migration: false,
        system_wide_services: true,
        voice_service: true,
        circuit_mode_data_service: false,
        sndcp_service: false,
        aie_service: false,
        advanced_link: false,
        system_code: 3, // 3 = ETSI EN 300 392-2 V3.1.1
        sharing_mode: 0,
        ts_reserved_frames: 0,
        u_plane_dtx: false,
        frame_18_ext: false,
        ms_txpwr_max_cell: 4,
        rxlev_access_min_dbm: -110,
        local_ssi_ranges: SortedDisjointSsiRanges::from_vec_ssirange(vec![]),
        timezone: None,
        home_mode_display: None,
        cell_reselect: Default::default(),
        sds_broadcast: None,
        wap_ip: None,
        neighbor_cells_ca: Vec::new(),
        hangtime_secs: 5,
        call_timeout_secs: 120,
        ul_inactivity_secs: 3,
        periodic_registration_secs: 3600,
        presence_check_secs: 30,
        presence_response_secs: 30,
        sds_command_control: None,
        release_group_on_same_speaker_retake: false,
        dgna_use_ss_facility: true,
    }
}

pub fn default_test_config_ms() -> StackConfig {
    let mut config = default_test_config_bs();
    config.stack_mode = StackMode::Ms;
    config
}
