//! The Internet side of the gateway: a small pool of download threads.
//!
//! The stack thread only queues requests and collects replies through bounded channels
//! ([`PoolFetcher`]); it never waits for the network. Each `wap-fetch` thread first leaves the
//! station's real-time scheduling (SCHED_OTHER, nice 10) and only then builds its HTTP client, so
//! the client's own runtime threads inherit the low priority.
//!
//! A download follows redirects by hand, checking every hop against the [`NetPolicy`]; the
//! client's DNS resolver refuses a name that resolves to a non-public address, and the connection
//! goes to the address that was checked. Bodies are capped, decoded to UTF-8 and either passed
//! through (small WML / XHTML-MP) or converted and cut into pages kept in a per-radio document
//! cache. Only the domain of what a radio fetched is logged.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, unbounded};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use reqwest::blocking::Client;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, HeaderMap, HeaderValue, LOCATION};
use tetra_config::bluestation::{CfgWap, CfgWapBrowse};
use url::Url;

use super::convert::{Block, Document, Inline, convert_html, convert_text, trim_wap_markup};
use super::doc_cache::DocCache;
use super::fetcher::{FetchReply, FetchRequest, FetchTarget, Fetcher, Page};
use super::home;
use super::netpolicy::{NetPolicy, Refusal};
use super::paginate::render_page;
use super::rid::{self, RidFound, RidQuery};
use super::router::form_param;
use super::wsp::{ContentKind, status};
use crate::net_dashboard::wx_service;
use crate::sndcp::wap_status::escape_xhtml_text;

/// Requests waiting for a download thread; a full queue answers "busy" at once.
const QUEUE_LEN: usize = 8;
const ACCEPT_VALUE: &str =
    "application/vnd.wap.xhtml+xml, application/xhtml+xml;q=0.9, text/vnd.wap.wml;q=0.8, text/html;q=0.7, text/plain;q=0.5";
/// Meta refreshes followed per request (on top of the HTTP redirects).
const MAX_REFRESHES: usize = 2;
/// A page with less text than this and a meta refresh is only a redirect.
const REFRESH_TEXT_CHARS: usize = 200;

/// Leave the station's SCHED_FIFO and run at nice 10 (Linux; on Linux the nice value is per
/// thread). Threads created afterwards inherit both.
fn background_priority() {
    super::leave_realtime_scheduling();
    #[cfg(target_os = "linux")]
    // SAFETY: one syscall on the calling thread.
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// DNS resolver of the download client: every address a name resolves to must pass the policy.
struct PolicyResolver {
    policy: NetPolicy,
}

impl Resolve for PolicyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            // getaddrinfo blocks: keep it off the client's runtime thread so its timers still run.
            let addrs: Vec<SocketAddr> =
                tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs().map(|a| a.collect::<Vec<_>>())).await??;
            let ips: Vec<IpAddr> = addrs.iter().map(SocketAddr::ip).collect();
            if ips.is_empty() {
                return Err("no address".into());
            }
            policy.check_resolved(&ips)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

#[derive(Debug)]
enum FetchError {
    Refused(Refusal),
    Timeout,
    Connect,
    TooManyRedirects,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Refused(r) => write!(f, "refused: {r}"),
            FetchError::Timeout => write!(f, "timed out"),
            FetchError::Connect => write!(f, "cannot connect"),
            FetchError::TooManyRedirects => write!(f, "too many redirects"),
            FetchError::Other(e) => write!(f, "{e}"),
        }
    }
}

fn classify(e: reqwest::Error) -> FetchError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(s) = source {
        if let Some(refusal) = s.downcast_ref::<Refusal>() {
            return FetchError::Refused(refusal.clone());
        }
        source = s.source();
    }
    if e.is_timeout() {
        FetchError::Timeout
    } else if e.is_connect() {
        FetchError::Connect
    } else {
        FetchError::Other(e.without_url().to_string())
    }
}

struct Downloaded {
    url: Url,
    status: u16,
    content_type: String,
    body: Vec<u8>,
    truncated: bool,
}

/// `charset` parameter of a Content-Type.
fn charset_param(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let value = &lower[lower.find("charset=")? + 8..];
    let value = value.trim_start_matches(['"', '\'']);
    let end = value
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')))
        .unwrap_or(value.len());
    (end > 0).then(|| value[..end].to_string())
}

/// Charset declared inside the document: `<meta charset>`, `<meta http-equiv content>` or the
/// XML declaration, in its first kilobyte.
fn sniff_charset(head: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&head[..head.len().min(1024)]).to_ascii_lowercase();
    if head.starts_with("<?xml")
        && let Some(decl) = head.split("?>").next()
        && let Some(i) = decl.find("encoding=")
    {
        return charset_param(&format!("charset={}", &decl[i + 9..]));
    }
    let i = head.find("<meta")?;
    charset_param(&head[i..])
}

/// The body as UTF-8: BOM, then the Content-Type charset, then the document's own declaration;
/// without any, UTF-8 when it is valid and Windows-1252 otherwise.
fn decode_body(body: &[u8], content_type: &str) -> String {
    if let Some((enc, bom)) = Encoding::for_bom(body) {
        return enc.decode_without_bom_handling(&body[bom..]).0.into_owned();
    }
    let declared = charset_param(content_type)
        .or_else(|| sniff_charset(body))
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        // A document that says UTF-16 in ASCII bytes is not UTF-16.
        .map(|enc| if enc == UTF_16LE || enc == UTF_16BE { UTF_8 } else { enc });
    let enc = declared.unwrap_or_else(|| match std::str::from_utf8(body) {
        Ok(_) => UTF_8,
        // Cut in the middle of a character by the size cap: still UTF-8.
        Err(e) if e.error_len().is_none() => UTF_8,
        Err(_) => WINDOWS_1252,
    });
    enc.decode_without_bom_handling(body).0.into_owned()
}

/// WSP status for an upstream HTTP error status.
fn wsp_status_for_http(code: u16) -> u8 {
    match code {
        400..=415 => 0x40 + (code - 400) as u8,
        500..=505 => 0x60 + (code - 500) as u8,
        500..=599 => status::INTERNAL_ERROR,
        _ => status::BAD_REQUEST,
    }
}

/// A gateway path served by the pool: page of a document, link of a document, the weather or a
/// radioid.net lookup.
#[derive(Debug, PartialEq, Eq)]
enum DocPath<'a> {
    Page { doc: u32, page: usize },
    Link { doc: u32, link: usize, query: Option<&'a str> },
    Weather { query: Option<&'a str> },
    RadioId { query: Option<&'a str> },
}

fn parse_doc_path(path: &str) -> Option<DocPath<'_>> {
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    if path == "/wx" {
        return Some(DocPath::Weather { query });
    }
    if path == "/rid" {
        return Some(DocPath::RadioId { query });
    }
    let mut parts = path.strip_prefix('/')?.split('/');
    let (kind, doc, n) = (parts.next()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    if parts.next().is_some() {
        return None;
    }
    match kind {
        "p" => Some(DocPath::Page { doc, page: n }),
        "l" => Some(DocPath::Link { doc, link: n, query }),
        _ => None,
    }
}

fn notice(status: u8, title: &str, text: &str, req: &FetchRequest) -> Page {
    Page {
        status,
        kind: ContentKind::Xhtml,
        body: home::notice_page(title, text, &req.home, req.budget).into_bytes(),
    }
}

fn error_page(e: &FetchError, req: &FetchRequest) -> Page {
    match e {
        FetchError::Refused(Refusal::Address(_) | Refusal::LocalName) => notice(
            status::FORBIDDEN,
            "Bloqueada",
            "Solo se pueden visitar sitios públicos de Internet.",
            req,
        ),
        FetchError::Refused(Refusal::Domain) => notice(status::FORBIDDEN, "No permitido", "Ese sitio no está permitido.", req),
        FetchError::Refused(Refusal::Scheme | Refusal::Port(_)) => notice(
            status::FORBIDDEN,
            "No permitido",
            "Solo direcciones http y https en los puertos permitidos.",
            req,
        ),
        FetchError::Timeout => notice(status::GATEWAY_TIMEOUT, "Sin respuesta", "El sitio tarda demasiado.", req),
        FetchError::Connect => notice(status::BAD_GATEWAY, "Sin conexión", "No se puede conectar con el sitio.", req),
        FetchError::TooManyRedirects => notice(status::BAD_GATEWAY, "Error", "Demasiadas redirecciones.", req),
        FetchError::Other(_) => notice(status::BAD_GATEWAY, "Error", "No se pudo descargar la página.", req),
    }
}

/// Only the domain goes to the log.
fn host_of(url: &Url) -> String {
    url.host_str().unwrap_or("?").to_string()
}

/// File, in the temporary directory, that keeps the document index across restarts.
const DOC_INDEX_FILE: &str = "flowstation-wap-docs.json";

/// One download thread's state. The document cache is shared by the pool.
struct Worker {
    cfg: CfgWapBrowse,
    policy: NetPolicy,
    docs: Arc<Mutex<DocCache>>,
    client: Option<Client>,
}

enum Presented {
    Page(Page),
    Refresh(Url),
}

impl Worker {
    fn new(cfg: CfgWapBrowse, policy: NetPolicy, docs: Arc<Mutex<DocCache>>) -> Self {
        Self {
            cfg,
            policy,
            docs,
            client: None,
        }
    }

    fn run(mut self, jobs: Receiver<FetchRequest>, replies: Sender<FetchReply>) {
        background_priority();
        while let Ok(req) = jobs.recv() {
            let (page, domain) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(&req))).unwrap_or_else(|_| {
                tracing::error!("WAP: download thread panicked on a request of ISSI {}", req.issi);
                (notice(status::INTERNAL_ERROR, "Error", "Error interno de la pasarela.", &req), None)
            });
            if replies.send(FetchReply { id: req.id, page, domain }).is_err() {
                return; // the gateway is gone
            }
        }
    }

    fn docs(&self) -> std::sync::MutexGuard<'_, DocCache> {
        self.docs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The HTTP client, built on first use (on this thread, after the priority change).
    fn client(&mut self) -> Result<Client, FetchError> {
        if let Some(client) = &self.client {
            return Ok(client.clone());
        }
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static(ACCEPT_VALUE));
        if let Ok(lang) = HeaderValue::from_str(self.cfg.accept_language.trim())
            && !lang.is_empty()
        {
            headers.insert(ACCEPT_LANGUAGE, lang);
        }
        let client = Client::builder()
            .user_agent(self.cfg.user_agent.clone())
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(self.cfg.timeout_secs.min(8)))
            .timeout(Duration::from_secs(self.cfg.timeout_secs))
            .dns_resolver(Arc::new(PolicyResolver {
                policy: self.policy.clone(),
            }))
            .build()
            .map_err(|e| FetchError::Other(e.to_string()))?;
        self.client = Some(client.clone());
        Ok(client)
    }

    fn handle(&mut self, req: &FetchRequest) -> (Page, Option<String>) {
        match &req.target {
            FetchTarget::Url(url) => match Url::parse(url.trim()) {
                Ok(url) => self.browse(req, url),
                Err(_) => (notice(status::BAD_REQUEST, "Error", "Dirección no válida.", req), None),
            },
            FetchTarget::Search(query) => {
                let url = format!("{}{}", self.cfg.search_url.trim(), home::percent_encode(query));
                match Url::parse(&url) {
                    Ok(url) => self.browse(req, url),
                    Err(_) => (notice(status::INTERNAL_ERROR, "Error", "Buscador mal configurado.", req), None),
                }
            }
            FetchTarget::Doc(path) => match parse_doc_path(path) {
                Some(DocPath::Page { doc, page }) => (self.page(req, doc, page), None),
                Some(DocPath::Link { doc, link, query }) => {
                    let target = {
                        let mut docs = self.docs();
                        match docs.get(req.issi, doc, Instant::now()) {
                            Some(d) => Some(d.doc.links.get(link).cloned()),
                            // Expired or from before a restart: the index still knows the links.
                            None => docs.recall(req.issi, doc).map(|i| i.links.get(link).cloned()),
                        }
                    };
                    match target {
                        Some(Some(url)) => match Url::parse(&url) {
                            Ok(mut url) => {
                                if let Some(query) = query {
                                    // A GET form: its fields replace the action's query.
                                    url.set_query(Some(query));
                                }
                                self.browse(req, url)
                            }
                            Err(_) => (notice(status::NOT_FOUND, "No encontrado", "Enlace no válido.", req), None),
                        },
                        Some(None) => (notice(status::NOT_FOUND, "No encontrado", "Ese enlace no existe.", req), None),
                        None => self.reload_back(req),
                    }
                }
                Some(DocPath::Weather { query }) => (self.weather(req, query), None),
                Some(DocPath::RadioId { query }) => (self.radioid(req, query), None),
                None => (notice(status::NOT_FOUND, "No encontrado", "Esa página no existe.", req), None),
            },
        }
    }

    /// A document nothing is known of any more (the radio showed its own old copy of a page):
    /// fetch again the page the link was on, so its links are new.
    fn reload_back(&mut self, req: &FetchRequest) -> (Page, Option<String>) {
        match &req.back {
            Some(back) => self.handle(&FetchRequest {
                target: back.clone(),
                back: None,
                ..req.clone()
            }),
            None => (expired(req), None),
        }
    }

    fn page_bytes(&self, req: &FetchRequest) -> usize {
        self.cfg.page_bytes.min(req.budget)
    }

    fn page(&mut self, req: &FetchRequest, doc: u32, n: usize) -> Page {
        let page_bytes = self.page_bytes(req);
        let mut docs = self.docs();
        if docs.get(req.issi, doc, Instant::now()).is_none() {
            // Expired or from before a restart: fetch its source again and show the same page.
            let source = docs.recall(req.issi, doc).and_then(|i| Url::parse(&i.source).ok());
            drop(docs);
            let Some(source) = source else {
                return self.reload_back(req).0;
            };
            let (first, _) = self.browse(req, source);
            if n <= 1 {
                return first;
            }
            let docs = self.docs();
            let again = docs
                .latest(req.issi)
                .and_then(|d| render_page(&d.doc, d.id, n, page_bytes, self.cfg.max_pages_per_doc, &req.home));
            return match again {
                Some((body, _)) => Page {
                    status: status::OK,
                    kind: ContentKind::Xhtml,
                    body: body.into_bytes(),
                },
                None => first,
            };
        }
        let stored = docs.get(req.issi, doc, Instant::now()).expect("checked above");
        match render_page(&stored.doc, doc, n, page_bytes, self.cfg.max_pages_per_doc, &req.home) {
            Some((body, _)) => Page {
                status: status::OK,
                kind: ContentKind::Xhtml,
                body: body.into_bytes(),
            },
            None => notice(status::NOT_FOUND, "No encontrado", "Esa página no existe.", req),
        }
    }

    fn browse(&mut self, req: &FetchRequest, start: Url) -> (Page, Option<String>) {
        let started = Instant::now();
        // One time budget for the whole chain: redirects and meta refreshes.
        let deadline = started + Duration::from_secs(self.cfg.timeout_secs);
        let mut url = start;
        for _ in 0..=MAX_REFRESHES {
            let dl = match self.download(url.clone(), deadline) {
                Ok(dl) => dl,
                Err(e) => {
                    tracing::info!("WAP: ISSI {} could not fetch {}: {}", req.issi, host_of(&url), e);
                    return (error_page(&e, req), Some(host_of(&url)));
                }
            };
            let host = host_of(&dl.url);
            let (bytes, code, truncated) = (dl.body.len(), dl.status, dl.truncated);
            match self.present(req, dl) {
                Presented::Refresh(next) => url = next,
                Presented::Page(page) => {
                    tracing::info!(
                        "WAP: ISSI {} fetched {} (HTTP {}, {} B{}, {} ms, {} B to the radio)",
                        req.issi,
                        host,
                        code,
                        bytes,
                        if truncated { ", cut" } else { "" },
                        started.elapsed().as_millis(),
                        page.body.len()
                    );
                    return (page, Some(host));
                }
            }
        }
        (
            notice(status::BAD_GATEWAY, "Error", "Demasiadas redirecciones.", req),
            Some(host_of(&url)),
        )
    }

    fn download(&mut self, start: Url, deadline: Instant) -> Result<Downloaded, FetchError> {
        let client = self.client()?;
        let max = self.cfg.max_download_kb * 1024;
        let mut url = start;
        for _ in 0..=self.cfg.max_redirects {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_fragment(None);
            self.policy.check_url(&url).map_err(FetchError::Refused)?;
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(FetchError::Timeout);
            }
            let resp = client.get(url.clone()).timeout(left).send().map_err(classify)?;
            let code = resp.status();
            if code.is_redirection()
                && let Some(location) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok())
            {
                url = url
                    .join(location.trim())
                    .map_err(|_| FetchError::Other("bad redirect".to_string()))?;
                continue;
            }
            let content_type = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let mut body = Vec::new();
            if let Err(e) = resp.take(max as u64 + 1).read_to_end(&mut body) {
                // The client reports a body timeout as an I/O error around its own error.
                let timed_out = e.kind() == std::io::ErrorKind::TimedOut
                    || e.get_ref()
                        .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
                        .is_some_and(reqwest::Error::is_timeout);
                return Err(if timed_out {
                    FetchError::Timeout
                } else {
                    FetchError::Other(e.to_string())
                });
            }
            let truncated = body.len() > max;
            body.truncate(max);
            return Ok(Downloaded {
                url,
                status: code.as_u16(),
                content_type,
                body,
                truncated,
            });
        }
        Err(FetchError::TooManyRedirects)
    }

    /// Turn a download into the page for the radio (or a meta refresh to follow).
    fn present(&mut self, req: &FetchRequest, dl: Downloaded) -> Presented {
        let source = dl.url.to_string();
        if !(200..300).contains(&dl.status) {
            return Presented::Page(notice(
                wsp_status_for_http(dl.status),
                "Error",
                &format!("El sitio respondió HTTP {}.", dl.status),
                req,
            ));
        }
        let mime = dl.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let text = |dl: &Downloaded| decode_body(&dl.body, &dl.content_type);
        let doc = match mime.as_str() {
            "text/vnd.wap.wml" | "application/vnd.wap.xhtml+xml" | "text/html" | "application/xhtml+xml" | "" => {
                let body = text(&dl);
                let wml = mime == "text/vnd.wap.wml";
                let xhtml_mp =
                    mime == "application/vnd.wap.xhtml+xml" || body.find("-//WAPFORUM//DTD XHTML Mobile").is_some_and(|i| i < 1024);
                if (wml || xhtml_mp) && !dl.truncated {
                    let trimmed = trim_wap_markup(&body, !wml, &dl.url);
                    if trimmed.len() <= req.budget {
                        return Presented::Page(Page {
                            status: status::OK,
                            kind: if wml { ContentKind::Wml } else { ContentKind::Xhtml },
                            body: trimmed.into_bytes(),
                        });
                    }
                }
                convert_html(&body, &dl.url)
            }
            "text/plain" => {
                let name = dl.url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("").to_string();
                let title = if name.is_empty() { host_of(&dl.url) } else { name };
                convert_text(&text(&dl), &title)
            }
            other => {
                return Presented::Page(notice(
                    status::UNSUPPORTED_MEDIA_TYPE,
                    "No se puede mostrar",
                    &format!("La radio no puede mostrar contenido {other}."),
                    req,
                ));
            }
        };
        if let Some(target) = doc.refresh.as_deref().filter(|_| doc.text_len() < REFRESH_TEXT_CHARS)
            && let Ok(target) = Url::parse(target)
        {
            return Presented::Refresh(target);
        }
        self.store_and_render(req, doc, dl.truncated, &source)
    }

    fn store_and_render(&mut self, req: &FetchRequest, mut doc: Document, truncated: bool, source: &str) -> Presented {
        if doc.blocks.is_empty() {
            return Presented::Page(notice(
                status::OK,
                "Sin texto",
                "La página no tiene texto que mostrar (quizá necesita JavaScript).",
                req,
            ));
        }
        if truncated {
            doc.blocks
                .push(Block::Para(vec![Inline::Text("(Página recortada: demasiado grande.)".to_string())]));
        }
        let page_bytes = self.page_bytes(req);
        let id = self.docs().insert(req.issi, doc, source, Instant::now());
        let mut docs = self.docs();
        let rendered = docs
            .get(req.issi, id, Instant::now())
            .and_then(|stored| render_page(&stored.doc, id, 1, page_bytes, self.cfg.max_pages_per_doc, &req.home));
        Presented::Page(match rendered {
            Some((body, _)) => Page {
                status: status::OK,
                kind: ContentKind::Xhtml,
                body: body.into_bytes(),
            },
            None => notice(status::INTERNAL_ERROR, "Error", "La página no cabe en la respuesta.", req),
        })
    }

    /// Reply of a web API (weather, radioid.net), downloaded like any page (network policy,
    /// redirects checked hop by hop, size cap) before `deadline`: HTTP status and body.
    fn api_get(&mut self, url: &str, deadline: Instant) -> Result<(u16, String), FetchError> {
        let url = Url::parse(url).map_err(|e| FetchError::Other(e.to_string()))?;
        let dl = self.download(url, deadline)?;
        Ok((dl.status, decode_body(&dl.body, &dl.content_type)))
    }

    /// Body of a 2xx reply of an API, or the sentence for the radio: `service` is how the API is
    /// called in it ("radioid.net", "El servicio del tiempo").
    fn api_text(&mut self, req: &FetchRequest, url: &str, deadline: Instant, service: &str) -> Result<String, String> {
        match self.api_get(url, deadline) {
            Ok((code, body)) if (200..300).contains(&code) => Ok(body),
            Ok((code, _)) => {
                tracing::debug!("WAP: {service} answered HTTP {code} to ISSI {}", req.issi);
                Err(format!("{service} respondió HTTP {code}."))
            }
            // The station's own network policy (allow or deny list, port, address): it will not
            // pass later either, so it is not an outage.
            Err(FetchError::Refused(r)) => {
                tracing::info!("WAP: {service} refused by the network policy for ISSI {}: {r}", req.issi);
                Err(format!("{service} no está permitido en esta estación."))
            }
            Err(e) => {
                tracing::debug!("WAP: {service} for ISSI {} failed: {e}", req.issi);
                Err(format!("{service} no responde. Prueba más tarde."))
            }
        }
    }

    /// `/wx?i=ICAO` (METAR, aviationweather.gov) or `/wx?l=place` (Open-Meteo: the place is looked
    /// for in Spain first, then anywhere), in Spanish over several lines, with the forms again.
    fn weather(&mut self, req: &FetchRequest, query: Option<&str>) -> Page {
        let icao = form_param(query, "i").filter(|v| !v.trim().is_empty());
        let place = form_param(query, "l").map(|l| {
            l.chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | ',' | '-' | '.' | '\''))
                .take(40)
                .collect::<String>()
        });
        // One time budget for all the requests of the answer.
        let deadline = Instant::now() + Duration::from_secs(self.cfg.timeout_secs);
        let lines = match (icao, place.filter(|p| !p.trim().is_empty())) {
            (Some(icao), _) => self.metar_lines(req, &icao, deadline),
            (None, Some(place)) => self.place_weather_lines(req, place.trim(), deadline),
            (None, None) => vec!["Escribe un lugar o el código de un aeropuerto.".to_string()],
        };
        Page {
            status: status::OK,
            kind: ContentKind::Xhtml,
            body: home::wx_page(&lines, &req.home, req.budget).into_bytes(),
        }
    }

    fn metar_lines(&mut self, req: &FetchRequest, icao: &str, deadline: Instant) -> Vec<String> {
        let Some(url) = wx_service::metar_url(icao) else {
            return vec!["Escribe el código de un aeropuerto (LELC).".to_string()];
        };
        let code: String = icao.trim().chars().filter(char::is_ascii_alphanumeric).take(4).collect();
        match self.api_text(req, &url, deadline, "El servicio del tiempo") {
            // An unknown code is an empty HTTP 204.
            Ok(body) => wx_service::metar_lines_es_from_body(&body)
                .unwrap_or_else(|| vec![format!("No hay METAR para {}.", code.to_ascii_uppercase())]),
            Err(sentence) => vec![sentence],
        }
    }

    /// "Place" or "place, region or country": the place with the geocoder, then its forecast.
    fn place_weather_lines(&mut self, req: &FetchRequest, place: &str, deadline: Instant) -> Vec<String> {
        const NOT_FOUND: &str = "No encuentro ese lugar.";
        const BAD_REPLY: &str = "Respuesta del servicio del tiempo no válida.";
        let (name, hint) = match place.split_once(',') {
            Some((name, hint)) => (name.trim(), Some(hint.trim())),
            None => (place, None),
        };
        // Without a hint, a Spanish province or community the geocoder has no town for is its
        // capital ("Navarra" is Pamplona, not a village in Mexico).
        let (name, label) = match hint {
            None => wx_service::place_query(name),
            Some(_) => (name.to_string(), None),
        };
        let name = name.as_str();
        if name.is_empty() {
            return vec![NOT_FOUND.to_string()];
        }
        let mut found = None;
        for spain in [true, false] {
            let body = match self.api_text(req, &wx_service::geocode_url(name, spain), deadline, "El servicio del tiempo") {
                Ok(body) => body,
                Err(sentence) => return vec![sentence],
            };
            match wx_service::place_from_body(&body, name, spain, hint) {
                Ok(Some(p)) => {
                    found = Some(p);
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("WAP: weather for ISSI {}: {e}", req.issi);
                    return vec![BAD_REPLY.to_string()];
                }
            }
        }
        let Some(mut found) = found else {
            return vec![NOT_FOUND.to_string()];
        };
        if let Some(label) = label {
            found.label = label;
        }
        let body = match self.api_text(req, &wx_service::forecast_url(&found), deadline, "El servicio del tiempo") {
            Ok(body) => body,
            Err(sentence) => return vec![sentence],
        };
        wx_service::forecast_lines_es(&found, &body).unwrap_or_else(|e| {
            tracing::debug!("WAP: weather for ISSI {}: {e}", req.issi);
            vec![BAD_REPLY.to_string()]
        })
    }

    /// `/rid?q=`: a callsign or a DMR ID in radioid.net, with the form again. One entry is shown
    /// in detail, several as a list with a link to each ID; the page stays within `page_bytes`.
    /// Not cached: every lookup is a request (and counts against the radio's limit).
    fn radioid(&mut self, req: &FetchRequest, query: Option<&str>) -> Page {
        let budget = self.page_bytes(req);
        let asked = form_param(query, "q").unwrap_or_default();
        let found = rid::parse_input(&asked).and_then(|asked| self.radioid_lookup(req, &asked).map(|found| (found, asked)));
        let body = match found {
            Ok((found, asked)) => (1..=rid::MAX_ROWS)
                .rev()
                .map(|rows| home::rid_page(Some(&rid::render(&found, &asked, rows)), &req.home, budget))
                .find(|page| page.len() <= budget)
                .unwrap_or_else(|| home::rid_page(Some(&rid::render(&found, &asked, 1)), &req.home, budget)),
            Err(sentence) => home::rid_page(Some(&format!("<p>{}</p>", escape_xhtml_text(&sentence))), &req.home, budget),
        };
        Page {
            status: status::OK,
            kind: ContentKind::Xhtml,
            body: body.into_bytes(),
        }
    }

    /// The users with that callsign or ID or, when there are none, the repeaters.
    fn radioid_lookup(&mut self, req: &FetchRequest, asked: &RidQuery) -> Result<RidFound, String> {
        const BAD_REPLY: &str = "Respuesta de radioid.net no válida.";
        // One time budget for both requests.
        let deadline = Instant::now() + Duration::from_secs(self.cfg.timeout_secs);
        let bad_reply = |e: String| {
            tracing::debug!("WAP: radioid.net for ISSI {}: {e}", req.issi);
            BAD_REPLY.to_string()
        };
        let body = self.api_text(req, &rid::user_url(asked), deadline, "radioid.net")?;
        let users = rid::parse_users(&body).map_err(bad_reply)?;
        if !users.is_empty() {
            return Ok(RidFound::Users(users));
        }
        let body = self.api_text(req, &rid::repeater_url(asked), deadline, "radioid.net")?;
        Ok(RidFound::Repeaters(rid::parse_repeaters(&body).map_err(bad_reply)?))
    }
}

/// Sent as 200: an Openwave browser (MXP600) shows only "HTTP error 410" for a 410 and never
/// this page with its way back to the home page.
fn expired(req: &FetchRequest) -> Page {
    notice(
        status::OK,
        "Caducada",
        "Esta página ya no está guardada. Vuelve a abrirla desde el inicio.",
        req,
    )
}

/// Whether a request goes out to the Internet (and counts against the radio's limit).
fn uses_internet(target: &FetchTarget) -> bool {
    match target {
        FetchTarget::Doc(path) => !path.starts_with("/p/"),
        FetchTarget::Url(_) | FetchTarget::Search(_) => true,
    }
}

/// The stack side of the pool: queues requests and collects replies without blocking, and
/// limits how many Internet requests each radio makes.
pub struct PoolFetcher {
    jobs: Sender<FetchRequest>,
    replies: Receiver<FetchReply>,
    /// Answers made on the stack side (over the limit).
    ready: VecDeque<FetchReply>,
    recent: HashMap<u32, VecDeque<Instant>>,
    per_issi: usize,
    window: Duration,
}

impl PoolFetcher {
    pub fn spawn(cfg: &CfgWap, policy: NetPolicy) -> std::io::Result<Self> {
        let (jobs_tx, jobs_rx) = bounded(QUEUE_LEN);
        let (replies_tx, replies_rx) = unbounded();
        // The document index outlives a restart of the station in a file (on the Pi /tmp is in
        // RAM: nothing is written to the SD card). Unit tests keep theirs in memory.
        let docs = if cfg!(test) {
            DocCache::default()
        } else {
            DocCache::persistent(std::env::temp_dir().join(DOC_INDEX_FILE))
        };
        let docs = Arc::new(Mutex::new(docs));
        for i in 0..cfg.browse.max_concurrent_fetches.max(1) {
            let worker = Worker::new(cfg.browse.clone(), policy.clone(), Arc::clone(&docs));
            let (jobs, replies) = (jobs_rx.clone(), replies_tx.clone());
            std::thread::Builder::new()
                .name(format!("wap-fetch-{i}"))
                .spawn(move || worker.run(jobs, replies))?;
        }
        Ok(Self {
            jobs: jobs_tx,
            replies: replies_rx,
            ready: VecDeque::new(),
            recent: HashMap::new(),
            per_issi: cfg.browse.requests_per_issi as usize,
            window: Duration::from_secs(cfg.browse.requests_window_secs),
        })
    }
}

impl Fetcher for PoolFetcher {
    fn submit(&mut self, req: FetchRequest) -> Result<(), FetchRequest> {
        let counted = uses_internet(&req.target);
        if counted {
            let now = Instant::now();
            let window = self.window;
            self.recent.retain(|_, q| {
                while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
                    q.pop_front();
                }
                !q.is_empty()
            });
            let recent = self.recent.entry(req.issi).or_default();
            if recent.len() >= self.per_issi {
                tracing::info!("WAP: ISSI {} is over its request limit", req.issi);
                let page = notice(
                    status::SERVICE_UNAVAILABLE,
                    "Demasiadas peticiones",
                    "Espera unos minutos antes de seguir navegando.",
                    &req,
                );
                self.ready.push_back(FetchReply {
                    id: req.id,
                    page,
                    domain: None,
                });
                return Ok(());
            }
            recent.push_back(now);
        }
        match self.jobs.try_send(req) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(req) | TrySendError::Disconnected(req)) => {
                if counted && let Some(recent) = self.recent.get_mut(&req.issi) {
                    recent.pop_back();
                }
                Err(req)
            }
        }
    }

    fn try_recv(&mut self) -> Option<FetchReply> {
        self.ready.pop_front().or_else(|| self.replies.try_recv().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::wapgw::paginate::assert_well_formed;
    use std::io::Write;
    use std::net::TcpListener;

    const ISSI: u32 = 2_260_618;

    /// A one-shot-per-connection HTTP server on 127.0.0.1: `routes` maps the request target
    /// (path and query) to the raw response; "SLOW" answers after 3 s.
    fn serve(routes: Vec<(String, Vec<u8>)>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let response = routes
                        .iter()
                        .find(|(path, _)| *path == target)
                        .map(|(_, r)| r.clone())
                        .unwrap_or_else(|| http("404 Not Found", "text/html", b"<p>missing</p>"));
                    if response.starts_with(b"SLOW") {
                        std::thread::sleep(Duration::from_secs(3));
                        let _ = stream.write_all(&response[4..]);
                    } else {
                        let _ = stream.write_all(&response);
                    }
                });
            }
        });
        port
    }

    fn http(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn redirect(location: &str) -> Vec<u8> {
        format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
    }

    fn browse_cfg(port: u16) -> CfgWapBrowse {
        CfgWapBrowse {
            enabled: true,
            allowed_issis: vec![ISSI],
            allowed_ports: vec![port, 80, 443],
            search_url: format!("http://127.0.0.1:{port}/lite/?q="),
            ..Default::default()
        }
    }

    fn worker(cfg: CfgWapBrowse) -> Worker {
        let policy = NetPolicy::new(&cfg).allowing_loopback();
        Worker::new(cfg, policy, Arc::new(Mutex::new(DocCache::default())))
    }

    fn request(target: FetchTarget, budget: usize) -> FetchRequest {
        FetchRequest {
            id: 1,
            issi: ISSI,
            target,
            budget,
            home: "http://10.0.0.1/".to_string(),
            back: None,
        }
    }

    fn get(w: &mut Worker, target: FetchTarget) -> (Page, Option<String>) {
        w.handle(&request(target, 4000))
    }

    fn body(page: &Page) -> String {
        String::from_utf8(page.body.clone()).unwrap()
    }

    const ARTICLE: &str = r#"<!DOCTYPE html><html><head><title>Noticia de prueba</title>
        <script>var x = "<p>nope</p>";</script><style>.a{}</style></head>
        <body><nav><a href="/">Portada</a></nav><article><h1>Titular</h1>
        <p>Primer párrafo con <a href="/otra">un enlace</a>.</p><img src="foto.jpg" alt="foto">
        <form action="/find"><input name="q"></form></article><footer>pie</footer></body></html>"#;

    #[test]
    fn fetches_converts_and_pages_an_article() {
        let port = serve(vec![
            (
                "/article".to_string(),
                http("200 OK", "text/html; charset=utf-8", ARTICLE.as_bytes()),
            ),
            (
                "/otra".to_string(),
                http("200 OK", "text/html", b"<title>Otra</title><p>Segunda</p>"),
            ),
            (
                "/find?q=tetra+radio".to_string(),
                http("200 OK", "text/html", b"<p>Resultados de tetra</p>"),
            ),
        ]);
        let mut w = worker(browse_cfg(port));
        let (page, domain) = get(&mut w, FetchTarget::Url(format!("http://127.0.0.1:{port}/article")));
        let html = body(&page);
        assert_eq!(page.status, status::OK, "{html}");
        assert_eq!(domain.as_deref(), Some("127.0.0.1"));
        assert!(html.contains("<title>Noticia de prueba (1/1)</title>"), "{html}");
        assert!(
            html.contains("<p><b>Titular</b></p><p>Primer párrafo con <a href=\"/l/1/0\">un enlace</a>.</p>"),
            "{html}"
        );
        assert!(
            !html.contains("nope") && !html.contains("Portada") && !html.contains("foto"),
            "{html}"
        );
        assert!(html.contains("<form action=\"/l/1/1\" method=\"get\">"));
        assert_well_formed(&html);

        // The shortened link and the form go to the right place.
        let (next, _) = get(&mut w, FetchTarget::Doc("/l/1/0".to_string()));
        assert!(body(&next).contains("Segunda"));
        let (found, _) = get(&mut w, FetchTarget::Doc("/l/1/1?q=tetra+radio".to_string()));
        assert!(body(&found).contains("Resultados de tetra"), "{}", body(&found));

        // Another radio cannot read this radio's documents; unknown documents have expired.
        let mut other = request(FetchTarget::Doc("/p/1/1".to_string()), 4000);
        other.issi = 1234;
        let (foreign, _) = w.handle(&other);
        assert_eq!(foreign.status, status::OK);
        assert!(body(&foreign).contains("Caducada"), "{}", body(&foreign));
        let (unknown, _) = get(&mut w, FetchTarget::Doc("/p/99/1".to_string()));
        assert!(body(&unknown).contains("Caducada"), "{}", body(&unknown));

        // A link in the radio's own old copy of a page: that page, fetched again.
        let mut stale = request(FetchTarget::Doc("/l/99/0".to_string()), 4000);
        stale.back = Some(FetchTarget::Url(format!("http://127.0.0.1:{port}/article")));
        let (reloaded, _) = w.handle(&stale);
        assert!(body(&reloaded).contains("Titular"), "{}", body(&reloaded));
    }

    #[test]
    fn long_page_is_split_with_siguiente() {
        let text: String = (0..60)
            .map(|i| format!("<p>Frase número {i} de un artículo largo de prueba.</p>"))
            .collect();
        let port = serve(vec![("/long".to_string(), http("200 OK", "text/html", text.as_bytes()))]);
        let mut w = worker(browse_cfg(port));
        let (first, _) = get(&mut w, FetchTarget::Url(format!("http://127.0.0.1:{port}/long")));
        let first = body(&first);
        assert!(first.contains("<a href=\"/p/1/2\">Siguiente</a>"), "{first}");
        assert!(first.len() <= 1200);
        let (second, _) = get(&mut w, FetchTarget::Doc("/p/1/2".to_string()));
        let second = body(&second);
        assert!(second.contains("<a href=\"/p/1/1\">Anterior</a>"), "{second}");
        assert_well_formed(&second);
    }

    #[test]
    fn redirect_to_private_is_blocked() {
        let port = serve(vec![
            ("/to-lan".to_string(), redirect("http://192.168.1.1/admin")),
            ("/to-localhost".to_string(), redirect("http://localhost/")),
            ("/to-self".to_string(), redirect("/final")),
            ("/final".to_string(), http("200 OK", "text/html", b"<p>llegamos</p>")),
            ("/loop".to_string(), redirect("/loop")),
        ]);
        let mut w = worker(browse_cfg(port));
        let base = format!("http://127.0.0.1:{port}");
        let (page, _) = get(&mut w, FetchTarget::Url(format!("{base}/to-lan")));
        assert_eq!(page.status, status::FORBIDDEN, "{}", body(&page));
        assert_eq!(
            get(&mut w, FetchTarget::Url(format!("{base}/to-localhost"))).0.status,
            status::FORBIDDEN
        );
        assert!(body(&get(&mut w, FetchTarget::Url(format!("{base}/to-self"))).0).contains("llegamos"));
        assert_eq!(get(&mut w, FetchTarget::Url(format!("{base}/loop"))).0.status, status::BAD_GATEWAY);
        // Without the test override the local server itself is out of reach.
        let cfg = browse_cfg(port);
        let mut strict = Worker::new(cfg.clone(), NetPolicy::new(&cfg), Arc::new(Mutex::new(DocCache::default())));
        assert_eq!(
            get(&mut strict, FetchTarget::Url(format!("{base}/final"))).0.status,
            status::FORBIDDEN
        );
        assert_eq!(
            get(&mut strict, FetchTarget::Url("http://10.0.0.1/".to_string())).0.status,
            status::FORBIDDEN
        );
    }

    #[test]
    fn weather_goes_through_the_network_policy() {
        // Refused before any network access: the weather services are fetched like any page.
        let cfg = CfgWapBrowse {
            domain_denylist: vec!["open-meteo.com".to_string(), "aviationweather.gov".to_string()],
            ..browse_cfg(80)
        };
        let mut w = worker(cfg);
        let deadline = Instant::now() + Duration::from_secs(5);
        let place = wx_service::Place {
            label: "Murcia".to_string(),
            latitude: 37.98704,
            longitude: -1.13004,
        };
        for url in [
            wx_service::geocode_url("Murcia", true),
            wx_service::forecast_url(&place),
            wx_service::metar_url("LEMD").unwrap(),
        ] {
            let err = w.api_get(&url, deadline).unwrap_err();
            assert!(matches!(err, FetchError::Refused(Refusal::Domain)), "{url}: {err}");
        }
        for asked in ["/wx?l=Murcia", "/wx?i=LEMD"] {
            let (page, _) = get(&mut w, FetchTarget::Doc(asked.to_string()));
            let html = body(&page);
            assert_eq!(page.status, status::OK);
            assert!(
                html.contains("<p>El servicio del tiempo no está permitido en esta estación.</p>"),
                "{asked}: {html}"
            );
            assert!(html.contains("name=\"l\"") && html.contains("name=\"i\""), "{html}");
            assert_well_formed(&html);
        }
        let (page, _) = get(&mut w, FetchTarget::Doc("/wx?l=+%2C+".to_string()));
        assert!(body(&page).contains("No encuentro ese lugar."), "{}", body(&page));
    }

    #[test]
    fn radioid_goes_through_the_network_policy() {
        let cfg = CfgWapBrowse {
            domain_denylist: vec!["radioid.net".to_string()],
            ..browse_cfg(80)
        };
        let mut w = worker(cfg);
        let deadline = Instant::now() + Duration::from_secs(5);
        for asked in [RidQuery::Id(2_145_007), RidQuery::Callsign("EA5GVK".to_string())] {
            for url in [rid::user_url(&asked), rid::repeater_url(&asked)] {
                let err = w.api_get(&url, deadline).unwrap_err();
                assert!(matches!(err, FetchError::Refused(Refusal::Domain)), "{url}: {err}");
            }
        }
        let (page, domain) = get(&mut w, FetchTarget::Doc("/rid?q=EA5GVK".to_string()));
        let html = body(&page);
        assert_eq!((page.status, domain), (status::OK, None));
        assert!(html.contains("<p>radioid.net no está permitido en esta estación.</p>"), "{html}");
        assert!(html.contains("name=\"q\"") && html.len() <= 1200, "{html}");
        assert_well_formed(&html);
    }

    #[test]
    fn radioid_input_checked_again_without_network() {
        // Nothing may be fetched: a request would be refused and say so.
        let mut w = worker(CfgWapBrowse {
            domain_denylist: vec!["radioid.net".to_string()],
            ..browse_cfg(80)
        });
        for (asked, expected) in [
            ("/rid?q=", "Escribe un indicativo"),
            ("/rid", "Escribe un indicativo"),
            ("/rid?q=EA5%25", "no es un indicativo ni un ID válido"),
            ("/rid?q=%2A", "no es un indicativo ni un ID válido"),
            ("/rid?q=Coru%F1a", "«Coruña» no es un indicativo"),
        ] {
            let (page, _) = get(&mut w, FetchTarget::Doc(asked.to_string()));
            let html = body(&page);
            assert!(html.contains(expected) && !html.contains("no está permitido"), "{asked}: {html}");
            assert_well_formed(&html);
        }
    }

    #[test]
    fn resolver_refuses_names_of_private_addresses() {
        let resolver = PolicyResolver {
            policy: NetPolicy::new(&CfgWapBrowse::default()),
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let result = rt.block_on(resolver.resolve("localhost".parse().unwrap()));
        let err = result.err().expect("localhost resolves to loopback");
        assert!(err.downcast_ref::<Refusal>().is_some(), "{err}");
    }

    #[test]
    fn size_cap_truncates() {
        let big: String = (0..3000).map(|i| format!("<p>Línea {i} de relleno.</p>")).collect();
        let port = serve(vec![("/big".to_string(), http("200 OK", "text/html", big.as_bytes()))]);
        let mut cfg = browse_cfg(port);
        cfg.max_download_kb = 16;
        let mut w = worker(cfg);
        let (page, _) = get(&mut w, FetchTarget::Url(format!("http://127.0.0.1:{port}/big")));
        assert_eq!(page.status, status::OK);
        let mut docs = w.docs();
        let doc = &docs.get(ISSI, 1, Instant::now()).unwrap().doc;
        assert!(doc.text_len() < 16 * 1024);
        assert_eq!(
            doc.blocks.last(),
            Some(&Block::Para(vec![Inline::Text(
                "(Página recortada: demasiado grande.)".to_string()
            )]))
        );
    }

    #[test]
    fn timeout() {
        let mut slow = b"SLOW".to_vec();
        slow.extend(http("200 OK", "text/html", b"<p>tarde</p>"));
        let port = serve(vec![("/slow".to_string(), slow)]);
        let mut cfg = browse_cfg(port);
        cfg.timeout_secs = 1;
        let mut w = worker(cfg);
        let started = Instant::now();
        let (page, _) = get(&mut w, FetchTarget::Url(format!("http://127.0.0.1:{port}/slow")));
        assert_eq!(page.status, status::GATEWAY_TIMEOUT, "{}", body(&page));
        assert!(started.elapsed() < Duration::from_millis(2500));
    }

    #[test]
    fn charset_latin1_to_utf8() {
        let port = serve(vec![
            (
                "/h".to_string(),
                http("200 OK", "text/html; charset=ISO-8859-1", b"<p>Canci\xf3n espa\xf1ola</p>"),
            ),
            (
                "/m".to_string(),
                http("200 OK", "text/html", b"<meta charset=\"windows-1252\"><p>\x93Hola\x94 \x80</p>"),
            ),
            ("/u".to_string(), http("200 OK", "text/html", "<p>ya en UTF-8: ñ</p>".as_bytes())),
            ("/x".to_string(), http("200 OK", "text/html", b"<p>sin declarar: a\xf1o</p>")),
        ]);
        let mut w = worker(browse_cfg(port));
        let base = format!("http://127.0.0.1:{port}");
        let text = |w: &mut Worker, p: &str| body(&get(w, FetchTarget::Url(format!("{base}{p}"))).0);
        assert!(text(&mut w, "/h").contains("Canción española"));
        assert!(text(&mut w, "/m").contains("“Hola” €"));
        assert!(text(&mut w, "/u").contains("ya en UTF-8: ñ"));
        assert!(text(&mut w, "/x").contains("sin declarar: año"));
    }

    #[test]
    fn wap_content_passes_through() {
        let wml = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<wml>\n  <card id=\"c\" title=\"T\"><p>Hola</p></card>\n</wml>";
        let xhtml = "<?xml version=\"1.0\"?><!DOCTYPE html PUBLIC \"-//WAPFORUM//DTD XHTML Mobile 1.0//EN\" \"x\"><html><head><title>M</title></head><body><p><a href=\"b.xhtml\">b</a></p></body></html>";
        let port = serve(vec![
            ("/w".to_string(), http("200 OK", "text/vnd.wap.wml", wml.as_bytes())),
            ("/x".to_string(), http("200 OK", "application/vnd.wap.xhtml+xml", xhtml.as_bytes())),
        ]);
        let mut w = worker(browse_cfg(port));
        let (page, _) = get(&mut w, FetchTarget::Url(format!("http://127.0.0.1:{port}/w")));
        assert_eq!(page.kind, ContentKind::Wml);
        assert_eq!(
            body(&page),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><wml><card id=\"c\" title=\"T\"><p>Hola</p></card></wml>"
        );
        let (page, _) = get(&mut w, FetchTarget::Url(format!("http://127.0.0.1:{port}/x")));
        assert_eq!(page.kind, ContentKind::Xhtml);
        assert!(body(&page).contains(&format!("<head><base href=\"http://127.0.0.1:{port}/x\"/><title>M</title>")));
        // Too big for the reply: converted and paged instead.
        let small = w.handle(&request(FetchTarget::Url(format!("http://127.0.0.1:{port}/x")), 100));
        assert!(body(&small.0).len() <= 100 || small.0.status != status::OK);
    }

    #[test]
    fn search_errors_and_unsupported_types() {
        let port = serve(vec![
            (
                "/lite/?q=radio%20tetra".to_string(),
                http("200 OK", "text/html", b"<p>resultado</p>"),
            ),
            ("/img".to_string(), http("200 OK", "image/png", b"\x89PNG")),
            ("/gone".to_string(), http("404 Not Found", "text/html", b"<p>no</p>")),
            ("/empty".to_string(), http("200 OK", "text/html", b"<script>app()</script>")),
        ]);
        let mut w = worker(browse_cfg(port));
        let base = format!("http://127.0.0.1:{port}");
        assert!(body(&get(&mut w, FetchTarget::Search("radio tetra".to_string())).0).contains("resultado"));
        assert_eq!(
            get(&mut w, FetchTarget::Url(format!("{base}/img"))).0.status,
            status::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(get(&mut w, FetchTarget::Url(format!("{base}/gone"))).0.status, status::NOT_FOUND);
        assert!(body(&get(&mut w, FetchTarget::Url(format!("{base}/empty"))).0).contains("Sin texto"));
        assert_eq!(
            get(&mut w, FetchTarget::Url("http://127.0.0.1:1/".to_string())).0.status,
            status::FORBIDDEN,
            "port not allowed"
        );
    }

    #[test]
    fn doc_paths() {
        assert_eq!(parse_doc_path("/p/3/2"), Some(DocPath::Page { doc: 3, page: 2 }));
        assert_eq!(
            parse_doc_path("/l/3/7?q=a"),
            Some(DocPath::Link {
                doc: 3,
                link: 7,
                query: Some("q=a")
            })
        );
        assert_eq!(parse_doc_path("/wx?i=LEMD"), Some(DocPath::Weather { query: Some("i=LEMD") }));
        assert_eq!(parse_doc_path("/wx"), Some(DocPath::Weather { query: None }));
        assert_eq!(parse_doc_path("/rid?q=EA5GVK"), Some(DocPath::RadioId { query: Some("q=EA5GVK") }));
        assert_eq!(parse_doc_path("/p/x/2"), None);
        assert_eq!(parse_doc_path("/p/1/2/3"), None);
        assert_eq!(parse_doc_path("/q/1/2"), None);
    }

    #[test]
    fn requests_per_issi_are_limited() {
        let cfg = CfgWap {
            enabled: true,
            browse: CfgWapBrowse {
                requests_per_issi: 2,
                // Nothing can be fetched: the replies come back quickly.
                allowed_ports: vec![1],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pool = PoolFetcher::spawn(&cfg, NetPolicy::new(&cfg.browse)).unwrap();
        let req = |id, target| FetchRequest {
            id,
            ..request(target, 1000)
        };
        for id in 1..=2 {
            pool.submit(req(id, FetchTarget::Url("http://example.org/".to_string()))).unwrap();
        }
        // Pages of a fetched document do not count.
        pool.submit(req(3, FetchTarget::Doc("/p/1/1".to_string()))).unwrap();
        pool.submit(req(4, FetchTarget::Search("x".to_string()))).unwrap();
        let mut replies = HashMap::new();
        let until = Instant::now() + Duration::from_secs(5);
        while replies.len() < 4 && Instant::now() < until {
            match pool.try_recv() {
                Some(r) => {
                    replies.insert(r.id, r.page.status);
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        assert_eq!(replies.get(&4), Some(&status::SERVICE_UNAVAILABLE), "{replies:?}");
        assert_eq!(replies.get(&1), Some(&status::FORBIDDEN));
        assert_eq!(replies.get(&3), Some(&status::OK), "an expired document is a page, not an error");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fetch_thread_is_sched_other() {
        std::thread::spawn(|| {
            background_priority();
            // SAFETY: plain queries on the calling thread.
            unsafe {
                assert_eq!(libc::sched_getscheduler(0), libc::SCHED_OTHER);
                assert!(libc::getpriority(libc::PRIO_PROCESS, 0) >= 10);
            }
        })
        .join()
        .unwrap();
    }
}
