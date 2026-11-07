use serde::Deserialize;
use std::sync::{Arc, RwLock};
use tetra_core::CellId;
use tetra_core::freqs::FreqInfo;

use crate::bluestation::{
    CfgAprsWx, CfgAsterisk, CfgCellInfo, CfgControl, CfgDapnet, CfgEmergency, CfgExtraCell, CfgGeoalarm, CfgHealth, CfgMultiNetwork, CfgNetInfo, CfgPhyIo, CfgRecovery,
    CfgSecurity, CfgSnomNotify, CfgSoapySdr, CfgTpg2200Action, CfgWxService, PhyBackend, StackState, rxlev_access_min_code,
};

use super::sec_brew::CfgBrew;
use super::sec_networks::CfgNetwork;
use super::sec_dashboard::CfgDashboard;
use super::sec_lst_dispatch::CfgLstDispatch;
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
    /// Identity this cell broadcasts: `[net_info]`, or its network's MCC/MNC with `[networks]`.
    pub net: CfgNetInfo,
    pub cell: CfgCellInfo,

    /// Every network the station serves, in name order. Without a `[networks]` section this is
    /// the one implicit network (`[net_info]` + `[brew]`), named [`super::DEFAULT_NETWORK`].
    pub networks: Vec<CfgNetwork>,
    /// True when the file has a `[networks]` section (several networks, cells bound by name).
    pub networks_explicit: bool,
    /// Name of the network this cell serves (an entry of `networks`). On a per-cell config
    /// `net` and `brew` are that network's.
    pub network: String,

    /// Additional cells from `[[cells]]` (multi-cell: one SDR each). Empty for single-cell
    /// configs. The primary cell (id 0) is always `cell` + `phy_io.soapysdr`.
    pub extra_cells: Vec<CfgExtraCell>,

    /// Set on an additional cell's derived config when its network traffic goes through the
    /// site switch (see [`StackConfig::is_site_linked`]). Always false in parsed configs.
    pub site_linked: bool,

    /// Main carriers of every cell of this station, set on an additional cell's derived config
    /// (that config has no `extra_cells`). Empty in parsed configs; see
    /// [`StackConfig::is_station_carrier`].
    pub station_carriers: Vec<u16>,

    /// Which cell of the station this config runs: `CellId::PRIMARY` for the file as parsed, the
    /// `[[cells]]` id on a derived per-cell config (see [`StackConfig::for_extra_cell`]).
    pub cell_id: CellId,

    /// Brew protocol (TetraPack/BrandMeister) configuration
    pub brew: Option<CfgBrew>,

    /// Local Site Trunking web dispatch (mutually exclusive with Brew when enabled).
    pub lst_dispatch: Option<CfgLstDispatch>,

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

    /// APRS weather-station SDS service via aprs.fi (extension of the WX service)
    pub aprs_wx: CfgAprsWx,

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

    /// Multi Network: radios of other networks by migration (clause 16.4.1.1 b)). Default off.
    pub multi_network: CfgMultiNetwork,
}

/// "Cell re-selection types supported" advertised for sibling cells, table 18.22: forward
/// registration supported, expedited cell reselection recommended (0 = no forward registration,
/// 1 = none + expedited, 2 = forward registration, 3 = forward registration + expedited).
const SIBLING_CELL_RESELECTION_TYPES: u8 = 3;

const RXLEV_ACCESS_MIN_RANGE: &str = "rxlev_access_min_dbm must be -125 to -50 in 5 dB steps (table 21.65)";

/// "Main carrier number extension" for a neighbour whose band plan differs from the serving
/// cell's (otherwise a radio would look for the neighbour's carrier number on the serving cell's
/// plan). See [`carrier_band_plan`].
fn carrier_number_extension(neighbour: &CfgCellInfo, serving: &CfgCellInfo) -> Option<u16> {
    let same_plan = neighbour.freq_band == serving.freq_band
        && neighbour.freq_offset_hz == serving.freq_offset_hz
        && neighbour.duplex_spacing_id == serving.duplex_spacing_id
        && neighbour.reverse_operation == serving.reverse_operation;
    if same_plan {
        return None;
    }
    carrier_band_plan(neighbour)
}

/// A cell's band plan in the layout of the "main carrier number extension": frequency band
/// (4 bits), offset (2), duplex spacing (3), reverse operation (1). Together with the main carrier
/// number it names the cell's frequency independently of any other cell's plan. `None` when the
/// frequency offset has no code.
pub fn carrier_band_plan(cell: &CfgCellInfo) -> Option<u16> {
    let offset_id = FreqInfo::freq_offset_hz_to_id(cell.freq_offset_hz)?;
    Some(
        ((cell.freq_band as u16 & 0xF) << 6)
            | ((offset_id as u16 & 0x3) << 4)
            | ((cell.duplex_spacing_id as u16 & 0x7) << 1)
            | cell.reverse_operation as u16,
    )
}

impl StackConfig {
    /// Return BS phase-modulated carrier numbers and their DL/UL frequencies.
    pub fn bs_phase_mod_carriers(&self) -> Result<Vec<(u16, u32, u32)>, String> {
        Self::cell_phase_mod_carriers(&self.cell)
    }

    /// Carrier numbers and DL/UL frequencies of any cell.
    pub fn cell_phase_mod_carriers(cell: &CfgCellInfo) -> Result<Vec<(u16, u32, u32)>, String> {
        let mut carriers = Vec::with_capacity(if cell.secondary_carrier.is_some() { 2 } else { 1 });
        for carrier in [Some(cell.main_carrier), cell.secondary_carrier].into_iter().flatten() {
            let freq_info = FreqInfo::from_components(
                cell.freq_band,
                carrier,
                cell.freq_offset_hz,
                cell.reverse_operation,
                cell.duplex_spacing_id,
                cell.custom_duplex_spacing,
            )?;
            let (dl_freq, ul_freq) = freq_info.get_freqs();
            carriers.push((carrier, dl_freq, ul_freq));
        }
        Ok(carriers)
    }

    fn frequencies_fit_center(center_hz: f64, sample_rate_hz: f64, freqs_hz: &[u32]) -> bool {
        let half_bw = sample_rate_hz / 2.0;
        freqs_hz.iter().all(|freq| ((*freq as f64) - center_hz).abs() <= half_bw)
    }

    /// Number of cells (SDRs) this station runs: the primary plus enabled `[[cells]]`.
    pub fn cell_count(&self) -> usize {
        1 + self.extra_cells.len()
    }

    /// All cells, primary first then `[[cells]]` in config order: (id, cell parameters, SDR settings).
    pub fn cells(&self) -> Vec<(CellId, &CfgCellInfo, Option<&CfgSoapySdr>)> {
        let mut v = vec![(CellId::PRIMARY, &self.cell, self.phy_io.soapysdr.as_ref())];
        v.extend(self.extra_cells.iter().map(|c| (c.id, &c.cell, c.soapysdr.as_ref())));
        v
    }

    /// Stand-alone config for running one additional cell's radio stack: that cell's
    /// `cell_info` and SDR, with the station-wide services that run beside the primary stack
    /// (dashboard, telemetry, control, recovery, alerts) switched off. `brew` / `lst_dispatch`
    /// and, when the cells are linked, `asterisk` are kept so the cell's CMCE still routes network
    /// traffic; in the cell's router those slots are links to the primary's entities, not second
    /// connections. WX runs on every cell (each answers its own radios' requests).
    pub fn for_extra_cell(&self, id: CellId) -> Option<StackConfig> {
        let extra = self.extra_cells.iter().find(|c| c.id == id)?;
        let network = self.network_of_cell(id);
        let mut cfg = self.clone();
        cfg.cell = extra.cell.clone();
        cfg.phy_io.soapysdr = extra.soapysdr.clone();
        // Carriers of the cells of this cell's network (the station's cells as radios see them).
        let same_net = self.network_cells(&network.name);
        cfg.station_carriers =
            self.cells().iter().filter(|(cid, _, _)| same_net.contains(cid)).map(|(_, c, _)| c.main_carrier).collect();
        cfg.cell_id = id;
        cfg.extra_cells = Vec::new();
        // The cell serves its own network: its identity on air and its core (or none).
        cfg.network = network.name.clone();
        cfg.net = network.net.clone();
        cfg.brew = network.brew.clone();
        if cfg.network != self.network {
            // LST dispatch belongs to the primary cell's network only.
            cfg.lst_dispatch = None;
        }
        cfg.site_linked = self.network_is_linked(&network.name);
        // The Asterisk bridge runs beside the primary stack and reaches the cells of its network.
        cfg.asterisk.enabled = self.asterisk.enabled && cfg.site_linked && cfg.network == self.network;
        cfg.dapnet.enabled = false;
        cfg.geoalarm.enabled = false;
        cfg.tpg2200_action.enabled = false;
        cfg.snom_notify.enabled = false;
        cfg.dashboard = None;
        cfg.telemetry = None;
        cfg.control = None;
        cfg.telegram = None;
        cfg.recovery.enabled = false;
        Some(cfg)
    }

    /// Multi-cell: advertise every sibling cell as a neighbour (D-NWRK-BROADCAST) so radios can
    /// reselect between the station's cells. Siblings already listed by carrier are left as
    /// configured; entries are added only while the 7-neighbour limit allows. Each entry says the
    /// sibling supports announced and unannounced reselection, and carries what a radio needs to
    /// find and rank it when it differs from the serving cell: the carrier number extension (band
    /// plan) and the maximum MS transmit power. Also enables the "D-NWRK-BROADCAST supported" bit in
    /// D-MLE-SYNC on every cell that got neighbours.
    pub fn add_sibling_neighbours(&mut self) {
        if self.extra_cells.is_empty() {
            return;
        }
        let siblings: Vec<(CfgCellInfo, CfgNetInfo)> =
            self.cells().iter().map(|(id, c, _)| ((*c).clone(), self.network_of_cell(*id).net)).collect();
        let nets: Vec<CfgNetInfo> = self.cells().iter().map(|(id, _, _)| self.network_of_cell(*id).net).collect();
        let lists = std::iter::once(&mut self.cell).chain(self.extra_cells.iter_mut().map(|c| &mut c.cell));
        for (cell, cell_net) in lists.zip(nets.iter()) {
            for (sibling, sibling_net) in &siblings {
                let carrier = sibling.main_carrier;
                if carrier == cell.main_carrier
                    || cell.neighbor_cells_ca.len() >= 7
                    || cell.neighbor_cells_ca.iter().any(|n| n.main_carrier_number == carrier)
                {
                    continue;
                }
                let Some(id) = (0u8..=0x1F).find(|id| !cell.neighbor_cells_ca.iter().any(|n| n.cell_identifier_ca == *id)) else {
                    break;
                };
                cell.neighbor_cells_ca.push(crate::bluestation::CfgNeighborCellCa {
                    cell_identifier_ca: id,
                    cell_reselection_types_supported: SIBLING_CELL_RESELECTION_TYPES,
                    // Separate SDRs: the cells' TDMA timing is independent.
                    neighbor_cell_synchronized: false,
                    cell_load_ca: 0,
                    main_carrier_number: carrier,
                    main_carrier_number_extension: carrier_number_extension(sibling, cell),
                    // A sibling on another of the station's networks is a foreign-network
                    // neighbour: name its identity so a radio that selects that network finds it.
                    mcc: (sibling_net.mcc != cell_net.mcc || sibling_net.mnc != cell_net.mnc).then_some(sibling_net.mcc),
                    mnc: (sibling_net.mcc != cell_net.mcc || sibling_net.mnc != cell_net.mnc).then_some(sibling_net.mnc),
                    location_area: (sibling.location_area != cell.location_area).then_some(sibling.location_area),
                    maximum_ms_transmit_power: (sibling.ms_txpwr_max_cell != cell.ms_txpwr_max_cell)
                        .then_some(sibling.ms_txpwr_max_cell),
                    // 18.5.12: needed by the MS for C2 when it differs from the serving cell.
                    minimum_rx_access_level: (sibling.rxlev_access_min_dbm != cell.rxlev_access_min_dbm)
                        .then(|| sibling.rxlev_access_min()),
                    subscriber_class: None,
                    bs_service_details: None,
                    timeshare_cell_information_or_security_parameters: None,
                    tdma_frame_offset: None,
                });
            }
            if !cell.neighbor_cells_ca.is_empty() {
                cell.neighbor_cell_broadcast |= 0b10;
            }
        }
    }

    /// True for every cell of a station that runs more than one cell.
    pub fn is_multi_cell(&self) -> bool {
        !self.extra_cells.is_empty() || !self.cell_id.is_primary()
    }

    /// True when `carrier` is the main carrier of a cell of this station (this one or a sibling).
    /// A neighbour on any other carrier belongs to another station.
    pub fn is_station_carrier(&self, carrier: u16) -> bool {
        // Only cells of this cell's network: another network's cell is no target of a handover.
        let same_net = self.network_cells(&self.network);
        self.cells().iter().any(|(id, c, _)| same_net.contains(id) && c.main_carrier == carrier) || self.station_carriers.contains(&carrier)
    }

    /// True when a network link (Brew, or enabled LST Dispatch) is configured.
    pub fn has_network_link(&self) -> bool {
        self.brew.is_some() || self.lst_dispatch.as_ref().is_some_and(|l| l.enabled)
    }

    /// True when this stack is one cell of a multi-cell station whose cells are linked through
    /// the site switch. The stack then reports every registration, group floor and call to its
    /// network slot (the switch applies the Brew routing rules before the real network sees it).
    pub fn is_site_linked(&self) -> bool {
        self.site_linked || (self.cell_id.is_primary() && self.network_is_linked(&self.network))
    }

    // ── Networks ────────────────────────────────────────────────────────────────────────────

    /// The network called `name`, if the station serves it.
    pub fn network(&self, name: &str) -> Option<&CfgNetwork> {
        self.networks.iter().find(|n| n.name == name)
    }

    /// The network a cell of this station serves. Only meaningful on the primary (parsed)
    /// config, which knows every cell; a cell without a binding serves the primary's network.
    /// A config built by hand with no `networks` entry serves the implicit one.
    pub fn network_of_cell(&self, id: CellId) -> CfgNetwork {
        let name = if id.is_primary() {
            Some(self.network.as_str())
        } else {
            self.extra_cells.iter().find(|c| c.id == id).and_then(|c| c.network.as_deref())
        };
        name.and_then(|n| self.network(n))
            .or_else(|| self.network(&self.network))
            .or_else(|| self.networks.first())
            .cloned()
            .unwrap_or_else(|| CfgNetwork {
                name: super::DEFAULT_NETWORK.to_owned(),
                net: self.net.clone(),
                brew: self.brew.clone(),
                via: None,
            })
    }

    /// True when this cell's network is reached through another network's Brew server (`via`):
    /// its radios and groups are named with this network's MCC/MNC towards the server (`mni`).
    pub fn network_is_via(&self) -> bool {
        self.network(&self.network).is_some_and(|n| n.via.is_some())
    }

    /// Multi Network towards the Brew server (`mni`): `[multi_network]` is on, or this cell's
    /// network is reached `via` another one.
    pub fn mni_enabled(&self) -> bool {
        self.multi_network.enabled || self.network_is_via()
    }

    /// Migration supported on this cell (BS service details, ETSI TS 100 392-2 18.5.2.1):
    /// `[cell_info] migration = true`, or `[multi_network]` is on, which exists to accept radios
    /// of other networks by migration (16.4.1.1 b) and so turns it on by itself.
    pub fn migration_supported(&self) -> bool {
        self.cell.migration || self.multi_network.enabled
    }

    /// The cells this config's Brew connection announces (SERVICE_BTS_INFO): on the station's
    /// config the cells of the primary's network, on a per-cell config that cell.
    pub fn cells_of_station_network(&self) -> Vec<(CellId, &CfgCellInfo, Option<&CfgSoapySdr>)> {
        let same_net = self.network_cells(&self.network);
        self.cells().into_iter().filter(|(id, _, _)| same_net.contains(id)).collect()
    }

    /// The cells serving network `name`, lowest id first (the primary comes first when it is one).
    pub fn network_cells(&self, name: &str) -> Vec<CellId> {
        let mut cells: Vec<CellId> = std::iter::once(CellId::PRIMARY)
            .chain(self.extra_cells.iter().map(|c| c.id))
            .filter(|&id| self.network_of_cell(id).name == name)
            .collect();
        cells.sort_by_key(|id| id.0);
        cells
    }

    /// The cell that hosts a network's switch and Brew link: the primary when it serves the
    /// network, else the network's lowest-numbered cell. `None` for a network with no cell.
    pub fn network_home_cell(&self, name: &str) -> Option<CellId> {
        self.network_cells(name).into_iter().next()
    }

    /// True when the cells of network `name` are joined through a site switch: more than one
    /// cell, and either a core to share or an explicit `[networks]` section (a local network of
    /// several cells is switched inside the station). A classic station without `[networks]`
    /// keeps its cells independent unless Brew or LST is configured, as before.
    pub fn network_is_linked(&self, name: &str) -> bool {
        let cells = self.network_cells(name);
        if cells.len() < 2 {
            return false;
        }
        let has_brew = match self.network(name) {
            Some(n) => n.brew.is_some(),
            None if name == self.network => self.brew.is_some(),
            None => return false,
        };
        self.networks_explicit
            || has_brew
            || (name == self.network && self.lst_dispatch.as_ref().is_some_and(|l| l.enabled))
    }

    /// Validate that all required configuration fields are properly set.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_primary().map_err(String::from)?;
        self.validate_extra_cells()
    }

    /// Carrier and SDR passband checks shared by every cell.
    fn validate_cell_rf(backend: PhyBackend, cell: &CfgCellInfo, soapy: Option<&CfgSoapySdr>) -> Result<(), &'static str> {
        if let Some(secondary_carrier) = cell.secondary_carrier
            && secondary_carrier == cell.main_carrier
        {
            return Err("cell.secondary_carrier must differ from cell.main_carrier");
        }

        if backend != PhyBackend::SoapySdr {
            return Ok(());
        }
        let soapy_cfg = soapy.ok_or("soapysdr configuration must be provided for Soapysdr backend")?;

        let carriers = Self::cell_phase_mod_carriers(cell).map_err(|_| "Invalid cell info frequency settings")?;
        let (main_dl, main_ul) = carriers
            .iter()
            .find(|(carrier_num, _, _)| *carrier_num == cell.main_carrier)
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

            if !Self::frequencies_fit_center(tx_center_hz, sample_rate_hz, &dl_freqs) {
                return Err("configured TX center/sample-rate do not cover all BS downlink carriers");
            }
            if !Self::frequencies_fit_center(rx_center_hz, sample_rate_hz, &ul_freqs) {
                return Err("configured RX center/sample-rate do not cover all BS uplink carriers");
            }
        };
        Ok(())
    }

    /// Per-cell and cross-cell checks for `[[cells]]`. No-op for single-cell configs.
    fn validate_extra_cells(&self) -> Result<(), String> {
        if self.extra_cells.is_empty() {
            return Ok(());
        }
        if self.stack_mode != StackMode::Bs {
            return Err("[[cells]] is only supported in Bs stack mode".into());
        }

        let mut seen_ids = std::collections::HashSet::from([CellId::PRIMARY]);
        for c in &self.extra_cells {
            if c.id.is_primary() {
                return Err("cells: id 0 is reserved for the primary [cell_info]".into());
            }
            if c.id.0 > CellId::MAX {
                return Err(format!("cells: id {} out of range (1-{})", c.id.0, CellId::MAX));
            }
            if !seen_ids.insert(c.id) {
                return Err(format!("cells: duplicate id {}", c.id.0));
            }
            if c.cell.ms_txpwr_max_cell > 7 {
                return Err(format!("{}: ms_txpwr_max_cell must be 0-7 (3 bits)", c.id));
            }
            if rxlev_access_min_code(c.cell.rxlev_access_min_dbm).is_none() {
                return Err(format!("{}: {RXLEV_ACCESS_MIN_RANGE}", c.id));
            }
            Self::validate_cell_rf(self.phy_io.backend, &c.cell, c.soapysdr.as_ref()).map_err(|e| format!("{}: {e}", c.id))?;
            validate_neighbours(&c.cell).map_err(|e| format!("{}: {e}", c.id))?;
        }

        let cells = self.cells();

        // Cells share the station's RF plan; identity comes from each cell's network.
        let primary = &self.cell;
        for (id, cell, _) in &cells[1..] {
            if cell.freq_band != primary.freq_band {
                return Err(format!("{id}: freq_band must match the primary cell"));
            }
            // Neighbour broadcasts can tell radios another offset / duplex setting, but not a
            // custom (non-standard) duplex spacing.
            if cell.custom_duplex_spacing != primary.custom_duplex_spacing {
                return Err(format!("{id}: custom_duplex_spacing must match the primary cell"));
            }
        }

        // No carrier may be used by two cells.
        let mut seen_carriers = std::collections::HashMap::new();
        for (id, cell, _) in &cells {
            for carrier in [Some(cell.main_carrier), cell.secondary_carrier].into_iter().flatten() {
                if let Some(other) = seen_carriers.insert(carrier, *id) {
                    return Err(format!("carrier {carrier} is used by both {other} and {id}"));
                }
            }
        }

        // With real SDRs every cell must name its device, and no two cells may share one.
        if self.phy_io.backend == PhyBackend::SoapySdr {
            let mut seen_devices = std::collections::HashMap::new();
            for (id, _, soapy) in &cells {
                let device = soapy
                    .and_then(|s| s.device.as_deref())
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .ok_or_else(|| format!("{id}: soapysdr.device must be set when running more than one cell"))?;
                if let Some(other) = seen_devices.insert(device.to_string(), *id) {
                    return Err(format!("{other} and {id} use the same SDR device '{device}'"));
                }
            }
        }

        Ok(())
    }

    /// Checks for the primary cell and all station-wide settings.
    fn validate_primary(&self) -> Result<(), &'static str> {
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

        Self::validate_cell_rf(self.phy_io.backend, &self.cell, self.phy_io.soapysdr.as_ref())?;

        if self.cell.ms_txpwr_max_cell > 7 {
            return Err("ms_txpwr_max_cell must be 0-7 (3 bits)");
        }
        if rxlev_access_min_code(self.cell.rxlev_access_min_dbm).is_none() {
            return Err(RXLEV_ACCESS_MIN_RANGE);
        }

        // The WAP/SNDCP gateway is driven solely by `[cell_info.wap_ip].enabled`
        // (CfgCellInfo::wap_enabled): when on, the cell advertises SNDCP + advanced-link and runs
        // the gateway; when off it does neither. There is deliberately no cross-field agreement
        // check here — a stale `sndcp_service`/`advanced_link` flag must never make the station
        // fail to start (disabling a feature can't be fatal).

        // Validate timezone if configured
        if let Some(ref tz) = self.cell.timezone
            && tz.parse::<chrono_tz::Tz>().is_err()
        {
            return Err("Invalid IANA timezone name in cell.timezone");
        }

        // LST dispatch and Brew backhaul are mutually exclusive.
        if self.brew.is_some()
            && self
                .lst_dispatch
                .as_ref()
                .is_some_and(|l| l.enabled)
        {
            return Err("lst_dispatch.enabled cannot be used together with [brew]");
        }

        validate_neighbours(&self.cell)?;

        self.multi_network.validate()?;

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

/// Checks of a cell's `neighbor_cells_ca` list (field widths of table 18.64, uniqueness).
pub(crate) fn validate_neighbours(cell_info: &CfgCellInfo) -> Result<(), &'static str> {
    if cell_info.neighbor_cells_ca.len() > 7 {
        return Err("cell.neighbor_cells_ca supports at most 7 entries");
    }

    // Check for duplicate cell_identifier_ca and main_carrier_number
    {
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_carriers = std::collections::HashSet::new();
        for cell in &cell_info.neighbor_cells_ca {
            if !seen_ids.insert(cell.cell_identifier_ca) {
                return Err("cell.neighbor_cells_ca: duplicate cell_identifier_ca — each neighbour must have a unique identifier");
            }
            if !seen_carriers.insert(cell.main_carrier_number) {
                return Err("cell.neighbor_cells_ca: duplicate main_carrier_number — each neighbour must be on a different carrier");
            }
            if cell.main_carrier_number == cell_info.main_carrier {
                return Err("cell.neighbor_cells_ca: main_carrier_number is this cell's own main_carrier — a cell cannot be its own neighbour");
            }
        }
    }

    for cell in &cell_info.neighbor_cells_ca {
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
    Ok(())
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
    /// Build a `SharedConfig`, validating the config and computing the carriers. Returns an error
    /// instead of panicking, so the boot path can exit with a clear message (primary config) or
    /// skip just the offending cell (extra cells) rather than crashing the whole station.
    pub fn try_from_parts(cfg: StackConfig, state: Option<StackState>) -> Result<Self, String> {
        cfg.validate()?;

        let mut state = state.unwrap_or_default();
        let carriers = cfg
            .bs_phase_mod_carriers()?
            .into_iter()
            .map(|(carrier_num, _, _)| carrier_num)
            .collect::<Vec<_>>();
        state.timeslot_alloc.configure_carriers(&carriers);

        Ok(Self {
            cfg: Arc::new(cfg),
            state: Arc::new(RwLock::new(state)),
        })
    }

    /// Infallible constructor that panics on an invalid config. Kept for internal and test
    /// callers that build a config already known to be valid; the boot path uses
    /// [`Self::try_from_parts`] so a config error never panics a running station.
    pub fn from_parts(cfg: StackConfig, state: Option<StackState>) -> Self {
        Self::try_from_parts(cfg, state).unwrap_or_else(|e| panic!("Invalid stack configuration: {e}"))
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

    /// Effective SDS U-STATUS command control (ISSI 9999): dashboard override if present,
    /// otherwise `[cell_info.sds_command_control]`. Returns `None` when disabled / absent.
    pub fn effective_sds_command_control(&self) -> Option<crate::bluestation::CfgSdsCommandControl> {
        if let Some(o) = self.state_read().sds_command_override.as_ref() {
            if !o.enabled {
                return None;
            }
            return Some(crate::bluestation::CfgSdsCommandControl {
                authorized_issis: o.authorized_issis.clone(),
                commands: o.commands.clone(),
            });
        }
        self.cfg.cell.sds_command_control.clone()
    }
}
