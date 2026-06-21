use serde::Deserialize;
use std::sync::{Arc, RwLock};
use tetra_core::freqs::FreqInfo;

use crate::bluestation::{CfgCellInfo, CfgControl, CfgEmergency, CfgHealth, CfgNetInfo, CfgPhyIo, CfgRecovery, CfgSecurity, CfgWxService, PhyBackend, StackState};

use super::sec_dashboard::CfgDashboard;
use super::sec_brew::CfgBrew;
use super::sec_telemetry::CfgTelemetry;
use super::sec_telegram::CfgTelegram;

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
}

impl StackConfig {
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

        // Sanity check on main carrier property fields in SYSINFO
        if self.phy_io.backend == PhyBackend::SoapySdr {
            let soapy_cfg = self
                .phy_io
                .soapysdr
                .as_ref()
                .expect("SoapySdr config must be set for SoapySdr PhyIo");

            let Ok(freq_info) = FreqInfo::from_components(
                self.cell.freq_band,
                self.cell.main_carrier,
                self.cell.freq_offset_hz,
                self.cell.reverse_operation,
                self.cell.duplex_spacing_id,
                self.cell.custom_duplex_spacing,
            ) else {
                return Err("Invalid cell info frequency settings");
            };

            let (dlfreq, ulfreq) = freq_info.get_freqs();

            println!("    {:?}", freq_info);
            println!("    Derived DL freq: {} Hz, UL freq: {} Hz\n", dlfreq, ulfreq);

            if soapy_cfg.dl_freq as u32 != dlfreq {
                return Err("PhyIo DlFrequency does not match computed FreqInfo");
            };
            if soapy_cfg.ul_freq as u32 != ulfreq {
                return Err("PhyIo UlFrequency does not match computed FreqInfo");
            };
        }

        if self.cell.ms_txpwr_max_cell > 7 {
            return Err("ms_txpwr_max_cell must be 0-7 (3 bits)");
        }

        // Validate timezone if configured
        if let Some(ref tz) = self.cell.timezone
            && tz.parse::<chrono_tz::Tz>().is_err() {
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
                && v > 0x3FF { return Err("cell.neighbor_cells_ca: main_carrier_number_extension must be 0-1023"); }
            if let Some(v) = cell.mcc
                && v > 0x3FF { return Err("cell.neighbor_cells_ca: mcc must be 0-1023"); }
            if let Some(v) = cell.mnc
                && v > 0x3FFF { return Err("cell.neighbor_cells_ca: mnc must be 0-16383"); }
            if let Some(v) = cell.location_area
                && v > 0x3FFF { return Err("cell.neighbor_cells_ca: location_area must be 0-16383"); }
            if let Some(v) = cell.maximum_ms_transmit_power
                && v > 0x7 { return Err("cell.neighbor_cells_ca: maximum_ms_transmit_power must be 0-7"); }
            if let Some(v) = cell.minimum_rx_access_level
                && v > 0xF { return Err("cell.neighbor_cells_ca: minimum_rx_access_level must be 0-15"); }
            if let Some(v) = cell.timeshare_cell_information_or_security_parameters
                && v > 0x1F { return Err("cell.neighbor_cells_ca: timeshare_cell_information_or_security_parameters must be 0-31"); }
            if let Some(v) = cell.tdma_frame_offset
                && v > 0x3F { return Err("cell.neighbor_cells_ca: tdma_frame_offset must be 0-63"); }
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

        Self {
            cfg: Arc::new(cfg),
            state: Arc::new(RwLock::new(state.unwrap_or_default())),
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
}
