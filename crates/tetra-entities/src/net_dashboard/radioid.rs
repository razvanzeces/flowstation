//! On-demand RadioID (radioid.net) callsign resolver with a local disk + memory cache.
//!
//! The ISSIs on this network are DMR IDs, so radioid.net resolves them to a callsign
//! ("indicativ"). To avoid hammering the public API, every ID is queried AT MOST ONCE: the
//! result — a found callsign, or a definitive "not in the database" — is cached in memory and
//! persisted to a small JSON file so it survives restarts. Lookups never block the caller:
//! [`RadioIdCache::get`] returns the cached value immediately or [`Lookup::Pending`] after
//! queuing a throttled background fetch handled by a dedicated worker thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Per-ID lookup endpoint. radioid.net answers `{"count":N,"results":[{"id":..,"callsign":..}]}`.
const API_URL_PREFIX:      &str = "https://radioid.net/api/dmr/user/?id=";
const RPTR_API_URL_PREFIX: &str = "https://radioid.net/api/dmr/repeater/?id=";
/// Politeness delay between successive API calls (each ID is fetched only once, ever).
const FETCH_THROTTLE: Duration = Duration::from_millis(1100);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Result of a callsign lookup.
pub enum Lookup {
    /// Resolved to a callsign.
    Found(String),
    /// Looked up and confirmed to have no RadioID entry — callers should not retry.
    NotFound,
    /// Not yet resolved; a background fetch has been queued. Callers may retry later.
    Pending,
}

#[derive(Clone)]
enum Entry {
    Found(String),
    NotFound,
}

struct Inner {
    map: HashMap<u32, Entry>,
    /// IDs currently queued / in-flight, to dedup fetch requests.
    pending: HashSet<u32>,
    path: PathBuf,
}

/// Cheap-to-clone handle to the shared cache + background fetch worker.
#[derive(Clone)]
pub struct RadioIdCache {
    inner: Arc<Mutex<Inner>>,
    tx: crossbeam_channel::Sender<u32>,
}

impl RadioIdCache {
    /// Build the cache, load any persisted entries from `path`, and spawn the background fetch
    /// worker. Cloning shares the same inner state and worker.
    pub fn new(path: PathBuf) -> Self {
        let map = load_disk(&path);
        if !map.is_empty() {
            tracing::info!("RadioID: loaded {} cached callsign(s) from {}", map.len(), path.display());
        }
        let inner = Arc::new(Mutex::new(Inner { map, pending: HashSet::new(), path }));
        let (tx, rx) = crossbeam_channel::unbounded::<u32>();
        let worker_inner = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("radioid-fetch".into())
            .spawn(move || worker_loop(rx, worker_inner))
            .ok();
        Self { inner, tx }
    }

    /// Look up `issi`, returning the cached result immediately or [`Lookup::Pending`] after
    /// queuing a background fetch (deduped) for an ID we have never resolved.
    pub fn get(&self, issi: u32) -> Lookup {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.map.get(&issi) {
            return match entry {
                Entry::Found(cs) => Lookup::Found(cs.clone()),
                Entry::NotFound => Lookup::NotFound,
            };
        }
        if inner.pending.insert(issi) {
            let _ = self.tx.send(issi);
        }
        Lookup::Pending
    }
}

fn worker_loop(rx: crossbeam_channel::Receiver<u32>, inner: Arc<Mutex<Inner>>) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("flowstation-dashboard")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("RadioID: HTTP client init failed ({e}) — callsign resolution disabled");
            return;
        }
    };
    while let Ok(issi) = rx.recv() {
        // Another request may have already resolved it between queue and now.
        if inner.lock().unwrap().map.contains_key(&issi) {
            inner.lock().unwrap().pending.remove(&issi);
            continue;
        }
        match fetch_callsign(&client, issi) {
            Ok(Some(cs)) => {
                let mut g = inner.lock().unwrap();
                g.map.insert(issi, Entry::Found(cs.clone()));
                g.pending.remove(&issi);
                persist(&g);
                tracing::debug!("RadioID: {issi} -> {cs}");
            }
            Ok(None) => {
                // Definitive: this ID is not in the RadioID database. Cache so we never re-query.
                let mut g = inner.lock().unwrap();
                g.map.insert(issi, Entry::NotFound);
                g.pending.remove(&issi);
                persist(&g);
                tracing::debug!("RadioID: {issi} not in database");
            }
            Err(e) => {
                // Transient (offline / parse error) — leave uncached so a later request retries.
                inner.lock().unwrap().pending.remove(&issi);
                tracing::debug!("RadioID: lookup for {issi} failed, will retry later: {e}");
            }
        }
        std::thread::sleep(FETCH_THROTTLE);
    }
}

/// Fetch one ID. `Ok(Some(callsign))` = found, `Ok(None)` = confirmed absent, `Err` = transient.
///
/// Tries the user database first. If the ID is not found there, tries the repeater database.
/// This covers both individual operators (7-digit IDs) and repeaters/gateways (6-digit IDs),
/// without hard-coding a digit-count heuristic that may not hold universally.
fn fetch_callsign(client: &reqwest::blocking::Client, issi: u32) -> Result<Option<String>, String> {
    // Try user database first.
    if let Some(cs) = query_radioid(client, &format!("{API_URL_PREFIX}{issi}"))? {
        return Ok(Some(cs));
    }
    // Fall back to repeater database.
    query_radioid(client, &format!("{RPTR_API_URL_PREFIX}{issi}"))
}

/// Query a RadioID API endpoint and extract the callsign from `results[0].callsign`.
/// Returns `Ok(Some(callsign))` = found, `Ok(None)` = confirmed absent, `Err` = transient.
fn query_radioid(client: &reqwest::blocking::Client, url: &str) -> Result<Option<String>, String> {
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    match json.get("results").and_then(|r| r.as_array()) {
        Some(arr) if !arr.is_empty() => {
            let cs = arr[0]
                .get("callsign")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if cs.is_empty() { Ok(None) } else { Ok(Some(cs)) }
        }
        Some(_) => Ok(None), // empty results array = not in DB
        None => Err("unexpected response shape".to_string()),
    }
}

/// On-disk format: `{ "2260324": "YO3XYZ", "2260575": "" }` — empty string means NotFound.
fn load_disk(path: &PathBuf) -> HashMap<u32, Entry> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return map };
    let Ok(json) = serde_json::from_str::<HashMap<String, String>>(&text) else { return map };
    for (k, v) in json {
        if let Ok(id) = k.parse::<u32>() {
            map.insert(id, if v.is_empty() { Entry::NotFound } else { Entry::Found(v) });
        }
    }
    map
}

fn persist(inner: &Inner) {
    let json: HashMap<String, String> = inner
        .map
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Entry::Found(c) => c.clone(),
                Entry::NotFound => String::new(),
            };
            (k.to_string(), s)
        })
        .collect();
    if let Ok(text) = serde_json::to_string(&json) {
        let _ = std::fs::write(&inner.path, text);
    }
}
