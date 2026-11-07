//! Process-global directory of currently-attached radios.
//!
//! Fed by the telemetry fan-out thread (the same source the dashboard and the health registry
//! use) and always available, independent of whether the HTTP dashboard is running. Read by the
//! WAP status page to show a per-radio table (signal + idle time) and the group/private split.
//!
//! This mirrors a subset of the dashboard's `ms_map`, deliberately, so a consumer like the SNDCP
//! WAP gateway never has to depend on the dashboard being enabled.

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

struct Entry {
    rssi_dbfs: Option<f32>,
    last_seen: Instant,
    groups: HashSet<u32>,
    /// The cell the radio is on (0 = primary; only multi-cell stations report it).
    cell: u8,
}

impl Entry {
    fn new() -> Self {
        Self { rssi_dbfs: None, last_seen: Instant::now(), groups: HashSet::new(), cell: 0 }
    }
}

static DIR: OnceLock<RwLock<HashMap<u32, Entry>>> = OnceLock::new();

fn dir() -> &'static RwLock<HashMap<u32, Entry>> {
    DIR.get_or_init(|| RwLock::new(HashMap::new()))
}

/// A row for the WAP per-radio table.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioLine {
    pub issi: u32,
    pub rssi_dbfs: Option<f32>,
    pub idle_secs: u64,
    pub group_count: usize,
}

// ── writers (called from the telemetry fan-out) ──────────────────────────────────────────────

/// A radio registered (attached). Idempotent: a re-registration just refreshes last_seen.
pub fn on_register(issi: u32) {
    let mut d = dir().write().unwrap_or_else(|e| e.into_inner());
    d.entry(issi).or_insert_with(Entry::new).last_seen = Instant::now();
}

/// A radio deregistered or timed out — drop it.
pub fn on_deregister(issi: u32) {
    dir().write().unwrap_or_else(|e| e.into_inner()).remove(&issi);
}

/// Multi-cell: the cell a radio just registered on.
pub fn on_cell(issi: u32, cell: u8) {
    if let Some(e) = dir().write().unwrap_or_else(|e| e.into_inner()).get_mut(&issi) {
        e.cell = cell;
    }
}

/// A radio left `cell` without deregistering. Still on that cell here means it went to another
/// station, so it is dropped; a move to a sibling cell already set the new cell and keeps it.
pub fn on_moved_away(issi: u32, cell: u8) {
    let mut d = dir().write().unwrap_or_else(|e| e.into_inner());
    if d.get(&issi).is_some_and(|e| e.cell == cell) {
        d.remove(&issi);
    }
}

/// Signal update for an attached radio (also counts as activity). RSSI alone cannot register it.
pub fn on_rssi(issi: u32, rssi_dbfs: f32) {
    let mut d = dir().write().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = d.get_mut(&issi) {
        e.rssi_dbfs = Some(rssi_dbfs);
        e.last_seen = Instant::now();
    }
}

/// Replace a radio's full group set (from a groups snapshot).
pub fn set_groups(issi: u32, gssis: &[u32]) {
    let mut d = dir().write().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = d.get_mut(&issi) {
        e.groups = gssis.iter().copied().collect();
        e.last_seen = Instant::now();
    }
}

/// Add group affiliations.
pub fn attach_groups(issi: u32, gssis: &[u32]) {
    let mut d = dir().write().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = d.get_mut(&issi) {
        e.groups.extend(gssis.iter().copied());
        e.last_seen = Instant::now();
    }
}

/// Remove group affiliations.
pub fn detach_groups(issi: u32, gssis: &[u32]) {
    let mut d = dir().write().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = d.get_mut(&issi) {
        for g in gssis {
            e.groups.remove(g);
        }
    }
}

#[cfg(test)]
fn clear() {
    dir().write().unwrap_or_else(|e| e.into_inner()).clear();
}

// ── readers ──────────────────────────────────────────────────────────────────────────────────

/// Number of attached radios.
pub fn registered_count() -> usize {
    dir().read().unwrap_or_else(|e| e.into_inner()).len()
}

/// Distinct talkgroups with at least one affiliated radio.
pub fn group_affiliation_count() -> usize {
    let d = dir().read().unwrap_or_else(|e| e.into_inner());
    let mut set = HashSet::new();
    for e in d.values() {
        set.extend(e.groups.iter().copied());
    }
    set.len()
}

/// Radios with no group affiliation (individual / private only).
pub fn private_count() -> usize {
    dir().read().unwrap_or_else(|e| e.into_inner()).values().filter(|e| e.groups.is_empty()).count()
}

/// Up to `max` radio rows, most-recently-seen first.
pub fn lines(max: usize) -> Vec<RadioLine> {
    let d = dir().read().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let mut rows: Vec<RadioLine> = d
        .iter()
        .map(|(&issi, e)| RadioLine {
            issi,
            rssi_dbfs: e.rssi_dbfs,
            idle_secs: now.saturating_duration_since(e.last_seen).as_secs(),
            group_count: e.groups.len(),
        })
        .collect();
    // Most recently heard first (smallest idle), then by ISSI for a stable order.
    rows.sort_by(|a, b| a.idle_secs.cmp(&b.idle_secs).then(a.issi.cmp(&b.issi)));
    rows.truncate(max);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The directory is a single process-global table (`DIR`), so these tests all mutate the same
    // shared state. Cargo runs tests in parallel by default, so without serialization one test's
    // `clear()` can wipe another's entries mid-run (observed as a flaky `group_affiliation_count`
    // reading 0 instead of 1 in CI). Hold this lock for the body of each test so they run one at a
    // time. `into_inner` recovers from a poisoned lock left by an earlier panicking test.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn register_rssi_groups_and_counts() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        on_register(1001);
        on_register(1002);
        on_register(1003);
        on_rssi(1001, -85.0);
        set_groups(1001, &[100, 200]);
        attach_groups(1002, &[200]); // shares gssi 200
        // 1003 has no groups → private

        assert_eq!(registered_count(), 3);
        assert_eq!(group_affiliation_count(), 2, "distinct gssis 100 and 200");
        assert_eq!(private_count(), 1, "only 1003 has no group");

        let rows = lines(10);
        assert_eq!(rows.len(), 3);
        let r1 = rows.iter().find(|r| r.issi == 1001).unwrap();
        assert_eq!(r1.rssi_dbfs, Some(-85.0));
        assert_eq!(r1.group_count, 2);
    }

    #[test]
    fn detach_and_deregister() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        on_register(2001);
        set_groups(2001, &[300, 400]);
        detach_groups(2001, &[300]);
        assert_eq!(group_affiliation_count(), 1);
        on_deregister(2001);
        assert_eq!(registered_count(), 0);
    }

    #[test]
    fn unknown_signal_and_group_updates_do_not_register_a_radio() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        on_rssi(3001, -23.4);
        set_groups(3001, &[268]);
        attach_groups(3002, &[2682]);
        detach_groups(3003, &[268]);
        assert_eq!(registered_count(), 0);
        assert_eq!(private_count(), 0);
        assert_eq!(group_affiliation_count(), 0);
        assert!(lines(16).is_empty());
    }

    #[test]
    fn late_telemetry_cannot_resurrect_a_deregistered_radio() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        on_register(4001);
        set_groups(4001, &[268]);
        on_deregister(4001);
        on_rssi(4001, -40.0);
        set_groups(4001, &[268]);
        attach_groups(4001, &[2682]);
        assert!(lines(16).is_empty(), "only a registration may restore the radio");
        on_register(4001);
        on_rssi(4001, -25.0);
        attach_groups(4001, &[2682]);
        let rows = lines(16);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rssi_dbfs, Some(-25.0));
        assert_eq!(rows[0].group_count, 1);
    }

    #[test]
    fn lines_cap_and_recency_order() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        for issi in 0..30u32 {
            on_register(9000 + issi);
        }
        let rows = lines(16);
        assert_eq!(rows.len(), 16, "capped to max");
    }

    #[test]
    fn a_radio_that_moved_to_another_station_is_dropped() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        // Single-cell station: leaving cell 0 means another station.
        on_register(5001);
        on_moved_away(5001, 0);
        assert_eq!(registered_count(), 0);
        // Multi-cell: cell 1 -> sibling cell 2 keeps it, cell 2 -> another station drops it.
        on_register(5002);
        on_cell(5002, 1);
        on_register(5002);
        on_cell(5002, 2);
        on_moved_away(5002, 1);
        assert_eq!(registered_count(), 1);
        on_moved_away(5002, 2);
        assert_eq!(registered_count(), 0);
    }
}
