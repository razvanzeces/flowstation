//! On-disk persistence for restart recovery.
//!
//! Holds a small JSON snapshot of the terminals the BS knew before a restart — their ISSI,
//! persistent group affiliations, and energy-saving mode — so that on the next startup the MM
//! layer can replay D-LOCATION-UPDATE-COMMANDs and re-attract them (see `mm_bs::init_recovery`
//! and `drive_recovery_replay`).
//!
//! Writes are best-effort: any I/O or serialization error is logged and swallowed — a corrupt or
//! unwritable cache must never panic or block the single-threaded stack loop. The write is atomic
//! (temp file + rename) so a crash mid-write cannot leave a half-written cache.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// On-disk schema version. Bump if the record shape changes incompatibly; an older/newer file is
/// then ignored (treated as empty) rather than mis-parsed.
const CACHE_VERSION: u32 = 1;

/// One persisted terminal. The L2 handle is deliberately NOT stored: it is inert in this stack
/// (MLE addresses downlink MM PDUs by ISSI; the handle plumbing is a stub), so the recovery
/// COMMAND is sent with handle 0 and reaches the camped radio by its ISSI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalRecord {
    pub issi: u32,
    #[serde(default)]
    pub groups: Vec<u32>,
    /// Energy-saving mode as the raw ETSI value (0 = StayAlive). Restored so the re-registering
    /// MS keeps its EE schedule; defaults to 0 when absent.
    #[serde(default)]
    pub energy_saving_mode: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    #[serde(default)]
    terminals: Vec<TerminalRecord>,
}

/// Owns the cache file path + a dirty/debounce flush gate. Single-threaded: lives inside MM's
/// entity tick, so it needs no locking.
pub struct RecoveryCache {
    path: PathBuf,
    dirty: bool,
    last_flush: Instant,
    debounce: Duration,
}

impl RecoveryCache {
    /// Construct without touching disk. `debounce` coalesces a burst of registry changes into one
    /// write (spares SD-card wear).
    pub fn new(path: PathBuf, debounce: Duration) -> Self {
        Self {
            path,
            dirty: false,
            last_flush: Instant::now(),
            debounce,
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Load the persisted terminals. Returns an empty list on ANY problem (missing file, bad
    /// JSON, version mismatch) and never panics, so a corrupt cache cannot wedge boot.
    pub fn load(&self) -> Vec<TerminalRecord> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        match serde_json::from_str::<CacheFile>(&text) {
            Ok(cf) if cf.version == CACHE_VERSION => cf.terminals,
            Ok(cf) => {
                tracing::warn!(
                    "recovery cache {} has version {} (expected {}); ignoring",
                    self.path.display(),
                    cf.version,
                    CACHE_VERSION
                );
                Vec::new()
            }
            Err(e) => {
                tracing::warn!("recovery cache {} parse error ({}); ignoring", self.path.display(), e);
                Vec::new()
            }
        }
    }

    /// Mark the in-memory registry as changed since the last flush.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Flush only if dirty AND the debounce window has elapsed. `snapshot` is invoked lazily, so
    /// callers don't pay to build the record list on ticks where no write is due.
    pub fn maybe_flush(&mut self, snapshot: impl FnOnce() -> Vec<TerminalRecord>) {
        if !self.dirty || self.last_flush.elapsed() < self.debounce {
            return;
        }
        let records = snapshot();
        self.flush_now(records);
    }

    /// Write the given records now. On failure the dirty flag is kept so the next opportunity
    /// retries; on success it is cleared and the debounce timer reset.
    pub fn flush_now(&mut self, records: Vec<TerminalRecord>) {
        let cf = CacheFile {
            version: CACHE_VERSION,
            terminals: records,
        };
        let text = match serde_json::to_string_pretty(&cf) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("recovery cache serialize failed: {}", e);
                return;
            }
        };
        match write_atomic(&self.path, &text) {
            Ok(()) => {
                self.dirty = false;
                self.last_flush = Instant::now();
                tracing::debug!("recovery cache written: {} terminal(s)", cf.terminals.len());
            }
            Err(e) => {
                // Keep `dirty` so we retry, but advance the debounce timer so a persistent
                // failure (e.g. read-only fs) retries at most once per debounce window instead
                // of a synchronous write + full snapshot on every PHY-paced tick.
                self.last_flush = Instant::now();
                tracing::warn!(
                    "recovery cache write to {} failed ({}); will retry after debounce",
                    self.path.display(),
                    e
                );
            }
        }
    }
}

/// Write `text` to `path` atomically: serialize to `<path>.tmp` then rename over the target.
/// The rename is atomic on the same filesystem, so a crash mid-write can never leave a
/// half-written cache (the old file stays intact until the rename completes).
fn write_atomic(path: &PathBuf, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Don't leave a stray temp file behind if the rename fails (best-effort cleanup).
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn round_trips_records() {
        let path = temp_path("fs_recovery_roundtrip.json");
        let _ = std::fs::remove_file(&path);
        let mut cache = RecoveryCache::new(path.clone(), Duration::ZERO);
        let records = vec![
            TerminalRecord {
                issi: 2260571,
                groups: vec![91, 92],
                energy_saving_mode: 0,
            },
            TerminalRecord {
                issi: 1000001,
                groups: vec![],
                energy_saving_mode: 2,
            },
        ];
        cache.flush_now(records.clone());
        let loaded = RecoveryCache::new(path.clone(), Duration::ZERO).load();
        assert_eq!(loaded, records);
        // No leftover temp file after a successful atomic write.
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_loads_empty() {
        let path = temp_path("fs_recovery_does_not_exist.json");
        let _ = std::fs::remove_file(&path);
        assert!(RecoveryCache::new(path, Duration::ZERO).load().is_empty());
    }

    #[test]
    fn garbage_loads_empty_without_panic() {
        let path = temp_path("fs_recovery_garbage.json");
        std::fs::write(&path, "{ this is not valid json ]").unwrap();
        assert!(RecoveryCache::new(path.clone(), Duration::ZERO).load().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_version_loads_empty() {
        let path = temp_path("fs_recovery_version.json");
        std::fs::write(&path, r#"{"version":999,"terminals":[{"issi":1}]}"#).unwrap();
        assert!(RecoveryCache::new(path.clone(), Duration::ZERO).load().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn debounce_blocks_then_allows() {
        let path = temp_path("fs_recovery_debounce.json");
        let _ = std::fs::remove_file(&path);
        // Long debounce: a dirty flush is suppressed until the window elapses.
        let mut cache = RecoveryCache::new(path.clone(), Duration::from_secs(3600));
        cache.mark_dirty();
        cache.maybe_flush(|| {
            vec![TerminalRecord {
                issi: 7,
                groups: vec![],
                energy_saving_mode: 0,
            }]
        });
        assert!(!path.exists(), "should not write before debounce elapses");
        // Zero debounce: flushes immediately when dirty.
        let mut cache = RecoveryCache::new(path.clone(), Duration::ZERO);
        cache.mark_dirty();
        cache.maybe_flush(|| {
            vec![TerminalRecord {
                issi: 7,
                groups: vec![],
                energy_saving_mode: 0,
            }]
        });
        assert!(path.exists(), "should write once debounce is satisfied");
        assert!(!cache.is_dirty());
        // Not dirty → no-op.
        let mut cache = RecoveryCache::new(path.clone(), Duration::ZERO);
        cache.maybe_flush(|| panic!("snapshot must not be called when clean"));
        let _ = std::fs::remove_file(&path);
    }
}
