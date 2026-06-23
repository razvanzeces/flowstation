//! Dashboard-side persistence helpers for GeoAlarm settings.

use tetra_config::bluestation::GeoalarmRuntimeOverride;

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn u32_set_toml(values: &std::collections::BTreeSet<u32>) -> String {
    values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
}

fn string_set_toml(values: &std::collections::BTreeSet<String>) -> String {
    values
        .iter()
        .map(|v| format!("\"{}\"", toml_escape(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rewrite (or insert) the `[geoalarm]` section in the TOML file. A `.geoalarm.bak` backup is made.
pub fn write_geoalarm_to_toml(config_path: &str, ov: &GeoalarmRuntimeOverride) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let section = format!(
        "[geoalarm]\n\
         enabled = {}\n\
         flowstation_lat = {:.8}\n\
         flowstation_lon = {:.8}\n\
         radius_m = {:.1}\n\
         cooldown_secs = {}\n\n\
         trigger_tetra = {}\n\
         trigger_meshcom = {}\n\n\
         forward_tpg2200 = {}\n\
         forward_sds = {}\n\
         forward_sip = {}\n\
         forward_telegram = {}\n\n\
         tetra_issi_whitelist = [{}]\n\
         tetra_issi_blacklist = [{}]\n\
         meshcom_source_whitelist = [{}]\n\
         meshcom_source_blacklist = [{}]\n\n\
         sds_source_issi = {}\n\
         sds_dest_issi = {}\n\
         sds_dest_is_group = {}\n\n\
         tpg2200_source_issi = {}\n\
         tpg2200_dest_issi = {}\n\
         tpg2200_incident_base = {}\n\
         tpg2200_text_prefix = \"{}\"\n\
         tpg2200_max_text_chars = {}\n\n\
         sip_title_prefix = \"{}\"\n\
         telegram_prefix = \"{}\"",
        ov.enabled,
        ov.flowstation_lat,
        ov.flowstation_lon,
        ov.radius_m.max(1.0),
        ov.cooldown_secs.clamp(1, 86_400),
        ov.trigger_tetra,
        ov.trigger_meshcom,
        ov.forward_tpg2200,
        ov.forward_sds,
        ov.forward_sip,
        ov.forward_telegram,
        u32_set_toml(&ov.tetra_issi_whitelist),
        u32_set_toml(&ov.tetra_issi_blacklist),
        string_set_toml(&ov.meshcom_source_whitelist),
        string_set_toml(&ov.meshcom_source_blacklist),
        ov.sds_source_issi.max(1),
        ov.sds_dest_issi,
        ov.sds_dest_is_group,
        ov.tpg2200_source_issi.max(1),
        ov.tpg2200_dest_issi,
        ov.tpg2200_incident_base.clamp(1, 256),
        toml_escape(&ov.tpg2200_text_prefix),
        ov.tpg2200_max_text_chars.clamp(8, 160),
        toml_escape(&ov.sip_title_prefix),
        toml_escape(&ov.telegram_prefix),
    );

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 32);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("[geoalarm]") {
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

    let backup = format!("{config_path}.geoalarm.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}
