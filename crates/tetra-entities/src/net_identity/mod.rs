use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};
use tetra_config::bluestation::{CfgIdentity, CfgManualIdentity, CfgRadioId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    Manual,
    RadioId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRecord {
    pub ssi: u32,
    pub mnemonic: Option<String>,
    pub label: Option<String>,
    pub source: IdentitySource,
}

#[derive(Debug, Clone)]
struct CachedIdentity {
    record: Option<IdentityRecord>,
    expires_at: Instant,
    inserted_at: Instant,
}

#[derive(Debug)]
struct IdentityResolverInner {
    cache: HashMap<u32, CachedIdentity>,
    last_remote_lookup: Option<Instant>,
    pending_remote: HashSet<u32>,
}

#[derive(Debug)]
pub struct IdentityResolver {
    enabled: bool,
    manual: HashMap<u32, IdentityRecord>,
    cache_ttl: Duration,
    negative_cache_ttl: Duration,
    cache_max_entries: usize,
    radioid: CfgRadioId,
    inner: Arc<Mutex<IdentityResolverInner>>,
}

impl IdentityResolver {
    pub fn new(config: &CfgIdentity) -> Self {
        let manual = config
            .manual
            .iter()
            .map(manual_identity_record)
            .map(|record| (record.ssi, record))
            .collect();

        Self {
            enabled: config.enabled,
            manual,
            cache_ttl: Duration::from_secs(config.cache_ttl_secs),
            negative_cache_ttl: Duration::from_secs(config.negative_cache_ttl_secs),
            cache_max_entries: config.cache_max_entries,
            radioid: config.radioid.clone(),
            inner: Arc::new(Mutex::new(IdentityResolverInner {
                cache: HashMap::new(),
                last_remote_lookup: None,
                pending_remote: HashSet::new(),
            })),
        }
    }

    pub fn disabled() -> Self {
        Self::new(&CfgIdentity::default())
    }

    pub fn lookup(&self, ssi: u32) -> Option<IdentityRecord> {
        if !self.enabled {
            return None;
        }

        if let Some(record) = self.manual.get(&ssi) {
            return Some(record.clone());
        }

        let now = Instant::now();
        if let Some(record) = self.cached_lookup(ssi, now) {
            return record;
        }

        if self.radioid.enabled {
            self.spawn_radioid_lookup(ssi, now);
        }

        None
    }

    pub fn invalidate(&self, ssi: u32) {
        let mut inner = self.inner.lock().expect("IdentityResolver mutex poisoned");
        inner.cache.remove(&ssi);
    }

    pub fn warm_cache<I>(&self, ssis: I)
    where
        I: IntoIterator<Item = u32>,
    {
        for ssi in ssis {
            let _ = self.lookup(ssi);
        }
    }

    fn cached_lookup(&self, ssi: u32, now: Instant) -> Option<Option<IdentityRecord>> {
        let mut inner = self.inner.lock().expect("IdentityResolver mutex poisoned");
        match inner.cache.get(&ssi) {
            Some(entry) if entry.expires_at > now => Some(entry.record.clone()),
            Some(_) => {
                inner.cache.remove(&ssi);
                None
            }
            None => None,
        }
    }

    fn try_take_remote_slot(&self, ssi: u32, now: Instant) -> bool {
        let min_interval = Duration::from_millis(self.radioid.min_lookup_interval_ms);
        let mut inner = self.inner.lock().expect("IdentityResolver mutex poisoned");
        if inner.pending_remote.contains(&ssi) {
            return false;
        }
        if let Some(last) = inner.last_remote_lookup {
            if now.duration_since(last) < min_interval {
                return false;
            }
        }
        inner.last_remote_lookup = Some(now);
        inner.pending_remote.insert(ssi);
        true
    }

    fn spawn_radioid_lookup(&self, ssi: u32, now: Instant) {
        if !self.try_take_remote_slot(ssi, now) {
            return;
        }

        let radioid = self.radioid.clone();
        let inner = Arc::clone(&self.inner);
        let cache_ttl = self.cache_ttl;
        let negative_cache_ttl = self.negative_cache_ttl;
        let cache_max_entries = self.cache_max_entries;

        thread::spawn(move || {
            let record = lookup_radioid_with_config(&radioid, ssi);
            let ttl = if record.is_some() { cache_ttl } else { negative_cache_ttl };
            let mut inner = inner.lock().expect("IdentityResolver mutex poisoned");
            inner.pending_remote.remove(&ssi);
            cache_insert_locked(&mut inner, cache_max_entries, ttl, ssi, record, Instant::now());
        });
    }
}

fn cache_insert_locked(
    inner: &mut IdentityResolverInner,
    cache_max_entries: usize,
    ttl: Duration,
    ssi: u32,
    record: Option<IdentityRecord>,
    now: Instant,
) {
    if ttl.is_zero() || cache_max_entries == 0 {
        return;
    }

    if inner.cache.len() >= cache_max_entries {
        if let Some(expired_key) = inner
            .cache
            .iter()
            .find_map(|(key, entry)| (entry.expires_at <= now).then_some(*key))
        {
            inner.cache.remove(&expired_key);
        } else if let Some(oldest_key) = inner.cache.iter().min_by_key(|(_, entry)| entry.inserted_at).map(|(key, _)| *key) {
            inner.cache.remove(&oldest_key);
        }
    }

    inner.cache.insert(
        ssi,
        CachedIdentity {
            record,
            expires_at: now + ttl,
            inserted_at: now,
        },
    );
}

fn lookup_radioid_with_config(radioid: &CfgRadioId, ssi: u32) -> Option<IdentityRecord> {
    let url = radioid_lookup_url(&radioid.endpoint, ssi);
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(radioid.timeout_secs)).build();

    let mut request = agent.get(&url).set("User-Agent", &radioid.user_agent);
    if let Some(token) = &radioid.api_token {
        request = request.set("X-API-Token", token.as_ref());
    }

    let response = match request.call() {
        Ok(response) => response,
        Err(err) => {
            tracing::debug!("RadioID lookup failed for SSI {}: {}", ssi, err);
            return None;
        }
    };

    match response.into_json::<Value>() {
        Ok(value) => parse_radioid_identity(ssi, &value),
        Err(err) => {
            tracing::debug!("RadioID JSON decode failed for SSI {}: {}", ssi, err);
            None
        }
    }
}

fn manual_identity_record(entry: &CfgManualIdentity) -> IdentityRecord {
    IdentityRecord {
        ssi: entry.ssi,
        mnemonic: entry.mnemonic.as_deref().and_then(normalize_mnemonic),
        label: entry.label.as_deref().and_then(normalize_label),
        source: IdentitySource::Manual,
    }
}

fn radioid_lookup_url(endpoint: &str, ssi: u32) -> String {
    let separator = if endpoint.contains('?') {
        if endpoint.ends_with('?') || endpoint.ends_with('&') {
            ""
        } else {
            "&"
        }
    } else {
        "?"
    };
    format!("{endpoint}{separator}id={ssi}")
}

fn parse_radioid_identity(ssi: u32, value: &Value) -> Option<IdentityRecord> {
    let user = first_user_object(value)?;
    let callsign = get_string(user, &["callsign", "callSign", "call_sign"]);
    let label = radioid_label(user);
    let resolved_ssi = get_u32(user, &["id", "radio_id", "radioid", "dmr_id"]).unwrap_or(ssi);

    Some(IdentityRecord {
        ssi: resolved_ssi,
        mnemonic: callsign.as_deref().and_then(normalize_mnemonic),
        label,
        source: IdentitySource::RadioId,
    })
    .filter(|record| record.mnemonic.is_some() || record.label.is_some())
}

fn first_user_object(value: &Value) -> Option<&Map<String, Value>> {
    match value {
        Value::Object(obj) if has_identity_keys(obj) => Some(obj),
        Value::Object(obj) => ["results", "data", "users", "items"]
            .iter()
            .find_map(|key| obj.get(*key))
            .and_then(first_user_object),
        Value::Array(items) => items.iter().find_map(first_user_object),
        _ => None,
    }
}

fn has_identity_keys(obj: &Map<String, Value>) -> bool {
    obj.contains_key("callsign") || obj.contains_key("id") || obj.contains_key("radio_id") || obj.contains_key("dmr_id")
}

fn radioid_label(user: &Map<String, Value>) -> Option<String> {
    if let Some(label) = get_string(user, &["name", "display_name", "displayName"]) {
        return normalize_label(&label);
    }

    let first = get_string(user, &["fname", "first_name", "firstName"]);
    let last = get_string(user, &["surname", "last_name", "lastName"]);
    normalize_label(
        &[first.as_deref().unwrap_or(""), last.as_deref().unwrap_or("")]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn get_string(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match obj.get(*key)? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn get_u32(obj: &Map<String, Value>, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| match obj.get(*key)? {
        Value::Number(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()),
        Value::String(value) => value.parse::<u32>().ok(),
        _ => None,
    })
}

pub fn normalize_mnemonic(value: &str) -> Option<String> {
    let normalized = value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/'))
        .take(15)
        .collect::<String>()
        .to_ascii_uppercase();

    if normalized.is_empty() { None } else { Some(normalized) }
}

pub fn normalize_label(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let label = trimmed.chars().filter(|ch| !ch.is_control()).take(64).collect::<String>();
    if label.is_empty() { None } else { Some(label) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::{CfgIdentity, CfgManualIdentity};

    #[test]
    fn manual_identity_has_priority() {
        let mut cfg = CfgIdentity {
            enabled: true,
            ..CfgIdentity::default()
        };
        cfg.manual.push(CfgManualIdentity {
            ssi: 2260571,
            mnemonic: Some("yo6rzv".to_string()),
            label: Some("Razvan".to_string()),
        });
        let resolver = IdentityResolver::new(&cfg);

        let record = resolver.lookup(2260571).expect("manual record");
        assert_eq!(record.mnemonic.as_deref(), Some("YO6RZV"));
        assert_eq!(record.label.as_deref(), Some("Razvan"));
        assert_eq!(record.source, IdentitySource::Manual);
    }

    #[test]
    fn radioid_parser_handles_array_payload() {
        let value = serde_json::json!([
            {
                "id": "2260571",
                "callsign": "yo6rzv",
                "fname": "Razvan",
                "surname": "Zeces"
            }
        ]);

        let record = parse_radioid_identity(2260571, &value).expect("record");
        assert_eq!(record.ssi, 2260571);
        assert_eq!(record.mnemonic.as_deref(), Some("YO6RZV"));
        assert_eq!(record.label.as_deref(), Some("Razvan Zeces"));
    }

    #[test]
    fn radioid_url_adds_id_query_parameter() {
        assert_eq!(
            radioid_lookup_url("https://radioid.net/api/dmr/user/", 123),
            "https://radioid.net/api/dmr/user/?id=123"
        );
        assert_eq!(
            radioid_lookup_url("https://example.test/users?foo=bar", 123),
            "https://example.test/users?foo=bar&id=123"
        );
    }
}
