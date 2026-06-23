//! Snom XML minibrowser notifications via Asterisk AMI.
//!
//! This worker stays outside the real-time TETRA path. It consumes cloned telemetry / alert
//! events, builds a one-line `SnomIPPhoneText` XML document, and asks Asterisk AMI to send a
//! `PJSIPNotify` to configured endpoints.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use tetra_config::bluestation::{CfgSnomNotify, SharedConfig, parse_ric_route_key};

use crate::net_telemetry::TelemetryEvent;

const READ_LIMIT: usize = 64 * 1024;

static ACTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub enum SnomNotifyMsg {
    Event(TelemetryEvent),
    TelegramHtml(String),
    Meshcom {
        title: String,
        src: String,
        dst: Option<String>,
        text: String,
        msg_id: Option<String>,
    },
    Geoalarm {
        title: String,
        source: String,
        text: String,
        distance_m: f64,
        lat: f64,
        lon: f64,
    },
}

#[derive(Clone)]
pub struct SnomNotifySink {
    tx: crossbeam_channel::Sender<SnomNotifyMsg>,
}

impl SnomNotifySink {
    #[inline]
    pub fn send_event(&self, event: TelemetryEvent) {
        let _ = self.tx.send(SnomNotifyMsg::Event(event));
    }

    #[inline]
    pub fn send_telegram_html(&self, html: String) {
        let _ = self.tx.send(SnomNotifyMsg::TelegramHtml(html));
    }

    #[inline]
    pub fn send_meshcom(&self, title: String, src: String, dst: Option<String>, text: String, msg_id: Option<String>) {
        let _ = self.tx.send(SnomNotifyMsg::Meshcom {
            title,
            src,
            dst,
            text,
            msg_id,
        });
    }

    #[inline]
    pub fn send_geoalarm(&self, title: String, source: String, text: String, distance_m: f64, lat: f64, lon: f64) {
        let _ = self.tx.send(SnomNotifyMsg::Geoalarm {
            title,
            source,
            text,
            distance_m,
            lat,
            lon,
        });
    }
}

pub struct SnomNotifySource {
    rx: crossbeam_channel::Receiver<SnomNotifyMsg>,
}

impl SnomNotifySource {
    fn recv(&self) -> Result<SnomNotifyMsg, crossbeam_channel::RecvError> {
        self.rx.recv()
    }
}

pub fn snom_notify_channel() -> (SnomNotifySink, SnomNotifySource) {
    let (tx, rx) = crossbeam_channel::unbounded();
    (SnomNotifySink { tx }, SnomNotifySource { rx })
}

pub fn spawn_snom_notify_worker(cfg: SharedConfig, source: SnomNotifySource) -> Option<thread::JoinHandle<()>> {
    match thread::Builder::new()
        .name("snom-notify".into())
        .spawn(move || SnomNotifyWorker::new(cfg, source).run())
    {
        Ok(handle) => Some(handle),
        Err(err) => {
            tracing::warn!("Snom notify: failed to spawn worker thread: {}", err);
            None
        }
    }
}

struct SnomNotifyWorker {
    cfg: SharedConfig,
    source: SnomNotifySource,
}

impl SnomNotifyWorker {
    fn new(cfg: SharedConfig, source: SnomNotifySource) -> Self {
        Self { cfg, source }
    }

    fn run(&self) {
        tracing::info!("Snom notify worker started");
        while let Ok(msg) = self.source.recv() {
            let cfg = self.cfg.effective_snom_notify();
            if !cfg.enabled {
                continue;
            }
            let Some(notification) = self.notification_for_msg(&cfg, msg) else {
                continue;
            };
            if let Err(err) = send_notification(&cfg, &notification.title, &notification.lines) {
                tracing::warn!("Snom notify: {}", err);
            }
        }
        tracing::info!("Snom notify worker exiting");
    }

    fn notification_for_msg(&self, cfg: &CfgSnomNotify, msg: SnomNotifyMsg) -> Option<SnomNotification> {
        match msg {
            SnomNotifyMsg::Event(TelemetryEvent::SdsLog {
                direction,
                source_issi,
                dest_issi,
                is_group,
                protocol_id,
                text,
            }) => {
                if !cfg.notify_sds || !direction_allowed(cfg, &direction) {
                    return None;
                }
                if !sds_issi_allowed(cfg, source_issi, dest_issi) {
                    return None;
                }
                let mut lines = vec![
                    format!("Dir: {}", direction.to_ascii_uppercase()),
                    format!("From: {source_issi}"),
                    format!("To: {}{}", if is_group { "GSSI " } else { "" }, dest_issi),
                ];
                if protocol_id != 0 {
                    lines.push(format!("PID: {protocol_id}"));
                }
                lines.push(format_message_line("Text", &text, cfg.max_text_chars));
                Some(SnomNotification {
                    title: prefixed_title(&cfg.title_prefix, "TETRA SDS"),
                    lines,
                })
            }
            SnomNotifyMsg::Event(TelemetryEvent::DapnetLog {
                direction,
                id: _,
                callsign,
                recipient,
                text,
                priority,
                paths,
            }) => {
                if !cfg.notify_dapnet {
                    return None;
                }
                if !dapnet_ric_allowed(cfg, &recipient) {
                    return None;
                }
                let mut lines = vec![
                    format!("Dir: {}", direction.to_ascii_uppercase()),
                    format!("From: {callsign}"),
                    format!("To: {recipient}"),
                ];
                if let Some(priority) = priority {
                    lines.push(format!("Priority: {priority}"));
                }
                if !paths.is_empty() {
                    lines.push(format!("Paths: {}", paths.join(",")));
                }
                lines.push(format_message_line("Text", &text, cfg.max_text_chars));
                Some(SnomNotification {
                    title: prefixed_title(&cfg.title_prefix, "DAPNET"),
                    lines,
                })
            }
            SnomNotifyMsg::TelegramHtml(html) => {
                if !cfg.notify_telegram {
                    return None;
                }
                let text = html_to_text(&html);
                Some(SnomNotification {
                    title: prefixed_title(&cfg.title_prefix, "Telegram"),
                    lines: split_for_snom(&text, cfg.max_text_chars),
                })
            }
            SnomNotifyMsg::Meshcom {
                title,
                src,
                dst,
                text,
                msg_id,
            } => {
                let mut lines = vec![format!("From: {src}")];
                if let Some(dst) = dst.filter(|v| !v.trim().is_empty()) {
                    lines.push(format!("To: {dst}"));
                }
                if let Some(msg_id) = msg_id.filter(|v| !v.trim().is_empty()) {
                    lines.push(format!("ID: {msg_id}"));
                }
                lines.push(format_message_line("Text", &text, cfg.max_text_chars));
                Some(SnomNotification {
                    title: prefixed_title(&cfg.title_prefix, &title),
                    lines,
                })
            }
            SnomNotifyMsg::Geoalarm {
                title,
                source,
                text,
                distance_m,
                lat,
                lon,
            } => {
                let lines = vec![
                    format!("From: {source}"),
                    format!("Distance: {:.0} m", distance_m.max(0.0)),
                    format!("Pos: {:.6}, {:.6}", lat, lon),
                    format_message_line("Text", &text, cfg.max_text_chars),
                ];
                Some(SnomNotification {
                    title: prefixed_title(&cfg.title_prefix, &title),
                    lines,
                })
            }
            _ => None,
        }
    }
}

struct SnomNotification {
    title: String,
    lines: Vec<String>,
}

fn direction_allowed(cfg: &CfgSnomNotify, direction: &str) -> bool {
    cfg.sds_directions.is_empty() || cfg.sds_directions.iter().any(|d| d.eq_ignore_ascii_case(direction.trim()))
}

fn sds_issi_allowed(cfg: &CfgSnomNotify, source_issi: u32, dest_issi: u32) -> bool {
    cfg.sds_allowed_issis.is_empty() || cfg.sds_allowed_issis.contains(&source_issi) || cfg.sds_allowed_issis.contains(&dest_issi)
}

fn dapnet_ric_allowed(cfg: &CfgSnomNotify, recipient: &str) -> bool {
    if cfg.dapnet_allowed_rics.is_empty() {
        return true;
    }
    dapnet_recipient_ric(recipient)
        .map(|ric| cfg.dapnet_allowed_rics.contains(&ric))
        .unwrap_or(false)
}

fn dapnet_recipient_ric(recipient: &str) -> Option<u32> {
    let trimmed = recipient.trim();
    let rest = trimmed.strip_prefix("RIC ")?;
    let token = rest.split_whitespace().next()?;
    parse_ric_route_key(token).ok()
}

fn prefixed_title(prefix: &str, title: &str) -> String {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        title.to_string()
    } else {
        format!("{prefix} {title}")
    }
}

fn format_message_line(label: &str, text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        format!("{label}: -")
    } else {
        format!("{label}: {}", truncate_chars(trimmed, max_chars))
    }
}

fn split_for_snom(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| truncate_chars(l, max_chars))
        .collect();
    if lines.is_empty() {
        lines.push("-".to_string());
    }
    lines
}

fn send_notification(cfg: &CfgSnomNotify, title: &str, lines: &[String]) -> Result<(), String> {
    if cfg.endpoints.is_empty() {
        return Err("enabled but no endpoints configured".to_string());
    }
    if cfg.ami_username.trim().is_empty() || cfg.ami_password.as_ref().trim().is_empty() {
        return Err("enabled but AMI credentials are incomplete".to_string());
    }

    let xml = snom_ip_phone_text_xml(title, lines);
    let timeout = Duration::from_secs(cfg.connect_timeout_secs);
    let mut stream = connect_ami(&cfg.ami_host, cfg.ami_port, timeout)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("AMI set_read_timeout failed: {}", e))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("AMI set_write_timeout failed: {}", e))?;

    ami_action(
        &mut stream,
        &[
            ("Action", "Login".to_string()),
            ("Username", cfg.ami_username.clone()),
            ("Secret", cfg.ami_password.as_ref().to_string()),
            ("Events", "off".to_string()),
            ("ActionID", next_action_id("login")),
        ],
    )
    .map_err(|e| format!("AMI login failed: {}", e))?;

    for endpoint in &cfg.endpoints {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            continue;
        }
        let fields = vec![
            ("Action", "PJSIPNotify".to_string()),
            ("ActionID", next_action_id("notify")),
            ("Endpoint", endpoint.to_string()),
            ("Variable", format!("Event={}", cfg.notify_event)),
            ("Variable", format!("Content-Type={}", cfg.content_type)),
            ("Variable", format!("Subscription-State={}", cfg.subscription_state)),
            ("Variable", format!("Content={}", sanitize_ami_value(&xml))),
        ];
        ami_action(&mut stream, &fields).map_err(|e| format!("PJSIPNotify endpoint {} failed: {}", endpoint, e))?;
    }

    let _ = ami_action(
        &mut stream,
        &[("Action", "Logoff".to_string()), ("ActionID", next_action_id("logoff"))],
    );
    Ok(())
}

fn connect_ami(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let addr = format!("{host}:{port}");
    let mut last_err = None;
    let addrs = addr.to_socket_addrs().map_err(|e| format!("AMI resolve {} failed: {}", addr, e))?;
    for socket in addrs {
        match TcpStream::connect_timeout(&socket, timeout) {
            Ok(stream) => {
                let mut stream = stream;
                // Consume the AMI greeting if it is immediately available; if not, the next
                // action response read still works because it waits for a complete AMI block.
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                let _ = read_ami_block(&mut stream);
                let _ = stream.set_read_timeout(Some(timeout));
                return Ok(stream);
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(format!(
        "AMI connect {} failed: {}",
        addr,
        last_err.map(|e| e.to_string()).unwrap_or_else(|| "no socket addresses".to_string())
    ))
}

fn ami_action(stream: &mut TcpStream, fields: &[(&str, String)]) -> Result<String, String> {
    let mut request = String::new();
    for (name, value) in fields {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(&sanitize_ami_value(value));
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write failed: {}", e))?;
    let block = read_ami_block(stream)?;
    if block.contains("Response: Error") {
        Err(sanitize_response(&block))
    } else {
        Ok(block)
    }
}

fn read_ami_block(stream: &mut TcpStream) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp).map_err(|e| format!("read failed: {}", e))?;
        if n == 0 {
            return Err("connection closed".to_string());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > READ_LIMIT {
            return Err("response too large".to_string());
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn next_action_id(kind: &str) -> String {
    let id = ACTION_ID.fetch_add(1, Ordering::Relaxed);
    format!("flowstation-snom-{kind}-{id}")
}

fn sanitize_ami_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn sanitize_response(response: &str) -> String {
    response
        .lines()
        .filter(|line| !line.starts_with("Secret:"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn snom_ip_phone_text_xml(title: &str, lines: &[String]) -> String {
    let text = lines.iter().map(|l| xml_escape(l)).collect::<Vec<_>>().join("<br/>");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><SnomIPPhoneText has_scrollbar="yes"><Title>{}</Title><Text>{}</Text></SnomIPPhoneText>"#,
        xml_escape(title),
        text
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                let normalized = tag
                    .trim()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if normalized == "br" || normalized == "p" || normalized == "div" {
                    out.push('\n');
                }
                in_tag = false;
            }
            _ if in_tag => tag.push(ch),
            _ => out.push(ch),
        }
    }
    decode_basic_html_entities(&out)
}

fn decode_basic_html_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snom_xml_escapes_dynamic_text_and_uses_br() {
        let xml = snom_ip_phone_text_xml("Flow <SDS>", &["From: 1".to_string(), "Text: a & b".to_string()]);
        assert!(xml.contains("<SnomIPPhoneText"));
        assert!(xml.contains("<Title>Flow &lt;SDS&gt;</Title>"));
        assert!(xml.contains("From: 1<br/>Text: a &amp; b"));
        assert!(!xml.contains('\n'));
    }

    #[test]
    fn html_to_text_strips_telegram_markup() {
        let text = html_to_text("X <b>DAPNET</b>\nCall: <code>DJ2TH</code>&amp;test");
        assert!(text.contains("DAPNET"));
        assert!(text.contains("DJ2TH&test"));
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn ami_value_is_single_line() {
        assert_eq!(sanitize_ami_value("a\r\nb"), "a  b");
    }

    #[test]
    fn dapnet_ric_filter_extracts_decimal_ric() {
        assert_eq!(dapnet_recipient_ric("RIC 0632585 / func 3"), Some(632585));
        assert_eq!(dapnet_recipient_ric("DJ2TH"), None);
    }
}
