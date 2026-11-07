//! Built-in WX/METAR fetch + decode.
//!
//! Fetches a station's raw METAR from aviationweather.gov and decodes it into a compact,
//! ASCII-only, human-readable line suitable for a TETRA SDS (which is ISO-8859-1 / ASCII
//! and length-limited). Ported from the standalone tetraflow-sds-bot so the capability is
//! built into FlowStation — no separate process needed.
//!
//! Blocking HTTP; always call from a dedicated worker thread, never from the stack loop.

use std::time::Duration;

const METAR_API: &str = "https://aviationweather.gov/api/data/metar";
const USER_AGENT: &str = "FlowStation-WX";

/// Fetch the raw METAR string for an ICAO code (e.g. "LROP" -> "LROP 301600Z ...").
/// Returns Err(message) on network failure or when no data is returned.
pub fn fetch_metar_raw(icao: &str) -> Result<String, String> {
    let icao = sanitize_icao(icao);
    if icao.is_empty() {
        return Err("empty ICAO".to_string());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let url = format!("{METAR_API}?ids={icao}&format=raw&taf=false");
    let body = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("request failed: {e}"))?
        .text()
        .map_err(|e| format!("read failed: {e}"))?;
    let line = body
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| "no METAR data".to_string())?;
    Ok(line.trim().to_string())
}

/// Fetch and decode in one step. Output is ASCII-only and SDS-safe.
pub fn fetch_metar_decoded(icao: &str) -> Result<String, String> {
    let raw = fetch_metar_raw(icao)?;
    Ok(ascii_only(&decode_metar(&raw)))
}

/// Keep only the leading letters/digits of an ICAO token, uppercased, max 4 chars.
fn sanitize_icao(icao: &str) -> String {
    icao.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(4)
        .collect::<String>()
        .to_uppercase()
}

/// Strip non-ASCII so the result fits an ISO-8859-1 SDS text payload.
fn ascii_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii()).collect()
}

/// Decode a raw METAR into a compact human-readable summary, fields separated by " | ".
/// Best-effort: unrecognised tokens are skipped. Ported from tetraflow-sds-bot.
pub fn decode_metar(raw: &str) -> String {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if tokens.is_empty() {
        return raw.to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;

    // Station identifier (4 letters)
    if i < tokens.len() && tokens[i].len() == 4 && tokens[i].chars().all(|c| c.is_ascii_alphabetic()) {
        parts.push(tokens[i].to_string());
        i += 1;
    }

    // Time e.g. 190900Z
    if i < tokens.len() && tokens[i].ends_with('Z') && tokens[i].len() == 7 {
        let t = tokens[i];
        if let (Ok(hh), Ok(mm)) = (t[2..4].parse::<u32>(), t[4..6].parse::<u32>()) {
            parts.push(format!("{hh:02}:{mm:02}Z"));
        }
        i += 1;
    }

    // AUTO / COR
    if i < tokens.len() && (tokens[i] == "AUTO" || tokens[i] == "COR") {
        i += 1;
    }

    // Wind e.g. 11005KT, 11005G15KT, VRB05KT
    if i < tokens.len() {
        let w = tokens[i];
        if w.ends_with("KT") || w.ends_with("MPS") {
            let unit = if w.ends_with("KT") { "kt" } else { "m/s" };
            let body = w.trim_end_matches("KT").trim_end_matches("MPS");
            if body.starts_with("VRB") {
                if let Ok(spd) = body[3..].parse::<u32>() {
                    parts.push(format!("Wind variable {spd}{unit}"));
                }
            } else if body.len() >= 5 {
                let dir = &body[..3];
                let spd_part = body[3..].split('G').next().unwrap_or("0");
                if let (Ok(d), Ok(spd)) = (dir.parse::<u32>(), spd_part.parse::<u32>()) {
                    let gust_str = if body.contains('G') {
                        let g: u32 = body.split('G').nth(1).unwrap_or("0").parse().unwrap_or(0);
                        format!(" gust {g}{unit}")
                    } else {
                        String::new()
                    };
                    parts.push(format!("Wind {d} {spd}{unit}{gust_str}"));
                }
            }
            i += 1;

            // Variable direction e.g. 080V160
            if i < tokens.len() && tokens[i].contains('V') && tokens[i].len() == 7 {
                let sides: Vec<&str> = tokens[i].split('V').collect();
                if sides.len() == 2 {
                    parts.push(format!("var {}-{}", sides[0], sides[1]));
                }
                i += 1;
            }
        }
    }

    // Visibility
    if i < tokens.len() {
        let v = tokens[i];
        if v == "CAVOK" {
            parts.push("CAVOK".to_string());
            i += 1;
        } else if v == "9999" {
            parts.push("Vis >10km".to_string());
            i += 1;
        } else if let Ok(m) = v.parse::<u32>() {
            if m <= 9999 {
                if m >= 1000 {
                    parts.push(format!("Vis {}km", m / 1000));
                } else {
                    parts.push(format!("Vis {m}m"));
                }
                i += 1;
            }
        }
    }

    // Remaining tokens
    while i < tokens.len() {
        let t = tokens[i];

        // Cloud layers
        if t.len() >= 6 && matches!(&t[..3], "FEW" | "SCT" | "BKN" | "OVC") {
            let cov = match &t[..3] {
                "FEW" => "Few",
                "SCT" => "SCT",
                "BKN" => "BKN",
                "OVC" => "OVC",
                other => other,
            };
            if let Ok(hundreds) = t[3..6].parse::<u32>() {
                parts.push(format!("{cov} {}ft", hundreds * 100));
            }
            i += 1;
            continue;
        }

        // Temp/dewpoint e.g. 17/01 or M02/M08
        if t.contains('/') && !t.starts_with('Q') && !t.starts_with('R') {
            let sides: Vec<&str> = t.split('/').collect();
            if sides.len() == 2 {
                let parse_t = |s: &str| -> Option<i32> {
                    if let Some(stripped) = s.strip_prefix('M') {
                        stripped.parse::<i32>().ok().map(|v| -v)
                    } else {
                        s.parse::<i32>().ok()
                    }
                };
                if let (Some(temp), Some(dew)) = (parse_t(sides[0]), parse_t(sides[1])) {
                    parts.push(format!("{temp}C/{dew}C"));
                    i += 1;
                    continue;
                }
            }
        }

        // QNH
        if let Some(rest) = t.strip_prefix('Q') {
            if let Ok(qnh) = rest.parse::<u32>() {
                parts.push(format!("Q{qnh}hPa"));
                i += 1;
                continue;
            }
        }
        if t.starts_with('A') && t.len() == 5 {
            if let Ok(v) = t[1..].parse::<u32>() {
                parts.push(format!("{:.2}inHg", v as f32 / 100.0));
                i += 1;
                continue;
            }
        }

        // Trend
        if t == "NOSIG" {
            parts.push("NOSIG".to_string());
            i += 1;
            continue;
        }
        if t == "TEMPO" {
            parts.push("TEMPO".to_string());
            i += 1;
            continue;
        }
        if t == "BECMG" {
            parts.push("BECMG".to_string());
            i += 1;
            continue;
        }

        // Weather phenomena
        let wx = match t {
            "BR" => Some("Mist"),
            "FG" => Some("Fog"),
            "HZ" => Some("Haze"),
            "TS" => Some("TS"),
            "SN" => Some("Snow"),
            "RA" => Some("Rain"),
            "DZ" => Some("Drizzle"),
            "GR" => Some("Hail"),
            "-RA" => Some("Lt rain"),
            "+RA" => Some("Hvy rain"),
            "-SN" => Some("Lt snow"),
            "+SN" => Some("Hvy snow"),
            "TSRA" | "+TSRA" => Some("TS+rain"),
            "MIFG" => Some("Shallow fog"),
            "FU" => Some("Smoke"),
            "SQ" => Some("Squall"),
            "FC" => Some("Funnel cloud"),
            _ => None,
        };
        if let Some(w) = wx {
            parts.push(w.to_string());
        }
        i += 1;
    }

    parts.join(" | ")
}

/// A recognised weather request: METAR (aviationweather, by ICAO), WX (wttr.in, by free-text
/// location) and APRSWX (aprs.fi weather station, by callsign). Anything else gets no reply.
pub enum WxRequest {
    /// `METAR <ICAO>` — decoded aviationweather METAR for the station.
    Metar(String),
    /// `WX <location>` — wttr.in current conditions for the location.
    Wx(String),
    /// `APRSWX <CALL>` — latest aprs.fi weather report of an APRS station. An empty string
    /// means a bare `APRSWX`; the caller substitutes `[aprs_wx].default_call`.
    Aprs(String),
}

/// Parse an SDS text payload into a weather request. Recognises exactly two commands,
/// case-insensitive: `METAR <ICAO>` and `WX <location>`. Returns `None` for anything else —
/// no PING/HELP/parrot/usage replies, matching the "only these two functions" service scope.
pub fn parse_wx_request(text: &str) -> Option<WxRequest> {
    let trimmed = text.trim();

    if let Some(rest) = strip_prefix_ci(trimmed, "METAR ") {
        let icao = sanitize_icao(rest);
        if icao.len() >= 3 {
            return Some(WxRequest::Metar(icao));
        }
        return None;
    }
    if trimmed.eq_ignore_ascii_case("APRSWX") {
        return Some(WxRequest::Aprs(String::new()));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "APRSWX ") {
        let call = sanitize_callsign(rest);
        if call.is_empty() {
            return Some(WxRequest::Aprs(String::new()));
        }
        if call.len() >= 3 {
            return Some(WxRequest::Aprs(call));
        }
        return None;
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "WX ") {
        let loc = rest.trim();
        if !loc.is_empty() {
            return Some(WxRequest::Wx(loc.to_string()));
        }
        return None;
    }

    None
}

/// Fetch current conditions for a free-text location from wttr.in and format them as a
/// compact, ASCII-only, SDS-safe line. Output is byte-identical to tetraflow-sds-bot's
/// `fetch_wx`. Blocking HTTP; always call from a worker thread.
pub fn fetch_wx(location: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("curl/7.0")
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;

    let encoded = location.replace(' ', "+");
    // %l=location %C=condition %t=temp %f=feels %h=humidity %w=wind
    let url = format!("https://wttr.in/{encoded}?format=%l||%C||%t||%f||%h||%w");
    let body = client
        .get(&url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("request failed: {e}"))?
        .text()
        .map_err(|e| format!("read failed: {e}"))?;

    let line = body
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| "no WX data".to_string())?
        .trim()
        .to_string();

    let lower = line.to_lowercase();
    if lower.contains("unknown location") || lower.contains("sorry") {
        return Err("Location not found".to_string());
    }

    let fields: Vec<&str> = line.split("||").collect();
    if fields.len() < 6 {
        return Ok(ascii_only(&line));
    }

    let loc = fields[0].trim();
    let condition = fields[1].trim();
    let temp = clean_temp(fields[2].trim());
    let feels = clean_temp(fields[3].trim());
    let humidity = fields[4].trim().trim_end_matches('%');
    let wind = clean_wind(fields[5].trim());

    Ok(format!(
        "WX {loc}: {condition} Temp: {temp}C Feels: {feels}C Hum: {humidity}% Wind: {wind}. A TetraFlow Project."
    ))
}

// ───────────────────────────── APRS weather (aprs.fi) ─────────────────────────────

const APRSFI_API: &str = "https://api.aprs.fi/api/get";
/// aprs.fi asks API clients to identify themselves with a descriptive User-Agent.
const APRSFI_USER_AGENT: &str = "FlowStation-APRS-WX (+https://github.com/Aitorrio/bost-flowstation)";

/// Uppercase an APRS callsign/object name and keep only the characters aprs.fi accepts
/// in a station name (letters, digits, '-'), max 9 chars (e.g. "dn9cho-13" -> "DN9CHO-13").
pub fn sanitize_callsign(s: &str) -> String {
    s.split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(9)
        .collect::<String>()
        .to_uppercase()
}

/// Fetch the latest weather report of an APRS station from aprs.fi and format it as a
/// compact, ASCII-only, SDS-safe line. `max_age_secs` > 0 marks older reports with "OLD".
/// Blocking HTTP; always call from a worker thread.
pub fn fetch_aprs_wx(call: &str, apikey: &str, max_age_secs: u64) -> Result<String, String> {
    let call = sanitize_callsign(call);
    if call.is_empty() {
        return Err("empty callsign".to_string());
    }
    if apikey.trim().is_empty() {
        return Err("no aprs.fi API key configured".to_string());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(APRSFI_USER_AGENT)
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let resp: serde_json::Value = client
        .get(APRSFI_API)
        .query(&[
            ("name", call.as_str()),
            ("what", "wx"),
            ("apikey", apikey.trim()),
            ("format", "json"),
        ])
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("request failed: {}", e.without_url()))?
        .json()
        .map_err(|e| format!("bad JSON: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_aprs_wx(&call, &resp, now, max_age_secs)
}

/// Turn an aprs.fi `what=wx` response into the SDS line. Pure function (unit-tested).
///
/// aprs.fi units: temp °C, pressure mbar(=hPa), humidity %, wind_direction °, wind_speed and
/// wind_gust m/s, rain_1h/rain_24h/rain_mn mm, luminosity W/m². All values arrive as strings.
/// Output example:
/// `DN9CHO-13 14:05Z | 12.3C | 78% | 1016.4hPa | Wind SW 11km/h gust 25 | Rain 1h 0.2 today 3.1mm`
pub fn format_aprs_wx(call: &str, resp: &serde_json::Value, now_unix: i64, max_age_secs: u64) -> Result<String, String> {
    if resp.get("result").and_then(|v| v.as_str()) != Some("ok") {
        let desc = resp.get("description").and_then(|v| v.as_str()).unwrap_or("aprs.fi error");
        return Err(desc.to_string());
    }
    let entry = resp
        .get("entries")
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| "no WX data".to_string())?;

    // aprs.fi returns numbers as strings; accept real numbers too.
    let num = |k: &str| -> Option<f64> {
        let v = entry.get(k)?;
        match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    };

    let name = entry
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| call.to_string());
    let mut parts: Vec<String> = Vec::new();

    let mut head = name;
    if let Some(ts) = num("time").map(|t| t as i64) {
        let secs = ts.rem_euclid(86_400);
        head.push_str(&format!(" {:02}:{:02}Z", secs / 3600, (secs % 3600) / 60));
        if max_age_secs > 0 && now_unix > 0 && now_unix - ts > max_age_secs as i64 {
            head.push_str(" OLD");
        }
    }
    parts.push(head);

    if let Some(t) = num("temp") {
        parts.push(format!("{t:.1}C"));
    }
    if let Some(h) = num("humidity") {
        parts.push(format!("{h:.0}%"));
    }
    if let Some(p) = num("pressure") {
        parts.push(format!("{p:.1}hPa"));
    }
    if let Some(ws) = num("wind_speed") {
        let kmh = ws * 3.6;
        let mut w = if kmh < 0.5 {
            "Wind calm".to_string()
        } else {
            match num("wind_direction") {
                Some(d) => format!("Wind {} {:.0}km/h", compass16(d), kmh),
                None => format!("Wind {:.0}km/h", kmh),
            }
        };
        if let Some(g) = num("wind_gust") {
            let gk = g * 3.6;
            if gk >= 0.5 && gk > kmh {
                w.push_str(&format!(" gust {gk:.0}"));
            }
        }
        parts.push(w);
    }
    let r1 = num("rain_1h");
    let rmn = num("rain_mn");
    let r24 = num("rain_24h");
    if r1.is_some() || rmn.is_some() || r24.is_some() {
        let mut r = String::from("Rain");
        if let Some(v) = r1 {
            r.push_str(&format!(" 1h {v:.1}"));
        }
        if let Some(v) = rmn {
            r.push_str(&format!(" today {v:.1}"));
        } else if let Some(v) = r24 {
            r.push_str(&format!(" 24h {v:.1}"));
        }
        r.push_str("mm");
        parts.push(r);
    }
    if let Some(l) = num("luminosity") {
        parts.push(format!("Sun {l:.0}W/m2"));
    }

    if parts.len() == 1 {
        return Err("no WX data".to_string());
    }
    Ok(ascii_only(&parts.join(" | ")))
}

/// Wind direction in degrees -> 16-point compass (N, NNE, NE, ...).
fn compass16(deg: f64) -> &'static str {
    const P: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW",
    ];
    let idx = ((deg.rem_euclid(360.0) / 22.5) + 0.5).floor() as usize % 16;
    P[idx]
}

/// Keep only digits and sign from a temperature string, e.g. "+18°C" -> "+18".
fn clean_temp(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit() || *c == '+' || *c == '-').collect()
}

/// Replace wttr.in arrow glyphs with compass text and strip any remaining non-ASCII,
/// e.g. "↗ 14km/h" -> "NE 14km/h".
fn clean_wind(s: &str) -> String {
    let s = s
        .replace('↑', "N")
        .replace('↗', "NE")
        .replace('→', "E")
        .replace('↘', "SE")
        .replace('↓', "S")
        .replace('↙', "SW")
        .replace('←', "W")
        .replace('↖', "NW");
    s.chars().filter(|c| c.is_ascii()).collect()
}

/// Case-insensitive prefix strip.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Rewrite (or insert) the `[wx_service]` section in the TOML file, preserving everything
/// else. The whole section is regenerated from the override values; an existing
/// `[wx_service]` block (from its header until the next section header or EOF) is replaced.
pub fn write_wx_to_toml(config_path: &str, ov: &tetra_config::bluestation::WxRuntimeOverride) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;

    let icao_escaped = ov.periodic_icao.replace('\\', "\\\\").replace('"', "\\\"");
    let section = format!(
        "[wx_service]\n\
         enabled = {}\n\
         service_issi = {}\n\
         periodic_enabled = {}\n\
         periodic_issi = {}\n\
         periodic_is_group = {}\n\
         periodic_icao = \"{}\"\n\
         periodic_interval_secs = {}",
        ov.enabled, ov.service_issi, ov.periodic_enabled, ov.periodic_issi, ov.periodic_is_group, icao_escaped, ov.periodic_interval_secs
    );

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 10);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("[wx_service]") {
            // Replace this whole section: emit the regenerated block, then skip the old
            // section's body up to (but not including) the next section header / EOF.
            out.push(section.clone());
            replaced = true;
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim_start();
                if t.starts_with('[') && t.contains(']') {
                    break; // next section starts; stop skipping
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

    let backup = format!("{config_path}.wx.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_icao(" lrop "), "LROP");
        assert_eq!(sanitize_icao("LROP123"), "LROP");
        assert_eq!(sanitize_icao("kjfk"), "KJFK");
    }

    #[test]
    fn decode_basic() {
        let raw = "LROP 301600Z 11005KT 9999 FEW040 SCT100 17/01 Q1018 NOSIG";
        let out = decode_metar(raw);
        assert!(out.contains("LROP"));
        assert!(out.contains("16:00Z"));
        assert!(out.contains("Wind 110 5kt"));
        assert!(out.contains("Vis >10km"));
        assert!(out.contains("Few 4000ft"));
        assert!(out.contains("17C/1C"));
        assert!(out.contains("Q1018hPa"));
        assert!(out.contains("NOSIG"));
    }

    #[test]
    fn decode_gust_and_negative_temp() {
        let raw = "EHAM 301600Z 27015G25KT 8000 BKN010 M02/M05 Q0998";
        let out = decode_metar(raw);
        assert!(out.contains("gust 25kt"));
        assert!(out.contains("-2C/-5C"));
        assert!(out.contains("Vis 8km"));
    }

    #[test]
    fn decode_cavok_vrb() {
        let raw = "LICJ 301600Z VRB03KT CAVOK 25/12 Q1015";
        let out = decode_metar(raw);
        assert!(out.contains("Wind variable 3kt"));
        assert!(out.contains("CAVOK"));
    }

    #[test]
    fn request_metar_prefix() {
        assert!(matches!(parse_wx_request("METAR LROP"), Some(WxRequest::Metar(ref s)) if s == "LROP"));
        assert!(matches!(parse_wx_request("metar kjfk"), Some(WxRequest::Metar(ref s)) if s == "KJFK"));
    }

    #[test]
    fn request_wx_prefix() {
        assert!(matches!(parse_wx_request("WX Bucharest"), Some(WxRequest::Wx(ref s)) if s == "Bucharest"));
        assert!(matches!(parse_wx_request("wx Cluj Napoca"), Some(WxRequest::Wx(ref s)) if s == "Cluj Napoca"));
    }

    #[test]
    fn request_aprs() {
        assert!(matches!(parse_wx_request("APRSWX DN9CHO-13"), Some(WxRequest::Aprs(ref s)) if s == "DN9CHO-13"));
        assert!(matches!(parse_wx_request("aprswx dn9cho-13 bitte"), Some(WxRequest::Aprs(ref s)) if s == "DN9CHO-13"));
        assert!(matches!(parse_wx_request("AprsWx"), Some(WxRequest::Aprs(ref s)) if s.is_empty()));
        assert!(matches!(parse_wx_request(" APRSWX "), Some(WxRequest::Aprs(ref s)) if s.is_empty()));
        assert!(parse_wx_request("APRSWX AB").is_none());
        // Plain "APRS" stays free (e.g. for a future position query) and is not a WX command.
        assert!(parse_wx_request("APRS").is_none());
        assert!(parse_wx_request("APRS DN9CHO-13").is_none());
        assert!(parse_wx_request("APRSWXX").is_none());
    }

    #[test]
    fn aprs_format_full() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"command":"get","result":"ok","found":1,"what":"wx","entries":[{"name":"DN9CHO-13","time":"1762524300",
            "temp":"12.3","pressure":"1016.4","humidity":"78","wind_direction":"225","wind_speed":"3.1","wind_gust":"6.9",
            "rain_1h":"0.2","rain_24h":"4.0","rain_mn":"3.1","luminosity":"120"}]}"#,
        )
        .unwrap();
        let out = format_aprs_wx("DN9CHO-13", &v, 1762524300 + 60, 3600).unwrap();
        assert_eq!(
            out,
            "DN9CHO-13 14:05Z | 12.3C | 78% | 1016.4hPa | Wind SW 11km/h gust 25 | Rain 1h 0.2 today 3.1mm | Sun 120W/m2"
        );
        assert!(out.len() <= 220);
    }

    #[test]
    fn aprs_format_partial_old_calm() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"result":"ok","entries":[{"name":"DL0ABC-13","time":"1000","temp":"-2.5","wind_speed":"0","wind_direction":"0"}]}"#,
        )
        .unwrap();
        let out = format_aprs_wx("DL0ABC-13", &v, 1000 + 7200, 3600).unwrap();
        assert_eq!(out, "DL0ABC-13 00:16Z OLD | -2.5C | Wind calm");
    }

    #[test]
    fn aprs_format_errors() {
        let fail: serde_json::Value = serde_json::from_str(r#"{"result":"fail","description":"authentication failed"}"#).unwrap();
        assert_eq!(format_aprs_wx("X", &fail, 0, 0).unwrap_err(), "authentication failed");
        let empty: serde_json::Value = serde_json::from_str(r#"{"result":"ok","found":0,"entries":[]}"#).unwrap();
        assert!(format_aprs_wx("X", &empty, 0, 0).is_err());
    }

    #[test]
    fn compass() {
        assert_eq!(compass16(0.0), "N");
        assert_eq!(compass16(359.0), "N");
        assert_eq!(compass16(225.0), "SW");
        assert_eq!(compass16(100.0), "E");
    }

    #[test]
    fn request_only_two_commands() {
        // Anything that is not METAR/WX yields no command (no PING/HELP/bare/usage).
        assert!(parse_wx_request("PING").is_none());
        assert!(parse_wx_request("HELP").is_none());
        assert!(parse_wx_request("LROP").is_none());
        assert!(parse_wx_request("hello there").is_none());
        assert!(parse_wx_request("METAR LR").is_none()); // too short
        assert!(parse_wx_request("WX ").is_none()); // empty location
    }

    #[test]
    fn write_toml_replace_section() {
        let cfg = "[cell]\nfoo = 1\n\n[wx_service]\nenabled = false\nservice_issi = 9998\n\n[security]\nbar = 2\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_wx_test_replace.toml");
        std::fs::write(&path, cfg).unwrap();
        let ov = tetra_config::bluestation::WxRuntimeOverride {
            enabled: true,
            service_issi: 9000,
            periodic_enabled: true,
            periodic_issi: 1234,
            periodic_is_group: false,
            periodic_icao: "LROP".to_string(),
            periodic_interval_secs: 600,
        };
        write_wx_to_toml(path.to_str().unwrap(), &ov).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("enabled = true"));
        assert!(out.contains("service_issi = 9000"));
        assert!(out.contains("periodic_icao = \"LROP\""));
        // Other sections preserved.
        assert!(out.contains("[cell]"));
        assert!(out.contains("[security]"));
        assert!(out.contains("bar = 2"));
        // Old value gone.
        assert!(!out.contains("service_issi = 9998"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_toml_append_when_missing() {
        let cfg = "[cell]\nfoo = 1\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_wx_test_append.toml");
        std::fs::write(&path, cfg).unwrap();
        let ov = tetra_config::bluestation::WxRuntimeOverride {
            enabled: true,
            service_issi: 9998,
            periodic_enabled: false,
            periodic_issi: 0,
            periodic_is_group: false,
            periodic_icao: String::new(),
            periodic_interval_secs: 1800,
        };
        write_wx_to_toml(path.to_str().unwrap(), &ov).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[wx_service]"));
        assert!(out.contains("enabled = true"));
        assert!(out.contains("[cell]"));
        let _ = std::fs::remove_file(&path);
    }
}
