//! RadioID callsign lookup for the dashboard.
//!
//! Callsigns are resolved in two stages:
//!
//! 1. **In-memory cache** — once a callsign is resolved it is kept for the
//!    lifetime of the process so repeat lookups are free.
//!
//! 2. **Local `dmrids.dat` bulk database** — downloaded once from RadioID on
//!    first use (or on explicit refresh) and stored at
//!    `<config_dir>/dmrids.dat`.  A background thread refreshes it every 24 h.
//!    The file format is CSV: `id,callsign,name,...` (RadioID bulk export).
//!
//! 3. **RadioID REST API** — used as a fallback when the local DB has no entry
//!    for an ISSI.  Results are inserted into the in-memory cache.
//!
//! The HTTP handler (`GET /api/callsign?issi=<n>`) is intentionally kept
//! synchronous (blocking reqwest) so it fits the existing server architecture
//! without introducing async complexity.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RADIOID_API: &str = "https://radioid.net/api/dmr/user/?id=";
const DMRIDS_URL:  &str = "https://radioid.net/static/dmrids.dat";
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Public shared handle
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CallsignDb(Arc<Mutex<Inner>>);

struct Inner {
    /// ISSI → callsign, populated from either the bulk DB or the REST API.
    cache: HashMap<u32, String>,
    /// Path to the on-disk dmrids.dat file.
    dat_path: PathBuf,
    /// When we last refreshed the bulk DB from RadioID.
    last_refresh: Option<Instant>,
}

impl CallsignDb {
    /// Create a new database. `data_dir` is where `dmrids.dat` will be stored
    /// (typically the same directory as the config file).
    pub fn new(data_dir: &Path) -> Self {
        let dat_path = data_dir.join("dmrids.dat");
        let mut inner = Inner {
            cache: HashMap::new(),
            dat_path: dat_path.clone(),
            last_refresh: None,
        };
        // Load existing on-disk DB immediately so lookups work before the
        // background refresh completes.
        if dat_path.exists() {
            inner.load_dat_file();
        }
        let db = CallsignDb(Arc::new(Mutex::new(inner)));
        // Spawn background refresh thread.
        {
            let db2 = db.clone();
            std::thread::Builder::new()
                .name("callsign-refresh".into())
                .spawn(move || db2.refresh_loop())
                .ok();
        }
        db
    }

    /// Look up a callsign for the given ISSI.
    /// Returns `None` if the ISSI is unknown to both the local DB and RadioID.
    pub fn lookup(&self, issi: u32) -> Option<String> {
        // Fast path: in-memory cache hit.
        {
            let inner = self.0.lock().unwrap();
            if let Some(cs) = inner.cache.get(&issi) {
                return Some(cs.clone());
            }
        }
        // Slow path: query RadioID REST API.
        self.api_lookup(issi)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn api_lookup(&self, issi: u32) -> Option<String> {
        let url = format!("{}{}", RADIOID_API, issi);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok()?;
        let resp: serde_json::Value = client.get(&url).send().ok()?.json().ok()?;
        let cs = resp["results"][0]["callsign"].as_str()?.to_string();
        // Store in cache for future lookups.
        self.0.lock().unwrap().cache.insert(issi, cs.clone());
        Some(cs)
    }

    fn refresh_loop(&self) {
        loop {
            let needs_refresh = {
                let inner = self.0.lock().unwrap();
                match inner.last_refresh {
                    None => true,
                    Some(t) => t.elapsed() >= REFRESH_INTERVAL,
                }
            };
            if needs_refresh {
                self.download_dat();
            }
            std::thread::sleep(Duration::from_secs(60 * 60)); // check every hour
        }
    }

    fn download_dat(&self) {
        tracing::info!("callsign: downloading dmrids.dat from RadioID…");
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
        {
            Ok(c) => c,
            Err(e) => { tracing::warn!("callsign: failed to build client: {e}"); return; }
        };
        let dat_path = self.0.lock().unwrap().dat_path.clone();
        let bytes = match client.get(DMRIDS_URL).send().and_then(|r| r.bytes()) {
            Ok(b) => b,
            Err(e) => { tracing::warn!("callsign: dmrids.dat download failed: {e}"); return; }
        };
        if let Err(e) = fs::write(&dat_path, &bytes) {
            tracing::warn!("callsign: failed to write dmrids.dat: {e}");
            return;
        }
        tracing::info!("callsign: dmrids.dat saved ({} bytes), loading…", bytes.len());
        let mut inner = self.0.lock().unwrap();
        inner.load_dat_file();
        inner.last_refresh = Some(Instant::now());
    }
}

impl Inner {
    /// Parse `dmrids.dat` into the in-memory cache.
    /// Format: `<id>,<callsign>,<name>,...` one entry per line, no header.
    fn load_dat_file(&mut self) {
        let file = match fs::File::open(&self.dat_path) {
            Ok(f) => f,
            Err(e) => { tracing::warn!("callsign: cannot open dmrids.dat: {e}"); return; }
        };
        let reader = BufReader::new(file);
        let mut count = 0usize;
        for line in reader.lines().map_while(Result::ok) {
            let mut parts = line.splitn(3, ',');
            let id_str = parts.next().unwrap_or("").trim();
            let callsign = parts.next().unwrap_or("").trim();
            if callsign.is_empty() { continue; }
            if let Ok(id) = id_str.parse::<u32>() {
                self.cache.insert(id, callsign.to_string());
                count += 1;
            }
        }
        tracing::info!("callsign: loaded {count} entries from dmrids.dat");
    }
}
