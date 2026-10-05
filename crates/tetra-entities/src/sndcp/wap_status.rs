// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original WAP 2.0/WML2 status renderer for TETRA packet-data experiments.
// SPDX-FileComment: Modified by flowstation-miura (EA5GVK): the status blocks and the WML status pages in Spanish, a link home, every health line shown, no group/private call counters.

pub const DEFAULT_WAP_STATUS_MAX_BYTES: usize = 548;
pub const WAP_STATUS_REFRESH_PATH: &str = "/status.xhtml";
pub const WAP_STATUS_LEGACY_WML_PATH: &str = "/status.wml";
pub const WAP_STATUS_HTML_PATH: &str = "/status.html";
pub const WAP_STATUS_TITLE_MAX_ESCAPED_BYTES: usize = 24;
pub const WAP_STATUS_STATE_MAX_ESCAPED_BYTES: usize = 20;
pub const WAP_STATUS_VERSION_MAX_ESCAPED_BYTES: usize = 32;
pub const WAP_STATUS_LAST_ACTIVITY_MAX_ESCAPED_BYTES: usize = 32;
pub const WAP_STATUS_HEALTH_MAX_ESCAPED_BYTES: usize = 32;
pub const WAP_STATUS_DETAIL_LINE_MAX_ESCAPED_BYTES: usize = 32;
pub const WAP_STATUS_HEALTH_LINE_MAX_ESCAPED_BYTES: usize = 28;
pub const WAP_STATUS_DETAIL_MAX_LINES: usize = 3;
pub const WAP_STATUS_SECTOR_QUERY: &str = "s";
const XHTML_MP_DOCTYPE: &str =
    "<!DOCTYPE html PUBLIC \"-//WAPFORUM//DTD XHTML Mobile 1.0//EN\" \"http://www.wapforum.org/DTD/xhtml-mobile10.dtd\">";
const TINY_XHTML_PREFIX: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>";
const TINY_XHTML_SUFFIX: &str = "</body></html>";
const TINY_XHTML_BR: &str = "<br />";
const TINY_LAST_PREFIX: &str = "Last ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WapStatusSnapshot {
    pub title: String,
    pub stack_version: String,
    pub service_state: String,
    pub registered_ms: usize,
    pub active_calls: usize,
    pub active_group_calls: usize,
    pub active_private_calls: usize,
    pub queued_sds: usize,
    pub uptime_secs: u64,
    pub last_activity: Option<String>,
    pub health_summary: Option<String>,
    pub health_lines: Vec<String>,
    pub radio_lines: Vec<String>,
    pub call_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapStatusError {
    EmptyTitle,
    RenderedTooLarge { len: usize, max: usize },
}

pub fn render_wml2_status(snapshot: &WapStatusSnapshot, max_bytes: usize) -> Result<String, WapStatusError> {
    if snapshot.title.trim().is_empty() {
        return Err(WapStatusError::EmptyTitle);
    }

    for detail_mode in [
        WapStatusRenderMode::Full,
        WapStatusRenderMode::Compact,
        WapStatusRenderMode::Text,
        WapStatusRenderMode::Tiny,
    ] {
        let page = render_wml2_status_page(snapshot, detail_mode, max_bytes);
        if page.len() <= max_bytes {
            return Ok(page);
        }
    }

    let page = render_wml2_status_page(snapshot, WapStatusRenderMode::Tiny, max_bytes);
    Err(WapStatusError::RenderedTooLarge {
        len: page.len(),
        max: max_bytes,
    })
}

pub fn render_wml2_status_sector(snapshot: &WapStatusSnapshot, max_bytes: usize, sector: usize) -> Result<String, WapStatusError> {
    if snapshot.title.trim().is_empty() {
        return Err(WapStatusError::EmptyTitle);
    }

    let pages = render_wml2_status_sector_pages(snapshot);
    let sector = sector.min(pages.len().saturating_sub(1));
    let mut page = render_wml2_status_sector_page(snapshot, &pages, sector, WapStatusSectorRenderMode::Normal);
    if page.len() <= max_bytes {
        return Ok(page);
    }

    page = render_wml2_status_sector_page(snapshot, &pages, sector, WapStatusSectorRenderMode::Compact);
    if page.len() <= max_bytes {
        return Ok(page);
    }

    page = render_wml2_status_sector_page(snapshot, &pages, sector, WapStatusSectorRenderMode::Tiny);
    if page.len() <= max_bytes {
        return Ok(page);
    }

    Err(WapStatusError::RenderedTooLarge {
        len: page.len(),
        max: max_bytes,
    })
}

pub fn render_wml2_status_browser_index(snapshot: &WapStatusSnapshot, max_bytes: usize) -> Result<String, WapStatusError> {
    if snapshot.title.trim().is_empty() {
        return Err(WapStatusError::EmptyTitle);
    }

    let title = escape_xhtml_text_limited(snapshot.title.trim(), 12);
    let state = escape_xhtml_text_limited(snapshot.service_state.trim(), 12);
    let uptime = compact_uptime(snapshot.uptime_secs);
    let registered_ms = rendered_registered_ms(snapshot);
    let page = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><b>{title}</b><br />{state}<br />MS {} G {} P {} S {}<br />Up {uptime}<br /><a href=\"{WAP_STATUS_LEGACY_WML_PATH}?{WAP_STATUS_SECTOR_QUERY}=1\">Next</a></body></html>",
        registered_ms, snapshot.active_group_calls, snapshot.active_private_calls, snapshot.queued_sds
    );
    if page.len() <= max_bytes {
        return Ok(page);
    }

    let version = compact_tiny_version(snapshot.stack_version.trim());
    let page = format!(
        "<html><body>Welcome to Nexus-BS!<br/>v{version}<br/><a href=\"{WAP_STATUS_LEGACY_WML_PATH}?{WAP_STATUS_SECTOR_QUERY}=1\">Next</a></body></html>"
    );
    if page.len() <= max_bytes {
        return Ok(page);
    }

    Err(WapStatusError::RenderedTooLarge {
        len: page.len(),
        max: max_bytes,
    })
}

pub fn render_wml_status_browser_index(snapshot: &WapStatusSnapshot, max_bytes: usize) -> Result<String, WapStatusError> {
    if snapshot.title.trim().is_empty() {
        return Err(WapStatusError::EmptyTitle);
    }

    let title = escape_xhtml_text_limited(snapshot.title.trim(), 12);
    let state = escape_xhtml_text_limited(snapshot.service_state.trim(), 12);
    let registered_ms = rendered_registered_ms(snapshot);
    let page = format!(
        "<wml><card><p>{title}<br/>{state}<br/>Radios {} SDS {}<br/><a href=\"{WAP_STATUS_LEGACY_WML_PATH}?{WAP_STATUS_SECTOR_QUERY}=1\">Sig</a></p></card></wml>",
        registered_ms, snapshot.queued_sds
    );
    if page.len() <= max_bytes {
        return Ok(page);
    }

    let page = format!(
        "<wml><card><p>{title}<br/>Radios {} SDS {}<br/><a href=\"?{WAP_STATUS_SECTOR_QUERY}=1\">Sig</a></p></card></wml>",
        registered_ms, snapshot.queued_sds
    );
    if page.len() <= max_bytes {
        return Ok(page);
    }

    Err(WapStatusError::RenderedTooLarge {
        len: page.len(),
        max: max_bytes,
    })
}

pub fn render_wml2_status_browser_sector(snapshot: &WapStatusSnapshot, max_bytes: usize, sector: usize) -> Result<String, WapStatusError> {
    if snapshot.title.trim().is_empty() {
        return Err(WapStatusError::EmptyTitle);
    }

    let pages = render_wml2_status_sector_pages(snapshot);
    let sector = sector.min(pages.len().saturating_sub(1));
    for line_limit in [16, 12, 8, 4] {
        let page = render_wml2_status_browser_sector_page(&pages, sector, line_limit, max_bytes);
        if page.len() <= max_bytes {
            return Ok(page);
        }
    }

    let page = render_wml2_status_browser_sector_page(&pages, sector, 4, max_bytes);
    Err(WapStatusError::RenderedTooLarge {
        len: page.len(),
        max: max_bytes,
    })
}

pub fn render_wml_status_browser_sector(snapshot: &WapStatusSnapshot, max_bytes: usize, sector: usize) -> Result<String, WapStatusError> {
    if snapshot.title.trim().is_empty() {
        return Err(WapStatusError::EmptyTitle);
    }

    let pages = render_wml2_status_sector_pages(snapshot);
    let sector = sector.min(pages.len().saturating_sub(1));
    for line_limit in [16, 12, 20, 8] {
        let page = render_wml_status_browser_sector_page(&pages, sector, line_limit, max_bytes);
        if page.len() <= max_bytes {
            return Ok(page);
        }
    }

    let page = render_wml_status_browser_sector_page(&pages, sector, 8, max_bytes);
    if page.len() <= max_bytes {
        return Ok(page);
    }

    Err(WapStatusError::RenderedTooLarge {
        len: page.len(),
        max: max_bytes,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WapStatusSectorPage {
    title: &'static str,
    lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WapStatusSectorRenderMode {
    Normal,
    Compact,
    Tiny,
}

fn render_wml2_status_sector_pages(snapshot: &WapStatusSnapshot) -> Vec<WapStatusSectorPage> {
    let registered_ms = rendered_registered_ms(snapshot);
    let mut pages = Vec::new();
    pages.push(WapStatusSectorPage {
        title: "Resumen",
        lines: vec![
            format!("Estado {}", snapshot.service_state.trim()),
            format!("Versión {}", snapshot.stack_version.trim()),
            format!("Activa {}", compact_uptime(snapshot.uptime_secs)),
            format!("Radios {} SDS {}", registered_ms, snapshot.queued_sds),
        ],
    });

    let mut health = Vec::new();
    if let Some(summary) = snapshot
        .health_summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        health.push(format!("General {summary}"));
    }
    health.push(format!("Activa {}", compact_browser_uptime(snapshot.uptime_secs)));
    health.extend(nonempty_lines(&snapshot.health_lines));
    if !health.is_empty() {
        pages.push(WapStatusSectorPage {
            title: "Salud",
            lines: health,
        });
    }

    push_chunked_sector_pages(&mut pages, "Radios", &snapshot.radio_lines, 5);
    push_chunked_sector_pages(&mut pages, "Llamadas", &snapshot.call_lines, 5);

    if let Some(activity) = snapshot
        .last_activity
        .as_deref()
        .map(str::trim)
        .filter(|activity| !activity.is_empty())
    {
        pages.push(WapStatusSectorPage {
            title: "Actividad",
            lines: vec![format!("Última {activity}")],
        });
    }

    pages
}

fn push_chunked_sector_pages(pages: &mut Vec<WapStatusSectorPage>, title: &'static str, lines: &[String], chunk_size: usize) {
    let lines = nonempty_lines(lines);
    for chunk in lines.chunks(chunk_size) {
        if !chunk.is_empty() {
            pages.push(WapStatusSectorPage {
                title,
                lines: chunk.to_vec(),
            });
        }
    }
}

fn nonempty_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn render_wml2_status_sector_page(
    snapshot: &WapStatusSnapshot,
    pages: &[WapStatusSectorPage],
    sector: usize,
    mode: WapStatusSectorRenderMode,
) -> String {
    let title_limit = match mode {
        WapStatusSectorRenderMode::Normal => WAP_STATUS_TITLE_MAX_ESCAPED_BYTES,
        WapStatusSectorRenderMode::Compact => 16,
        WapStatusSectorRenderMode::Tiny => 8,
    };
    let line_limit = match mode {
        WapStatusSectorRenderMode::Normal => 44,
        WapStatusSectorRenderMode::Compact => 28,
        WapStatusSectorRenderMode::Tiny => 18,
    };
    let max_lines = match mode {
        // Every line of the block (the health block has six).
        WapStatusSectorRenderMode::Normal => usize::MAX,
        WapStatusSectorRenderMode::Compact => 4,
        WapStatusSectorRenderMode::Tiny => 3,
    };
    let title = escape_xhtml_text_limited(snapshot.title.trim(), title_limit);
    let page = &pages[sector];
    let page_title = escape_xhtml_text_limited(page.title, 12);
    let sector_label = format!("{}/{}", sector + 1, pages.len());
    let mut body = vec![format!("<b>{title}: {page_title}</b>"), format!("Bloque {sector_label}")];
    body.extend(
        page.lines
            .iter()
            .take(max_lines)
            .map(|line| escape_xhtml_text_limited(line, line_limit)),
    );
    if sector + 1 < pages.len() {
        body.push(format!(
            "<a href=\"{WAP_STATUS_REFRESH_PATH}?{WAP_STATUS_SECTOR_QUERY}={}\">Siguiente</a>",
            sector + 1
        ));
    }
    if sector > 0 {
        body.push(format!(
            "<a href=\"{WAP_STATUS_REFRESH_PATH}?{WAP_STATUS_SECTOR_QUERY}={}\">Anterior</a>",
            sector - 1
        ));
    }
    if sector > 1 {
        body.push(format!("<a href=\"{WAP_STATUS_REFRESH_PATH}\">Primer bloque</a>"));
    }
    body.push("<a href=\"/\">Inicio</a>".to_string());

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title><meta http-equiv=\"Cache-Control\" content=\"no-cache\" /></head><body>{}</body></html>",
        body.join(TINY_XHTML_BR)
    )
}

fn render_wml2_status_browser_sector_page(pages: &[WapStatusSectorPage], sector: usize, line_limit: usize, max_bytes: usize) -> String {
    let page = &pages[sector];
    let page_title = escape_xhtml_text_limited(&browser_sector_heading(page, sector, pages.len()), 14);
    let mut body = vec![page_title];
    let mut lines: Vec<String> = page
        .lines
        .iter()
        .map(|line| compact_browser_sector_line(page.title, line))
        .map(|line| escape_xhtml_text_limited(&line, line_limit))
        .collect();
    if page.title == "Radios" {
        lines = vec![lines.join(" ")];
    }
    let mut nav = String::new();
    if sector + 1 < pages.len() {
        nav.push_str(&format!("<a href=\"?{WAP_STATUS_SECTOR_QUERY}={}\">Sig</a>", sector + 1));
    }
    if sector > 0 {
        nav.push_str(&format!("<a href=\"?{WAP_STATUS_SECTOR_QUERY}={}\">Ant</a>", sector - 1));
    }
    nav.push_str("<a href=\"/\">Ini</a>");
    body.push(nav);
    let nav_count = 1;

    if line_limit > 0 {
        for line in lines {
            let insert_at = body.len().saturating_sub(nav_count);
            let mut candidate = body.clone();
            candidate.insert(insert_at, line);
            let page = format!("<html><body>{}</body></html>", candidate.join("<br/>"));
            if page.len() > max_bytes {
                break;
            }
            body = candidate;
        }
    }

    format!("<html><body>{}</body></html>", body.join("<br/>"))
}

fn render_wml_status_browser_sector_page(pages: &[WapStatusSectorPage], sector: usize, line_limit: usize, max_bytes: usize) -> String {
    let page = &pages[sector];
    let page_title = escape_xhtml_text_limited(&browser_sector_heading(page, sector, pages.len()), 14);
    let mut body = vec![page_title];
    let mut lines: Vec<String> = page
        .lines
        .iter()
        .map(|line| compact_browser_sector_line(page.title, line))
        .map(|line| escape_xhtml_text_limited(&line, line_limit))
        .collect();
    if page.title == "Radios" {
        lines = vec![lines.join(" ")];
    }
    let mut nav = String::new();
    if sector + 1 < pages.len() {
        nav.push_str(&format!("<a href=\"?{WAP_STATUS_SECTOR_QUERY}={}\">Sig</a>", sector + 1));
    }
    if sector > 0 {
        nav.push_str(&format!("<a href=\"?{WAP_STATUS_SECTOR_QUERY}={}\">Ant</a>", sector - 1));
    }
    nav.push_str("<a href=\"/\">Ini</a>");
    body.push(nav);
    let nav_count = 1;

    for line in lines {
        let insert_at = body.len().saturating_sub(nav_count);
        let mut candidate = body.clone();
        candidate.insert(insert_at, line);
        let page = format!("<wml><card><p>{}</p></card></wml>", candidate.join("<br/>"));
        if page.len() > max_bytes {
            break;
        }
        body = candidate;
    }

    format!("<wml><card><p>{}</p></card></wml>", body.join("<br/>"))
}

fn browser_sector_title(title: &str) -> &str {
    match title {
        "Resumen" => "Estado",
        "Actividad" => "Última",
        other => other,
    }
}

fn browser_sector_heading(page: &WapStatusSectorPage, sector: usize, page_count: usize) -> String {
    match page.title {
        "Radios" => format!("Radios {}", page.lines.len()),
        "Llamadas" => format!("Llamadas {}", page.lines.len()),
        title => format!("{} {}/{}", browser_sector_title(title), sector + 1, page_count),
    }
}

fn compact_browser_sector_line(title: &str, line: &str) -> String {
    match title {
        "Radios" => compact_browser_radio_line(line),
        _ => line.to_string(),
    }
}

fn compact_browser_radio_line(line: &str) -> String {
    line.split_whitespace()
        .find(|part| part.chars().all(|ch| ch.is_ascii_digit()) && part.len() >= 5)
        .map(str::to_string)
        .unwrap_or_else(|| line.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WapStatusRenderMode {
    Full,
    Compact,
    Text,
    Tiny,
}

fn render_wml2_status_page(snapshot: &WapStatusSnapshot, detail_mode: WapStatusRenderMode, max_bytes: usize) -> String {
    let title = escape_xhtml_text_limited(snapshot.title.trim(), WAP_STATUS_TITLE_MAX_ESCAPED_BYTES);
    let stack_version = escape_xhtml_text_limited(snapshot.stack_version.trim(), WAP_STATUS_VERSION_MAX_ESCAPED_BYTES);
    let service_state = escape_xhtml_text_limited(snapshot.service_state.trim(), WAP_STATUS_STATE_MAX_ESCAPED_BYTES);
    let uptime = compact_uptime(snapshot.uptime_secs);
    let refresh_path = WAP_STATUS_REFRESH_PATH;
    let last_activity = snapshot
        .last_activity
        .as_deref()
        .map(str::trim)
        .filter(|activity| !activity.is_empty())
        .map(|activity| {
            format!(
                "<br />Last: {}",
                escape_xhtml_text_limited(activity, WAP_STATUS_LAST_ACTIVITY_MAX_ESCAPED_BYTES)
            )
        })
        .unwrap_or_default();
    let (health_summary, health_lines, detail_lines) = render_wml2_dashboard_details(snapshot, detail_mode);
    let registered_ms = rendered_registered_ms(snapshot);
    let subtitle = match detail_mode {
        WapStatusRenderMode::Full => "WAP 2.0 / WML2 live core",
        WapStatusRenderMode::Compact => "WML2 compact",
        WapStatusRenderMode::Text => "WML2 text",
        WapStatusRenderMode::Tiny => "WML2",
    };

    match detail_mode {
        WapStatusRenderMode::Full => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{XHTML_MP_DOCTYPE}\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title><meta http-equiv=\"Cache-Control\" content=\"no-cache\" /><meta http-equiv=\"refresh\" content=\"8;url={refresh_path}\" /><style type=\"text/css\">body{{margin:0;background:#00140a;color:#eaffea;font-family:sans-serif}}.hero{{background:#17e36d;color:#00140a;padding:4px}}.box{{border:1px solid #33aa66;margin:3px;padding:3px}}.k{{color:#8cff9e}}.ok{{color:#69ff69}}.warn{{color:#ffd84d}}.bad{{color:#ff5a5a}}a{{color:#9ff}}</style></head><body><div class=\"hero\"><b>Welcome to {title}</b><br /><span>{subtitle}</span></div><div class=\"box\"><b>{service_state}</b><br /><span class=\"k\">MS</span> {} <span class=\"k\">G</span> {} <span class=\"k\">P</span> {} <span class=\"k\">SDS</span> {}<br /><span class=\"k\">Up</span> {uptime}<br /><span class=\"k\">Ver</span> {stack_version}{last_activity}</div><div class=\"box\"><b>Core Health</b>{health_summary}{health_lines}</div><div class=\"box\">{detail_lines}</div><p><a href=\"{refresh_path}\">Refresh</a></p></body></html>",
            registered_ms, snapshot.active_group_calls, snapshot.active_private_calls, snapshot.queued_sds
        ),
        WapStatusRenderMode::Compact => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{XHTML_MP_DOCTYPE}\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title><meta http-equiv=\"refresh\" content=\"10;url={refresh_path}\" /><style type=\"text/css\">body{{background:#00140a;color:#eaffea;font-family:sans-serif}}.box{{border:1px solid #33aa66;margin:2px;padding:2px}}.ok{{color:#69ff69}}.warn{{color:#ffd84d}}.bad{{color:#ff5a5a}}</style></head><body><p><b>Welcome to {title}</b><br />{subtitle}<br />{service_state} MS:{} G:{} P:{} SDS:{}<br />Up {uptime}</p><div class=\"box\"><b>Health</b>{health_summary}{health_lines}</div><div class=\"box\">{detail_lines}</div><p><a href=\"{refresh_path}\">Refresh</a></p></body></html>",
            registered_ms, snapshot.active_group_calls, snapshot.active_private_calls, snapshot.queued_sds
        ),
        WapStatusRenderMode::Text => render_text_wml2_status_page(snapshot, max_bytes),
        WapStatusRenderMode::Tiny => render_tiny_wml2_status_page(snapshot, max_bytes),
    }
}

fn render_text_wml2_status_page(snapshot: &WapStatusSnapshot, max_bytes: usize) -> String {
    let title = escape_xhtml_text_limited(snapshot.title.trim(), WAP_STATUS_TITLE_MAX_ESCAPED_BYTES);
    let state = compact_tiny_state(snapshot);
    let version = compact_tiny_version(&snapshot.stack_version);
    let uptime = compact_tiny_uptime(snapshot.uptime_secs);
    let prefix = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title><meta http-equiv=\"refresh\" content=\"10;url={WAP_STATUS_REFRESH_PATH}\" /></head><body>"
    );
    let suffix = "</body></html>";
    let mut lines = vec![
        format!("<b>{title}: {state}</b>"),
        format!("Version: {version}"),
        format!("Uptime {uptime}"),
        format!(
            "MS {} G {} P {} SDS {}",
            compact_count(rendered_registered_ms(snapshot)),
            compact_count(snapshot.active_group_calls),
            compact_count(snapshot.active_private_calls),
            compact_count(snapshot.queued_sds)
        ),
    ];

    if let Some(activity) = snapshot
        .last_activity
        .as_deref()
        .map(str::trim)
        .filter(|activity| !activity.is_empty())
    {
        lines.push(format!(
            "Last {}",
            escape_xhtml_text_limited(activity, WAP_STATUS_LAST_ACTIVITY_MAX_ESCAPED_BYTES)
        ));
    }
    if let Some(health) = snapshot
        .health_summary
        .as_deref()
        .map(str::trim)
        .filter(|health| !health.is_empty())
    {
        lines.push(format!(
            "Health {}",
            escape_xhtml_text_limited(health, WAP_STATUS_HEALTH_MAX_ESCAPED_BYTES)
        ));
    }
    for line in snapshot.health_lines.iter().take(3) {
        let line = line.trim();
        if !line.is_empty() {
            lines.push(escape_xhtml_text_limited(line, WAP_STATUS_HEALTH_LINE_MAX_ESCAPED_BYTES));
        }
    }
    append_text_section_lines(&mut lines, "Radios", &snapshot.radio_lines, 3);
    append_text_section_lines(&mut lines, "Calls", &snapshot.call_lines, 2);
    lines.push(format!("<a href=\"{WAP_STATUS_REFRESH_PATH}\">Refresh</a>"));

    let mut body = String::new();
    for line in lines {
        let candidate = if body.is_empty() {
            line
        } else {
            format!("{body}{TINY_XHTML_BR}{line}")
        };
        if prefix.len().saturating_add(candidate.len()).saturating_add(suffix.len()) <= max_bytes {
            body = candidate;
        } else {
            break;
        }
    }

    format!("{prefix}{body}{suffix}")
}

fn append_text_section_lines(lines: &mut Vec<String>, title: &str, details: &[String], max_lines: usize) {
    let mut section = Vec::new();
    for detail in details.iter().take(max_lines) {
        let detail = detail.trim();
        if !detail.is_empty() {
            section.push(escape_xhtml_text_limited(detail, WAP_STATUS_DETAIL_LINE_MAX_ESCAPED_BYTES));
        }
    }
    if !section.is_empty() {
        lines.push(format!("{title}:"));
        lines.extend(section);
    }
}

fn render_tiny_wml2_status_page(snapshot: &WapStatusSnapshot, max_bytes: usize) -> String {
    let title = escape_xhtml_text_limited(snapshot.title.trim(), 8);
    let state = compact_tiny_state(snapshot);
    let version = compact_tiny_version(&snapshot.stack_version);
    let registered_ms = compact_count(rendered_registered_ms(snapshot));
    let active_group_calls = compact_count(snapshot.active_group_calls);
    let active_private_calls = compact_count(snapshot.active_private_calls);
    let queued_sds = compact_count(snapshot.queued_sds);
    let uptime = compact_tiny_uptime(snapshot.uptime_secs);
    let body_with_counts = format!(
        "{title}: {state}{TINY_XHTML_BR}Version: {version}{TINY_XHTML_BR}MS {registered_ms} G {active_group_calls} P {active_private_calls} SDS {queued_sds}{TINY_XHTML_BR}Uptime {uptime}"
    );
    let body_without_counts = format!("{title}: {state}{TINY_XHTML_BR}Version: {version}{TINY_XHTML_BR}Uptime {uptime}");
    let mut body = if TINY_XHTML_PREFIX
        .len()
        .saturating_add(body_with_counts.len())
        .saturating_add(TINY_XHTML_SUFFIX.len())
        <= max_bytes
    {
        body_with_counts
    } else {
        body_without_counts
    };

    if let Some(activity) = snapshot
        .last_activity
        .as_deref()
        .map(str::trim)
        .filter(|activity| !activity.is_empty())
        .map(compact_tiny_last_activity)
    {
        let used = TINY_XHTML_PREFIX
            .len()
            .saturating_add(body.len())
            .saturating_add(TINY_XHTML_BR.len())
            .saturating_add(TINY_LAST_PREFIX.len())
            .saturating_add(TINY_XHTML_SUFFIX.len());
        if used < max_bytes {
            let remaining = max_bytes - used;
            let activity = escape_xhtml_text_limited(&activity, remaining);
            if !activity.is_empty() {
                body.push_str(TINY_XHTML_BR);
                body.push_str(TINY_LAST_PREFIX);
                body.push_str(&activity);
            }
        }
    }

    format!("{TINY_XHTML_PREFIX}{body}{TINY_XHTML_SUFFIX}")
}

pub fn render_wml2_detail_lines(lines: &[String]) -> String {
    render_wml2_detail_lines_limited(lines, WAP_STATUS_DETAIL_MAX_LINES, WAP_STATUS_DETAIL_LINE_MAX_ESCAPED_BYTES)
}

fn render_wml2_dashboard_details(snapshot: &WapStatusSnapshot, detail_mode: WapStatusRenderMode) -> (String, String, String) {
    match detail_mode {
        WapStatusRenderMode::Full => (
            render_wml2_health_summary(snapshot, WAP_STATUS_HEALTH_MAX_ESCAPED_BYTES),
            render_wml2_health_lines(snapshot, WAP_STATUS_DETAIL_MAX_LINES + 2, WAP_STATUS_HEALTH_LINE_MAX_ESCAPED_BYTES),
            render_wml2_dashboard_detail_sections(snapshot, WAP_STATUS_DETAIL_MAX_LINES, WAP_STATUS_DETAIL_LINE_MAX_ESCAPED_BYTES),
        ),
        WapStatusRenderMode::Compact => (
            render_wml2_health_summary(snapshot, 20),
            render_wml2_health_lines(snapshot, 3, 24),
            render_wml2_dashboard_detail_sections(snapshot, 1, 24),
        ),
        WapStatusRenderMode::Text => (
            render_wml2_health_summary(snapshot, 18),
            render_wml2_health_lines(snapshot, 2, 24),
            render_wml2_dashboard_detail_sections(snapshot, 1, 24),
        ),
        WapStatusRenderMode::Tiny => (render_wml2_health_summary(snapshot, 18), String::new(), String::new()),
    }
}

fn render_wml2_health_summary(snapshot: &WapStatusSnapshot, max_escaped_bytes: usize) -> String {
    snapshot
        .health_summary
        .as_deref()
        .map(str::trim)
        .filter(|health| !health.is_empty())
        .map(|health| format!("<br />Health:{}", escape_xhtml_text_limited(health, max_escaped_bytes)))
        .unwrap_or_default()
}

fn render_wml2_health_lines(snapshot: &WapStatusSnapshot, max_lines: usize, max_line_bytes: usize) -> String {
    let lines = render_wml2_detail_lines_limited(&snapshot.health_lines, max_lines, max_line_bytes);
    format!("<br />{lines}")
}

fn render_wml2_dashboard_detail_sections(snapshot: &WapStatusSnapshot, max_lines: usize, max_line_bytes: usize) -> String {
    let radio_lines = render_wml2_detail_lines_limited(&snapshot.radio_lines, max_lines, max_line_bytes);
    let call_lines = render_wml2_detail_lines_limited(&snapshot.call_lines, max_lines, max_line_bytes);
    format!("<b>Radios</b><br />{radio_lines}<br /><b>Calls</b><br />{call_lines}")
}

fn render_wml2_detail_lines_limited(lines: &[String], max_lines: usize, max_line_bytes: usize) -> String {
    let mut rendered = Vec::new();
    for line in lines {
        if rendered.len() >= max_lines {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        rendered.push(escape_xhtml_text_limited(line, max_line_bytes));
    }

    if rendered.is_empty() {
        "None".to_string()
    } else {
        rendered.join("<br />")
    }
}

pub fn escape_xhtml_text(text: &str) -> String {
    escape_xhtml_text_limited(text, usize::MAX)
}

pub fn escape_xhtml_text_limited(text: &str, max_bytes: usize) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut truncated = false;
    let mut ch_buf = [0; 4];

    for ch in text.chars() {
        let fragment = escaped_xhtml_fragment(ch, &mut ch_buf);
        if escaped.len().saturating_add(fragment.len()) > max_bytes {
            truncated = true;
            break;
        }
        escaped.push_str(fragment);
    }

    if truncated && max_bytes > 0 {
        if escaped.len().saturating_add(1) <= max_bytes {
            escaped.push('~');
        }
    }

    escaped
}

fn escaped_xhtml_fragment<'a>(ch: char, ch_buf: &'a mut [u8; 4]) -> &'a str {
    match ch {
        '&' => "&amp;",
        '<' => "&lt;",
        '>' => "&gt;",
        '"' => "&quot;",
        '\'' => "&apos;",
        '\n' | '\r' | '\t' => " ",
        ch if ch.is_control() => "?",
        _ => ch.encode_utf8(ch_buf),
    }
}

pub fn compact_uptime(uptime_secs: u64) -> String {
    let days = uptime_secs / 86_400;
    let hours = (uptime_secs % 86_400) / 3_600;
    let minutes = (uptime_secs % 3_600) / 60;
    let seconds = uptime_secs % 60;

    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn compact_count(value: usize) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value < 1_000_000 {
        format!("{}k", value / 1_000)
    } else {
        "999k".to_string()
    }
}

fn rendered_registered_ms(snapshot: &WapStatusSnapshot) -> usize {
    let radio_count = nonempty_lines(&snapshot.radio_lines).len();
    if radio_count > 0 { radio_count } else { snapshot.registered_ms }
}

fn compact_tiny_state(snapshot: &WapStatusSnapshot) -> &'static str {
    let health = snapshot.health_summary.as_deref().unwrap_or_default();
    let has = |word: &str| snapshot.service_state.contains(word) || health.contains(word);
    if has("CRITICAL") || has("CRÍTICO") {
        "Err"
    } else if has("DEGRADED") || has("DEGRADADO") {
        "Warn"
    } else {
        "OK"
    }
}

fn compact_tiny_uptime(uptime_secs: u64) -> String {
    const MAX_TINY_UPTIME_SECS: u64 = 99 * 86_400 + 23 * 3_600 + 59 * 60 + 59;

    let uptime_secs = uptime_secs.min(MAX_TINY_UPTIME_SECS);
    let days = uptime_secs / 86_400;
    let hours = (uptime_secs % 86_400) / 3_600;
    let minutes = (uptime_secs % 3_600) / 60;
    let seconds = uptime_secs % 60;

    format!("{days}d{hours}h{minutes}m{seconds}s")
}

fn compact_browser_uptime(uptime_secs: u64) -> String {
    let uptime_secs = uptime_secs.min(99 * 86_400 + 23 * 3_600 + 59 * 60 + 59);
    let days = uptime_secs / 86_400;
    let hours = (uptime_secs % 86_400) / 3_600;
    let minutes = (uptime_secs % 3_600) / 60;
    let seconds = uptime_secs % 60;

    if minutes > 0 {
        format!("{days}d{hours}h{minutes}m{seconds}s")
    } else {
        format!("{days}d{hours}h{seconds}s")
    }
}

fn compact_tiny_version(version: &str) -> String {
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let version = version
        .split(['-', '_'])
        .next()
        .filter(|version| !version.is_empty())
        .unwrap_or("?");
    escape_xhtml_text_limited(version, 12)
}

fn compact_tiny_last_activity(activity: &str) -> String {
    let activity = activity.trim();
    let compact = activity
        .strip_prefix("SDS ")
        .map(|rest| format!("S{rest}"))
        .or_else(|| activity.strip_prefix("GRP ").map(|rest| format!("G{rest}")))
        .or_else(|| activity.strip_prefix("P2P-S ").map(|rest| format!("P{rest}")))
        .or_else(|| activity.strip_prefix("P2P-D ").map(|rest| format!("P{rest}")))
        .unwrap_or_else(|| activity.to_string());
    compact.replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: "v0.1.69_dev-test".to_string(),
            service_state: "ON AIR".to_string(),
            registered_ms: 3,
            active_calls: 1,
            active_group_calls: 0,
            active_private_calls: 1,
            queued_sds: 2,
            uptime_secs: 93_784,
            last_activity: Some("SDS 2260082>2260618".to_string()),
            health_summary: Some("OK".to_string()),
            health_lines: vec![
                "CORE OK".to_string(),
                "RF OK".to_string(),
                "VOICE OK".to_string(),
                "P2P OK".to_string(),
                "SDS OK".to_string(),
            ],
            radio_lines: vec!["MS 2260082 -52dB G1 SA".to_string(), "MS 2260618 -47dB G1 SA".to_string()],
            call_lines: vec!["P2P-S 2260082>2260618 TS2".to_string()],
        }
    }

    #[test]
    fn render_wml2_status_produces_small_terminal_page() {
        let page = render_wml2_status(&sample_snapshot(), 2048).expect("WAP 2.0 status should render");

        assert!(page.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(page.contains("-//WAPFORUM//DTD XHTML Mobile 1.0//EN"));
        assert!(page.contains("<html xmlns=\"http://www.w3.org/1999/xhtml\">"));
        assert!(page.contains("<title>Nexus-BS</title>"));
        assert!(page.contains("Welcome to Nexus-BS"));
        assert!(page.contains("WAP 2.0 / WML2 live core"));
        assert!(page.contains("content=\"8;url=/status.xhtml\""));
        assert!(page.contains("<style type=\"text/css\">"));
        assert!(!page.contains("<wml"));
        assert!(!page.contains("<card"));
        assert!(page.contains("<b>ON AIR</b>"));
        assert!(page.contains("<span class=\"k\">MS</span> 2"));
        assert!(page.contains("<span class=\"k\">G</span> 0"));
        assert!(page.contains("<span class=\"k\">P</span> 1"));
        assert!(page.contains("<span class=\"k\">SDS</span> 2"));
        assert!(page.contains("Up</span> 1d02h"));
        assert!(page.contains("Ver</span> v0.1.69_dev-test"));
        assert!(page.contains("Last: SDS 2260082&gt;2260618"));
        assert!(page.contains("Health:OK"));
        assert!(page.contains("CORE OK"));
        assert!(page.contains("RF OK"));
        assert!(page.contains("<b>Radios</b><br />"));
        assert!(page.contains("<b>Calls</b><br />"));
        assert!(page.contains("MS 2260082 -52dB G1 SA"));
        assert!(page.contains("P2P-S 2260082&gt;2260618 TS2"));
        assert!(page.len() <= 2048);
    }

    #[test]
    fn render_wml2_status_escapes_operator_text() {
        let mut snapshot = sample_snapshot();
        snapshot.title = "Nexus <BS> & \"WAP\"".to_string();
        snapshot.service_state = "RX < TX & ok".to_string();
        snapshot.last_activity = Some("P2P 1<2 & ok".to_string());

        let page = render_wml2_status(&snapshot, 2048).expect("escaped WML2 should render");

        assert!(page.contains("Nexus &lt;BS&gt; &amp;"));
        assert!(page.contains("RX &lt; TX &amp; ok"));
        assert!(page.contains("P2P 1&lt;2 &amp; ok"));
        assert!(!page.contains("Nexus <BS>"));
    }

    #[test]
    fn render_wml2_status_bounds_detail_lines_for_tiny_terminals() {
        let mut snapshot = sample_snapshot();
        snapshot.radio_lines = vec![
            " ".to_string(),
            "MS 3 &&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&".to_string(),
            "MS 2".to_string(),
            "MS 1".to_string(),
            "MS 0 should not render".to_string(),
        ];
        snapshot.call_lines = Vec::new();

        let page = render_wml2_status(&snapshot, 2048).expect("bounded details should render");

        assert!(page.contains("MS 3 "));
        assert!(page.contains("MS 2"));
        assert!(page.contains("MS 1"));
        assert!(page.contains("<b>Calls</b><br />None"));
        assert!(!page.contains("should not render"));
    }

    #[test]
    fn render_wml2_status_sanitizes_xml_control_characters() {
        let mut snapshot = sample_snapshot();
        snapshot.service_state = "ON\nAIR\tOK".to_string();
        snapshot.last_activity = Some("SDS\u{0001}2260082".to_string());

        let page = render_wml2_status(&snapshot, 2048).expect("sanitized WML2 should render");

        assert!(page.contains("ON AIR OK"));
        assert!(page.contains("SDS?2260082"));
        assert!(!page.contains('\u{0001}'));
    }

    #[test]
    fn render_wml2_status_bounds_escaped_dashboard_text_for_small_mtu() {
        let mut snapshot = sample_snapshot();
        snapshot.title = "Nexus &&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&".to_string();
        snapshot.stack_version = "v0.1.69_dev-with-a-very-long-build-identifier-and-extra-channel-data".to_string();
        snapshot.service_state = "DEGRADED &&&&&&&&&&&&&&&&&&&&&&&&&&".to_string();
        snapshot.last_activity = Some("SDS 2260082>2260618 &&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&".to_string());

        let page = render_wml2_status(&snapshot, 548).expect("bounded WML2 status should fit IPv4/UDP payload budget");

        assert!(page.len() <= 548);
        assert!(page.contains("Nexus "));
        assert!(page.contains("Last: SDS 2260082&gt;2260618") || !page.contains("Last:"));
    }

    #[test]
    fn render_wml2_status_tiny_page_keeps_dynamic_demo_fields() {
        let page = render_wml2_status(&sample_snapshot(), 128).expect("tiny WML2 status should render");

        assert!(page.len() <= 128);
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("Nexus-BS: OK"));
        assert!(page.contains("Version: 0.1.69"));
        assert!(page.contains("Uptime 1d2h3m4s"));
        assert!(!page.contains("MS 3 Calls 1 SDS 2"));
        assert!(!page.contains("Last S2260082"));
        assert!(!page.contains("Voice"));
        assert!(!page.contains("text=\"#0f0\""));
        assert_eq!(page.matches("<br />").count(), 2);
        assert!(!page.contains("<br/>"));
    }

    #[test]
    fn render_wml2_status_text_page_uses_one_slot_wsp_budget() {
        let page = render_wml2_status(&sample_snapshot(), 541).expect("text WML2 status should fit one-slot WSP budget");

        assert!(page.len() <= 541);
        assert!(page.len() > 128);
        assert!(page.contains("<html xmlns=\"http://www.w3.org/1999/xhtml\">"));
        assert!(page.contains("Nexus-BS: OK"));
        assert!(page.contains("Version: 0.1.69"));
        assert!(page.contains("Uptime 1d2h3m4s"));
        assert!(page.contains("MS 2 G 0 P 1 SDS 2"));
        assert!(page.contains("Last SDS 2260082&gt;2260618"));
        assert!(page.contains("CORE OK"));
        assert!(page.contains("Radios:"));
        assert!(page.contains("Calls:"));
        assert!(page.matches("<br />").count() > 3);
        assert!(!page.contains("<style"));
        assert!(!page.contains("text=\"#0f0\""));
        assert!(!page.contains("<br/>"));
    }

    #[test]
    fn render_wml2_status_sector_pages_use_browser_links_and_fit_wsp_budget() {
        let mut snapshot = sample_snapshot();
        snapshot.radio_lines = (0..14).map(|idx| format!("MS 226{idx:04} -4{idx}dB G1 SA")).collect();
        snapshot.call_lines = (0..7).map(|idx| format!("G{idx} S226{idx:04} TS2")).collect();

        let first = render_wml2_status_sector(&snapshot, 541, 0).expect("first sector should fit WSP budget");
        let second = render_wml2_status_sector(&snapshot, 541, 1).expect("second sector should fit WSP budget");
        let later = render_wml2_status_sector(&snapshot, 541, 3).expect("later sector should fit WSP budget");

        assert!(first.len() <= 541);
        assert!(second.len() <= 541);
        assert!(later.len() <= 541);
        assert!(first.contains("Bloque 1/"));
        assert!(first.contains("href=\"/status.xhtml?s=1\""));
        assert!(second.contains("href=\"/status.xhtml?s=0\""));
        assert!(later.contains("href=\"/status.xhtml\""));
        assert!(later.contains("Radios") || later.contains("Llamadas"));
        for page in [&first, &second, &later] {
            assert!(page.contains("<a href=\"/\">Inicio</a>"), "{page}");
        }
        assert!(!first.contains("<script"));
        assert!(!first.contains("<wml"));
    }

    #[test]
    fn render_wml2_status_browser_index_is_small_and_links_to_sectors() {
        let page = render_wml2_status_browser_index(&sample_snapshot(), 220).expect("browser index should fit");

        assert!(page.len() <= 220);
        assert!(page.contains("<b>Nexus-BS</b>"));
        assert!(page.contains("href=\"/status.wml?s=1\""));
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(!page.contains("<script"));
        assert!(!page.contains("<wml"));
    }

    #[test]
    fn render_wml2_status_browser_sector_is_small_and_navigable() {
        let mut snapshot = sample_snapshot();
        snapshot.radio_lines = (0..14).map(|idx| format!("MS 226{idx:04} -4{idx}dB G1 SA")).collect();
        snapshot.call_lines = (0..7).map(|idx| format!("G{idx} S226{idx:04} TS2")).collect();

        let page = render_wml2_status_browser_sector(&snapshot, 220, 1).expect("browser sector should fit");

        assert!(page.len() <= 220);
        assert!(page.contains("2/"));
        assert!(page.contains("Salud 2/"), "page={page:?}");
        assert!(page.contains("General OK"), "page={page:?}");
        assert!(page.contains(">Ant<"), "page={page:?}");
        assert!(page.contains(">Ini<"), "page={page:?}");
        assert!(page.contains("href=\"?s=0\""));
        assert!(page.contains("href=\"/\""));
        assert!(!page.contains("http://www.w3.org/1999/xhtml"));
        assert!(!page.contains("<script"));
        assert!(!page.contains("<wml"));
    }

    #[test]
    fn render_wml_status_browser_pages_keep_radio_count_consistent_with_live_lines() {
        let mut snapshot = sample_snapshot();
        snapshot.registered_ms = 3;
        snapshot.radio_lines = vec!["MS 2260618".to_string()];

        let index = render_wml_status_browser_index(&snapshot, 160).expect("browser index should fit");
        assert!(index.contains("Radios 1 SDS 2"), "index={index:?}");
        assert!(!index.contains(" G "), "index={index:?}");

        let radios = render_wml_status_browser_sector(&snapshot, 144, 2).expect("radio sector should fit");
        assert!(radios.contains("Radios 1"), "radios={radios:?}");
        assert!(!radios.contains("Radios 3/3"), "radios={radios:?}");
        assert!(radios.contains("2260618"), "radios={radios:?}");
        assert!(!radios.contains("MS 2260618"), "radios={radios:?}");
        assert!(radios.len() <= 144, "radios={radios:?}");
    }

    #[test]
    fn render_wml2_status_browser_pages_keep_radio_count_consistent_with_live_lines() {
        let mut snapshot = sample_snapshot();
        snapshot.registered_ms = 3;
        snapshot.radio_lines = vec!["MS 2260618".to_string()];

        let index = render_wml2_status_browser_index(&snapshot, 220).expect("browser index should fit");
        assert!(index.contains("MS 1 G 0 P 1 S 2"), "index={index:?}");

        let radios = render_wml2_status_browser_sector(&snapshot, 144, 2).expect("radio sector should fit");
        assert!(radios.contains("Radios 1"), "radios={radios:?}");
        assert!(!radios.contains("Radios 3/3"), "radios={radios:?}");
        assert!(radios.contains("2260618"), "radios={radios:?}");
        assert!(!radios.contains("MS 2260618"), "radios={radios:?}");
        assert!(radios.len() <= 144, "radios={radios:?}");
    }

    #[test]
    fn render_wml_status_browser_health_includes_tiny_uptime() {
        let mut snapshot = sample_snapshot();
        snapshot.uptime_secs = 0;

        let health = render_wml_status_browser_sector(&snapshot, 144, 1).expect("health sector should fit");

        assert!(health.contains("General OK"), "health={health:?}");
        assert!(health.contains("Activa 0d0h0s"), "health={health:?}");
        assert!(health.len() <= 144, "health={health:?}");
    }

    #[test]
    fn render_wml2_status_browser_health_includes_tiny_uptime() {
        let mut snapshot = sample_snapshot();
        snapshot.uptime_secs = 0;

        let health = render_wml2_status_browser_sector(&snapshot, 144, 1).expect("health sector should fit");

        assert!(health.contains("General OK"), "health={health:?}");
        assert!(health.contains("Activa 0d0h0s"), "health={health:?}");
        assert!(health.len() <= 144, "health={health:?}");
    }

    #[test]
    fn render_wml_status_browser_radios_compacts_issis_to_fit_page() {
        let mut snapshot = sample_snapshot();
        snapshot.radio_lines = vec![
            "MS 2260618 -38dB G1 SA".to_string(),
            "MS 2260616 -27dB G1 SA".to_string(),
            "MS 2260082 -31dB G1 SA".to_string(),
        ];

        let radios = render_wml_status_browser_sector(&snapshot, 144, 2).expect("radio sector should fit");

        assert!(radios.contains("Radios 3"), "radios={radios:?}");
        assert!(radios.contains("2260618"), "radios={radios:?}");
        assert!(radios.contains("2260616"), "radios={radios:?}");
        assert!(radios.contains("2260082"), "radios={radios:?}");
        assert!(!radios.contains("-38dB"), "radios={radios:?}");
        assert!(radios.len() <= 144, "radios={radios:?}");
    }

    #[test]
    fn compact_tiny_version_strips_build_channel_suffix() {
        assert_eq!(compact_tiny_version("v0.1.71_dev-4fc71583"), "0.1.71");
        assert_eq!(compact_tiny_version("0.1.62"), "0.1.62");
        assert_eq!(compact_tiny_version(""), "?");
    }

    #[test]
    fn compact_tiny_uptime_includes_days_hours_minutes_and_seconds() {
        assert_eq!(compact_tiny_uptime(0), "0d0h0m0s");
        assert_eq!(compact_tiny_uptime(93_784), "1d2h3m4s");
        assert_eq!(compact_tiny_uptime(u64::MAX), "99d23h59m59s");
    }

    #[test]
    fn compact_tiny_state_uses_short_status_labels() {
        let mut snapshot = sample_snapshot();
        assert_eq!(compact_tiny_state(&snapshot), "OK");

        snapshot.service_state = "DEGRADED".to_string();
        assert_eq!(compact_tiny_state(&snapshot), "Warn");

        snapshot.service_state = "CRITICAL".to_string();
        assert_eq!(compact_tiny_state(&snapshot), "Err");
    }

    #[test]
    fn render_wml2_status_tiny_page_fits_maximum_compact_uptime() {
        let mut snapshot = sample_snapshot();
        snapshot.uptime_secs = u64::MAX;
        let page = render_wml2_status(&snapshot, 128).expect("maximum compact uptime should still fit tiny WML2");

        assert!(page.len() <= 128);
        assert!(page.contains("Uptime 99d23h59m59s"));
    }

    #[test]
    fn render_wml2_status_enforces_nonempty_title_and_size_limit() {
        let mut snapshot = sample_snapshot();
        snapshot.title = "   ".to_string();
        assert_eq!(
            render_wml2_status(&snapshot, DEFAULT_WAP_STATUS_MAX_BYTES),
            Err(WapStatusError::EmptyTitle)
        );

        let snapshot = sample_snapshot();
        let err = render_wml2_status(&snapshot, 16).expect_err("tiny max size should reject rendered WML2");
        assert!(matches!(err, WapStatusError::RenderedTooLarge { max: 16, .. }));
    }

    #[test]
    fn compact_uptime_uses_short_wap_friendly_units() {
        assert_eq!(compact_uptime(12), "12s");
        assert_eq!(compact_uptime(61), "1m01s");
        assert_eq!(compact_uptime(3_661), "1h01m");
        assert_eq!(compact_uptime(90_000), "1d01h");
    }

    #[test]
    fn escape_xhtml_text_limited_truncates_after_xml_escaping() {
        assert_eq!(escape_xhtml_text_limited("&&&&", 10), "&amp;&amp;");
        assert_eq!(escape_xhtml_text_limited("&&&&", 11), "&amp;&amp;~");
        assert_eq!(escape_xhtml_text_limited("<tag>", 8), "&lt;tag~");
        assert_eq!(escape_xhtml_text_limited("abc", 0), "");
    }
}
