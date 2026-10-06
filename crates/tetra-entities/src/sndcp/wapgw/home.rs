//! Local XHTML-MP pages: the home page (station status, URL and search forms, bookmarks) and the
//! short notice pages used for errors and refusals.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sndcp::wap_status::{WapStatusSnapshot, compact_uptime, escape_xhtml_text};

pub(crate) const PROLOG: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";
pub(crate) const DOCTYPE: &str =
    "<!DOCTYPE html PUBLIC \"-//WAPFORUM//DTD XHTML Mobile 1.0//EN\" \"http://www.wapforum.org/DTD/xhtml-mobile10.dtd\">";
pub(crate) const HTML_OPEN: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\">";

/// The home page is shown on every browser start: keep it within two SAR packets (with an MTU of
/// 576, 1080 bytes of body after the short WSP header).
pub const HOME_MAX_BYTES: usize = 1000;

/// What the requesting radio may do beyond the local pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browse {
    Allowed,
    /// `[wap.browse]` is off.
    Disabled,
    /// Browsing is on, but not for this ISSI.
    NotListed,
}

/// Base URL of the gateway, used for absolute links back home.
pub fn home_url(gateway: Ipv4Addr) -> String {
    format!("http://{gateway}/")
}

/// Percent-encode a query value (everything but the RFC 3986 unreserved characters).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Action of the weather forms: `/wx/N`, with an N no earlier page had. The MXP600 keeps its own
/// copy of the pages it got before they were sent no-cache, and when a form builds one of those
/// URLs again ("/wx?l=Murcia") it shows that copy without asking the gateway: no request leaves
/// the radio. With N in the path every submission is a URL the radio has never seen, whether or
/// not it sends hidden fields.
fn wx_form_action() -> String {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // From the clock, so a restart does not hand out old values, and never twice the same.
    let previous = LAST
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| Some(last.max(now) + 1))
        .unwrap_or_default();
    format!("/wx/{}", previous.max(now) + 1)
}

/// Label of a bookmark: its host.
fn bookmark_label(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty()).unwrap_or(url)
}

fn document(full: bool, title: &str, base: Option<&str>, body: &str) -> String {
    let (prolog, doctype) = if full { (PROLOG, DOCTYPE) } else { ("", "") };
    let base = base.map(|b| format!("<base href=\"{b}\"/>")).unwrap_or_default();
    format!("{prolog}{doctype}{HTML_OPEN}<head><title>{title}</title>{base}</head><body>{body}</body></html>")
}

pub fn home_page(s: &WapStatusSnapshot, browse: Browse, gateway: Ipv4Addr, bookmarks: &[String], budget: usize) -> String {
    let budget = budget.min(HOME_MAX_BYTES);
    let title = escape_xhtml_text(&s.title);
    let base = home_url(gateway);
    let status = format!(
        "<p><b>{title}</b> {}<br/>Estado: {}<br/>Radios {} SDS {}<br/>Activa {}</p>",
        escape_xhtml_text(&s.stack_version),
        escape_xhtml_text(&s.service_state),
        s.registered_ms,
        s.queued_sds,
        compact_uptime(s.uptime_secs)
    );
    let browse_html = match browse {
        Browse::Allowed => "<form action=\"/go\" method=\"get\"><p>URL <input type=\"text\" name=\"u\" size=\"12\"/>\
             <input type=\"submit\" value=\"Ir\"/></p></form>\
             <form action=\"/s\" method=\"get\"><p>Buscar <input type=\"text\" name=\"q\" size=\"12\"/>\
             <input type=\"submit\" value=\"Buscar\"/></p></form>"
            .to_string(),
        Browse::Disabled => "<p>Navegación desactivada.</p>".to_string(),
        Browse::NotListed => "<p>Esta radio no tiene permiso para navegar.</p>".to_string(),
    };
    let bookmarks_html = if browse == Browse::Allowed && !bookmarks.is_empty() {
        let links: Vec<String> = bookmarks
            .iter()
            .map(|url| {
                format!(
                    "<a href=\"/go?u={}\">{}</a>",
                    percent_encode(url),
                    escape_xhtml_text(bookmark_label(url))
                )
            })
            .collect();
        format!("<p>{}</p>", links.join("<br/>"))
    } else {
        String::new()
    };
    let status_link = if browse == Browse::Allowed {
        "<p><a href=\"/status.xhtml\">Estado de la BS</a> | <a href=\"/wx\">Tiempo</a><br/>\
         <a href=\"/rid\">Consulta en radioid.net</a></p>"
    } else {
        "<p><a href=\"/status.xhtml\">Estado de la BS</a></p>"
    };

    let candidates = [
        document(
            true,
            &title,
            Some(&base),
            &format!("{status}{browse_html}{bookmarks_html}{status_link}"),
        ),
        document(true, &title, Some(&base), &format!("{status}{browse_html}{status_link}")),
        document(false, &title, Some(&base), &format!("{status}{browse_html}")),
        document(false, &title, None, &status),
    ];
    let smallest = candidates[candidates.len() - 1].clone();
    candidates.into_iter().find(|page| page.len() <= budget).unwrap_or(smallest)
}

/// Weather page (`/wx`): the lines of the last answer, if any, and the forms for a place or an
/// airport.
pub fn wx_page(result: &[String], home: &str, budget: usize) -> String {
    let result = if result.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = result.iter().map(|line| escape_xhtml_text(line)).collect();
        format!("<p>{}</p>", lines.join("<br/>"))
    };
    let action = wx_form_action();
    let forms = format!(
        "<form action=\"{action}\" method=\"get\"><p>Lugar <input type=\"text\" name=\"l\" size=\"10\"/>\
         <input type=\"submit\" value=\"Ver\"/></p></form>\
         <form action=\"{action}\" method=\"get\"><p>METAR <input type=\"text\" name=\"i\" size=\"4\"/>\
         <input type=\"submit\" value=\"Ver\"/></p></form>"
    );
    let link = format!("<p><a href=\"{home}\">Inicio</a></p>");
    let full = document(true, "Tiempo", Some(home), &format!("{result}{forms}{link}"));
    if full.len() <= budget {
        return full;
    }
    let short = document(false, "Tiempo", Some(home), &format!("{result}{forms}"));
    if short.len() <= budget {
        return short;
    }
    document(false, "Tiempo", None, &result)
}

/// radioid.net page (`/rid`): the answer, if any (markup, already escaped), the form for a
/// callsign or a DMR ID and a link home.
pub fn rid_page(result_html: Option<&str>, home: &str, budget: usize) -> String {
    let result = result_html.unwrap_or_default();
    let form = "<form action=\"/rid\" method=\"get\"><p>Indicativo o ID \
         <input type=\"text\" name=\"q\" size=\"10\" maxlength=\"10\"/>\
         <input type=\"submit\" value=\"Buscar\"/></p></form>";
    let link = format!("<p><a href=\"{home}\">Inicio</a></p>");
    let full = document(true, "radioid.net", Some(home), &format!("{result}{form}{link}"));
    if full.len() <= budget {
        return full;
    }
    let short = document(false, "radioid.net", Some(home), &format!("{result}{form}"));
    if short.len() <= budget {
        return short;
    }
    document(false, "radioid.net", Some(home), result)
}

/// A short page with a title, a sentence and a link home.
pub fn notice_page(title: &str, text: &str, home: &str, budget: usize) -> String {
    let title = escape_xhtml_text(title);
    let text = escape_xhtml_text(text);
    let link = format!("<p><a href=\"{home}\">Inicio</a></p>");
    let body = format!("<p><b>{title}</b><br/>{text}</p>{link}");
    let full = document(true, &title, None, &body);
    if full.len() <= budget {
        return full;
    }
    let short = document(false, &title, None, &body);
    if short.len() <= budget {
        return short;
    }
    document(false, &title, None, &format!("<p>{title}</p>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "FlowStation".to_string(),
            stack_version: "v0.4.0".to_string(),
            service_state: "OK".to_string(),
            registered_ms: 3,
            active_calls: 0,
            active_group_calls: 0,
            active_private_calls: 0,
            queued_sds: 1,
            uptime_secs: 3725,
            last_activity: None,
            health_summary: Some("OK".to_string()),
            health_lines: Vec::new(),
            radio_lines: Vec::new(),
            call_lines: Vec::new(),
        }
    }

    fn bookmarks() -> Vec<String> {
        vec![
            "http://68k.news/".to_string(),
            "http://wiby.me/".to_string(),
            "http://text.npr.org/".to_string(),
        ]
    }

    fn assert_well_formed(page: &str) {
        let mut reader = quick_xml::Reader::from_str(page);
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("not well-formed ({e}): {page}"),
            }
        }
        // quick-xml does not match end tags against start tags by default: count them.
        let opens = page.matches('<').count() - page.matches("</").count() - page.matches("/>").count();
        let closes = page.matches("</").count();
        let decls = page.matches("<?").count() + page.matches("<!").count();
        assert_eq!(opens - decls, closes, "unbalanced tags: {page}");
    }

    #[test]
    fn home_fits_budget_and_is_wellformed() {
        let gw = Ipv4Addr::new(10, 0, 0, 1);
        let page = home_page(&snapshot(), Browse::Allowed, gw, &bookmarks(), 8192);
        assert!(page.len() <= HOME_MAX_BYTES, "{} bytes", page.len());
        assert!(page.contains("<base href=\"http://10.0.0.1/\"/>"));
        assert!(page.contains("action=\"/go\"") && page.contains("action=\"/s\""));
        assert!(page.contains("<a href=\"/go?u=http%3A%2F%2F68k.news%2F\">68k.news</a>"));
        assert!(page.contains("Radios 3 SDS 1"));
        assert!(page.contains("<a href=\"/rid\">Consulta en radioid.net</a>"), "{page}");
        assert_well_formed(&page);

        // The longest station values still leave room for the bookmarks and every link.
        let mut worst = snapshot();
        worst.service_state = "CRÍTICO".to_string();
        worst.stack_version = "v0.4.10".to_string();
        worst.registered_ms = 999;
        worst.queued_sds = 999;
        worst.uptime_secs = 99 * 86_400 + 23 * 3_600;
        let page = home_page(&worst, Browse::Allowed, gw, &bookmarks(), 8192);
        assert!(page.len() <= HOME_MAX_BYTES, "{} bytes", page.len());
        assert!(
            page.contains("text.npr.org") && page.contains("/rid") && page.contains("/wx"),
            "{page}"
        );

        // One datagram without SAR (MTU 296): the page sheds the extras but stays valid.
        let small = home_page(&snapshot(), Browse::Allowed, gw, &bookmarks(), 230);
        assert!(small.len() <= 230, "{} bytes: {small}", small.len());
        assert_well_formed(&small);
    }

    #[test]
    fn browse_disabled_shows_notice() {
        let page = home_page(&snapshot(), Browse::Disabled, Ipv4Addr::new(10, 0, 0, 1), &bookmarks(), 8192);
        assert!(page.contains("Navegación desactivada"));
        assert!(!page.contains("<form") && !page.contains("/go?u="));
        assert_well_formed(&page);
    }

    #[test]
    fn unauthorized_issi_shows_notice() {
        let page = home_page(&snapshot(), Browse::NotListed, Ipv4Addr::new(10, 0, 0, 1), &bookmarks(), 8192);
        assert!(page.contains("no tiene permiso"));
        assert!(!page.contains("<form"));
        assert_well_formed(&page);
    }

    #[test]
    fn notice_page_escapes_and_shrinks() {
        let page = notice_page("A<B", "x & y", "http://10.0.0.1/", 4096);
        assert!(page.contains("A&lt;B") && page.contains("x &amp; y"));
        assert!(page.contains("<a href=\"http://10.0.0.1/\">Inicio</a>"));
        assert_well_formed(&page);
        let tiny = notice_page("Error", "texto largo", "http://10.0.0.1/", 150);
        assert!(tiny.len() <= 150, "{} bytes", tiny.len());
        assert_well_formed(&tiny);
    }

    #[test]
    fn wx_page_has_forms_and_fits() {
        let lines = ["Madrid".to_string(), "Soleado <25 °C>".to_string()];
        let page = wx_page(&lines, "http://10.0.0.1/", 4096);
        assert!(page.contains("<p>Madrid<br/>Soleado &lt;25 °C&gt;</p>") && page.contains("name=\"i\""));
        assert_well_formed(&page);
        let small = wx_page(&[], "http://10.0.0.1/", 500);
        assert!(small.len() <= 500 && small.contains("name=\"l\""), "{small}");
        assert_well_formed(&small);
    }

    /// Actions of the weather forms of a page.
    fn wx_actions(page: &str) -> Vec<String> {
        page.split("<form action=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap_or_default().to_string())
            .collect()
    }

    /// 06-10, MXP600 on the .75: for "Murcia" and "Alguazas" the radio opened no WTP transaction
    /// at all (its next GET, of the home page, carried "/wx?l=Murcia" as Referer and the very next
    /// TID): it showed its own copy of a page with that URL. Every weather form now builds a URL
    /// that no earlier page could have produced.
    #[test]
    fn weather_forms_build_a_new_url_every_time() {
        let home = "http://10.0.0.1/";
        let first = wx_page(&[], home, 4096);
        let second = wx_page(&["Murcia".to_string()], home, 4096);
        let (a, b) = (wx_actions(&first), wx_actions(&second));
        // The two forms (place and METAR) of a page go to the same "/wx/N".
        assert_eq!(a.len(), 2, "{first}");
        assert_eq!(a[0], a[1]);
        assert_eq!(b.len(), 2, "{second}");
        assert_ne!(a[0], b[0], "a new page, a new URL for the same text");
        let n: u64 = a[0].strip_prefix("/wx/").unwrap().parse().unwrap();
        assert!(n > 1_700_000_000, "from the clock: {}", a[0]);
        assert!(!first.contains("type=\"hidden\""), "{first}");
        assert_well_formed(&first);
        // The radioid.net form is unchanged: it was never sent cacheable.
        assert!(rid_page(None, home, 4096).contains("<form action=\"/rid\" method=\"get\">"));
    }

    const GEO_MADRID: &str = r#"{"results":[{"id":3117735,"name":"Madrid","latitude":40.4165,"longitude":-3.70256,"elevation":665.0,"feature_code":"PPLC","country_code":"ES","admin1_id":3117732,"admin2_id":6355233,"admin3_id":6359304,"timezone":"Europe/Madrid","population":3255944,"postcodes":["28706","28754"],"country_id":2510769,"country":"España","admin1":"Comunidad Autónoma de Madrid","admin2":"Provincia de Madrid","admin3":"Madrid"},{"id":6544494,"name":"Madrid Centro","latitude":40.41831,"longitude":-3.70275,"elevation":673.0,"feature_code":"PPLX","country_code":"ES","admin1_id":3117732,"admin2_id":6355233,"admin3_id":6359304,"admin4_id":11549889,"timezone":"Europe/Madrid","population":149718,"country_id":2510769,"country":"España","admin1":"Comunidad Autónoma de Madrid","admin2":"Provincia de Madrid","admin3":"Madrid","admin4":"Centro"}],"generationtime_ms":0.3234148}"#;
    const FORECAST_MADRID: &str = r#"{"latitude":40.39599,"longitude":-3.6793447,"generationtime_ms":0.2034902572631836,"utc_offset_seconds":7200,"timezone":"Europe/Madrid","timezone_abbreviation":"GMT+2","elevation":651.0,"current_units":{"time":"iso8601","interval":"seconds","temperature_2m":"°C","relative_humidity_2m":"%","apparent_temperature":"°C","weather_code":"wmo code","wind_speed_10m":"km/h","wind_direction_10m":"°","wind_gusts_10m":"km/h"},"current":{"time":"2026-10-06T21:00","interval":900,"temperature_2m":17.0,"relative_humidity_2m":91,"apparent_temperature":18.0,"weather_code":95,"wind_speed_10m":5.6,"wind_direction_10m":153,"wind_gusts_10m":24.1},"daily_units":{"time":"iso8601","weather_code":"wmo code","temperature_2m_max":"°C","temperature_2m_min":"°C","precipitation_probability_max":"%"},"daily":{"time":["2026-10-06","2026-10-07"],"weather_code":[95,95],"temperature_2m_max":[19.8,22.0],"temperature_2m_min":[16.3,14.3],"precipitation_probability_max":[95,80]}}"#;
    const GEO_MURCIA: &str = r#"{"results":[{"id":2513416,"name":"Murcia","latitude":37.98704,"longitude":-1.13004,"elevation":53.0,"feature_code":"PPLA","country_code":"ES","admin1_id":2513413,"admin2_id":6355234,"admin3_id":6362997,"timezone":"Europe/Madrid","population":471982,"postcodes":["30005","30010"],"country_id":2510769,"country":"España","admin1":"Región de Murcia","admin2":"Provincia de Murcia","admin3":"Murcia"},{"id":6299341,"name":"Aeropuerto de Murcia-San Javier","latitude":37.77497,"longitude":-0.81239,"elevation":3.0,"feature_code":"AIRP","country_code":"ES","admin1_id":2513413,"admin2_id":6355234,"admin3_id":6359539,"timezone":"Europe/Madrid","country_id":2510769,"country":"España","admin1":"Región de Murcia","admin2":"Provincia de Murcia","admin3":"San Javier"}],"generationtime_ms":0.23126602}"#;
    const FORECAST_MURCIA: &str = r#"{"latitude":37.991886,"longitude":-1.1586335,"generationtime_ms":0.2796649932861328,"utc_offset_seconds":7200,"timezone":"Europe/Madrid","timezone_abbreviation":"GMT+2","elevation":46.0,"current_units":{"time":"iso8601","interval":"seconds","temperature_2m":"°C","relative_humidity_2m":"%","apparent_temperature":"°C","weather_code":"wmo code","wind_speed_10m":"km/h","wind_direction_10m":"°","wind_gusts_10m":"km/h"},"current":{"time":"2026-10-06T21:00","interval":900,"temperature_2m":24.1,"relative_humidity_2m":74,"apparent_temperature":26.9,"weather_code":3,"wind_speed_10m":4.8,"wind_direction_10m":153,"wind_gusts_10m":15.5},"daily_units":{"time":"iso8601","weather_code":"wmo code","temperature_2m_max":"°C","temperature_2m_min":"°C","precipitation_probability_max":"%"},"daily":{"time":["2026-10-06","2026-10-07"],"weather_code":[61,80],"temperature_2m_max":[28.1,28.5],"temperature_2m_min":[19.8,20.0],"precipitation_probability_max":[30,23]}}"#;
    const GEO_ALGUAZAS: &str = r#"{"results":[{"id":2522000,"name":"Alguazas","latitude":38.05356,"longitude":-1.25051,"elevation":100.0,"feature_code":"PPLA3","country_code":"ES","admin1_id":2513413,"admin2_id":6355234,"admin3_id":6359512,"timezone":"Europe/Madrid","population":7725,"postcodes":["30560","30561"],"country_id":2510769,"country":"España","admin1":"Región de Murcia","admin2":"Provincia de Murcia","admin3":"Alguazas"}],"generationtime_ms":0.6401539}"#;
    const FORECAST_ALGUAZAS: &str = r#"{"latitude":38.05062,"longitude":-1.2699038,"generationtime_ms":0.5398988723754883,"utc_offset_seconds":7200,"timezone":"Europe/Madrid","timezone_abbreviation":"GMT+2","elevation":101.0,"current_units":{"time":"iso8601","interval":"seconds","temperature_2m":"°C","relative_humidity_2m":"%","apparent_temperature":"°C","weather_code":"wmo code","wind_speed_10m":"km/h","wind_direction_10m":"°","wind_gusts_10m":"km/h"},"current":{"time":"2026-10-06T21:00","interval":900,"temperature_2m":23.8,"relative_humidity_2m":71,"apparent_temperature":26.1,"weather_code":3,"wind_speed_10m":5.2,"wind_direction_10m":164,"wind_gusts_10m":13.0},"daily_units":{"time":"iso8601","weather_code":"wmo code","temperature_2m_max":"°C","temperature_2m_min":"°C","precipitation_probability_max":"%"},"daily":{"time":["2026-10-06","2026-10-07"],"weather_code":[61,45],"temperature_2m_max":[26.7,28.1],"temperature_2m_min":[20.0,19.7],"precipitation_probability_max":[33,23]}}"#;

    /// The three places of the field test of 06-10 (the radio showed Madrid; Murcia and Alguazas
    /// never left it), from the real replies of that evening (postcodes cut): the same kind of
    /// page for the three, complete, with every line, only Latin-1 text and well within two SAR
    /// packets, like the pages the radio shows.
    #[test]
    fn weather_pages_of_the_field_test() {
        use crate::net_dashboard::wx_service::{forecast_lines_es, place_from_body};
        let home = "http://10.0.0.1/";
        // The .75: Client-SDU 8192, minus the largest Reply header.
        let budget = 8192 - super::super::wsp::REPLY_OVERHEAD_MAX;
        for (asked, geo, forecast, label) in [
            ("Madrid", GEO_MADRID, FORECAST_MADRID, "Madrid"),
            ("Murcia", GEO_MURCIA, FORECAST_MURCIA, "Murcia"),
            ("Alguazas", GEO_ALGUAZAS, FORECAST_ALGUAZAS, "Alguazas (Murcia)"),
        ] {
            let place = place_from_body(geo, asked, true, None).unwrap().unwrap();
            assert_eq!(place.label, label);
            let lines = forecast_lines_es(&place, forecast).unwrap();
            assert_eq!(lines.len(), 7, "{asked}: {lines:?}");
            let page = wx_page(&lines, home, budget);
            assert_well_formed(&page);
            assert!(page.starts_with(PROLOG), "{asked}: the full page");
            assert!(page.contains(&format!("<body><p>{label}<br/>")), "{page}");
            assert!(page.contains("(hora local)</p><form action=\"/wx/"), "{page}");
            assert!(page.ends_with("<a href=\"http://10.0.0.1/\">Inicio</a></p></body></html>"));
            assert!(page.len() <= HOME_MAX_BYTES, "{asked}: {} bytes", page.len());
            assert!(page.chars().all(|c| u32::from(c) < 0x100), "{asked}: {page}");
            assert_eq!(wx_actions(&page).len(), 2, "{page}");
        }
        let murcia = place_from_body(GEO_MURCIA, "Murcia", true, None).unwrap().unwrap();
        assert_eq!(
            forecast_lines_es(&murcia, FORECAST_MURCIA).unwrap(),
            [
                "Murcia",
                "Cubierto, 24 °C",
                "Sensación 27 °C, humedad 74 %",
                "Viento SE 5 km/h, rachas 16 km/h",
                "Hoy: lluvia débil, 20 a 28 °C, lluvia 30 %",
                "Mañana: chubascos débiles, 20 a 29 °C, lluvia 23 %",
                "Dato de las 21:00 (hora local)",
            ]
        );
    }

    #[test]
    fn rid_page_fits_page_bytes() {
        use crate::sndcp::wapgw::rid::{self, RidFound, RidQuery, samples};
        let home = "http://10.0.0.1/";
        let form = rid_page(None, home, 4096);
        assert!(form.contains("action=\"/rid\"") && form.contains("name=\"q\"") && form.contains("Indicativo o ID"));
        assert_well_formed(&form);
        let asked = RidQuery::Callsign("EA5GVK".to_string());
        let list = RidFound::Users(rid::parse_users(samples::USERS_EA5GVK).unwrap());
        let one = RidFound::Users(rid::parse_users(samples::USER_2145007).unwrap());
        let repeater = RidFound::Repeaters(rid::parse_repeaters(samples::REPEATER_ED3ZAQ).unwrap());
        for found in [&list, &one, &repeater] {
            let page = rid_page(Some(&rid::render(found, &asked, rid::MAX_ROWS)), home, 1200);
            assert!(page.len() <= 1200, "{} bytes: {page}", page.len());
            assert!(page.contains("name=\"q\"") && page.contains(">Inicio</a>"), "{page}");
            assert_well_formed(&page);
        }
        // Eight rows, the most a list shows, still fit with the form.
        let eight = RidFound::Users(
            (0..12)
                .map(|i| rid::RidUser {
                    id: 2_145_000 + i,
                    callsign: "EA5GVK".to_string(),
                    name: "Joaquin".to_string(),
                    place: "Alguazas, Murcia (Spain)".to_string(),
                    last_heard: "Mon, 05 Oct 2026 15:26:19 GMT".to_string(),
                    last_tg: "214".to_string(),
                    last_master: "OBP-PORTUG".to_string(),
                })
                .collect(),
        );
        let page = rid_page(Some(&rid::render(&eight, &asked, rid::MAX_ROWS)), home, 1200);
        assert!(page.len() <= 1200 && page.contains("… y 4 más"), "{} bytes: {page}", page.len());
        assert_well_formed(&page);
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("http://a.b/c?d=e f"), "http%3A%2F%2Fa.b%2Fc%3Fd%3De%20f");
        assert_eq!(bookmark_label("http://text.npr.org/x"), "text.npr.org");
    }
}
