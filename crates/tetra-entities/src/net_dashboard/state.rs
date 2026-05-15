use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Per-MS state tracked by the dashboard
#[derive(Debug, Clone, serde::Serialize)]
pub struct MsState {
    pub issi: u32,
    pub groups: Vec<u32>,
    pub rssi_dbfs: Option<f32>,
    pub registered_at: u64,
    pub last_seen_secs_ago: u64,
    pub energy_saving_mode: u8,   // 0=StayAlive, 1=Eg1..7=Eg7
}

/// Active call state
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallState {
    pub call_id: u16,
    pub call_type: &'static str,  // "group" or "individual"
    pub gssi: u32,
    pub caller_issi: u32,
    pub called_issi: u32,
    pub active_speaker: Option<u32>,
    pub started_secs_ago: u64,
    pub simplex: bool,
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
    pub ts: String,           // HH:MM:SS timestamp
    pub issi: u32,            // source ISSI
    pub activity: String,     // "call_group", "call_individual", "sds"
    pub dest: u32,            // destination GSSI or ISSI (0 if unknown)
}

/// Shared mutable state for the dashboard, protected by RwLock
#[derive(Debug, Default)]
pub struct DashboardStateInner {
    pub ms_map: HashMap<u32, MsEntry>,
    pub calls: HashMap<u16, CallEntry>,
    pub log_ring: std::collections::VecDeque<LogEntry>,
    pub last_heard: std::collections::VecDeque<LastHeardEntry>,
    pub config_path: String,
    pub brew_online: bool,
}

pub const LAST_HEARD_MAX: usize = 50;

#[derive(Debug)]
pub struct MsEntry {
    pub issi: u32,
    pub groups: Vec<u32>,
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
}

pub type DashboardState = Arc<RwLock<DashboardStateInner>>;

impl DashboardStateInner {
    pub fn new(config_path: String) -> Self {
        Self {
            ms_map: HashMap::new(),
            calls: HashMap::new(),
            log_ring: std::collections::VecDeque::with_capacity(500),
            last_heard: std::collections::VecDeque::with_capacity(LAST_HEARD_MAX + 1),
            config_path,
            brew_online: false,
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

    pub fn snapshot_ms(&self) -> Vec<MsState> {
        self.ms_map.values().map(|e| MsState {
            issi: e.issi,
            groups: e.groups.clone(),
            rssi_dbfs: e.rssi_dbfs,
            registered_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(e.registered_at.elapsed().as_secs()),
            last_seen_secs_ago: e.last_seen.elapsed().as_secs(),
            energy_saving_mode: e.energy_saving_mode,
        }).collect()
    }

    pub fn snapshot_calls(&self) -> Vec<CallState> {
        self.calls.values().map(|c| CallState {
            call_id: c.call_id,
            call_type: if c.is_group { "group" } else { "individual" },
            gssi: c.gssi,
            caller_issi: c.caller_issi,
            called_issi: c.called_issi,
            active_speaker: c.speaker_issi,
            started_secs_ago: c.started_at.elapsed().as_secs(),
            simplex: c.simplex,
        }).collect()
    }
}
