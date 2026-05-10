use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Per-MS state tracked by the dashboard
#[derive(Debug, Clone, serde::Serialize)]
pub struct MsState {
    pub issi: u32,
    pub groups: Vec<u32>,
    pub rssi_dbfs: Option<f32>,
    pub registered_at: u64,      // unix seconds
    pub last_seen_secs_ago: u64,  // seconds since last activity
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

/// Shared mutable state for the dashboard, protected by RwLock
#[derive(Debug, Default)]
pub struct DashboardStateInner {
    pub ms_map: HashMap<u32, MsEntry>,
    pub calls: HashMap<u16, CallEntry>,
    pub log_ring: std::collections::VecDeque<LogEntry>,
    pub config_path: String,
}

#[derive(Debug)]
pub struct MsEntry {
    pub issi: u32,
    pub groups: Vec<u32>,
    pub rssi_dbfs: Option<f32>,
    pub registered_at: Instant,
    pub last_seen: Instant,
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
            config_path,
        }
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
