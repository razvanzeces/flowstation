use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Per-MS state tracked by the dashboard
#[derive(Debug, Clone, serde::Serialize)]
pub struct MsState {
    pub issi: u32,
    pub groups: Vec<u32>,
    pub group_catalog: Vec<MsGroupState>,
    /// The talkgroup this MS most recently keyed up on (originated/spoke in a group call).
    /// This is the BS's best inference of the MS's actively-selected TG, as opposed to the
    /// other entries in `groups` which are merely scanned/affiliated. None until the MS is
    /// heard on a group call — so right after a restart all groups show as scanned, not active.
    pub selected_group: Option<u32>,
    pub rssi_dbfs: Option<f32>,
    pub registered_at: u64,
    pub last_seen_secs_ago: u64,
    pub energy_saving_mode: u8, // 0=StayAlive, 1=Eg1..7=Eg7
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MsGroupState {
    pub gssi: u32,
    pub mnemonic: Option<String>,
    pub attachment_mode: Option<u8>,
    pub is_dynamic: bool,
    pub is_attached: bool,
}

/// Active call state
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallState {
    pub call_id: u16,
    pub call_type: &'static str, // "group" or "individual"
    pub gssi: u32,
    pub caller_issi: u32,
    pub called_issi: u32,
    pub active_speaker: Option<u32>,
    pub started_secs_ago: u64,
    pub simplex: bool,
    pub carrier_num: u16,
    pub ts: u8,
    pub peer_carrier_num: Option<u16>,
    pub peer_ts: Option<u8>,
    /// ETSI call priority (0..=15). 15 = emergency call; 12..=15 = pre-emptive priority.
    pub priority: u8,
}

/// Active emergency state sent to the dashboard (wire form of `EmergencyEntry`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct EmergencyState {
    pub issi: u32,
    pub dest_ssi: u32,
    pub started_secs_ago: u64,
}

/// Log entry
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: String,
    pub msg: String,
}

/// Last Heard entry — one entry per call start or SDS activity
#[derive(Debug, Clone, serde::Serialize)]
pub struct LastHeardEntry {
    pub ts: String,       // HH:MM:SS timestamp
    pub issi: u32,        // source ISSI
    pub activity: String, // "call_group", "call_individual", "sds"
    pub dest: u32,        // destination GSSI or ISSI (0 if unknown)
}

/// SDS Log entry — one SDS message the BS sent or received locally. Persisted to disk
/// (`sds_log.json` next to the active config) so the log survives a restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SdsLogEntry {
    pub ts: String,        // "YYYY-MM-DD HH:MM:SS" local time
    pub direction: String, // "rx" (from MS) | "net" (from network) | "tx" (from dashboard)
    pub source_issi: u32,
    pub dest_issi: u32,
    pub is_group: bool,
    pub protocol_id: u8,
    pub text: String,
}

/// DAPNET Log entry — one inbound RWTH-core message or outbound Hampager API send. Persisted to
/// disk (`dapnet_log.json` next to the active config) so the history survives a restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DapnetLogEntry {
    pub ts: String,        // "YYYY-MM-DD HH:MM:SS" local time
    pub direction: String, // "rx" | "tx"
    pub id: String,
    pub callsign: String,
    pub recipient: String,
    pub text: String,
    pub priority: Option<u8>,
    pub paths: Vec<String>,
}

/// DGNA Activity entry — one DGNA status transition emitted by MM/CMCE/SS. Persisted to disk
/// (`dgna_log.json` next to the active config) so the history survives restart and page reload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DgnaLogEntry {
    pub ts: String, // "YYYY-MM-DD HH:MM:SS" local time
    pub issi: u32,
    pub gssi: u32,
    pub attach: bool,
    pub accepted: bool,
    pub source: String,
    pub detail: String,
}

/// Shared mutable state for the dashboard, protected by RwLock
#[derive(Debug, Default)]
pub struct DashboardStateInner {
    pub ms_map: HashMap<u32, MsEntry>,
    pub calls: HashMap<u16, CallEntry>,
    /// Active emergencies keyed by originating ISSI. Non-empty drives the dashboard emergency
    /// banner. Populated from the EmergencyAlarm / EmergencyCancel telemetry (emergency status).
    pub emergencies: HashMap<u32, EmergencyEntry>,
    pub log_ring: std::collections::VecDeque<LogEntry>,
    pub last_heard: std::collections::VecDeque<LastHeardEntry>,
    /// SDS Log ring (chronological, oldest at the front). Backed by an on-disk JSON file.
    pub sds_log: std::collections::VecDeque<SdsLogEntry>,
    /// Where `sds_log` is persisted. Empty disables persistence (e.g. config without a parent dir).
    sds_log_path: std::path::PathBuf,
    /// DAPNET Log ring (chronological, oldest at the front). Backed by an on-disk JSON file.
    pub dapnet_log: std::collections::VecDeque<DapnetLogEntry>,
    /// Where `dapnet_log` is persisted. Empty disables persistence.
    dapnet_log_path: std::path::PathBuf,
    /// DGNA activity ring (chronological, oldest at the front). Backed by an on-disk JSON file.
    pub dgna_log: std::collections::VecDeque<DgnaLogEntry>,
    /// Where `dgna_log` is persisted. Empty disables persistence.
    dgna_log_path: std::path::PathBuf,
    pub config_path: String,
    pub brew_online: bool,
    pub brew_version: u8,
    /// Set when the stack started on the fallback config instead of the primary.
    /// Contains the parse error that caused the primary config to be rejected.
    pub fallback_config_active: bool,
    pub fallback_config_reason: String,
    /// Most recent fast visual snapshot (spectrum + IQ + RMS/peak). Sent on init
    /// so the RF page paints instantly on connect.
    pub last_tx_visual: Option<TxVisualSnapshot>,
    /// Most recent slow quality snapshot (EVM, PAPR, etc).
    pub last_tx_quality: Option<TxQualitySnapshot>,
    /// Most recent SDR hardware health snapshot.
    pub last_sdr_health: Option<SdrHealthSnapshot>,
    /// Most recent host system health snapshot (temps, voltages, power).
    pub last_sys_health: Option<SysHealthSnapshot>,
    /// Most recent lite stack-health roll-up (Service/Backhaul/Radios/Congestion). Sent on init
    /// so the System Health tile paints immediately on connect.
    pub last_health: Option<crate::health::HealthSnapshot>,
}

/// Fast-path visual snapshot — spectrum + IQ + RMS/peak. Refreshed several times
/// per second so the RF page renders fluidly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TxVisualSnapshot {
    pub sample_rate: f32,
    pub center_freq_hz: f64,
    pub carriers: Vec<(u16, f64)>,
    pub constellation_carrier: Option<(u16, f64)>,
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
    pub spectrum_db_tenths: Vec<i16>,
    pub constellation_iq: Vec<i16>,
}

/// Slow-path quality snapshot — derived metrics shown on the RF page as stable
/// readouts. Refreshed once per second.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TxQualitySnapshot {
    pub papr_db: f32,
    pub evm_pct: f32,
    pub evm_carrier: Option<(u16, f64)>,
    pub dc_offset_i: f32,
    pub dc_offset_q: f32,
    pub iq_amplitude_imbalance_db: f32,
    pub iq_phase_imbalance_deg: f32,
    pub carrier_leakage_db: f32,
    pub occupied_bandwidth_hz: f32,
}

/// Snapshot of host system health (temperatures, voltages, currents, power).
/// Mirrored from TelemetryEvent::SysHealth.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SysHealthSnapshot {
    pub total_power_w: Option<f32>,
    pub sensors: Vec<crate::net_telemetry::events::SysSensor>,
}

/// SDR hardware health snapshot, mirrored from TelemetryEvent::SdrHealth.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SdrHealthSnapshot {
    pub temperature_c: Option<f32>,
    pub tx_gains: Vec<(String, f32)>,
    pub rx_gains: Vec<(String, f32)>,
}

pub const LAST_HEARD_MAX: usize = 50;
/// Max SDS Log entries kept in memory and on disk. The log is human-messaging volume, so a
/// few hundred entries cover a long history while keeping the JSON file small.
pub const SDS_LOG_MAX: usize = 500;
/// Max DAPNET Log entries kept in memory and on disk.
pub const DAPNET_LOG_MAX: usize = 500;
/// Max DGNA activity entries kept in memory and on disk.
pub const DGNA_LOG_MAX: usize = 200;

#[derive(Debug)]
pub struct MsEntry {
    pub issi: u32,
    pub groups: Vec<u32>,
    pub group_catalog: Vec<MsGroupState>,
    /// Best inference of the actively-selected TG: the last group this MS keyed up on.
    /// See MsState::selected_group.
    pub selected_group: Option<u32>,
    pub rssi_dbfs: Option<f32>,
    pub registered_at: Instant,
    pub last_seen: Instant,
    pub energy_saving_mode: u8,
}

#[derive(Debug)]
pub struct CallEntry {
    pub call_id: u16,
    pub is_group: bool,
    pub gssi: u32,
    pub caller_issi: u32,
    pub called_issi: u32,
    pub speaker_issi: Option<u32>,
    pub started_at: Instant,
    pub simplex: bool,
    pub carrier_num: u16,
    pub ts: u8,
    pub peer_carrier_num: Option<u16>,
    pub peer_ts: Option<u8>,
    /// ETSI call priority (0..=15); 15 = emergency. Mirrored from the call-started telemetry.
    pub priority: u8,
}

/// One active emergency in the dashboard's live view (keyed by ISSI in `emergencies`). Raised by
/// an emergency status (U-STATUS) via EmergencyAlarm/EmergencyCancel telemetry.
#[derive(Debug, Clone)]
pub struct EmergencyEntry {
    pub issi: u32,
    pub dest_ssi: u32,
    pub started_at: Instant,
}

pub type DashboardState = Arc<RwLock<DashboardStateInner>>;

impl DashboardStateInner {
    pub fn new(config_path: String) -> Self {
        // The SDS Log is persisted next to the active config, mirroring radioid_cache.json.
        let sds_log_path = std::path::Path::new(&config_path)
            .parent()
            .map(|d| d.join("sds_log.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("sds_log.json"));
        let sds_log = load_sds_log(&sds_log_path);
        if !sds_log.is_empty() {
            tracing::info!("SDS Log: loaded {} entries from {}", sds_log.len(), sds_log_path.display());
        }
        let dapnet_log_path = std::path::Path::new(&config_path)
            .parent()
            .map(|d| d.join("dapnet_log.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("dapnet_log.json"));
        let dapnet_log = load_dapnet_log(&dapnet_log_path);
        if !dapnet_log.is_empty() {
            tracing::info!("DAPNET Log: loaded {} entries from {}", dapnet_log.len(), dapnet_log_path.display());
        }
        let dgna_log_path = std::path::Path::new(&config_path)
            .parent()
            .map(|d| d.join("dgna_log.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("dgna_log.json"));
        let dgna_log = load_dgna_log(&dgna_log_path);
        if !dgna_log.is_empty() {
            tracing::info!("DGNA Log: loaded {} entries from {}", dgna_log.len(), dgna_log_path.display());
        }
        Self {
            ms_map: HashMap::new(),
            calls: HashMap::new(),
            emergencies: HashMap::new(),
            log_ring: std::collections::VecDeque::with_capacity(500),
            last_heard: std::collections::VecDeque::with_capacity(LAST_HEARD_MAX + 1),
            sds_log,
            sds_log_path,
            dapnet_log,
            dapnet_log_path,
            dgna_log,
            dgna_log_path,
            config_path,
            brew_online: false,
            brew_version: 0,
            fallback_config_active: false,
            fallback_config_reason: String::new(),
            last_tx_visual: None,
            last_tx_quality: None,
            last_sdr_health: None,
            last_sys_health: None,
            last_health: None,
        }
    }

    pub fn push_last_heard(&mut self, issi: u32, activity: &str, dest: u32) {
        let entry = LastHeardEntry {
            ts: chrono::Local::now().format("%H:%M:%S").to_string(),
            issi,
            activity: activity.to_string(),
            dest,
        };
        if self.last_heard.len() >= LAST_HEARD_MAX {
            self.last_heard.pop_back();
        }
        self.last_heard.push_front(entry);
    }

    pub fn push_log(&mut self, level: &str, msg: String) {
        let entry = LogEntry {
            ts: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            level: level.to_string(),
            msg,
        };
        if self.log_ring.len() >= 500 {
            self.log_ring.pop_front();
        }
        self.log_ring.push_back(entry);
    }

    /// Append one SDS to the log (newest at the back), evicting the oldest past SDS_LOG_MAX,
    /// then persist the whole ring to disk so it survives a restart. The file is small and
    /// SDS traffic is low-volume, so a full rewrite per entry is cheap.
    pub fn push_sds_log(&mut self, direction: &str, source_issi: u32, dest_issi: u32, is_group: bool, protocol_id: u8, text: String) {
        let entry = SdsLogEntry {
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            direction: direction.to_string(),
            source_issi,
            dest_issi,
            is_group,
            protocol_id,
            text,
        };
        if self.sds_log.len() >= SDS_LOG_MAX {
            self.sds_log.pop_front();
        }
        self.sds_log.push_back(entry);
        self.persist_sds_log();
    }

    fn persist_sds_log(&self) {
        if self.sds_log_path.as_os_str().is_empty() {
            return;
        }
        if let Ok(text) = serde_json::to_string(&self.sds_log) {
            let _ = std::fs::write(&self.sds_log_path, text);
        }
    }

    pub fn clear_sds_log(&mut self) {
        self.sds_log.clear();
        self.persist_sds_log();
    }

    /// Append one DAPNET event to the log, evicting the oldest past DAPNET_LOG_MAX, then persist
    /// the ring to disk. Best-effort only: write failures never affect radio operation.
    pub fn push_dapnet_log(
        &mut self,
        direction: &str,
        id: String,
        callsign: String,
        recipient: String,
        text: String,
        priority: Option<u8>,
        paths: Vec<String>,
    ) {
        let entry = DapnetLogEntry {
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            direction: direction.to_string(),
            id,
            callsign,
            recipient,
            text,
            priority,
            paths,
        };
        if self.dapnet_log.len() >= DAPNET_LOG_MAX {
            self.dapnet_log.pop_front();
        }
        self.dapnet_log.push_back(entry);
        self.persist_dapnet_log();
    }

    fn persist_dapnet_log(&self) {
        if self.dapnet_log_path.as_os_str().is_empty() {
            return;
        }
        if let Ok(text) = serde_json::to_string(&self.dapnet_log) {
            let _ = std::fs::write(&self.dapnet_log_path, text);
        }
    }

    pub fn clear_dapnet_log(&mut self) {
        self.dapnet_log.clear();
        self.persist_dapnet_log();
    }

    /// Append one DGNA status to the log, evicting the oldest past DGNA_LOG_MAX, then persist
    /// the ring to disk. DGNA operator actions are low-volume, so rewriting the small JSON file
    /// on every update is acceptable.
    pub fn push_dgna_log(&mut self, status: crate::net_telemetry::events::DgnaStatusInfo) {
        let entry = DgnaLogEntry {
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            issi: status.issi,
            gssi: status.gssi,
            attach: status.attach,
            accepted: status.accepted,
            source: status.source,
            detail: status.detail,
        };
        if self.dgna_log.len() >= DGNA_LOG_MAX {
            self.dgna_log.pop_front();
        }
        self.dgna_log.push_back(entry);
        self.persist_dgna_log();
    }

    fn persist_dgna_log(&self) {
        if self.dgna_log_path.as_os_str().is_empty() {
            return;
        }
        if let Ok(text) = serde_json::to_string(&self.dgna_log) {
            let _ = std::fs::write(&self.dgna_log_path, text);
        }
    }

    pub fn clear_dgna_log(&mut self) {
        self.dgna_log.clear();
        self.persist_dgna_log();
    }

    pub fn snapshot_ms(&self) -> Vec<MsState> {
        self.ms_map
            .values()
            .map(|e| MsState {
                issi: e.issi,
                groups: e.groups.clone(),
                group_catalog: e.group_catalog.clone(),
                selected_group: e.selected_group,
                rssi_dbfs: e.rssi_dbfs,
                registered_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(e.registered_at.elapsed().as_secs()),
                last_seen_secs_ago: e.last_seen.elapsed().as_secs(),
                energy_saving_mode: e.energy_saving_mode,
            })
            .collect()
    }

    pub fn snapshot_calls(&self) -> Vec<CallState> {
        self.calls
            .values()
            .map(|c| CallState {
                call_id: c.call_id,
                call_type: if c.is_group { "group" } else { "individual" },
                gssi: c.gssi,
                caller_issi: c.caller_issi,
                called_issi: c.called_issi,
                active_speaker: c.speaker_issi,
                started_secs_ago: c.started_at.elapsed().as_secs(),
                simplex: c.simplex,
                carrier_num: c.carrier_num,
                ts: c.ts,
                peer_carrier_num: c.peer_carrier_num,
                peer_ts: c.peer_ts,
                priority: c.priority,
            })
            .collect()
    }

    pub fn snapshot_emergencies(&self) -> Vec<EmergencyState> {
        self.emergencies
            .values()
            .map(|e| EmergencyState {
                issi: e.issi,
                dest_ssi: e.dest_ssi,
                started_secs_ago: e.started_at.elapsed().as_secs(),
            })
            .collect()
    }

    /// Raise (or refresh) an emergency for `issi`. Returns true only on the idle→emergency
    /// transition, so the caller broadcasts `emergency_added` + logs only once per session.
    pub fn emergency_enter(&mut self, issi: u32, dest_ssi: u32) -> bool {
        match self.emergencies.get_mut(&issi) {
            Some(e) => {
                if dest_ssi != 0 {
                    e.dest_ssi = dest_ssi;
                }
                false
            }
            None => {
                self.emergencies.insert(
                    issi,
                    EmergencyEntry {
                        issi,
                        dest_ssi,
                        started_at: Instant::now(),
                    },
                );
                true
            }
        }
    }

    /// Clear an emergency for `issi`. Returns true if one was present (caller broadcasts removal).
    pub fn emergency_clear(&mut self, issi: u32) -> bool {
        self.emergencies.remove(&issi).is_some()
    }
}

/// Load the persisted SDS Log from disk. Returns an empty ring when the file is missing or
/// unparseable (e.g. first run, or a schema change) — the log is best-effort, never fatal.
fn load_sds_log(path: &std::path::Path) -> std::collections::VecDeque<SdsLogEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::VecDeque::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn load_dapnet_log(path: &std::path::Path) -> std::collections::VecDeque<DapnetLogEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::VecDeque::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn load_dgna_log(path: &std::path::Path) -> std::collections::VecDeque<DgnaLogEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::VecDeque::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}
