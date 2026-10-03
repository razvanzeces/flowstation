use serde::Deserialize;
use std::sync::{Arc, RwLock};
use tetra_core::freqs::FreqInfo;

use crate::bluestation::{
    CfgAsterisk, CfgCellInfo, CfgControl, CfgDapnet, CfgEmergency, CfgGeoalarm, CfgHealth, CfgNetInfo, CfgPhyIo, CfgRecovery, CfgSecurity,
    CfgSnomNotify, CfgTpg2200Action, CfgWap, CfgWxService, PhyBackend, RX_LO_OFFSET_HZ, StackState,
};

use super::sec_brew::CfgBrew;
use super::sec_dashboard::CfgDashboard;
use super::sec_packet_data::{CfgPacketData, PacketDataBearer};
use super::sec_telegram::CfgTelegram;
use super::sec_telemetry::CfgTelemetry;

/// Wrapper for a string that should be treated as a secret. Display and Debug will redact the actual value,
/// to prevent accidental logging of secrets.
#[derive(Clone)]
pub struct SecretField {
    pub val: String,
}

impl From<String> for SecretField {
    fn from(val: String) -> Self {
        Self { val }
    }
}

impl From<SecretField> for String {
    fn from(secret: SecretField) -> Self {
        secret.val
    }
}

impl AsRef<str> for SecretField {
    fn as_ref(&self) -> &str {
        &self.val
    }
}

impl std::fmt::Display for SecretField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "********")
    }
}

impl std::fmt::Debug for SecretField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretField").field("val", &"********").finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum StackMode {
    Bs,
    Ms,
    Mon,
}

#[derive(Debug, Clone)]
pub struct StackConfig {
    pub stack_mode: StackMode,
    pub debug_log: Option<String>,

    /// Optional explicit systemd service unit name (e.g. "tetra", "tetra-flowstation",
    /// "bluestation"). Used by SDS command control (restart/shutdown) and dashboard
    /// OTA update. When unset, FlowStation auto-detects the unit from /proc/self/cgroup,
    /// then falls back to "tetra". Override via env var FLOWSTATION_SERVICE_UNIT also works.
    pub service_name: Option<String>,

    pub phy_io: CfgPhyIo,
    pub net: CfgNetInfo,
    pub cell: CfgCellInfo,

    /// Brew protocol (TetraPack/BrandMeister) configuration
    pub brew: Option<CfgBrew>,

    /// Asterisk SIP/RTP bridge configuration.
    pub asterisk: CfgAsterisk,

    /// DAPNET inbound-message forwarding configuration.
    pub dapnet: CfgDapnet,

    /// Geo-fence alarm configuration for TETRA/MeshCom positions.
    pub geoalarm: CfgGeoalarm,

    /// Token-protected ActionURL trigger for Motorola TPG2200 Call-Out.
    pub tpg2200_action: CfgTpg2200Action,

    /// Snom XML minibrowser notifications via Asterisk AMI PJSIPNotify.
    pub snom_notify: CfgSnomNotify,

    /// Dashboard HTTP server configuration (None = disabled)
    pub dashboard: Option<CfgDashboard>,

    /// Telemetry endpoint configuration
    pub telemetry: Option<CfgTelemetry>,

    /// Control endpoint configuration
    pub control: Option<CfgControl>,

    /// Access control / security configuration
    pub security: CfgSecurity,

    /// Built-in WX/METAR SDS service configuration
    pub wx_service: CfgWxService,

    /// Restart-recovery configuration (proactive cold-start re-registration). Default disabled.
    pub recovery: CfgRecovery,

    /// Telegram alerts configuration (None = no `[telegram_alerts]` section in the config file).
    pub telegram: Option<CfgTelegram>,

    /// Lite stack-health monitor configuration. Always present (defaults: monitor ON, watchdog
    /// restart OFF).
    pub health: CfgHealth,

    /// Emergency-state handling. Always present (defaults: LOCAL-only — no Brew forward,
    /// telegram_alert ON, clear_timeout_secs 30). See [`CfgEmergency`].
    pub emergency: CfgEmergency,

    /// WAP gateway for packet-data terminals. Always present (default: disabled).
    pub wap: CfgWap,

    /// SNDCP packet-data bearer to the WAP gateway. Always present (default: disabled).
    pub packet_data: CfgPacketData,
}

/// The packet-data carrier in use (`[packet_data] pdch_carrier`): packet-data channels of up to
/// four slots on a carrier of the cell other than the main one, sent there from the MCCH with a
/// channel allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdchCarrierCfg {
    pub carrier: u16,
    /// Its slots a PDCH may take, in order of preference (1..=4).
    pub timeslots: Vec<u8>,
    /// Voice never goes there.
    pub exclusive: bool,
}

/// Carriers one cell may have: the main one, the secondary and two extra.
pub const MAX_CELL_CARRIERS: usize = 4;

/// Half the band of one channel in the SDR passband: the modulator and the demodulator work at
/// 72 kHz, and the filter bank weights reach +/-35.5 kHz.
pub const CHANNEL_HALF_SPAN_HZ: f64 = 36_000.0;

impl StackConfig {
    /// The packet-data carrier in use: `[packet_data] pdch_carrier` when packet data is on with
    /// bearer "pdch" and it is a carrier of the cell in use other than the main one (the secondary
    /// or one of the extra ones, after `dual_carrier_enabled`). Anything else is ignored (logged by
    /// the SNDCP at start), never an error: the dashboard turns the secondary carrier off without
    /// touching `[packet_data]`.
    pub fn pdch_carrier(&self) -> Option<PdchCarrierCfg> {
        let pd = &self.packet_data;
        let carrier = pd.pdch_carrier?;
        (pd.enabled
            && pd.bearer == PacketDataBearer::Pdch
            && carrier != self.cell.main_carrier
            && self.cell.other_carriers().any(|c| c == carrier))
        .then(|| PdchCarrierCfg {
            carrier,
            timeslots: pd.pdch_carrier_timeslots.clone(),
            exclusive: pd.pdch_carrier_exclusive,
        })
    }

    /// Timeslots one radio's packet-data channel may have: `pdch_max_slots` within the distinct
    /// slots of `pdch_timeslots` or, with a packet-data carrier in use, of
    /// `pdch_carrier_timeslots` if more; 1 with the bearer on the MCCH.
    pub fn pdch_slots_per_radio(&self) -> u8 {
        let main = self.packet_data.pdch_slots_per_radio();
        match self.pdch_carrier() {
            Some(c) => {
                let distinct = (1..=4u8).filter(|ts| c.timeslots.contains(ts)).count() as u8;
                main.max(self.packet_data.pdch_max_slots.min(distinct).max(1))
            }
            None => main,
        }
    }

    /// Whether the UMAC runs its uplink adjacent-channel copy rules and the PHY hands the carriers
    /// of a slot over strongest burst first: with a packet-data carrier in use, or always with
    /// three or more carriers. Up to two carriers exactly `pdch_carrier().is_some()`.
    pub fn uplink_copy_rules(&self) -> bool {
        self.pdch_carrier().is_some() || self.cell.carriers().len() >= 3
    }

    /// Return BS phase-modulated carrier numbers and their DL/UL frequencies.
    pub fn bs_phase_mod_carriers(&self) -> Result<Vec<(u16, u32, u32)>, String> {
        let cell_carriers = self.cell.carriers();
        let mut carriers = Vec::with_capacity(cell_carriers.len());
        for carrier in cell_carriers {
            let freq_info = FreqInfo::from_components(
                self.cell.freq_band,
                carrier,
                self.cell.freq_offset_hz,
                self.cell.reverse_operation,
                self.cell.duplex_spacing_id,
                self.cell.custom_duplex_spacing,
            )?;
            let (dl_freq, ul_freq) = freq_info.get_freqs();
            carriers.push((carrier, dl_freq, ul_freq));
        }
        Ok(carriers)
    }

    /// Whether every frequency, with `margin_hz` on either side, is within the passband of
    /// `sample_rate_hz` centred on `center_hz`.
    fn frequencies_fit_center(center_hz: f64, sample_rate_hz: f64, freqs_hz: &[u32], margin_hz: f64) -> bool {
        let half_bw = sample_rate_hz / 2.0;
        freqs_hz.iter().all(|freq| ((*freq as f64) - center_hz).abs() + margin_hz <= half_bw)
    }

    /// A third-order intermodulation product of the carriers `n` (2a - b, or a + b - c of three
    /// others) that equals carrier `t`, written out.
    fn im3_product_on(n: &[i32], t: i32) -> Option<String> {
        for &a in n {
            for &b in n {
                if a != b && 2 * a - b == t {
                    return Some(format!("2 x {a} - {b}"));
                }
            }
        }
        for &a in n {
            for &b in n {
                for &c in n {
                    if a < b && ![a, b, t].contains(&c) && t != a && t != b && a + b - c == t {
                        return Some(format!("{a} + {b} - {c}"));
                    }
                }
            }
        }
        None
    }

    /// With three or more carriers, what in the carrier plan is legal but worth a look, logged once
    /// at start: two carriers 25 kHz apart (the receive filter takes about 7 dB off the
    /// neighbour), a third-order intermodulation product (2a - b, a + b - c) on a carrier of the
    /// cell, and with the SoapySDR backend the RX LO within 12.5 kHz of an uplink channel. Empty
    /// below three carriers.
    pub fn carrier_plan_warnings(&self) -> Vec<String> {
        let carriers = self.cell.carriers();
        if carriers.len() < 3 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, a) in carriers.iter().enumerate() {
            for b in &carriers[i + 1..] {
                if a.abs_diff(*b) == 1 {
                    out.push(format!(
                        "carriers {a} and {b} are 25 kHz apart: the receive filter takes only about 7 dB off the neighbour, expect adjacent-channel copies"
                    ));
                }
            }
        }
        // In carrier numbers: band, offset and duplex spacing are common to the cell. One line per
        // carrier hit, the first product found.
        let n: Vec<i32> = carriers.iter().map(|c| i32::from(*c)).collect();
        for &t in &n {
            if let Some(product) = Self::im3_product_on(&n, t) {
                out.push(format!(
                    "the third-order intermodulation product {product} falls on carrier {t}: avoid equal spacings"
                ));
            }
        }
        if self.phy_io.backend == PhyBackend::SoapySdr
            && let Some(soapy_cfg) = self.phy_io.soapysdr.as_ref()
            && let Ok(phase_mod) = self.bs_phase_mod_carriers()
        {
            let lo = soapy_cfg.effective_rx_center_freq_corrected().0 - RX_LO_OFFSET_HZ;
            for (carrier, _, ul) in phase_mod {
                let off = (f64::from(ul) - lo).abs();
                if off < 12_500.0 {
                    out.push(format!(
                        "the RX LO ({lo:.0} Hz, rx_center_freq - 20 kHz) is {:.1} kHz from the uplink of carrier {carrier}: it lies inside that channel",
                        off / 1000.0
                    ));
                }
            }
        }
        out
    }

    /// Validate that all required configuration fields are properly set.
    pub fn validate(&self) -> Result<(), &str> {
        // Check input device settings
        match self.phy_io.backend {
            PhyBackend::SoapySdr => {
                if self.phy_io.soapysdr.is_none() {
                    return Err("soapysdr configuration must be provided for Soapysdr backend");
                };
            }
            PhyBackend::None => {} // For testing
            PhyBackend::Undefined => {
                return Err("phy_io backend must be defined");
            }
        };

        if let Some(secondary_carrier) = self.cell.secondary_carrier
            && secondary_carrier == self.cell.main_carrier
        {
            return Err("cell.secondary_carrier must differ from cell.main_carrier");
        }

        if !self.cell.extra_carriers.is_empty() {
            if self.cell.secondary_carrier.is_none() {
                return Err("cell.extra_carriers needs cell.secondary_carrier");
            }
            let all = self.cell.carriers();
            if all.len() > MAX_CELL_CARRIERS {
                return Err("cell.extra_carriers: at most 4 carriers in the cell (main, secondary and two extra)");
            }
            if (1..all.len()).any(|i| all[..i].contains(&all[i])) {
                return Err("cell.extra_carriers must differ from each other, from main_carrier and from secondary_carrier");
            }
        }

        // Sanity check on computed BS carrier frequencies and SDR settings.
        if self.phy_io.backend == PhyBackend::SoapySdr {
            let soapy_cfg = self
                .phy_io
                .soapysdr
                .as_ref()
                .expect("SoapySdr config must be set for SoapySdr PhyIo");

            let carriers = self.bs_phase_mod_carriers().map_err(|_| "Invalid cell info frequency settings")?;
            let (main_dl, main_ul) = carriers
                .iter()
                .find(|(carrier_num, _, _)| *carrier_num == self.cell.main_carrier)
                .map(|(_, dl, ul)| (*dl, *ul))
                .ok_or("main carrier missing from computed carrier list")?;

            println!("    Derived BS carriers: {:?}\n", carriers);

            if soapy_cfg.dl_freq as u32 != main_dl {
                return Err("PhyIo DlFrequency does not match computed FreqInfo");
            };
            if soapy_cfg.ul_freq as u32 != main_ul {
                return Err("PhyIo UlFrequency does not match computed FreqInfo");
            };

            if carriers.len() > 1 {
                // A secondary carrier is in use: the SDR center + sample rate MUST be proven to cover
                // both carriers. A missing sample rate fails closed — we cannot prove the passband
                // fits, and silently skipping the check let an out-of-passband secondary carrier
                // through (defeating the dashboard toggle's pre-restart validation).
                let Some(sample_rate_hz) = soapy_cfg.fs else {
                    return Err(
                        "dual carrier requires phy_io.soapysdr.sample_rate to be set so the secondary carrier can be proven to fit the SDR passband",
                    );
                };
                let dl_freqs: Vec<u32> = carriers.iter().map(|(_, dl, _)| *dl).collect();
                let ul_freqs: Vec<u32> = carriers.iter().map(|(_, _, ul)| *ul).collect();
                let (tx_center_hz, _) = soapy_cfg.effective_tx_center_freq_corrected();
                let (rx_center_hz, _) = soapy_cfg.effective_rx_center_freq_corrected();

                if !Self::frequencies_fit_center(tx_center_hz, sample_rate_hz, &dl_freqs, 0.0) {
                    return Err("configured TX center/sample-rate do not cover all BS downlink carriers");
                }
                if !Self::frequencies_fit_center(rx_center_hz, sample_rate_hz, &ul_freqs, 0.0) {
                    return Err("configured RX center/sample-rate do not cover all BS uplink carriers");
                }
                // From three carriers on the whole channel must fit, the uplink around the LO the
                // receive filter bank is centred on (up to two carriers, today's check above).
                if carriers.len() >= 3 {
                    if !Self::frequencies_fit_center(tx_center_hz, sample_rate_hz, &dl_freqs, CHANNEL_HALF_SPAN_HZ) {
                        return Err(
                            "with three or more carriers every downlink channel (+/-36 kHz) must fit the TX passband: move tx_center_freq or raise sample_rate",
                        );
                    }
                    if !Self::frequencies_fit_center(rx_center_hz - RX_LO_OFFSET_HZ, sample_rate_hz, &ul_freqs, CHANNEL_HALF_SPAN_HZ) {
                        return Err(
                            "with three or more carriers every uplink channel (+/-36 kHz) must fit the RX passband, centred 20 kHz below rx_center_freq",
                        );
                    }
                }
            };
        }

        if self.cell.ms_txpwr_max_cell > 7 {
            return Err("ms_txpwr_max_cell must be 0-7 (3 bits)");
        }

        // Validate timezone if configured
        if let Some(ref tz) = self.cell.timezone
            && tz.parse::<chrono_tz::Tz>().is_err()
        {
            return Err("Invalid IANA timezone name in cell.timezone");
        }

        // Validate neighbor cells
        if self.cell.neighbor_cells_ca.len() > 7 {
            return Err("cell.neighbor_cells_ca supports at most 7 entries");
        }

        // Check for duplicate cell_identifier_ca and main_carrier_number
        {
            let mut seen_ids = std::collections::HashSet::new();
            let mut seen_carriers = std::collections::HashSet::new();
            for cell in &self.cell.neighbor_cells_ca {
                if !seen_ids.insert(cell.cell_identifier_ca) {
                    return Err("cell.neighbor_cells_ca: duplicate cell_identifier_ca — each neighbour must have a unique identifier");
                }
                if !seen_carriers.insert(cell.main_carrier_number) {
                    return Err("cell.neighbor_cells_ca: duplicate main_carrier_number — each neighbour must be on a different carrier");
                }
            }
        }

        for cell in &self.cell.neighbor_cells_ca {
            if cell.cell_identifier_ca > 0x1F {
                return Err("cell.neighbor_cells_ca: cell_identifier_ca must be 0-31");
            }
            if cell.cell_reselection_types_supported > 0x3 {
                return Err("cell.neighbor_cells_ca: cell_reselection_types_supported must be 0-3");
            }
            if cell.cell_load_ca > 0x3 {
                return Err("cell.neighbor_cells_ca: cell_load_ca must be 0-3");
            }
            if cell.main_carrier_number > 0xFFF {
                return Err("cell.neighbor_cells_ca: main_carrier_number must be 0-4095");
            }
            if let Some(v) = cell.main_carrier_number_extension
                && v > 0x3FF
            {
                return Err("cell.neighbor_cells_ca: main_carrier_number_extension must be 0-1023");
            }
            if let Some(v) = cell.mcc
                && v > 0x3FF
            {
                return Err("cell.neighbor_cells_ca: mcc must be 0-1023");
            }
            if let Some(v) = cell.mnc
                && v > 0x3FFF
            {
                return Err("cell.neighbor_cells_ca: mnc must be 0-16383");
            }
            if let Some(v) = cell.location_area
                && v > 0x3FFF
            {
                return Err("cell.neighbor_cells_ca: location_area must be 0-16383");
            }
            if let Some(v) = cell.maximum_ms_transmit_power
                && v > 0x7
            {
                return Err("cell.neighbor_cells_ca: maximum_ms_transmit_power must be 0-7");
            }
            if let Some(v) = cell.minimum_rx_access_level
                && v > 0xF
            {
                return Err("cell.neighbor_cells_ca: minimum_rx_access_level must be 0-15");
            }
            if let Some(v) = cell.timeshare_cell_information_or_security_parameters
                && v > 0x1F
            {
                return Err("cell.neighbor_cells_ca: timeshare_cell_information_or_security_parameters must be 0-31");
            }
            if let Some(v) = cell.tdma_frame_offset
                && v > 0x3F
            {
                return Err("cell.neighbor_cells_ca: tdma_frame_offset must be 0-63");
            }
        }

        // Restart recovery: an explicit allowlist must not exceed the cache cap (the numeric
        // ranges are already clamped in apply_recovery_patch). Only meaningful when enabled.
        if self.recovery.enabled
            && !self.recovery.issi_allowlist.is_empty()
            && self.recovery.issi_allowlist.len() as u32 > self.recovery.max_cached_issis
        {
            return Err("recovery.issi_allowlist has more entries than recovery.max_cached_issis");
        }

        Ok(())
    }
}

/// Global shared configuration: immutable config + mutable state.
#[derive(Clone)]
pub struct SharedConfig {
    /// Read-only configuration (immutable after construction).
    cfg: Arc<StackConfig>,
    /// Mutable state guarded with RwLock (write by the stack, read by others).
    state: Arc<RwLock<StackState>>,
}

impl SharedConfig {
    pub fn from_parts(cfg: StackConfig, state: Option<StackState>) -> Self {
        // Check config for validity before returning the SharedConfig object
        match cfg.validate() {
            Ok(_) => {}
            Err(e) => panic!("Invalid stack configuration: {}", e),
        }

        let mut state = state.unwrap_or_default();
        let mut carriers = cfg
            .bs_phase_mod_carriers()
            .expect("validated carrier configuration should compute")
            .into_iter()
            .map(|(carrier_num, _, _)| carrier_num)
            .collect::<Vec<_>>();
        // With three or more carriers voice fills the packet-data carrier last (the main carrier
        // stays first): calls go to the other extra carriers before taking slots data may want.
        if carriers.len() >= 3
            && let Some(p) = cfg.pdch_carrier()
        {
            carriers.retain(|c| *c != p.carrier);
            carriers.push(p.carrier);
        }
        state.timeslot_alloc.configure_carriers(&carriers);
        state
            .timeslot_alloc
            .set_data_only_carrier(cfg.pdch_carrier().filter(|p| p.exclusive).map(|p| p.carrier));

        Self {
            cfg: Arc::new(cfg),
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// Access immutable config.
    pub fn config(&self) -> Arc<StackConfig> {
        Arc::clone(&self.cfg)
    }

    /// Read guard for mutable state.
    pub fn state_read(&self) -> std::sync::RwLockReadGuard<'_, StackState> {
        self.state.read().expect("StackState RwLock blocked")
    }

    /// Write guard for mutable state.
    pub fn state_write(&self) -> std::sync::RwLockWriteGuard<'_, StackState> {
        self.state.write().expect("StackState RwLock blocked")
    }

    /// Effective `[wap]` settings: the dashboard runtime override of the browse switch and of the
    /// list of radios allowed to browse if present, otherwise the config file values. Returns an
    /// owned CfgWap so callers don't hold the state lock.
    pub fn effective_wap(&self) -> CfgWap {
        let mut wap = self.cfg.wap.clone();
        if let Some(o) = self.state_read().wap_override.as_ref() {
            wap.browse.enabled = o.browse_enabled;
            wap.browse.allowed_issis = o.browse_allowed_issis.clone();
        }
        wap
    }

    /// Effective WX/METAR service settings: the dashboard runtime override if present,
    /// otherwise the config file values. Returns an owned CfgWxService so callers don't
    /// hold the state lock.
    pub fn effective_wx_service(&self) -> crate::bluestation::CfgWxService {
        let base = self.cfg.wx_service.clone();
        if let Some(o) = self.state_read().wx_override.as_ref() {
            crate::bluestation::CfgWxService {
                enabled: o.enabled,
                service_issi: o.service_issi,
                periodic_enabled: o.periodic_enabled,
                periodic_issi: o.periodic_issi,
                periodic_is_group: o.periodic_is_group,
                periodic_icao: o.periodic_icao.clone(),
                periodic_interval_secs: o.periodic_interval_secs,
            }
        } else {
            base
        }
    }

    /// Effective Telegram alerts settings: the dashboard runtime override if present, otherwise
    /// the config file values (or defaults when there is no `[telegram_alerts]` section). Returns
    /// an owned [`CfgTelegram`] so callers don't hold the state lock. The alerter and the
    /// dashboard both read through this so a live edit applies without a restart.
    pub fn effective_telegram(&self) -> crate::bluestation::CfgTelegram {
        let base = self.cfg.telegram.clone().unwrap_or_default();
        if let Some(o) = self.state_read().telegram_override.as_ref() {
            crate::bluestation::CfgTelegram {
                enabled: o.enabled,
                bot_token: crate::bluestation::SecretField::from(o.bot_token.clone()),
                chat_ids: o.chat_ids.clone(),
                alert_connect: o.alert_connect,
                alert_disconnect: o.alert_disconnect,
                alert_t351: o.alert_t351,
                alert_lip: o.alert_lip,
                alert_backhaul: o.alert_backhaul,
                alert_critical_logs: o.alert_critical_logs,
                // Health alerts aren't part of the dashboard live-edit override yet — take the
                // base config value so the field is always populated.
                alert_health: base.alert_health,
            }
        } else {
            base
        }
    }

    /// Effective DAPNET settings: the dashboard runtime override if present, otherwise the config
    /// file values. Returns an owned [`CfgDapnet`] so callers don't hold the state lock.
    pub fn effective_dapnet(&self) -> crate::bluestation::CfgDapnet {
        let base = self.cfg.dapnet.clone();
        if let Some(o) = self.state_read().dapnet_override.as_ref() {
            crate::bluestation::CfgDapnet {
                enabled: o.enabled,
                api_url: o.api_url.clone(),
                username: o.username.clone(),
                password: crate::bluestation::SecretField::from(o.password.clone()),
                poll_interval_secs: o.poll_interval_secs.max(1),
                forward_sds: o.forward_sds,
                forward_callout: o.forward_callout,
                forward_telegram: o.forward_telegram,
                sds_source_issi: o.sds_source_issi,
                sds_dest_issi: o.sds_dest_issi,
                sds_dest_is_group: o.sds_dest_is_group,
                ric_issi_routes: o.ric_issi_routes.clone(),
                ric_gssi_routes: o.ric_gssi_routes.clone(),
                sds_allowed_rics: o.sds_allowed_rics.clone(),
                callout_allowed_rics: o.callout_allowed_rics.clone(),
                telegram_allowed_rics: o.telegram_allowed_rics.clone(),
                callout_source_issi: o.callout_source_issi,
                callout_dest_issi: o.callout_dest_issi,
                callout_incident_base: o.callout_incident_base.clamp(1, 256),
                callout_text_prefix: o.callout_text_prefix.clone(),
                telegram_prefix: o.telegram_prefix.clone(),
                rwth_core_enabled: o.rwth_core_enabled,
                rwth_core_host: o.rwth_core_host.clone(),
                rwth_core_port: o.rwth_core_port,
                rwth_core_device: o.rwth_core_device.clone(),
                rwth_core_version: o.rwth_core_version.clone(),
                rwth_core_callsign: o.rwth_core_callsign.clone(),
                rwth_core_authkey: crate::bluestation::SecretField::from(o.rwth_core_authkey.clone()),
                rwth_messages_limit: o.rwth_messages_limit.max(1),
            }
        } else {
            base
        }
    }

    /// Effective GeoAlarm settings: the dashboard runtime override if present, otherwise the
    /// config file values. Returns an owned [`CfgGeoalarm`] so callers don't hold the state lock.
    pub fn effective_geoalarm(&self) -> crate::bluestation::CfgGeoalarm {
        let base = self.cfg.geoalarm.clone();
        if let Some(o) = self.state_read().geoalarm_override.as_ref() {
            crate::bluestation::CfgGeoalarm {
                enabled: o.enabled,
                flowstation_lat: o.flowstation_lat,
                flowstation_lon: o.flowstation_lon,
                radius_m: if o.radius_m.is_finite() && o.radius_m > 0.0 {
                    o.radius_m
                } else {
                    base.radius_m
                },
                cooldown_secs: o.cooldown_secs.clamp(1, 86_400),
                trigger_tetra: o.trigger_tetra,
                trigger_meshcom: o.trigger_meshcom,
                forward_tpg2200: o.forward_tpg2200,
                forward_sds: o.forward_sds,
                forward_sip: o.forward_sip,
                forward_telegram: o.forward_telegram,
                tetra_issi_whitelist: o.tetra_issi_whitelist.clone(),
                tetra_issi_blacklist: o.tetra_issi_blacklist.clone(),
                meshcom_source_whitelist: o.meshcom_source_whitelist.clone(),
                meshcom_source_blacklist: o.meshcom_source_blacklist.clone(),
                sds_source_issi: o.sds_source_issi.max(1),
                sds_dest_issi: o.sds_dest_issi,
                sds_dest_is_group: o.sds_dest_is_group,
                tpg2200_source_issi: o.tpg2200_source_issi.max(1),
                tpg2200_dest_issi: o.tpg2200_dest_issi,
                tpg2200_incident_base: o.tpg2200_incident_base.clamp(1, 256),
                tpg2200_text_prefix: o.tpg2200_text_prefix.clone(),
                tpg2200_max_text_chars: o.tpg2200_max_text_chars.clamp(8, 160),
                sip_title_prefix: o.sip_title_prefix.clone(),
                telegram_prefix: o.telegram_prefix.clone(),
            }
        } else {
            base
        }
    }

    /// Effective Snom XML NOTIFY settings: the dashboard runtime override if present, otherwise
    /// the config file values. Returns an owned [`CfgSnomNotify`] so callers don't hold the
    /// state lock while sending AMI requests.
    pub fn effective_snom_notify(&self) -> crate::bluestation::CfgSnomNotify {
        let base = self.cfg.snom_notify.clone();
        if let Some(o) = self.state_read().snom_notify_override.as_ref() {
            crate::bluestation::CfgSnomNotify {
                enabled: o.enabled,
                ami_host: o.ami_host.clone(),
                ami_port: o.ami_port,
                ami_username: o.ami_username.clone(),
                ami_password: crate::bluestation::SecretField::from(o.ami_password.clone()),
                endpoints: o.endpoints.clone(),
                notify_sds: o.notify_sds,
                notify_dapnet: o.notify_dapnet,
                notify_telegram: o.notify_telegram,
                sds_directions: o.sds_directions.clone(),
                dapnet_allowed_rics: o.dapnet_allowed_rics.clone(),
                sds_allowed_issis: o.sds_allowed_issis.clone(),
                title_prefix: o.title_prefix.clone(),
                notify_event: o.notify_event.clone(),
                content_type: o.content_type.clone(),
                subscription_state: o.subscription_state.clone(),
                max_text_chars: o.max_text_chars.clamp(40, 2000),
                connect_timeout_secs: o.connect_timeout_secs.clamp(1, 30),
            }
        } else {
            base
        }
    }
}
