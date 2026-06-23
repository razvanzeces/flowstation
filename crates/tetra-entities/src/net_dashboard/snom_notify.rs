//! Dashboard-side persistence and helpers for Snom XML NOTIFY settings.

use tetra_config::bluestation::SnomNotifyRuntimeOverride;

pub fn mask_secret(secret: &str) -> String {
    crate::net_dashboard::dapnet::mask_secret(secret)
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn string_array_toml(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("\"{}\"", toml_escape(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ric_set_toml(rics: &std::collections::BTreeSet<u32>) -> String {
    rics.iter()
        .map(|ric| format!("\"{}\"", tetra_config::bluestation::format_ric_route_key(*ric)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn u32_set_toml(values: &std::collections::BTreeSet<u32>) -> String {
    values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
}

/// Rewrite (or insert) the `[snom_notify]` section in the TOML file. A `.snom.bak` backup is made.
pub fn write_snom_notify_to_toml(config_path: &str, ov: &SnomNotifyRuntimeOverride) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let section = format!(
        "[snom_notify]\n\
         enabled = {}\n\
         ami_host = \"{}\"\n\
         ami_port = {}\n\
         ami_username = \"{}\"\n\
         ami_password = \"{}\"\n\
         endpoints = [{}]\n\
         notify_sds = {}\n\
         notify_dapnet = {}\n\
         notify_telegram = {}\n\
         sds_directions = [{}]\n\
         dapnet_allowed_rics = [{}]\n\
         sds_allowed_issis = [{}]\n\
         title_prefix = \"{}\"\n\
         notify_event = \"{}\"\n\
         content_type = \"{}\"\n\
         subscription_state = \"{}\"\n\
         max_text_chars = {}\n\
         connect_timeout_secs = {}",
        ov.enabled,
        toml_escape(&ov.ami_host),
        ov.ami_port,
        toml_escape(&ov.ami_username),
        toml_escape(&ov.ami_password),
        string_array_toml(&ov.endpoints),
        ov.notify_sds,
        ov.notify_dapnet,
        ov.notify_telegram,
        string_array_toml(&ov.sds_directions),
        ric_set_toml(&ov.dapnet_allowed_rics),
        u32_set_toml(&ov.sds_allowed_issis),
        toml_escape(&ov.title_prefix),
        toml_escape(&ov.notify_event),
        toml_escape(&ov.content_type),
        toml_escape(&ov.subscription_state),
        ov.max_text_chars.clamp(40, 2000),
        ov.connect_timeout_secs.clamp(1, 30),
    );

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 24);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("[snom_notify]") {
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

    let backup = format!("{config_path}.snom.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}
