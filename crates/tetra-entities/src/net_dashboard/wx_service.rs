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
    let url = metar_url(icao).ok_or_else(|| "empty ICAO".to_string())?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let body = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("request failed: {e}"))?
        .text()
        .map_err(|e| format!("read failed: {e}"))?;
    metar_line(&body)
}

/// URL of the raw METAR of an ICAO code; None when the code is empty.
pub fn metar_url(icao: &str) -> Option<String> {
    let icao = sanitize_icao(icao);
    (!icao.is_empty()).then(|| format!("{METAR_API}?ids={icao}&format=raw&taf=false"))
}

/// The raw METAR in a reply of the METAR API.
fn metar_line(body: &str) -> Result<String, String> {
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

/// 1 when the report starts with its type ("METAR LELC 051530Z ...", as aviationweather.gov
/// now sends it), 0 when it starts with the station.
fn report_type_len(tokens: &[&str]) -> usize {
    usize::from(tokens.first().is_some_and(|t| *t == "METAR" || *t == "SPECI"))
}

/// Decode a raw METAR into a compact human-readable summary, fields separated by " | ".
/// Best-effort: unrecognised tokens are skipped. Ported from tetraflow-sds-bot.
pub fn decode_metar(raw: &str) -> String {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if tokens.is_empty() {
        return raw.to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    let mut i = report_type_len(&tokens);

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

/// A recognised weather request. Only two commands exist: METAR (aviationweather, by ICAO)
/// and WX (wttr.in, by free-text location). Anything else is not a command and gets no reply.
pub enum WxRequest {
    /// `METAR <ICAO>` — decoded aviationweather METAR for the station.
    Metar(String),
    /// `WX <location>` — wttr.in current conditions for the location.
    Wx(String),
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

    let body = client
        .get(wx_url(location))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("request failed: {e}"))?
        .text()
        .map_err(|e| format!("read failed: {e}"))?;
    wx_from_body(&body)
}

/// wttr.in URL of the current conditions at a free-text location.
pub fn wx_url(location: &str) -> String {
    let encoded = location.replace(' ', "+");
    // %l=location %C=condition %t=temp %f=feels %h=humidity %w=wind
    format!("https://wttr.in/{encoded}?format=%l||%C||%t||%f||%h||%w")
}

/// The compact, ASCII-only line for a wttr.in reply (for a caller that downloads it itself).
pub fn wx_from_body(body: &str) -> Result<String, String> {
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

// ── WAP weather page: Spanish, several lines ──────────────────────────────────
//
// The WAP gateway is UTF-8 (XHTML-MP), so these keep accents and the degree sign. The SDS
// replies above stay as they are (ASCII, one line).

const GEOCODING_API: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_API: &str = "https://api.open-meteo.com/v1/forecast";

/// A place found by the Open-Meteo geocoder.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// The place and, in brackets, its province (Spain) or its country.
    pub label: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// Open-Meteo geocoding URL for a place name: answers in Spanish, five candidates, only Spain
/// when `spain` (first pass: "Cartagena" is then the one in Murcia, not the one in Colombia).
pub fn geocode_url(name: &str, spain: bool) -> String {
    let name: String = url::form_urlencoded::byte_serialize(name.trim().as_bytes()).collect();
    let country = if spain { "&countryCode=ES" } else { "" };
    format!("{GEOCODING_API}?name={name}&count=5&language=es&format=json{country}")
}

/// Lower case without accents, to compare place names.
fn fold(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// The place in a geocoding reply, `None` when there is none that fits. In the Spain-only pass
/// only towns count (feature code PPL...: otherwise "Paris" is a park in Madrid). With a `hint`
/// (what followed a comma, as in "Cartagena, Colombia"), the first candidate whose country or
/// region contains it. An error when the reply is not the geocoder's JSON.
pub fn place_from_body(body: &str, spain: bool, hint: Option<&str>) -> Result<Option<Place>, String> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad geocoding JSON: {e}"))?;
    if !value.is_object() {
        return Err("bad geocoding JSON".to_string());
    }
    let Some(results) = value.get("results").and_then(|r| r.as_array()) else {
        return Ok(None);
    };
    let hint = hint.map(fold).filter(|h| !h.is_empty());
    let field = |r: &serde_json::Value, key: &str| r.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let found = results.iter().find(|r| {
        (!spain || field(r, "feature_code").starts_with("PPL"))
            && hint.as_ref().is_none_or(|h| {
                ["country", "country_code", "admin1", "admin2"]
                    .iter()
                    .any(|key| fold(&field(r, key)).contains(h.as_str()))
            })
    });
    let Some(r) = found else {
        return Ok(None);
    };
    let (Some(latitude), Some(longitude)) = (
        r.get("latitude").and_then(|v| v.as_f64()),
        r.get("longitude").and_then(|v| v.as_f64()),
    ) else {
        return Err("geocoding result without coordinates".to_string());
    };
    let name = field(r, "name");
    let region = if field(r, "country_code") == "ES" {
        let province = field(r, "admin2");
        let province = province.strip_prefix("Provincia de ").unwrap_or(&province).to_string();
        if province.is_empty() { field(r, "admin1") } else { province }
    } else {
        field(r, "country")
    };
    let label = if region.is_empty() || region == name {
        name
    } else {
        format!("{name} ({region})")
    };
    Ok(Some(Place {
        label,
        latitude,
        longitude,
    }))
}

/// Open-Meteo URL of the current weather and today's and tomorrow's forecast at a place.
pub fn forecast_url(place: &Place) -> String {
    format!(
        "{FORECAST_API}?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m,wind_direction_10m,wind_gusts_10m\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&forecast_days=2&timezone=auto",
        place.latitude, place.longitude
    )
}

/// WMO weather interpretation code (Open-Meteo `weather_code`) in Spanish.
fn wmo_es(code: i64) -> String {
    let text = match code {
        0 => "Despejado",
        1 => "Casi despejado",
        2 => "Parcialmente nuboso",
        3 => "Cubierto",
        45 => "Niebla",
        48 => "Niebla con escarcha",
        51 => "Llovizna débil",
        53 => "Llovizna",
        55 => "Llovizna intensa",
        56 | 57 => "Llovizna helada",
        61 => "Lluvia débil",
        63 => "Lluvia",
        65 => "Lluvia fuerte",
        66 | 67 => "Lluvia helada",
        71 => "Nieve débil",
        73 => "Nieve",
        75 => "Nieve fuerte",
        77 => "Granizo fino",
        80 => "Chubascos débiles",
        81 => "Chubascos",
        82 => "Chubascos fuertes",
        85 | 86 => "Chubascos de nieve",
        95 => "Tormenta",
        96 | 99 => "Tormenta con granizo",
        other => return format!("Código {other}"),
    };
    text.to_string()
}

/// Compass point (8) of a direction in degrees, in Spanish (O = oeste).
fn compass_es(degrees: f64) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SO", "O", "NO"];
    POINTS[((degrees.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8]
}

/// Rounded to a whole number, half away from zero.
fn whole(x: f64) -> i64 {
    x.round() as i64
}

/// The lines of the weather page for a forecast reply: place, sky and temperature, feel and
/// humidity, wind, today, tomorrow and the time of the data. An error when the reply is not
/// Open-Meteo's JSON.
pub fn forecast_lines_es(place: &Place, body: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad forecast JSON: {e}"))?;
    let current = value.get("current").ok_or("forecast without current weather")?;
    let now = |key: &str| current.get(key).and_then(|v| v.as_f64());
    let temperature = now("temperature_2m").ok_or("forecast without temperature")?;
    let mut lines = vec![place.label.clone()];
    lines.push(match now("weather_code") {
        Some(code) => format!("{}, {} °C", wmo_es(code as i64), whole(temperature)),
        None => format!("{} °C", whole(temperature)),
    });
    let feel = [
        now("apparent_temperature").map(|t| format!("sensación {} °C", whole(t))),
        now("relative_humidity_2m").map(|h| format!("humedad {} %", whole(h))),
    ];
    let feel = feel.into_iter().flatten().collect::<Vec<_>>().join(", ");
    if let Some(first) = feel.chars().next() {
        lines.push(format!("{}{}", first.to_uppercase(), &feel[first.len_utf8()..]));
    }
    if let Some(speed) = now("wind_speed_10m") {
        let mut wind = if whole(speed) == 0 {
            "Viento en calma".to_string()
        } else {
            let from = now("wind_direction_10m").map(|d| format!("{} ", compass_es(d))).unwrap_or_default();
            format!("Viento {from}{} km/h", whole(speed))
        };
        if let Some(gusts) = now("wind_gusts_10m").filter(|g| whole(*g) > whole(speed)) {
            wind.push_str(&format!(", rachas {} km/h", whole(gusts)));
        }
        lines.push(wind);
    }
    if let Some(daily) = value.get("daily") {
        let day = |key: &str, i: usize| daily.get(key).and_then(|a| a.get(i)).and_then(|v| v.as_f64());
        for (i, label) in ["Hoy", "Mañana"].iter().enumerate() {
            let (Some(low), Some(high)) = (day("temperature_2m_min", i), day("temperature_2m_max", i)) else {
                continue;
            };
            let sky = day("weather_code", i)
                .map(|c| format!("{}, ", wmo_es(c as i64).to_lowercase()))
                .unwrap_or_default();
            let rain = day("precipitation_probability_max", i)
                .map(|p| format!(", lluvia {} %", whole(p)))
                .unwrap_or_default();
            lines.push(format!("{label}: {sky}{} a {} °C{rain}", whole(low), whole(high)));
        }
    }
    if let Some(time) = current
        .get("time")
        .and_then(|t| t.as_str())
        .and_then(|t| t.split_once('T'))
        .map(|(_, hhmm)| hhmm)
    {
        lines.push(format!("Dato de las {time} (hora local)"));
    }
    Ok(lines)
}

/// The lines of the weather page for a reply of the METAR API; `None` when the API has no
/// METAR for the code (an unknown code is an empty HTTP 204).
pub fn metar_lines_es_from_body(body: &str) -> Option<Vec<String>> {
    metar_line(body).ok().map(|raw| metar_lines_es(&raw))
}

/// A METAR in Spanish, one item per line, with the raw METAR as the last line. Best-effort like
/// [`decode_metar`]: unrecognised tokens are skipped.
pub fn metar_lines_es(raw: &str) -> Vec<String> {
    // A METAR is ASCII: anything else is skipped, so that the slicing below stays on char bounds.
    let tokens: Vec<&str> = raw.split_whitespace().filter(|t| t.is_ascii()).collect();
    let mut lines = Vec::new();
    let mut i = report_type_len(&tokens);

    // Station and time, e.g. "LELC 051530Z".
    let mut head = Vec::new();
    if let Some(station) = tokens.get(i).filter(|t| t.len() == 4 && t.chars().all(|c| c.is_ascii_alphabetic())) {
        head.push(station.to_string());
        i += 1;
    }
    if let Some(t) = tokens.get(i).filter(|t| t.ends_with('Z') && t.len() == 7) {
        if let (Ok(dd), Ok(hh), Ok(mm)) = (t[..2].parse::<u32>(), t[2..4].parse::<u32>(), t[4..6].parse::<u32>()) {
            head.push(format!("día {dd:02}, {hh:02}:{mm:02} UTC"));
        }
        i += 1;
    }
    if !head.is_empty() {
        lines.push(head.join(" "));
    }
    if tokens.get(i).is_some_and(|t| *t == "AUTO" || *t == "COR") {
        i += 1;
    }

    // Wind, e.g. 13006KT, 27015G25KT, VRB03KT, 00000KT, then a variable range 080V170.
    if let Some(w) = tokens.get(i).filter(|w| w.ends_with("KT") || w.ends_with("MPS")) {
        let unit = if w.ends_with("KT") { "kt" } else { "m/s" };
        let body = w.trim_end_matches("KT").trim_end_matches("MPS");
        let (main, gust) = match body.split_once('G') {
            Some((main, gust)) => (main, gust.parse::<u32>().ok()),
            None => (body, None),
        };
        let gust = gust.map(|g| format!(", rachas {g} {unit}")).unwrap_or_default();
        let wind = if let Some(speed) = main.strip_prefix("VRB").and_then(|s| s.parse::<u32>().ok()) {
            Some(format!("Viento variable {speed} {unit}{gust}"))
        } else if main.len() >= 5 {
            match (main[..3].parse::<u32>(), main[3..].parse::<u32>()) {
                (Ok(_), Ok(0)) => Some("Viento en calma".to_string()),
                (Ok(dir), Ok(speed)) => Some(format!("Viento {dir:03}° {speed} {unit}{gust}")),
                _ => None,
            }
        } else {
            None
        };
        i += 1;
        if let Some(mut wind) = wind {
            if let Some(range) = tokens.get(i).filter(|t| t.len() == 7 && t.as_bytes()[3] == b'V') {
                wind.push_str(&format!(" (entre {} y {})", &range[..3], &range[4..]));
                i += 1;
            }
            lines.push(wind);
        }
    }

    // Visibility.
    if let Some(v) = tokens.get(i) {
        let visibility = match *v {
            "CAVOK" => Some("Visibilidad 10 km o más, sin nubes bajas (CAVOK)".to_string()),
            "9999" => Some("Visibilidad 10 km o más".to_string()),
            v => v.parse::<u32>().ok().filter(|m| v.len() == 4 && *m <= 9999).map(|m| {
                if m >= 1000 {
                    format!("Visibilidad {} km", m / 1000)
                } else {
                    format!("Visibilidad {m} m")
                }
            }),
        };
        if let Some(visibility) = visibility {
            lines.push(visibility);
            i += 1;
        }
    }

    let mut clouds = Vec::new();
    let mut weather = Vec::new();
    let mut rest = Vec::new();
    for t in &tokens[i.min(tokens.len())..] {
        let t = *t;
        if t.len() >= 6 && matches!(&t[..3], "FEW" | "SCT" | "BKN" | "OVC") {
            let cover = match &t[..3] {
                "FEW" => "pocas",
                "SCT" => "dispersas",
                "BKN" => "muy nubosas",
                _ => "cubierto",
            };
            if let Ok(hundreds) = t[3..6].parse::<u32>() {
                let kind = match &t[6..] {
                    "CB" => " (cumulonimbos)",
                    "TCU" => " (cúmulos de desarrollo)",
                    _ => "",
                };
                clouds.push(format!("{cover} a {} ft{kind}", hundreds * 100));
            }
            continue;
        }
        if matches!(t, "NSC" | "NCD" | "SKC" | "CLR") {
            rest.push("Sin nubes significativas".to_string());
            continue;
        }
        if t.contains('/') && !t.starts_with('Q') && !t.starts_with('R') {
            let parse_t = |s: &str| match s.strip_prefix('M') {
                Some(minus) => minus.parse::<i32>().ok().map(|v| -v),
                None => s.parse::<i32>().ok(),
            };
            if let Some((Some(temp), Some(dew))) = t.split_once('/').map(|(a, b)| (parse_t(a), parse_t(b))) {
                rest.push(format!("Temperatura {temp} °C, rocío {dew} °C"));
                continue;
            }
        }
        if let Some(qnh) = t.strip_prefix('Q').and_then(|q| q.parse::<u32>().ok()) {
            rest.push(format!("QNH {qnh} hPa"));
            continue;
        }
        if t.starts_with('A') && t.len() == 5 {
            if let Ok(v) = t[1..].parse::<u32>() {
                rest.push(format!("QNH {:.2} inHg", v as f32 / 100.0));
                continue;
            }
        }
        let trend = match t {
            "NOSIG" => Some("Sin cambios previstos (NOSIG)"),
            "TEMPO" => Some("Cambios temporales (TEMPO)"),
            "BECMG" => Some("Cambio previsto (BECMG)"),
            _ => None,
        };
        if let Some(trend) = trend {
            rest.push(trend.to_string());
            continue;
        }
        let phenomenon = match t {
            "BR" => Some("neblina"),
            "FG" => Some("niebla"),
            "MIFG" => Some("niebla baja"),
            "BCFG" => Some("bancos de niebla"),
            "HZ" => Some("calima"),
            "FU" => Some("humo"),
            "DU" => Some("polvo"),
            "SA" => Some("arena"),
            "DZ" => Some("llovizna"),
            "-DZ" => Some("llovizna débil"),
            "RA" => Some("lluvia"),
            "-RA" => Some("lluvia débil"),
            "+RA" => Some("lluvia fuerte"),
            "SHRA" => Some("chubascos"),
            "-SHRA" => Some("chubascos débiles"),
            "+SHRA" => Some("chubascos fuertes"),
            "VCSH" => Some("chubascos en las cercanías"),
            "SN" => Some("nieve"),
            "-SN" => Some("nieve débil"),
            "+SN" => Some("nieve fuerte"),
            "GR" => Some("granizo"),
            "TS" => Some("tormenta"),
            "VCTS" => Some("tormenta en las cercanías"),
            "TSRA" => Some("tormenta con lluvia"),
            "-TSRA" => Some("tormenta con lluvia débil"),
            "+TSRA" => Some("tormenta con lluvia fuerte"),
            "SQ" => Some("turbonada"),
            "FC" => Some("tromba"),
            _ => None,
        };
        if let Some(p) = phenomenon {
            weather.push(p);
        }
    }
    if !weather.is_empty() {
        lines.push(format!("Tiempo: {}", weather.join(", ")));
    }
    if !clouds.is_empty() {
        lines.push(format!("Nubes: {}", clouds.join(", ")));
    }
    lines.extend(rest);
    lines.push(raw.trim().to_string());
    lines
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
    fn decode_with_report_type_prefix() {
        // aviationweather.gov, 2026-10-05: the report type comes first.
        let raw = "METAR LELC 051530Z 13006KT 080V170 9999 FEW030 27/22 Q1019 NOSIG";
        let plain = "LELC 051530Z 13006KT 080V170 9999 FEW030 27/22 Q1019 NOSIG";
        let out = decode_metar(raw);
        assert_eq!(out, decode_metar(plain));
        assert_eq!(
            out,
            "LELC | 15:30Z | Wind 130 6kt | var 080-170 | Vis >10km | Few 3000ft | 27C/22C | Q1019hPa | NOSIG"
        );
        assert!(decode_metar("SPECI LEMD 051545Z 18007KT 9999 FEW045 25/14 Q1019").starts_with("LEMD | 15:45Z | Wind 180 7kt"));
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
    fn request_only_two_commands() {
        // Anything that is not METAR/WX yields no command (no PING/HELP/bare/usage).
        assert!(parse_wx_request("PING").is_none());
        assert!(parse_wx_request("HELP").is_none());
        assert!(parse_wx_request("LROP").is_none());
        assert!(parse_wx_request("hello there").is_none());
        assert!(parse_wx_request("METAR LR").is_none()); // too short
        assert!(parse_wx_request("WX ").is_none()); // empty location
    }

    // Replies of Open-Meteo captured on 2026-10-05 (postcode lists shortened).
    const GEO_SAN_JAVIER_ES: &str = r#"{"results":[{"id":2511366,"name":"San Javier","latitude":37.80626,"longitude":-0.83736,"elevation":27.0,"feature_code":"PPLA3","country_code":"ES","admin1_id":2513413,"admin2_id":6355234,"admin3_id":6359539,"timezone":"Europe/Madrid","population":31432,"postcodes":["30730"],"country_id":2510769,"country":"España","admin1":"Región de Murcia","admin2":"Provincia de Murcia","admin3":"San Javier"}],"generationtime_ms":0.5072355}"#;
    const GEO_CORUNA_ES: &str = r#"{"results":[{"id":3119841,"name":"La Coruña","latitude":43.37135,"longitude":-8.396,"elevation":22.0,"feature_code":"PPLA3","country_code":"ES","admin1_id":3336902,"admin2_id":3119840,"admin3_id":6357300,"timezone":"Europe/Madrid","population":250438,"postcodes":["15190","15005"],"country_id":2510769,"country":"España","admin1":"Comunidad Autónoma de Galicia","admin2":"Provincia de La Coruña","admin3":"A Coruña"}],"generationtime_ms":0.2539158}"#;
    const GEO_PARIS_ES: &str = r#"{"results":[{"id":10342161,"name":"París Park","latitude":40.49039,"longitude":-3.88427,"elevation":700.0,"feature_code":"PRK","country_code":"ES","admin1_id":3117732,"admin2_id":6355233,"admin3_id":6359348,"timezone":"Europe/Madrid","country_id":2510769,"country":"España","admin1":"Comunidad Autónoma de Madrid","admin2":"Provincia de Madrid","admin3":"Las Rozas de Madrid"}],"generationtime_ms":0.96678734}"#;
    const GEO_CARTAGENA_WORLD: &str = r#"{"results":[{"id":3687238,"name":"Cartagena de Indias","latitude":10.39817,"longitude":-75.49328,"elevation":20.0,"feature_code":"PPLA","country_code":"CO","admin1_id":3688650,"admin2_id":3687247,"timezone":"America/Bogota","population":914552,"country_id":3686110,"country":"Colombia","admin1":"Departamento de Bolívar","admin2":"Cartagena de Indias"},{"id":2520058,"name":"Cartagena","latitude":37.60197,"longitude":-0.98397,"elevation":1.0,"feature_code":"PPLA3","country_code":"ES","admin1_id":2513413,"admin2_id":6355234,"admin3_id":6359521,"timezone":"Europe/Madrid","population":213943,"postcodes":["30200"],"country_id":2510769,"country":"España","admin1":"Región de Murcia","admin2":"Provincia de Murcia","admin3":"Cartagena"},{"id":3896433,"name":"Cartagena","latitude":-33.55384,"longitude":-71.60761,"elevation":82.0,"feature_code":"PPL","country_code":"CL","admin1_id":3868621,"admin2_id":3872374,"admin3_id":8261450,"timezone":"America/Santiago","population":17978,"country_id":3895114,"country":"Chile","admin1":"Región de Valparaíso","admin2":"San Antonio","admin3":"Cartagena"}],"generationtime_ms":0.48339367}"#;
    const GEO_NONE: &str = r#"{"generationtime_ms":0.23961067}"#;
    const FORECAST_MURCIA: &str = r#"{"latitude":38.0,"longitude":-1.125,"generationtime_ms":0.22614002227783203,"utc_offset_seconds":7200,"timezone":"Europe/Madrid","timezone_abbreviation":"GMT+2","elevation":51.0,"current_units":{"time":"iso8601","interval":"seconds","temperature_2m":"°C","relative_humidity_2m":"%","apparent_temperature":"°C","weather_code":"wmo code","wind_speed_10m":"km/h","wind_direction_10m":"°","wind_gusts_10m":"km/h"},"current":{"time":"2026-10-05T18:00","interval":900,"temperature_2m":27.8,"relative_humidity_2m":64,"apparent_temperature":30.1,"weather_code":3,"wind_speed_10m":12.6,"wind_direction_10m":149,"wind_gusts_10m":23.0},"daily_units":{"time":"iso8601","weather_code":"wmo code","temperature_2m_max":"°C","temperature_2m_min":"°C","precipitation_probability_max":"%"},"daily":{"time":["2026-10-05","2026-10-06"],"weather_code":[45,61],"temperature_2m_max":[28.7,27.0],"temperature_2m_min":[18.9,19.5],"precipitation_probability_max":[3,33]}}"#;

    #[test]
    fn geocoding_urls_prefer_spain() {
        assert_eq!(
            geocode_url("San Javier", true),
            "https://geocoding-api.open-meteo.com/v1/search?name=San+Javier&count=5&language=es&format=json&countryCode=ES"
        );
        assert_eq!(
            geocode_url(" A Coruña ", false),
            "https://geocoding-api.open-meteo.com/v1/search?name=A+Coru%C3%B1a&count=5&language=es&format=json"
        );
    }

    #[test]
    fn geocoding_picks_the_place() {
        let place = place_from_body(GEO_SAN_JAVIER_ES, true, None).unwrap().unwrap();
        assert_eq!(place.label, "San Javier (Murcia)");
        assert_eq!((place.latitude, place.longitude), (37.80626, -0.83736));
        // The province is the place itself: shown once.
        assert_eq!(place_from_body(GEO_CORUNA_ES, true, None).unwrap().unwrap().label, "La Coruña");
        // In the Spain-only pass a park is not a town: the second pass looks worldwide.
        assert_eq!(place_from_body(GEO_PARIS_ES, true, None).unwrap(), None);
        assert_eq!(
            place_from_body(GEO_PARIS_ES, false, None).unwrap().unwrap().label,
            "París Park (Madrid)"
        );
        // "Cartagena, Chile": the hint chooses among the worldwide candidates (accents ignored).
        let chile = place_from_body(GEO_CARTAGENA_WORLD, false, Some("chile")).unwrap().unwrap();
        assert_eq!(chile.label, "Cartagena (Chile)");
        let valparaiso = place_from_body(GEO_CARTAGENA_WORLD, false, Some("Valparaiso")).unwrap().unwrap();
        assert_eq!(valparaiso.label, "Cartagena (Chile)");
        let spain = place_from_body(GEO_CARTAGENA_WORLD, false, Some("España")).unwrap().unwrap();
        assert_eq!(spain.label, "Cartagena (Murcia)");
        assert_eq!(
            place_from_body(GEO_CARTAGENA_WORLD, false, None).unwrap().unwrap().label,
            "Cartagena de Indias (Colombia)"
        );
        assert_eq!(place_from_body(GEO_CARTAGENA_WORLD, false, Some("Peru")).unwrap(), None);
        assert_eq!(place_from_body(GEO_NONE, true, None).unwrap(), None);
        assert!(place_from_body("<!DOCTYPE html>", true, None).is_err());
        assert!(place_from_body("[]", true, None).is_err());
    }

    #[test]
    fn forecast_in_spanish_over_several_lines() {
        let place = place_from_body(GEO_SAN_JAVIER_ES, true, None).unwrap().unwrap();
        assert!(
            forecast_url(&place)
                .starts_with("https://api.open-meteo.com/v1/forecast?latitude=37.8063&longitude=-0.8374&current=temperature_2m,")
        );
        let murcia = Place {
            label: "Murcia".to_string(),
            latitude: 37.98704,
            longitude: -1.13004,
        };
        assert_eq!(
            forecast_lines_es(&murcia, FORECAST_MURCIA).unwrap(),
            [
                "Murcia",
                "Cubierto, 28 °C",
                "Sensación 30 °C, humedad 64 %",
                "Viento SE 13 km/h, rachas 23 km/h",
                "Hoy: niebla, 19 a 29 °C, lluvia 3 %",
                "Mañana: lluvia débil, 20 a 27 °C, lluvia 33 %",
                "Dato de las 18:00 (hora local)",
            ]
        );
        let calm = r#"{"current":{"time":"2026-10-05T06:15","temperature_2m":-0.4,"weather_code":0,"wind_speed_10m":0.3}}"#;
        assert_eq!(
            forecast_lines_es(&murcia, calm).unwrap(),
            ["Murcia", "Despejado, 0 °C", "Viento en calma", "Dato de las 06:15 (hora local)"]
        );
        assert!(forecast_lines_es(&murcia, "{}").is_err());
        assert!(forecast_lines_es(&murcia, "busy").is_err());
    }

    #[test]
    fn spanish_words() {
        assert_eq!(wmo_es(95), "Tormenta");
        assert_eq!(wmo_es(42), "Código 42");
        assert_eq!(
            [0.0, 30.0, 149.0, 200.0, 250.0, 300.0, 337.6, 360.0].map(compass_es),
            ["N", "NE", "SE", "S", "O", "NO", "N", "N"]
        );
        assert_eq!(whole(2.5), 3);
        assert_eq!(whole(-2.5), -3);
    }

    #[test]
    fn metar_in_spanish_with_and_without_prefix() {
        let raw = "METAR LELC 051530Z 13006KT 080V170 9999 FEW030 27/22 Q1019 NOSIG";
        let lines = metar_lines_es_from_body(&format!("{raw}\n")).unwrap();
        assert_eq!(
            lines,
            [
                "LELC día 05, 15:30 UTC",
                "Viento 130° 6 kt (entre 080 y 170)",
                "Visibilidad 10 km o más",
                "Nubes: pocas a 3000 ft",
                "Temperatura 27 °C, rocío 22 °C",
                "QNH 1019 hPa",
                "Sin cambios previstos (NOSIG)",
                raw,
            ]
        );
        let plain = metar_lines_es("LELC 051530Z 13006KT 080V170 9999 FEW030 27/22 Q1019 NOSIG");
        assert_eq!(plain[..7], lines[..7]);
        let gusty = metar_lines_es("SPECI EHAM 301600Z 27015G25KT 0800 -RA BR BKN010 OVC020CB M02/M05 Q0998 TEMPO");
        assert_eq!(gusty[0], "EHAM día 30, 16:00 UTC");
        assert_eq!(gusty[1], "Viento 270° 15 kt, rachas 25 kt");
        assert_eq!(gusty[2], "Visibilidad 800 m");
        assert_eq!(gusty[3], "Tiempo: lluvia débil, neblina");
        assert_eq!(gusty[4], "Nubes: muy nubosas a 1000 ft, cubierto a 2000 ft (cumulonimbos)");
        assert_eq!(gusty[5], "Temperatura -2 °C, rocío -5 °C");
        let calm = metar_lines_es("LICJ 301600Z 00000KT CAVOK 25/12 Q1015");
        assert_eq!(calm[1..3], ["Viento en calma", "Visibilidad 10 km o más, sin nubes bajas (CAVOK)"]);
        assert_eq!(metar_lines_es("LICJ 301600Z VRB03KT 9999 NSC")[1], "Viento variable 3 kt");
        // Unknown code: an empty HTTP 204.
        assert_eq!(metar_lines_es_from_body(""), None);
        assert_eq!(metar_lines_es_from_body("\n\n"), None);
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
