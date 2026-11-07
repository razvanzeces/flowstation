//! Visual configurator profiles (Cell × Brew), inspired by BTS Mia `bts-web`.
//!
//! Cell profiles store RF + network/cell identity (`phy_io`, `net_info`, `cell_info`),
//! plus Cell-bound `security` (ISSI whitelist) and `recovery` (proactive restart recovery).
//! Brew profiles store the optional `[brew]` backhaul. Apply merges a Cell × Brew
//! selection into the live `config.toml`, preserving Station/Integration sections,
//! then validates with the same parse+`validate()` path as the raw editor.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value as TomlValue;

use crate::net_lst_dispatch::LST_DISPATCH_PROFILE;

const MASKED_PASSWORD: &str = "••••••••";

/// Active profile selection on disk (`profiles/active.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveProfiles {
    pub cell: String,
    /// `None` / JSON null → offline (no `[brew]`).
    /// `Some("__lst_dispatch__")` → LST Dispatch profile (no Brew).
    /// `Some(name)` → named Brew profile.
    pub brew: Option<String>,
}

impl Default for ActiveProfiles {
    fn default() -> Self {
        Self {
            cell: "Default".to_string(),
            brew: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileListItem {
    pub name: String,
    pub active: bool,
}

fn profiles_root(config_path: &str) -> PathBuf {
    Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("profiles")
}

/// Directory containing Cell/Brew profile JSON (`…/profiles`).
pub fn profiles_dir(config_path: &str) -> PathBuf {
    profiles_root(config_path)
}

fn cell_dir(config_path: &str) -> PathBuf {
    profiles_root(config_path).join("cell")
}

fn brew_dir(config_path: &str) -> PathBuf {
    profiles_root(config_path).join("brew")
}

fn active_path(config_path: &str) -> PathBuf {
    profiles_root(config_path).join("active.json")
}

fn sanitize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("profile name is empty".into());
    }
    if name.len() > 64 {
        return Err("profile name too long (max 64)".into());
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("profile name contains illegal characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.')
    {
        return Err("profile name may only contain letters, digits, space, ._-".into());
    }
    Ok(name.to_string())
}

fn profile_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

/// Absolute path to a Cell profile JSON (creates dirs via [`ensure_seeded`]).
pub fn profile_file_for_cell(config_path: &str, name: &str) -> Result<PathBuf, String> {
    let _ = ensure_seeded(config_path);
    let name = sanitize_name(name)?;
    Ok(profile_file(&cell_dir(config_path), &name))
}

/// Ensure profile directories exist and seed from the live TOML on first run.
pub fn ensure_seeded(config_path: &str) -> Result<(), String> {
    let cell = cell_dir(config_path);
    let brew = brew_dir(config_path);
    fs::create_dir_all(&cell).map_err(|e| e.to_string())?;
    fs::create_dir_all(&brew).map_err(|e| e.to_string())?;

    let cell_empty = fs::read_dir(&cell)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if !cell_empty {
        return Ok(());
    }

    let toml_txt = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let table: toml::Table = toml::from_str(&toml_txt).map_err(|e| format!("parse config: {e}"))?;

    let mut cell_json = Map::new();
    for key in ["phy_io", "net_info", "cell_info"] {
        if let Some(v) = table.get(key) {
            cell_json.insert(key.to_string(), toml_to_json(v));
        }
    }
    // Seed access control into the Default Cell so existing installs keep their whitelist
    // when they start using Cell-bound access (missing key = open network).
    if let Some(v) = table.get("security") {
        cell_json.insert("security".into(), toml_to_json(v));
    }
    // Restart recovery travels with the Cell (proactive D-LOCATION-UPDATE-COMMAND on boot).
    if let Some(v) = table.get("recovery") {
        cell_json.insert("recovery".into(), toml_to_json(v));
    }
    // Keep stack_mode / config_version for completeness when re-materialising.
    if let Some(v) = table.get("config_version") {
        cell_json.insert("config_version".into(), toml_to_json(v));
    }
    if let Some(v) = table.get("stack_mode") {
        cell_json.insert("stack_mode".into(), toml_to_json(v));
    }

    save_json_profile(&cell, "Default", &JsonValue::Object(cell_json))?;

    let mut active = ActiveProfiles::default();
    if let Some(brew_val) = table.get("brew") {
        let mut brew_obj = match toml_to_json(brew_val) {
            JsonValue::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("value".into(), other);
                m
            }
        };
        // Prefer a friendly default name from host when present.
        let name = brew_obj
            .get("host")
            .and_then(|h| h.as_str())
            .map(|h| format!("Brew {h}"))
            .unwrap_or_else(|| "Default Brew".to_string());
        let name = sanitize_name(&name).unwrap_or_else(|_| "Default Brew".into());
        if let Some(JsonValue::String(p)) = brew_obj.get("password").cloned() {
            if p.is_empty() {
                brew_obj.insert("password".into(), JsonValue::String(String::new()));
            }
        }
        save_json_profile(&brew, &name, &JsonValue::Object(brew_obj))?;
        active.brew = Some(name);
    }
    write_active(config_path, &active)?;
    Ok(())
}

fn save_json_profile(dir: &Path, name: &str, value: &JsonValue) -> Result<(), String> {
    let name = sanitize_name(name)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = profile_file(dir, &name);
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    atomic_write_str(&path, &body)
}

fn atomic_write_str(path: &Path, body: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn read_active(config_path: &str) -> ActiveProfiles {
    let _ = ensure_seeded(config_path);
    fs::read_to_string(active_path(config_path))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn write_active(config_path: &str, active: &ActiveProfiles) -> Result<(), String> {
    fs::create_dir_all(profiles_root(config_path)).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(active).map_err(|e| e.to_string())?;
    atomic_write_str(&active_path(config_path), &body)
}

fn list_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return names;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            names.push(stem.to_string());
        }
    }
    names.sort();
    names
}

pub fn list_cell_profiles(config_path: &str) -> Result<Vec<ProfileListItem>, String> {
    let _ = ensure_seeded(config_path);
    let active = read_active(config_path);
    Ok(list_names(&cell_dir(config_path))
        .into_iter()
        .map(|name| ProfileListItem {
            active: name == active.cell,
            name,
        })
        .collect())
}

pub fn list_brew_profiles(config_path: &str) -> Result<Vec<ProfileListItem>, String> {
    let _ = ensure_seeded(config_path);
    let active = read_active(config_path);
    Ok(list_names(&brew_dir(config_path))
        .into_iter()
        .map(|name| ProfileListItem {
            active: active.brew.as_deref() == Some(name.as_str()),
            name,
        })
        .collect())
}

pub fn get_cell_profile(config_path: &str, name: &str) -> Result<JsonValue, String> {
    let name = sanitize_name(name)?;
    let path = profile_file(&cell_dir(config_path), &name);
    let txt = fs::read_to_string(&path).map_err(|e| format!("cell profile '{name}': {e}"))?;
    serde_json::from_str(&txt).map_err(|e| e.to_string())
}

pub fn get_brew_profile(config_path: &str, name: &str, mask_secrets: bool) -> Result<JsonValue, String> {
    let name = sanitize_name(name)?;
    let path = profile_file(&brew_dir(config_path), &name);
    let txt = fs::read_to_string(&path).map_err(|e| format!("brew profile '{name}': {e}"))?;
    let mut v: JsonValue = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    if mask_secrets {
        if let Some(obj) = v.as_object_mut() {
            if let Some(JsonValue::String(p)) = obj.get("password") {
                if !p.is_empty() {
                    obj.insert("password".into(), JsonValue::String(MASKED_PASSWORD.into()));
                    obj.insert("password_set".into(), JsonValue::Bool(true));
                } else {
                    obj.insert("password_set".into(), JsonValue::Bool(false));
                }
            }
        }
    }
    Ok(v)
}

pub fn put_cell_profile(config_path: &str, name: &str, body: &JsonValue) -> Result<(), String> {
    let name = sanitize_name(name)?;
    if !body.is_object() {
        return Err("cell profile body must be a JSON object".into());
    }
    // Light shape check — full ETSI validation happens on apply against a merged TOML.
    let obj = body.as_object().unwrap();
    if !obj.contains_key("phy_io") && !obj.contains_key("net_info") && !obj.contains_key("cell_info") {
        return Err("cell profile must include phy_io, net_info and/or cell_info".into());
    }
    let mut cleaned = body.clone();
    if let Some(ci) = cleaned.get_mut("cell_info") {
        strip_sds_command_control_json(ci);
    }
    save_json_profile(&cell_dir(config_path), &name, &cleaned)
}

pub fn put_brew_profile(config_path: &str, name: &str, body: &JsonValue) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let mut obj = body
        .as_object()
        .cloned()
        .ok_or_else(|| "brew profile body must be a JSON object".to_string())?;
    let host = obj
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if host.is_empty() {
        return Err("brew.host is required".into());
    }
    // Rehydrate masked password from existing profile when the client did not retype it.
    if matches!(obj.get("password"), Some(JsonValue::String(p)) if p == MASKED_PASSWORD || p.chars().all(|c| c == '•'))
    {
        if let Ok(existing) = get_brew_profile_raw(config_path, &name) {
            if let Some(JsonValue::String(real)) = existing.get("password") {
                obj.insert("password".into(), JsonValue::String(real.clone()));
            }
        } else {
            return Err("password is masked; re-enter the password when creating a new brew profile".into());
        }
    }
    obj.remove("password_set");
    save_json_profile(&brew_dir(config_path), &name, &JsonValue::Object(obj))
}

fn get_brew_profile_raw(config_path: &str, name: &str) -> Result<JsonValue, String> {
    let path = profile_file(&brew_dir(config_path), name);
    let txt = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&txt).map_err(|e| e.to_string())
}

pub fn delete_cell_profile(config_path: &str, name: &str) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let active = read_active(config_path);
    if active.cell == name {
        return Err("cannot delete the active cell profile".into());
    }
    let names = list_names(&cell_dir(config_path));
    if names.len() <= 1 {
        return Err("cannot delete the last cell profile".into());
    }
    let path = profile_file(&cell_dir(config_path), &name);
    fs::remove_file(&path).map_err(|e| e.to_string())
}

pub fn delete_brew_profile(config_path: &str, name: &str) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let active = read_active(config_path);
    if active.brew.as_deref() == Some(name.as_str()) {
        return Err("cannot delete the active brew profile".into());
    }
    let path = profile_file(&brew_dir(config_path), &name);
    fs::remove_file(&path).map_err(|e| e.to_string())
}

/// Rename a cell profile file and update `active.json` when needed.
pub fn rename_cell_profile(config_path: &str, old_name: &str, new_name: &str) -> Result<(), String> {
    let old = sanitize_name(old_name)?;
    let new = sanitize_name(new_name)?;
    if old == new {
        return Ok(());
    }
    let dir = cell_dir(config_path);
    let from = profile_file(&dir, &old);
    let to = profile_file(&dir, &new);
    if !from.exists() {
        return Err(format!("cell profile '{old}' not found"));
    }
    if to.exists() {
        return Err(format!("cell profile '{new}' already exists"));
    }
    fs::rename(&from, &to).map_err(|e| e.to_string())?;
    let mut active = read_active(config_path);
    if active.cell == old {
        active.cell = new;
        write_active(config_path, &active)?;
    }
    Ok(())
}

/// Rename a brew profile file and update `active.json` when needed.
pub fn rename_brew_profile(config_path: &str, old_name: &str, new_name: &str) -> Result<(), String> {
    let old = sanitize_name(old_name)?;
    let new = sanitize_name(new_name)?;
    if old == new {
        return Ok(());
    }
    let dir = brew_dir(config_path);
    let from = profile_file(&dir, &old);
    let to = profile_file(&dir, &new);
    if !from.exists() {
        return Err(format!("brew profile '{old}' not found"));
    }
    if to.exists() {
        return Err(format!("brew profile '{new}' already exists"));
    }
    fs::rename(&from, &to).map_err(|e| e.to_string())?;
    let mut active = read_active(config_path);
    if active.brew.as_deref() == Some(old.as_str()) {
        active.brew = Some(new);
        write_active(config_path, &active)?;
    }
    Ok(())
}

pub fn duplicate_cell_profile(config_path: &str, name: &str, new_name: &str) -> Result<(), String> {
    let src = get_cell_profile(config_path, name)?;
    put_cell_profile(config_path, new_name, &src)
}

pub fn duplicate_brew_profile(config_path: &str, name: &str, new_name: &str) -> Result<(), String> {
    let src = get_brew_profile_raw(config_path, &sanitize_name(name)?)?;
    put_brew_profile(config_path, new_name, &src)
}

/// Extract a visual-config JSON view from the live TOML (secrets masked).
pub fn visual_config_from_toml(config_path: &str) -> Result<JsonValue, String> {
    let txt = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let table: toml::Table = toml::from_str(&txt).map_err(|e| format!("parse config: {e}"))?;
    let mut out = Map::new();
    for key in ["config_version", "stack_mode", "phy_io", "net_info", "cell_info"] {
        if let Some(v) = table.get(key) {
            out.insert(key.to_string(), toml_to_json(v));
        }
    }
    // Live access control for the visual Config form (MHz UI + Cell-bound whitelist).
    if let Some(v) = table.get("security") {
        out.insert("security".into(), toml_to_json(v));
    } else {
        let mut sec = Map::new();
        sec.insert("issi_whitelist".into(), JsonValue::Array(vec![]));
        out.insert("security".into(), JsonValue::Object(sec));
    }
    if let Some(v) = table.get("recovery") {
        out.insert("recovery".into(), toml_to_json(v));
    } else {
        let mut rec = Map::new();
        rec.insert("enabled".into(), JsonValue::Bool(false));
        out.insert("recovery".into(), JsonValue::Object(rec));
    }
    if let Some(brew) = table.get("brew") {
        let mut brew_json = toml_to_json(brew);
        if let Some(obj) = brew_json.as_object_mut() {
            let set = obj
                .get("password")
                .and_then(|p| p.as_str())
                .map(|p| !p.is_empty())
                .unwrap_or(false);
            if set {
                obj.insert("password".into(), JsonValue::String(MASKED_PASSWORD.into()));
            }
            obj.insert("password_set".into(), JsonValue::Bool(set));
            obj.insert("enabled".into(), JsonValue::Bool(true));
        }
        out.insert("brew".into(), brew_json);
    } else {
        let mut brew = Map::new();
        brew.insert("enabled".into(), JsonValue::Bool(false));
        brew.insert("host".into(), JsonValue::String(String::new()));
        brew.insert("port".into(), JsonValue::Number(3003.into()));
        brew.insert("tls".into(), JsonValue::Bool(true));
        brew.insert("username".into(), JsonValue::Number(0.into()));
        brew.insert("password".into(), JsonValue::String(String::new()));
        brew.insert("password_set".into(), JsonValue::Bool(false));
        brew.insert("reconnect_delay_secs".into(), JsonValue::Number(15.into()));
        brew.insert("feature_sds_enabled".into(), JsonValue::Bool(true));
        brew.insert("feature_rssi_export".into(), JsonValue::Bool(false));
        brew.insert("feature_lip_forward".into(), JsonValue::Bool(false));
        brew.insert("lip_forward_issi".into(), JsonValue::Null);
        out.insert("brew".into(), JsonValue::Object(brew));
    }
    out.insert(
        "active".into(),
        serde_json::to_value(read_active(config_path)).unwrap_or(JsonValue::Null),
    );
    Ok(JsonValue::Object(out))
}

/// Optional Soapy keys the visual Config form owns. Absent in a Save/Apply payload
/// means “use device defaults” — they must be removed from TOML, not left behind by
/// deep_merge (which only inserts/overwrites).
const SOAPY_VISUAL_OPTIONAL: &[&str] = &[
    "rx_gain_lna",
    "rx_gain_pga",
    "rx_gain_tia",
    "tx_gain_pad",
    "tx_gain_dac",
    "tx_gain_mixer",
    "tx_gain_iamp",
    "tx_gain_pga",
    "rx_antenna",
    "tx_antenna",
    "sample_rate",
    "tx_center_freq",
    "rx_center_freq",
];

/// After merging `phy_io` from the visual form / Cell profile, drop optional Soapy
/// keys that were intentionally cleared in the UI.
fn prune_soapysdr_visual_optional(table: &mut toml::Table, incoming_phy: &JsonValue) {
    let Some(incoming_soapy) = incoming_phy.get("soapysdr").and_then(|v| v.as_object()) else {
        return;
    };
    let Some(TomlValue::Table(phy)) = table.get_mut("phy_io") else {
        return;
    };
    let Some(TomlValue::Table(soapy)) = phy.get_mut("soapysdr") else {
        return;
    };
    for key in SOAPY_VISUAL_OPTIONAL {
        if !incoming_soapy.contains_key(*key) {
            soapy.remove(*key);
        }
    }
}

/// Optional `cell_info` keys the visual form owns (empty field = omit from JSON).
/// Timers: absent → engine defaults (hangtime 5, call_timeout 120, ul 3, T351 3600).
const CELL_VISUAL_OPTIONAL: &[&str] = &[
    "custom_duplex_spacing",
    "timezone",
    "hangtime_secs",
    "call_timeout_secs",
    "ul_inactivity_secs",
    "periodic_registration_secs",
    "secondary_carrier",
    "dual_carrier_enabled",
    "rxlev_access_min_dbm",
];

fn prune_cell_info_visual_optional(table: &mut toml::Table, incoming_cell: &JsonValue) {
    let Some(incoming) = incoming_cell.as_object() else {
        return;
    };
    let Some(TomlValue::Table(cell)) = table.get_mut("cell_info") else {
        return;
    };
    for key in CELL_VISUAL_OPTIONAL {
        if !incoming.contains_key(*key) {
            cell.remove(*key);
        }
    }
}

/// Merge a visual-config POST body into the live TOML and validate.
pub fn write_visual_config(config_path: &str, body: &JsonValue) -> Result<(), String> {
    let obj = body
        .as_object()
        .ok_or_else(|| "body must be a JSON object".to_string())?;
    let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;

    for key in ["phy_io", "net_info", "cell_info"] {
        if let Some(v) = obj.get(key) {
            let mut cleaned = v.clone();
            if key == "cell_info" {
                strip_sds_command_control_json(&mut cleaned);
            }
            deep_merge_toml(&mut table, key, json_to_toml(&cleaned)?);
            if key == "phy_io" {
                prune_soapysdr_visual_optional(&mut table, v);
            }
            if key == "cell_info" {
                prune_cell_info_visual_optional(&mut table, &cleaned);
            }
        }
    }
    // Optional: Save live may also write whitelist when the form includes security.
    if let Some(v) = obj.get("security") {
        deep_merge_toml(&mut table, "security", json_to_toml(v)?);
    }
    // Optional: proactive restart recovery (Cell-bound; default off).
    if let Some(v) = obj.get("recovery") {
        deep_merge_toml(&mut table, "recovery", json_to_toml(v)?);
    }

    if let Some(brew) = obj.get("brew") {
        let brew_obj = brew
            .as_object()
            .ok_or_else(|| "brew must be an object".to_string())?;
        let enabled = brew_obj.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        if enabled {
            let mut brew_toml = json_to_toml(brew)?.as_table().cloned().unwrap_or_default();
            brew_toml.remove("enabled");
            brew_toml.remove("password_set");
            // Empty LIP dest in the form omits the key — drop any prior TOML value.
            if !brew_obj.contains_key("lip_forward_issi") {
                brew_toml.remove("lip_forward_issi");
            }
            rehydrate_brew_password(&mut brew_toml, table.get("brew"));
            table.insert("brew".into(), TomlValue::Table(brew_toml));
        } else {
            table.remove("brew");
        }
    }

    ensure_dual_carrier_rf_keys(&mut table)?;

    let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    validate_toml_str(&rendered)?;
    backup_and_write(config_path, &rendered)
}

/// When Dual Carrier is on, ensure `sample_rate` + midway TX/RX centers exist so validate()
/// can prove the secondary fits the passband (Fs defaults to 600 kHz like SXceiver).
fn ensure_dual_carrier_rf_keys(table: &mut toml::Table) -> Result<(), String> {
    let Some(TomlValue::Table(cell)) = table.get("cell_info") else {
        return Ok(());
    };
    let dual_on = cell
        .get("dual_carrier_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let secondary = cell.get("secondary_carrier").and_then(|v| v.as_integer()).map(|n| n as u16);
    let main = cell.get("main_carrier").and_then(|v| v.as_integer()).map(|n| n as u16);
    if !dual_on || secondary.is_none() || main.is_none() {
        return Ok(());
    }
    let secondary = secondary.unwrap();
    let main = main.unwrap();

    let rendered = toml::to_string(table).map_err(|e| e.to_string())?;
    let cfg = tetra_config::bluestation::parsing::from_toml_str(&rendered).map_err(|e| e.to_string())?;
    // Temporarily force secondary into cfg view: cell_dto already has it if dual_on.
    let carriers = cfg.bs_phase_mod_carriers().map_err(|e| e)?;
    let (main_dl, main_ul) = carriers
        .iter()
        .find(|(n, _, _)| *n == main)
        .map(|(_, d, u)| (*d as f64, *u as f64))
        .ok_or_else(|| "main carrier missing from frequency table".to_string())?;
    let (sec_dl, sec_ul) = carriers
        .iter()
        .find(|(n, _, _)| *n == secondary)
        .map(|(_, d, u)| (*d as f64, *u as f64))
        .ok_or_else(|| "secondary carrier missing from frequency table".to_string())?;

    let phy = table
        .entry("phy_io".to_string())
        .or_insert_with(|| TomlValue::Table(toml::Table::new()));
    let TomlValue::Table(phy_t) = phy else {
        return Err("phy_io must be a table".into());
    };
    phy_t
        .entry("backend".to_string())
        .or_insert_with(|| TomlValue::String("SoapySdr".into()));
    let soapy = phy_t
        .entry("soapysdr".to_string())
        .or_insert_with(|| TomlValue::Table(toml::Table::new()));
    let TomlValue::Table(soapy_t) = soapy else {
        return Err("soapysdr must be a table".into());
    };
    if !soapy_t.contains_key("sample_rate") {
        soapy_t.insert("sample_rate".into(), TomlValue::Float(600_000.0));
    }
    soapy_t.insert(
        "tx_center_freq".into(),
        TomlValue::Float((main_dl + sec_dl) / 2.0),
    );
    soapy_t.insert(
        "rx_center_freq".into(),
        TomlValue::Float((main_ul + sec_ul) / 2.0),
    );
    Ok(())
}

/// Apply named Cell × Brew profiles into the live config.toml.
///
/// `brew_name`:
/// - `None` → Offline (remove `[brew]` and `[lst_dispatch]`)
/// - `Some("__lst_dispatch__")` → LST Dispatch (remove `[brew]`, enable `[lst_dispatch]`)
/// - `Some(name)` → named Brew profile (remove `[lst_dispatch]`, set `[brew]`)
pub fn apply_profiles(config_path: &str, cell_name: &str, brew_name: Option<&str>) -> Result<(), String> {
    let cell = get_cell_profile(config_path, cell_name)?;
    let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;

    // With [networks] the core links live under the networks, not in a root [brew]; a Brew
    // profile here would be refused by the config loader, so say so before writing anything.
    if table.contains_key("networks") && brew_name.is_some() {
        return Err("this station uses [networks]: edit the network's brew link in the configuration instead of applying a Brew profile".into());
    }

    let cell_obj = cell
        .as_object()
        .ok_or_else(|| "cell profile is not an object".to_string())?;

    // Preserve live SDS command control — station-wide, not part of Cell profiles.
    let preserved_sds_cmd = table
        .get("cell_info")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("sds_command_control"))
        .cloned();

    // Deep-merge so advanced cell keys (neighbors, HMD, …) survive a partial Cell profile.
    // SDS command control is stripped from the profile payload and restored after merge.
    for key in ["phy_io", "net_info", "cell_info"] {
        if let Some(v) = cell_obj.get(key) {
            let mut cleaned = v.clone();
            if key == "cell_info" {
                strip_sds_command_control_json(&mut cleaned);
            }
            deep_merge_toml(&mut table, key, json_to_toml(&cleaned)?);
            if key == "phy_io" {
                prune_soapysdr_visual_optional(&mut table, v);
            }
            if key == "cell_info" {
                prune_cell_info_visual_optional(&mut table, &cleaned);
            }
        }
    }
    // Access control is part of the Cell profile. Missing key = open network for this
    // Cell (empty whitelist), so Apply switches access when changing profiles.
    let security_val = if let Some(v) = cell_obj.get("security") {
        v.clone()
    } else {
        let mut sec = Map::new();
        sec.insert("issi_whitelist".into(), JsonValue::Array(vec![]));
        JsonValue::Object(sec)
    };
    deep_merge_toml(&mut table, "security", json_to_toml(&security_val)?);
    // Restart recovery is part of the Cell profile. Missing key = proactive OFF for this Cell.
    let recovery_val = if let Some(v) = cell_obj.get("recovery") {
        v.clone()
    } else {
        let mut rec = Map::new();
        rec.insert("enabled".into(), JsonValue::Bool(false));
        JsonValue::Object(rec)
    };
    deep_merge_toml(&mut table, "recovery", json_to_toml(&recovery_val)?);
    if let Some(v) = cell_obj.get("stack_mode") {
        table.insert("stack_mode".into(), json_to_toml(v)?);
    }
    // Always keep schema version required by current parser.
    table.insert(
        "config_version".into(),
        TomlValue::String(
            cell_obj
                .get("config_version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.6")
                .to_string(),
        ),
    );

    if let Some(sds) = preserved_sds_cmd {
        let cell_tbl = table
            .entry("cell_info".to_string())
            .or_insert_with(|| TomlValue::Table(toml::Table::new()));
        if let TomlValue::Table(t) = cell_tbl {
            t.insert("sds_command_control".into(), sds);
        }
    } else if let Some(TomlValue::Table(t)) = table.get_mut("cell_info") {
        t.remove("sds_command_control");
    }

    match brew_name {
        Some(name) if name == LST_DISPATCH_PROFILE => {
            table.remove("brew");
            let mut lst = toml::Table::new();
            // Preserve operator_issi / default_gssi if already present.
            let prev = table
                .get("lst_dispatch")
                .and_then(|v| v.as_table())
                .cloned()
                .unwrap_or_default();
            lst.insert("enabled".into(), TomlValue::Boolean(true));
            lst.insert(
                "operator_issi".into(),
                prev.get("operator_issi")
                    .cloned()
                    .unwrap_or(TomlValue::Integer(9_990_001)),
            );
            if let Some(g) = prev.get("default_gssi") {
                lst.insert("default_gssi".into(), g.clone());
            }
            table.insert("lst_dispatch".into(), TomlValue::Table(lst));
        }
        Some(name) => {
            let brew = get_brew_profile_raw(config_path, &sanitize_name(name)?)?;
            let mut brew_toml = json_to_toml(&brew)?.as_table().cloned().unwrap_or_default();
            brew_toml.remove("enabled");
            brew_toml.remove("password_set");
            table.insert("brew".into(), TomlValue::Table(brew_toml));
            table.remove("lst_dispatch");
        }
        None => {
            table.remove("brew");
            table.remove("lst_dispatch");
        }
    }

    ensure_dual_carrier_rf_keys(&mut table)?;

    let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    validate_toml_str(&rendered)?;
    backup_and_write(config_path, &rendered)?;
    write_active(
        config_path,
        &ActiveProfiles {
            cell: sanitize_name(cell_name)?,
            brew: match brew_name {
                Some(name) if name == LST_DISPATCH_PROFILE => Some(LST_DISPATCH_PROFILE.to_string()),
                Some(name) => Some(sanitize_name(name)?),
                None => None,
            },
        },
    )?;
    Ok(())
}

/// Capture current visual form sections into a new Cell profile.
pub fn save_cell_from_visual(config_path: &str, name: &str, visual: &JsonValue) -> Result<(), String> {
    let obj = visual
        .as_object()
        .ok_or_else(|| "body must be a JSON object".to_string())?;
    let mut cell = Map::new();
    for key in ["config_version", "stack_mode", "phy_io", "net_info", "cell_info"] {
        if let Some(v) = obj.get(key) {
            let mut cleaned = v.clone();
            if key == "cell_info" {
                strip_sds_command_control_json(&mut cleaned);
            }
            cell.insert(key.to_string(), cleaned);
        }
    }
    // Always persist Access Control from the visual form (empty = open for this Cell).
    if let Some(v) = obj.get("security") {
        cell.insert("security".to_string(), v.clone());
    } else {
        let mut sec = Map::new();
        sec.insert("issi_whitelist".into(), JsonValue::Array(vec![]));
        cell.insert("security".into(), JsonValue::Object(sec));
    }
    // Always persist restart recovery from the visual form (default off).
    if let Some(v) = obj.get("recovery") {
        cell.insert("recovery".to_string(), v.clone());
    } else {
        let mut rec = Map::new();
        rec.insert("enabled".into(), JsonValue::Bool(false));
        cell.insert("recovery".into(), JsonValue::Object(rec));
    }
    put_cell_profile(config_path, name, &JsonValue::Object(cell))
}

fn strip_sds_command_control_json(v: &mut JsonValue) {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("sds_command_control");
    }
}

/// Read `security.issi_whitelist` from a Cell profile JSON. `None` = key absent (legacy).
pub fn extract_issi_whitelist(cell: &JsonValue) -> Option<Vec<u32>> {
    let sec = cell.get("security")?;
    let arr = sec.get("issi_whitelist")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect(),
    )
}

/// Patch only the Cell profile's ISSI whitelist. Returns `true` if that Cell is the active one.
pub fn set_cell_whitelist(config_path: &str, name: &str, list: &[u32]) -> Result<bool, String> {
    let name = sanitize_name(name)?;
    let path = profile_file(&cell_dir(config_path), &name);
    let txt = fs::read_to_string(&path).map_err(|e| format!("cell profile: {e}"))?;
    let mut v: JsonValue = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| "cell profile is not an object".to_string())?;
    let mut sec = Map::new();
    sec.insert(
        "issi_whitelist".into(),
        JsonValue::Array(
            list.iter()
                .map(|i| JsonValue::Number((*i).into()))
                .collect(),
        ),
    );
    obj.insert("security".into(), JsonValue::Object(sec));
    save_json_profile(&cell_dir(config_path), &name, &v)?;
    Ok(read_active(config_path).cell == name)
}

/// Capture brew form into a Brew profile.
pub fn save_brew_from_visual(config_path: &str, name: &str, visual: &JsonValue) -> Result<(), String> {
    let brew = visual
        .get("brew")
        .cloned()
        .ok_or_else(|| "missing brew object".to_string())?;
    put_brew_profile(config_path, name, &brew)
}

fn rehydrate_brew_password(brew_toml: &mut toml::Table, existing: Option<&TomlValue>) {
    let incoming = brew_toml.get("password").and_then(|v| v.as_str()).unwrap_or("");
    let masked = incoming == MASKED_PASSWORD || incoming.chars().all(|c| c == '•');
    if masked {
        if let Some(TomlValue::Table(prev)) = existing {
            if let Some(TomlValue::String(real)) = prev.get("password") {
                brew_toml.insert("password".into(), TomlValue::String(real.clone()));
            }
        }
    }
}

fn validate_toml_str(toml_str: &str) -> Result<(), String> {
    match tetra_config::bluestation::parsing::from_toml_str(toml_str) {
        Ok(cfg) => {
            cfg.validate()
                .map_err(|e| format!("config is invalid: {e}"))?;
            if let Some(soapy) = cfg.phy_io.soapysdr.as_ref() {
                tetra_config::bluestation::validate_soapysdr_driver_keys(soapy)?;
            }
            Ok(())
        }
        Err(e) => Err(format!("config does not parse: {e}")),
    }
}

fn backup_and_write(config_path: &str, body: &str) -> Result<(), String> {
    let backup = format!("{config_path}.bak");
    let _ = fs::copy(config_path, &backup);
    let tmp = format!("{config_path}.tmp");
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, config_path).map_err(|e| e.to_string())
}

/// Validate a full TOML document then atomically replace `config_path` (with `.bak`).
pub fn validate_and_write_toml(config_path: &str, body: &str) -> Result<(), String> {
    validate_toml_str(body)?;
    backup_and_write(config_path, body)
}

fn deep_merge_toml(table: &mut toml::Table, key: &str, incoming: TomlValue) {
    match (table.get_mut(key), incoming) {
        (Some(TomlValue::Table(dst)), TomlValue::Table(src)) => {
            for (k, v) in src {
                if let (Some(TomlValue::Table(d)), TomlValue::Table(s)) = (dst.get_mut(&k), v.clone()) {
                    for (ik, iv) in s {
                        d.insert(ik, iv);
                    }
                } else {
                    dst.insert(k, v);
                }
            }
        }
        (_, incoming) => {
            table.insert(key.to_string(), incoming);
        }
    }
}

fn toml_to_json(v: &TomlValue) -> JsonValue {
    match v {
        TomlValue::String(s) => JsonValue::String(s.clone()),
        TomlValue::Integer(i) => JsonValue::Number((*i).into()),
        TomlValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        TomlValue::Boolean(b) => JsonValue::Bool(*b),
        TomlValue::Datetime(d) => JsonValue::String(d.to_string()),
        TomlValue::Array(a) => JsonValue::Array(a.iter().map(toml_to_json).collect()),
        TomlValue::Table(t) => {
            let mut m = Map::new();
            for (k, v) in t {
                m.insert(k.clone(), toml_to_json(v));
            }
            JsonValue::Object(m)
        }
    }
}

fn json_to_toml(v: &JsonValue) -> Result<TomlValue, String> {
    match v {
        JsonValue::Null => Err("null is not valid in TOML config values".into()),
        JsonValue::Bool(b) => Ok(TomlValue::Boolean(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(TomlValue::Integer(i))
            } else if let Some(u) = n.as_u64() {
                Ok(TomlValue::Integer(u as i64))
            } else if let Some(f) = n.as_f64() {
                Ok(TomlValue::Float(f))
            } else {
                Err("unsupported number".into())
            }
        }
        JsonValue::String(s) => Ok(TomlValue::String(s.clone())),
        JsonValue::Array(a) => Ok(TomlValue::Array(
            a.iter().map(json_to_toml).collect::<Result<Vec<_>, _>>()?,
        )),
        JsonValue::Object(o) => {
            let mut t = toml::Table::new();
            for (k, v) in o {
                // Skip UI helper flags that must not land in TOML.
                if k == "password_set" || k == "enabled" {
                    continue;
                }
                // Optional form fields (e.g. lip_forward_issi) may be JSON null — omit them.
                if v.is_null() {
                    continue;
                }
                t.insert(k.clone(), json_to_toml(v)?);
            }
            Ok(TomlValue::Table(t))
        }
    }
}

/// Band-4 / non-reverse helper used by the dashboard Auto button (and unit-tested here).
pub fn auto_main_carrier_rx(
    tx_freq_hz: f64,
    duplex_hz: f64,
    freq_offset_hz: f64,
    freq_band: u8,
    reverse_operation: bool,
) -> Result<(u16, f64), String> {
    if freq_band != 4 {
        return Err("Auto currently supports freq_band = 4".into());
    }
    if reverse_operation {
        return Err("Auto currently supports reverse_operation = false".into());
    }
    let main = (tx_freq_hz - 400_000_000.0 - freq_offset_hz) / 25_000.0;
    if (main - main.round()).abs() > 1e-9 || main < 0.0 || main > u16::MAX as f64 {
        return Err("main_carrier is not an integer with the current values".into());
    }
    let rx = tx_freq_hz - duplex_hz;
    Ok((main.round() as u16, rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> String {
        r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "SoapySdr"

[phy_io.soapysdr]
tx_freq = 438025000.0
rx_freq = 433025000.0

[net_info]
mcc = 204
mnc = 1337

[cell_info]
freq_band = 4
main_carrier = 1521
duplex_spacing = 4
freq_offset = 0
reverse_operation = false
location_area = 2
colour_code = 1
system_wide_services = true
late_entry_supported = true
voice_service = true

[brew]
host = "core.example"
port = 3003
tls = true
username = 12345
password = "secret"
"#
        .to_string()
    }

    #[test]
    fn seed_split_cell_and_brew() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        fs::write(&cfg, minimal_toml()).unwrap();
        ensure_seeded(cfg.to_str().unwrap()).unwrap();
        let cells = list_cell_profiles(cfg.to_str().unwrap()).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].name, "Default");
        let brews = list_brew_profiles(cfg.to_str().unwrap()).unwrap();
        assert_eq!(brews.len(), 1);
        let active = read_active(cfg.to_str().unwrap());
        assert_eq!(active.cell, "Default");
        assert!(active.brew.is_some());
    }

    #[test]
    fn apply_offline_removes_brew() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        fs::write(&cfg, minimal_toml()).unwrap();
        ensure_seeded(cfg.to_str().unwrap()).unwrap();
        apply_profiles(cfg.to_str().unwrap(), "Default", None).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(!txt.contains("[brew]"), "brew section should be gone: {txt}");
        assert!(txt.contains("main_carrier"));
        let active = read_active(cfg.to_str().unwrap());
        assert!(active.brew.is_none());
    }

    #[test]
    fn write_visual_clears_omitted_tx_gains() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        let mut toml = minimal_toml();
        toml = toml.replace(
            "rx_freq = 433025000.0\n",
            "rx_freq = 433025000.0\ntx_gain_pad = 24.0\ntx_gain_dac = 24.0\n",
        );
        fs::write(&cfg, &toml).unwrap();
        let visual = serde_json::json!({
            "phy_io": {
                "backend": "SoapySdr",
                "soapysdr": {
                    "tx_freq": 438_025_000.0,
                    "rx_freq": 433_025_000.0,
                    "ppm_err": 0,
                    "device": "driver=sx"
                }
            },
            "net_info": { "mcc": 204, "mnc": 1337 },
            "cell_info": {
                "freq_band": 4,
                "main_carrier": 1521,
                "duplex_spacing": 4,
                "freq_offset": 0,
                "reverse_operation": false,
                "colour_code": 1,
                "location_area": 2,
                "system_wide_services": true,
                "late_entry_supported": true,
                "voice_service": true,
                "local_ssi_ranges": [[0, 90]]
            }
        });
        write_visual_config(cfg.to_str().unwrap(), &visual).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(
            !txt.contains("tx_gain_pad") && !txt.contains("tx_gain_dac"),
            "cleared gains must leave TOML: {txt}"
        );
        assert!(txt.contains("driver=sx"));
    }

    #[test]
    fn apply_profile_clears_omitted_tx_gains() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        let mut toml = minimal_toml();
        toml = toml.replace(
            "rx_freq = 433025000.0\n",
            "rx_freq = 433025000.0\ntx_gain_pad = 24.0\n",
        );
        fs::write(&cfg, &toml).unwrap();
        ensure_seeded(cfg.to_str().unwrap()).unwrap();
        // Default seed comes from live TOML (with pad). Re-save Cell without gains.
        let mut cell = get_cell_profile(cfg.to_str().unwrap(), "Default").unwrap();
        if let Some(soapy) = cell
            .pointer_mut("/phy_io/soapysdr")
            .and_then(|v| v.as_object_mut())
        {
            soapy.remove("tx_gain_pad");
            soapy.remove("tx_gain_dac");
        }
        put_cell_profile(cfg.to_str().unwrap(), "Default", &cell).unwrap();
        apply_profiles(cfg.to_str().unwrap(), "Default", None).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(
            !txt.contains("tx_gain_pad"),
            "apply must drop omitted pad: {txt}"
        );
    }

    #[test]
    fn write_visual_sets_and_clears_rxlev_access_min_and_migration() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        fs::write(&cfg, minimal_toml()).unwrap();
        let visual = |cell_extra: serde_json::Value| {
            let mut cell = serde_json::json!({
                "freq_band": 4,
                "main_carrier": 1521,
                "duplex_spacing": 4,
                "freq_offset": 0,
                "reverse_operation": false,
                "colour_code": 1,
                "location_area": 2,
                "system_wide_services": true,
                "late_entry_supported": true,
                "voice_service": true
            });
            for (k, v) in cell_extra.as_object().unwrap() {
                cell[k] = v.clone();
            }
            serde_json::json!({
                "phy_io": {
                    "backend": "SoapySdr",
                    "soapysdr": { "tx_freq": 438_025_000.0, "rx_freq": 433_025_000.0, "ppm_err": 0 }
                },
                "net_info": { "mcc": 204, "mnc": 1337 },
                "cell_info": cell
            })
        };
        write_visual_config(cfg.to_str().unwrap(), &visual(serde_json::json!({ "rxlev_access_min_dbm": -100, "migration": true }))).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(txt.contains("rxlev_access_min_dbm = -100"), "rxlev must be written: {txt}");
        assert!(txt.contains("migration = true"), "migration must be written: {txt}");

        // Default selected (key omitted) and migration unticked: back to -110 / off.
        write_visual_config(cfg.to_str().unwrap(), &visual(serde_json::json!({ "migration": false }))).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(!txt.contains("rxlev_access_min_dbm"), "omitted rxlev must be pruned: {txt}");
        assert!(txt.contains("migration = false"), "migration off must be written: {txt}");
    }

    #[test]
    fn write_visual_clears_omitted_timers_keeps_explicit_zero() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        let mut toml = minimal_toml();
        toml = toml.replace(
            "voice_service = true\n",
            "voice_service = true\nhangtime_secs = 2\ncall_timeout_secs = 90\nul_inactivity_secs = 3\nperiodic_registration_secs = 1800\n",
        );
        fs::write(&cfg, &toml).unwrap();
        let visual = serde_json::json!({
            "phy_io": {
                "backend": "SoapySdr",
                "soapysdr": {
                    "tx_freq": 438_025_000.0,
                    "rx_freq": 433_025_000.0,
                    "ppm_err": 0
                }
            },
            "net_info": { "mcc": 204, "mnc": 1337 },
            "cell_info": {
                "freq_band": 4,
                "main_carrier": 1521,
                "duplex_spacing": 4,
                "freq_offset": 0,
                "reverse_operation": false,
                "colour_code": 1,
                "location_area": 2,
                "system_wide_services": true,
                "late_entry_supported": true,
                "voice_service": true,
                "local_ssi_ranges": [[0, 90]],
                // hangtime / ul / t351 omitted → prune to engine defaults
                "call_timeout_secs": 0 // explicit 0 = unlimited
            }
        });
        write_visual_config(cfg.to_str().unwrap(), &visual).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(
            !txt.contains("hangtime_secs"),
            "omitted hangtime must be pruned: {txt}"
        );
        assert!(
            !txt.contains("ul_inactivity_secs"),
            "omitted ul inactivity must be pruned: {txt}"
        );
        assert!(
            !txt.contains("periodic_registration_secs"),
            "omitted T351 must be pruned: {txt}"
        );
        assert!(
            txt.contains("call_timeout_secs = 0"),
            "explicit 0 call timeout must remain: {txt}"
        );
    }

    #[test]
    fn apply_profile_clears_omitted_timers() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        let mut toml = minimal_toml();
        toml = toml.replace(
            "voice_service = true\n",
            "voice_service = true\nhangtime_secs = 2\nul_inactivity_secs = 8\n",
        );
        fs::write(&cfg, &toml).unwrap();
        ensure_seeded(cfg.to_str().unwrap()).unwrap();
        let mut cell = get_cell_profile(cfg.to_str().unwrap(), "Default").unwrap();
        if let Some(ci) = cell.get_mut("cell_info").and_then(|v| v.as_object_mut()) {
            ci.remove("hangtime_secs");
            ci.remove("ul_inactivity_secs");
            ci.remove("call_timeout_secs");
            ci.remove("periodic_registration_secs");
        }
        put_cell_profile(cfg.to_str().unwrap(), "Default", &cell).unwrap();
        apply_profiles(cfg.to_str().unwrap(), "Default", None).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(
            !txt.contains("hangtime_secs") && !txt.contains("ul_inactivity_secs"),
            "apply must drop omitted timers so engine defaults apply: {txt}"
        );
    }

    #[test]
    fn auto_helper_band4() {
        let (carrier, rx) = auto_main_carrier_rx(438_025_000.0, 5_000_000.0, 0.0, 4, false).unwrap();
        assert_eq!(carrier, 1521);
        assert_eq!(rx, 433_025_000.0);
    }

    fn tempfile_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("fs-profiles-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
