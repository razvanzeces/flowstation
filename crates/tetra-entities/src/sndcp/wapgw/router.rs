//! Maps the URI of a WSP GET to a local page or to a request for the fetcher.
//!
//! Local (relative URI, or the gateway's own address as host):
//! - `/`: home page;
//! - `/status.xhtml` (also `/status`, `/status.html`) and `/status.wml`, with `?s=N` for a
//!   section: station status (Nexus-BS renderers);
//! - `/go?u=URL`, `/s?q=QUERY`, `/p/...`, `/l/...`, `/wx?i=ICAO` / `/wx?l=PLACE` (also under
//!   `/wx/N`, where the weather forms go) and `/rid?q=CALLSIGN-OR-ID`: Internet side, only for
//!   the radios allowed to browse (`/wx` and `/rid` alone are the local forms).
//!
//! Any other host is a page the terminal asks for through the gateway (WSP proxy): Internet side
//! as well, with the same permission.

use tetra_config::bluestation::CfgWap;

use super::fetcher::{FetchTarget, Page};
use super::home::{self, Browse};
use super::rid;
use super::wsp::{ContentKind, status};
use crate::sndcp::wap_status::{
    WAP_STATUS_SECTOR_QUERY, WapStatusSnapshot, escape_xhtml_text, render_wml_status_browser_index, render_wml_status_browser_sector,
    render_wml2_status_sector,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Page(Page),
    Fetch(FetchTarget),
}

pub struct RouteCtx<'a> {
    pub cfg: &'a CfgWap,
    pub issi: u32,
    /// Largest body the reply may carry.
    pub budget: usize,
    pub snapshot: &'a dyn Fn() -> WapStatusSnapshot,
}

impl RouteCtx<'_> {
    fn browse(&self) -> Browse {
        if !self.cfg.browse.enabled {
            Browse::Disabled
        } else if self.cfg.browse_allowed(self.issi) {
            Browse::Allowed
        } else {
            Browse::NotListed
        }
    }

    fn notice(&self, status: u8, title: &str, text: &str) -> Route {
        let body = home::notice_page(title, text, &home::home_url(self.cfg.gateway_ipv4), self.budget);
        Route::Page(Page {
            status,
            kind: ContentKind::Xhtml,
            body: body.into_bytes(),
        })
    }

    /// The Internet side, if this radio may use it.
    fn fetch(&self, target: FetchTarget) -> Route {
        match self.browse() {
            Browse::Allowed => Route::Fetch(target),
            Browse::Disabled => self.notice(status::FORBIDDEN, "Sin Internet", "Navegación desactivada."),
            Browse::NotListed => self.notice(status::FORBIDDEN, "Sin permiso", "Esta radio no tiene permiso para navegar."),
        }
    }
}

struct Uri<'a> {
    scheme: Option<&'a str>,
    host: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
}

fn split_uri(uri: &str) -> Uri<'_> {
    let uri = uri.trim();
    let uri = uri.split('#').next().unwrap_or(uri);
    let (scheme, host, rest) = match uri.split_once("://") {
        Some((scheme, rest)) => {
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            (Some(scheme), Some(&rest[..end]), &rest[end..])
        }
        None => (None, None, uri),
    };
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    Uri {
        scheme,
        host,
        path: if path.is_empty() { "/" } else { path },
        query,
    }
}

fn is_local_host(host: &str, cfg: &CfgWap) -> bool {
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host).to_ascii_lowercase();
    host == cfg.gateway_ipv4.to_string() || host == "localhost" || host == "127.0.0.1"
}

fn is_http(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

/// Value of `key` in a form-encoded query ('+' is a space).
pub fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query_bytes(query, key).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Text typed in one of the gateway's own forms: UTF-8, or Windows-1252 when its octets are not
/// UTF-8 (a terminal that sends the form in Latin-1: "A Coru%F1a" is "A Coruña").
pub fn form_param(query: Option<&str>, key: &str) -> Option<String> {
    let bytes = query_bytes(query, key)?;
    Some(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(e.as_bytes()).0.into_owned(),
    })
}

/// Octets of the value of `key` in a form-encoded query.
fn query_bytes(query: Option<&str>, key: &str) -> Option<Vec<u8>> {
    let pair = query?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then_some(v)
    })?;
    let bytes = pair.as_bytes();
    let hex = |i: usize| bytes.get(i).and_then(|&b| (b as char).to_digit(16));
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'+', _, _) => out.push(b' '),
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 2;
            }
            (b, _, _) => out.push(b),
        }
        i += 1;
    }
    Some(out)
}

/// The URL typed in the home page's form: http/https only, `http://` when no scheme is given.
/// Octets that were not UTF-8 (decoded as U+FFFD) make it invalid.
fn typed_url(u: &str) -> Option<String> {
    let u = u.trim();
    if u.is_empty() || u.chars().any(|c| c.is_whitespace() || c == char::REPLACEMENT_CHARACTER) {
        return None;
    }
    match u.split_once("://") {
        Some((scheme, rest)) if is_http(scheme) && !rest.is_empty() => Some(u.to_string()),
        Some(_) => None,
        None => Some(format!("http://{u}")),
    }
}

fn status_page(ctx: &RouteCtx<'_>, wml: bool, section: Option<usize>) -> Route {
    let snapshot = (ctx.snapshot)();
    // XHTML: the sector pages, which carry no meta refresh (the full page reloads itself every
    // 8 s, which would keep the data slot busy while a radio leaves it open).
    let rendered = match (wml, section) {
        (false, section) => render_wml2_status_sector(&snapshot, ctx.budget, section.unwrap_or(0)),
        (true, None) => render_wml_status_browser_index(&snapshot, ctx.budget),
        (true, Some(s)) => render_wml_status_browser_sector(&snapshot, ctx.budget, s),
    };
    match rendered {
        Ok(body) => Route::Page(Page {
            status: status::OK,
            kind: if wml { ContentKind::Wml } else { ContentKind::Xhtml },
            body: body.into_bytes(),
        }),
        Err(e) => {
            tracing::warn!("WAP: status page does not fit {} bytes: {:?}", ctx.budget, e);
            ctx.notice(status::INTERNAL_ERROR, "Error", "La página de estado no cabe.")
        }
    }
}

pub fn route(uri: &str, ctx: &RouteCtx<'_>) -> Route {
    let parts = split_uri(uri);
    if let Some(host) = parts.host
        && !is_local_host(host, ctx.cfg)
    {
        return match parts.scheme {
            Some(scheme) if is_http(scheme) => ctx.fetch(FetchTarget::Url(uri.trim().to_string())),
            _ => ctx.notice(status::BAD_REQUEST, "Error", "Solo se admiten direcciones http:// y https://."),
        };
    }

    let section = query_param(parts.query, WAP_STATUS_SECTOR_QUERY).and_then(|s| s.parse().ok());
    match parts.path {
        "/" | "/index.xhtml" | "/index.html" => {
            let body = home::home_page(
                &(ctx.snapshot)(),
                ctx.browse(),
                ctx.cfg.gateway_ipv4,
                &ctx.cfg.browse.bookmarks,
                ctx.budget,
            );
            Route::Page(Page {
                status: status::OK,
                kind: ContentKind::Xhtml,
                body: body.into_bytes(),
            })
        }
        "/status" | "/status.xhtml" | "/status.html" => status_page(ctx, false, section),
        "/status.wml" => status_page(ctx, true, section),
        "/go" => match query_param(parts.query, "u").as_deref().and_then(typed_url) {
            Some(url) => ctx.fetch(FetchTarget::Url(url)),
            None => ctx.notice(status::BAD_REQUEST, "Error", "Dirección no válida."),
        },
        "/s" => match query_param(parts.query, "q").filter(|q| !q.trim().is_empty()) {
            Some(q) => ctx.fetch(FetchTarget::Search(q.trim().to_string())),
            None => ctx.notice(status::BAD_REQUEST, "Error", "Escribe algo que buscar."),
        },
        path if path.starts_with("/p/") || path.starts_with("/l/") => {
            let doc = match parts.query {
                Some(q) => format!("{path}?{q}"),
                None => path.to_string(),
            };
            ctx.fetch(FetchTarget::Doc(doc))
        }
        // Weather: the forms are local, the answer comes from the Internet side. The forms go to
        // "/wx/N" (see `home::wx_page`): N only makes the URL new for the radio.
        path if path == "/wx" || path.starts_with("/wx/") => {
            let asked = ["i", "l"]
                .iter()
                .any(|k| query_param(parts.query, k).is_some_and(|v| !v.trim().is_empty()));
            if !asked && ctx.browse() == Browse::Allowed {
                Route::Page(Page {
                    status: status::OK,
                    kind: ContentKind::Xhtml,
                    body: home::wx_page(&[], &home::home_url(ctx.cfg.gateway_ipv4), ctx.budget).into_bytes(),
                })
            } else {
                ctx.fetch(FetchTarget::Doc(format!("/wx?{}", parts.query.unwrap_or(""))))
            }
        }
        // radioid.net: the form, and what was typed in it checked here, are local; the lookup
        // comes from the Internet side.
        "/rid" => {
            if ctx.browse() != Browse::Allowed {
                return ctx.fetch(FetchTarget::Doc("/rid".to_string()));
            }
            let local = |result: Option<String>| {
                let home = home::home_url(ctx.cfg.gateway_ipv4);
                Route::Page(Page {
                    status: status::OK,
                    kind: ContentKind::Xhtml,
                    body: home::rid_page(result.as_deref(), &home, ctx.budget).into_bytes(),
                })
            };
            match form_param(parts.query, "q").map(|q| rid::parse_input(&q)) {
                None => local(None),
                Some(Ok(asked)) => ctx.fetch(FetchTarget::Doc(format!("/rid?q={}", home::percent_encode(&asked.as_text())))),
                Some(Err(sentence)) => local(Some(format!("<p>{}</p>", escape_xhtml_text(&sentence)))),
            }
        }
        _ => ctx.notice(status::NOT_FOUND, "No encontrado", "Esa página no existe en la estación."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "FlowStation".to_string(),
            stack_version: "v0.4.0".to_string(),
            service_state: "OK".to_string(),
            registered_ms: 0,
            active_calls: 0,
            active_group_calls: 0,
            active_private_calls: 0,
            queued_sds: 0,
            uptime_secs: 60,
            last_activity: None,
            health_summary: Some("OK".to_string()),
            health_lines: vec!["service ok".to_string(), "backhaul ok".to_string()],
            radio_lines: Vec::new(),
            call_lines: Vec::new(),
        }
    }

    fn cfg(browse: bool, issis: &[u32]) -> CfgWap {
        let mut cfg = CfgWap {
            enabled: true,
            ..Default::default()
        };
        cfg.browse.enabled = browse;
        cfg.browse.allowed_issis = issis.to_vec();
        cfg
    }

    fn route_for(uri: &str, cfg: &CfgWap, issi: u32, budget: usize) -> Route {
        let snap = snapshot;
        route(
            uri,
            &RouteCtx {
                cfg,
                issi,
                budget,
                snapshot: &snap,
            },
        )
    }

    fn page(route: Route) -> Page {
        match route {
            Route::Page(p) => p,
            Route::Fetch(t) => panic!("expected a page, got fetch {t:?}"),
        }
    }

    #[test]
    fn home_is_local_under_any_local_spelling() {
        let cfg = cfg(false, &[]);
        for uri in ["/", "http://10.0.0.1/", "http://10.0.0.1", "http://10.0.0.1:9201/index.html", ""] {
            let p = page(route_for(uri, &cfg, 1, 900));
            assert_eq!(p.status, status::OK, "{uri}");
            assert!(String::from_utf8(p.body).unwrap().contains("Estado"), "{uri}");
        }
    }

    #[test]
    fn unknown_local_path_404() {
        let p = page(route_for("/nope", &cfg(true, &[1]), 1, 900));
        assert_eq!(p.status, status::NOT_FOUND);
    }

    #[test]
    fn browse_needs_permission() {
        let allowed = cfg(true, &[2260618]);
        assert_eq!(
            route_for("/go?u=wiby.me", &allowed, 2260618, 900),
            Route::Fetch(FetchTarget::Url("http://wiby.me".to_string()))
        );
        assert_eq!(
            route_for("/s?q=radio+tetra%21", &allowed, 2260618, 900),
            Route::Fetch(FetchTarget::Search("radio tetra!".to_string()))
        );
        assert_eq!(
            route_for("http://68k.news/article.php?a=1", &allowed, 2260618, 900),
            Route::Fetch(FetchTarget::Url("http://68k.news/article.php?a=1".to_string()))
        );
        assert_eq!(
            route_for("/p/3/2?x=1", &allowed, 2260618, 900),
            Route::Fetch(FetchTarget::Doc("/p/3/2?x=1".to_string()))
        );
        let p = page(route_for("/go?u=wiby.me", &allowed, 1234, 900));
        assert_eq!(p.status, status::FORBIDDEN, "unlisted ISSI");
        let p = page(route_for("http://68k.news/", &cfg(false, &[2260618]), 2260618, 900));
        assert_eq!(p.status, status::FORBIDDEN, "browsing off");
    }

    #[test]
    fn weather_form_local_answer_fetched() {
        let allowed = cfg(true, &[1]);
        let form = page(route_for("/wx", &allowed, 1, 900));
        assert_eq!(form.status, status::OK);
        assert!(String::from_utf8(form.body).unwrap().contains("name=\"i\""));
        assert_eq!(
            route_for("/wx?i=LEMD", &allowed, 1, 900),
            Route::Fetch(FetchTarget::Doc("/wx?i=LEMD".to_string()))
        );
        assert_eq!(page(route_for("/wx", &allowed, 2, 900)).status, status::FORBIDDEN);
        assert_eq!(page(route_for("/wx?l=Madrid", &allowed, 2, 900)).status, status::FORBIDDEN);
    }

    /// The weather requests as the MXP600 sends them (06-10, decoded from the .75's journal): an
    /// absolute URI on the gateway's address with the text as typed, to the form's "/wx" before,
    /// to its "/wx/N" now; N takes no part in the answer.
    #[test]
    fn weather_requests_as_the_mxp600_sends_them() {
        let allowed = cfg(true, &[1]);
        for (uri, doc) in [
            ("http://10.0.0.1/wx?l=Madrid", "/wx?l=Madrid"),
            ("http://10.0.0.1/wx?i=Lelc", "/wx?i=Lelc"),
            ("http://10.0.0.1/wx/1791312000?l=Murcia", "/wx?l=Murcia"),
            ("http://10.0.0.1/wx/1791312001?l=Alguazas", "/wx?l=Alguazas"),
            ("http://10.0.0.1/wx/1791312002?l=San+Javier", "/wx?l=San+Javier"),
            ("http://10.0.0.1/wx/1791312003?l=A+Coru%F1a", "/wx?l=A+Coru%F1a"),
            ("http://10.0.0.1/wx/1791312004?i=Lelc", "/wx?i=Lelc"),
        ] {
            assert_eq!(
                route_for(uri, &allowed, 1, 900),
                Route::Fetch(FetchTarget::Doc(doc.to_string())),
                "{uri}"
            );
        }
        // Nothing typed: the forms again, without going out.
        let form = page(route_for("http://10.0.0.1/wx/1791312000?l=", &allowed, 1, 900));
        assert_eq!(form.status, status::OK);
        assert!(String::from_utf8(form.body).unwrap().contains("name=\"l\""));
        // Not for the radios that may not browse.
        assert_eq!(
            page(route_for("http://10.0.0.1/wx/1791312000?l=Murcia", &allowed, 2, 900)).status,
            status::FORBIDDEN
        );
    }

    #[test]
    fn radioid_form_and_checks_local_lookup_fetched() {
        let allowed = cfg(true, &[1]);
        let text = |route: Route| {
            let p = page(route);
            assert_eq!(p.status, status::OK);
            String::from_utf8(p.body).unwrap()
        };
        assert!(text(route_for("/rid", &allowed, 1, 900)).contains("name=\"q\""));
        assert_eq!(
            route_for("/rid?q=+ea5gvk+", &allowed, 1, 900),
            Route::Fetch(FetchTarget::Doc("/rid?q=EA5GVK".to_string()))
        );
        assert_eq!(
            route_for("http://10.0.0.1/rid?q=2145007", &allowed, 1, 900),
            Route::Fetch(FetchTarget::Doc("/rid?q=2145007".to_string()))
        );
        for with_slash in ["/rid?q=EA5GVK%2FP", "/rid?q=EA8%2FEA5GVK"] {
            assert_eq!(
                route_for(with_slash, &allowed, 1, 900),
                Route::Fetch(FetchTarget::Doc("/rid?q=EA5GVK".to_string())),
                "{with_slash}"
            );
        }
        // Refused before any request to radioid.net, with the form again.
        let empty = text(route_for("/rid?q=", &allowed, 1, 900));
        assert!(empty.contains("Escribe un indicativo") && empty.contains("name=\"q\""), "{empty}");
        for bad in ["/rid?q=EA5%25", "/rid?q=%2A", "/rid?q=EA5GVK%2FP%25", "/rid?q=%3Cb%3E"] {
            let body = text(route_for(bad, &allowed, 1, 900));
            assert!(body.contains("no es un indicativo ni un ID válido"), "{bad}: {body}");
        }
        assert!(text(route_for("/rid?q=%3Cb%3E", &allowed, 1, 900)).contains("«&lt;b&gt;»"));
        // Not for the radios that may not browse.
        assert_eq!(page(route_for("/rid", &allowed, 2, 900)).status, status::FORBIDDEN);
        assert_eq!(page(route_for("/rid?q=EA5GVK", &allowed, 2, 900)).status, status::FORBIDDEN);
        assert_eq!(page(route_for("/rid", &cfg(false, &[1]), 1, 900)).status, status::FORBIDDEN);
    }

    #[test]
    fn bad_urls_rejected() {
        let allowed = cfg(true, &[1]);
        assert_eq!(page(route_for("/go?u=ftp%3A%2F%2Fx", &allowed, 1, 900)).status, status::BAD_REQUEST);
        assert_eq!(page(route_for("/go", &allowed, 1, 900)).status, status::BAD_REQUEST);
        assert_eq!(page(route_for("/s?q=+", &allowed, 1, 900)).status, status::BAD_REQUEST);
        assert_eq!(page(route_for("wtai://wp/mc;112", &allowed, 1, 900)).status, status::BAD_REQUEST);
    }

    #[test]
    fn status_pages_fit_the_budget_without_refresh() {
        let cfg = cfg(false, &[]);
        for uri in ["/status.xhtml", "/status", "/status.xhtml?s=1"] {
            let p = page(route_for(uri, &cfg, 1, 4000));
            let body = String::from_utf8(p.body).unwrap();
            assert_eq!(p.status, status::OK, "{uri}");
            // A radio left on the status page must not reload it by itself (air time).
            assert!(!body.contains("refresh"), "{uri}: {body}");
            assert!(body.contains("href=\"/status.xhtml"), "{uri}: navigation between blocks");
        }
        // A smaller reply budget gets a more compact rendering.
        let large = page(route_for("/status.xhtml", &cfg, 1, 4000));
        let small = page(route_for("/status.xhtml", &cfg, 1, large.body.len() - 1));
        assert_eq!((small.status, large.status), (status::OK, status::OK));
        assert!(large.body.len() > small.body.len(), "{} vs {}", large.body.len(), small.body.len());
        let wml = page(route_for("/status.wml?s=1", &cfg, 1, 500));
        assert_eq!(wml.kind, ContentKind::Wml);
    }

    #[test]
    fn typed_url_that_was_not_utf8_is_refused() {
        // WSP hands a Latin-1 URI over percent-encoded; its "ñ" does not decode as UTF-8.
        let p = page(route_for("/go?u=Espa%F1a", &cfg(true, &[1]), 1, 900));
        assert_eq!(p.status, status::BAD_REQUEST);
        assert!(String::from_utf8(p.body).unwrap().contains("Dirección no válida"));
    }

    #[test]
    fn query_decoding() {
        assert_eq!(query_param(Some("a=1&u=http%3A%2F%2Fx.y%2F"), "u").as_deref(), Some("http://x.y/"));
        assert_eq!(query_param(Some("q=%E2%82%AC+5"), "q").as_deref(), Some("€ 5"));
        assert_eq!(query_param(Some("q=100%"), "q").as_deref(), Some("100%"));
        assert_eq!(query_param(None, "q"), None);
    }

    #[test]
    fn form_text_utf8_or_windows_1252() {
        assert_eq!(form_param(Some("l=A+Coru%C3%B1a"), "l").as_deref(), Some("A Coruña"));
        // The same place sent in Latin-1 / Windows-1252.
        assert_eq!(form_param(Some("l=A+Coru%F1a"), "l").as_deref(), Some("A Coruña"));
        assert_eq!(form_param(Some("l=Alcal%E1+de+Henares"), "l").as_deref(), Some("Alcalá de Henares"));
        assert_eq!(form_param(Some("q=%80+5"), "q").as_deref(), Some("€ 5"));
        assert_eq!(form_param(Some("q=EA5GVK"), "q").as_deref(), Some("EA5GVK"));
        assert_eq!(form_param(Some("q="), "q").as_deref(), Some(""));
        assert_eq!(form_param(Some("i=LELC"), "q"), None);
        // The URL form keeps refusing octets that are not UTF-8 (see typed_url).
        assert_eq!(query_param(Some("u=Espa%F1a"), "u").as_deref(), Some("Espa\u{FFFD}a"));
    }
}
