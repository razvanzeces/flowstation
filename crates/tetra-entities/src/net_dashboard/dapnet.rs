//! Dashboard-side persistence and helpers for DAPNET settings.
//!
//! Mirrors the Telegram/WX dashboard helpers: rewrite only the `[dapnet]` section while
//! preserving the rest of the active config file, and mask secrets before returning them to UI.

use tetra_config::bluestation::DapnetRuntimeOverride;

/// Mask a secret for display. Returns an empty string for an empty value.
pub fn mask_secret(secret: &str) -> String {
    let secret = secret.trim();
    if secret.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 10 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn ric_set_toml(rics: &std::collections::BTreeSet<u32>) -> String {
    rics.iter()
        .map(|ric| format!("\"{}\"", tetra_config::bluestation::format_ric_route_key(*ric)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ric_routes_toml(routes: &std::collections::BTreeMap<u32, u32>) -> String {
    routes
        .iter()
        .map(|(ric, ssi)| format!("\"{}\" = {}", tetra_config::bluestation::format_ric_route_key(*ric), ssi))
        .collect::<Vec<_>>()
        .join(", ")
}

fn issi_priority_routes_toml(routes: &std::collections::BTreeMap<u32, u8>) -> String {
    routes
        .iter()
        .map(|(issi, priority)| format!("\"{}\" = {}", issi, priority))
        .collect::<Vec<_>>()
        .join(", ")
}

fn tpg_ric_priority_routes_toml(routes: &std::collections::BTreeMap<u32, u8>) -> String {
    routes
        .iter()
        .map(|(ric, priority)| format!("\"0x{ric:08X}\" = {}", priority))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rewrite (or insert) the `[dapnet]` section in the TOML file. A `.dapnet.bak` backup is made.
pub fn write_dapnet_to_toml(config_path: &str, ov: &DapnetRuntimeOverride) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let ric_issi_routes = ric_routes_toml(&ov.ric_issi_routes);
    let ric_gssi_routes = ric_routes_toml(&ov.ric_gssi_routes);
    let sds_allowed_rics = ric_set_toml(&ov.sds_allowed_rics);
    let callout_allowed_rics = ric_set_toml(&ov.callout_allowed_rics);
    let telegram_allowed_rics = ric_set_toml(&ov.telegram_allowed_rics);
    let callout_issi_priorities = issi_priority_routes_toml(&ov.callout_issi_priorities);
    let callout_tpg_ric_priorities = tpg_ric_priority_routes_toml(&ov.callout_tpg_ric_priorities);
    let section = format!(
        "[dapnet]\n\
         enabled = {}\n\
         api_url = \"{}\"\n\
         username = \"{}\"\n\
         password = \"{}\"\n\
         poll_interval_secs = {}\n\n\
         forward_sds = {}\n\
         forward_callout = {}\n\
         forward_telegram = {}\n\n\
         sds_source_issi = {}\n\
         sds_dest_issi = {}\n\
         sds_dest_is_group = {}\n\
         ric_issi_routes = {{{}}}\n\
         ric_gssi_routes = {{{}}}\n\
         sds_allowed_rics = [{}]\n\
         callout_allowed_rics = [{}]\n\
         telegram_allowed_rics = [{}]\n\n\
         callout_source_issi = {}\n\
         callout_dest_issi = {}\n\
         callout_tpg_ric = {}\n\
         callout_id_base = {}\n\
         callout_priority = {}\n\
         callout_issi_priorities = {{{}}}\n\
         callout_tpg_ric_priorities = {{{}}}\n\
         callout_text_prefix = \"{}\"\n\n\
         telegram_prefix = \"{}\"\n\n\
         rwth_core_enabled = {}\n\
         rwth_core_host = \"{}\"\n\
         rwth_core_port = {}\n\
         rwth_core_device = \"{}\"\n\
         rwth_core_version = \"{}\"\n\
         rwth_core_callsign = \"{}\"\n\
         rwth_core_authkey = \"{}\"\n\
         rwth_messages_limit = {}",
        ov.enabled,
        toml_escape(&ov.api_url),
        toml_escape(&ov.username),
        toml_escape(&ov.password),
        ov.poll_interval_secs.max(1),
        ov.forward_sds,
        ov.forward_callout,
        ov.forward_telegram,
        ov.sds_source_issi,
        ov.sds_dest_issi,
        ov.sds_dest_is_group,
        ric_issi_routes,
        ric_gssi_routes,
        sds_allowed_rics,
        callout_allowed_rics,
        telegram_allowed_rics,
        ov.callout_source_issi,
        ov.callout_dest_issi,
        ov.callout_tpg_ric,
        ov.callout_incident_base.min(255),
        ov.callout_priority.min(15),
        callout_issi_priorities,
        callout_tpg_ric_priorities,
        toml_escape(&ov.callout_text_prefix),
        toml_escape(&ov.telegram_prefix),
        ov.rwth_core_enabled,
        toml_escape(&ov.rwth_core_host),
        ov.rwth_core_port,
        toml_escape(&ov.rwth_core_device),
        toml_escape(&ov.rwth_core_version),
        toml_escape(&ov.rwth_core_callsign),
        toml_escape(&ov.rwth_core_authkey),
        ov.rwth_messages_limit.max(1),
    );

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 32);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("[dapnet]") {
            out.push(section.clone());
            replaced = true;
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim_start();
                if t.starts_with('[') && t.contains(']') {
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }

    if !replaced {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push(section);
    }

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }

    let backup = format!("{config_path}.dapnet.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}
