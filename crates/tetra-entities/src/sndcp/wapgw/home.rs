//! Local XHTML-MP pages: the home page (station status, URL and search forms, bookmarks) and the
//! short notice pages used for errors and refusals.

use std::net::Ipv4Addr;

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
    let forms = "<form action=\"/wx\" method=\"get\"><p>Lugar <input type=\"text\" name=\"l\" size=\"10\"/>\
         <input type=\"submit\" value=\"Ver\"/></p></form>\
         <form action=\"/wx\" method=\"get\"><p>METAR <input type=\"text\" name=\"i\" size=\"4\"/>\
         <input type=\"submit\" value=\"Ver\"/></p></form>";
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
