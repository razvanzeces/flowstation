//! DAPNET inbound-message forwarding.
//!
//! The receiver uses the DAPNET RWTH core transmitter TCP protocol. It does not transmit POCSAG;
//! it only consumes incoming calls from the core feed, acknowledges them, normalizes the message,
//! and forwards it through existing FlowStation paths.

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use tetra_config::bluestation::{CfgDapnet, DapnetRuntimeStatus, SharedConfig};

use crate::net_control::commands::ControlCommand;
use crate::net_telegram::TelegramAlertSink;
use crate::net_telemetry::{TelemetryEvent, TelemetrySink};
use crate::tpg2200::{build_sds_text_payload, build_tpg2200_callout_payload, format_hex_bytes};

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

const TCP_READ_TIMEOUT: Duration = Duration::from_secs(30);
const CALLOUT_TEXT_MAX_CHARS: usize = 80;

#[derive(Debug, Clone)]
struct DapnetMessage {
    id: String,
    callsign: String,
    recipient: String,
    text: String,
    timestamp: String,
    priority: Option<u8>,
    msg_type: u8,
    speed: Option<u8>,
    ric: Option<u32>,
    function: Option<u8>,
}

pub fn spawn_dapnet_worker(
    cfg: SharedConfig,
    cmce_cmd_tx: Option<CmdSender>,
    telegram_sink: Option<TelegramAlertSink>,
    telemetry_sink: Option<TelemetrySink>,
) -> Option<thread::JoinHandle<()>> {
    match thread::Builder::new()
        .name("dapnet-worker".into())
        .spawn(move || DapnetWorker::new(cfg, cmce_cmd_tx, telegram_sink, telemetry_sink).run())
    {
        Ok(handle) => Some(handle),
        Err(err) => {
            tracing::warn!("DAPNET: failed to spawn worker thread: {}", err);
            None
        }
    }
}

struct DapnetWorker {
    cfg: SharedConfig,
    cmce_cmd_tx: Option<CmdSender>,
    telegram_sink: Option<TelegramAlertSink>,
    telemetry_sink: Option<TelemetrySink>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    next_callout_incident: u16,
    last_callout_incident_base: Option<u16>,
    last_enabled: Option<bool>,
}

impl DapnetWorker {
    fn new(
        cfg: SharedConfig,
        cmce_cmd_tx: Option<CmdSender>,
        telegram_sink: Option<TelegramAlertSink>,
        telemetry_sink: Option<TelemetrySink>,
    ) -> Self {
        let next_callout_incident = cfg.effective_dapnet().callout_incident_base.clamp(1, 256);
        Self {
            cfg,
            cmce_cmd_tx,
            telegram_sink,
            telemetry_sink,
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            next_callout_incident,
            last_callout_incident_base: None,
            last_enabled: None,
        }
    }

    fn refresh_status(&self, dapnet: &CfgDapnet, rwth_core_status: impl Into<String>, last_rx: Option<String>, last_error: Option<String>) {
        let mut state = self.cfg.state_write();
        let previous_last_rx = state.dapnet_status.last_rx.clone();
        state.dapnet_status = DapnetRuntimeStatus {
            configured: true,
            enabled: dapnet.enabled,
            rwth_core_enabled: dapnet.rwth_core_enabled,
            rwth_core_status: rwth_core_status.into(),
            endpoint: format!("{}:{}", dapnet.rwth_core_host, dapnet.rwth_core_port),
            callsign: dapnet.rwth_core_callsign.clone(),
            forward_sds: dapnet.forward_sds,
            forward_callout: dapnet.forward_callout,
            forward_telegram: dapnet.forward_telegram,
            seen_messages: self.seen.len(),
            last_rx: last_rx.or(previous_last_rx),
            last_error,
        };
    }

    fn run(&mut self) {
        loop {
            let dapnet = self.cfg.effective_dapnet();
            let sleep = Duration::from_secs(dapnet.effective_poll_interval_secs());

            if !dapnet.enabled {
                self.refresh_status(&dapnet, "disabled", None, None);
                if self.last_enabled != Some(false) {
                    tracing::info!("DAPNET integration disabled");
                    self.last_enabled = Some(false);
                }
                thread::sleep(sleep);
                continue;
            }
            if self.last_enabled != Some(true) {
                tracing::info!(
                    "DAPNET integration enabled (rwth_core={}, forward_sds={}, forward_callout={}, forward_telegram={})",
                    dapnet.rwth_core_enabled,
                    dapnet.forward_sds,
                    dapnet.forward_callout,
                    dapnet.forward_telegram
                );
                if !(dapnet.forward_sds || dapnet.forward_callout || dapnet.forward_telegram) {
                    tracing::warn!("DAPNET: enabled but no forwarding target is enabled");
                }
                self.last_callout_incident_base = None;
                self.last_enabled = Some(true);
            }
            let incident_base = dapnet.callout_incident_base.clamp(1, 256);
            if self.last_callout_incident_base != Some(incident_base) {
                self.next_callout_incident = incident_base;
                self.last_callout_incident_base = Some(incident_base);
            }

            if dapnet.rwth_core_enabled {
                self.refresh_status(&dapnet, "connecting", None, None);
                if let Err(err) = self.run_rwth_core(&dapnet) {
                    tracing::warn!("DAPNET: RWTH core receive failed: {}", err);
                    self.refresh_status(&dapnet, "error", None, Some(err));
                }
            } else {
                tracing::warn!(
                    "DAPNET: enabled, but rwth_core_enabled=false; no inbound receiver is active (api_url={})",
                    dapnet.api_url
                );
                self.refresh_status(&dapnet, "receive disabled", None, None);
            }

            thread::sleep(sleep);
        }
    }

    fn run_rwth_core(&mut self, dapnet: &CfgDapnet) -> Result<(), String> {
        let host = dapnet.rwth_core_host.trim();
        let callsign = dapnet.rwth_core_callsign.trim();
        let authkey = dapnet.rwth_core_authkey.as_ref().trim();
        if host.is_empty() {
            return Err("RWTH core host is empty".to_string());
        }
        if callsign.is_empty() {
            return Err("RWTH core callsign is empty".to_string());
        }
        if authkey.is_empty() {
            return Err("RWTH core authkey is empty".to_string());
        }

        let addr = format!("{}:{}", host, dapnet.rwth_core_port);
        tracing::info!("DAPNET: connecting to RWTH core {} as {}", addr, callsign);
        let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {} failed: {}", addr, e))?;
        self.refresh_status(dapnet, "connected", None, None);
        if let Err(err) = stream.set_read_timeout(Some(TCP_READ_TIMEOUT)) {
            tracing::warn!("DAPNET: could not set TCP read timeout: {}", err);
        }

        self.write_login(&mut stream, dapnet)?;
        let reader_stream = stream
            .try_clone()
            .map_err(|e| format!("failed to clone RWTH core TCP stream: {}", e))?;
        let mut reader = BufReader::new(reader_stream);
        let mut logged_in = false;

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return Err("RWTH core closed connection".to_string()),
                Ok(_) => {
                    let line = line.trim_end_matches(|c| c == '\r' || c == '\n');
                    if line.is_empty() {
                        continue;
                    }
                    match self.handle_rwth_line(dapnet, &mut stream, line, &mut logged_in) {
                        Ok(()) => {}
                        Err(err) => return Err(err),
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                    continue;
                }
                Err(err) => return Err(format!("read failed: {}", err)),
            }
        }
    }

    fn write_login(&self, stream: &mut TcpStream, dapnet: &CfgDapnet) -> Result<(), String> {
        let device = non_empty_or(&dapnet.rwth_core_device, "FlowStation");
        let version = dapnet_version(&dapnet.rwth_core_version);
        let callsign = dapnet.rwth_core_callsign.trim();
        let authkey = dapnet.rwth_core_authkey.as_ref().trim();
        let login = format!("[{} {} {} {}]\r\n", device, version, callsign, authkey);
        write_wire(stream, &login)
    }

    fn handle_rwth_line(&mut self, dapnet: &CfgDapnet, stream: &mut TcpStream, line: &str, logged_in: &mut bool) -> Result<(), String> {
        if line.starts_with('+') {
            return Ok(());
        }
        if line.starts_with('-') {
            tracing::warn!("DAPNET: RWTH core reported an error");
            return Ok(());
        }
        if line.starts_with('2') {
            if !*logged_in {
                tracing::info!("DAPNET: logged into RWTH core");
                *logged_in = true;
                self.refresh_status(dapnet, "logged in", None, None);
            }
            write_wire(stream, &format!("{line}:0000\r\n+\r\n"))?;
            return Ok(());
        }
        if line.starts_with('3') {
            write_wire(stream, "+\r\n")?;
            return Ok(());
        }
        if let Some(schedule) = line.strip_prefix("4:") {
            tracing::info!("DAPNET: RWTH core schedule received ({})", schedule);
            write_wire(stream, "+\r\n")?;
            return Ok(());
        }
        if line.starts_with('7') {
            return Err(format!("login rejected by RWTH core: {}", sanitize_log_line(line)));
        }
        if line.starts_with('#') {
            self.handle_rwth_message(dapnet, stream, line)?;
            return Ok(());
        }

        tracing::warn!("DAPNET: unknown RWTH core message type '{}'", sanitize_log_line(line));
        write_wire(stream, "-\r\n")
    }

    fn handle_rwth_message(&mut self, dapnet: &CfgDapnet, stream: &mut TcpStream, line: &str) -> Result<(), String> {
        let msg_id = match rwth_line_id(line) {
            Some(id) => id,
            None => {
                tracing::warn!("DAPNET: malformed RWTH core message without valid id");
                write_wire(stream, "-\r\n")?;
                return Ok(());
            }
        };
        match parse_rwth_message(line) {
            Ok(message) => {
                write_wire(stream, &rwth_ack_line(msg_id, true))?;
                if message.msg_type != 6 {
                    tracing::debug!(
                        "DAPNET: ignoring non-text RWTH core message id={} type={}",
                        message.id,
                        message.msg_type
                    );
                    return Ok(());
                }
                if !self.remember_seen(&message.id, dapnet.effective_messages_limit()) {
                    tracing::debug!("DAPNET: duplicate message id={} ignored", message.id);
                    return Ok(());
                }
                self.refresh_status(
                    dapnet,
                    "logged in",
                    Some(format!("{} from {}", message.id, message.recipient)),
                    None,
                );
                self.forward_message(dapnet, &message);
                Ok(())
            }
            Err(err) => {
                tracing::warn!("DAPNET: malformed RWTH core message: {}", err);
                write_wire(stream, &rwth_ack_line(msg_id, false))
            }
        }
    }

    fn remember_seen(&mut self, id: &str, limit: usize) -> bool {
        if !self.seen.insert(id.to_string()) {
            return false;
        }
        self.seen_order.push_back(id.to_string());
        while self.seen_order.len() > limit {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }

    fn forward_message(&mut self, dapnet: &CfgDapnet, msg: &DapnetMessage) {
        let mut paths: Vec<&str> = Vec::new();
        let mut filtered: Vec<&str> = Vec::new();

        if dapnet.forward_sds {
            if ric_allowed(&dapnet.sds_allowed_rics, msg) {
                match self.forward_sds(dapnet, msg) {
                    Ok(()) => paths.push("sds"),
                    Err(err) => tracing::warn!("DAPNET: SDS forward failed for id={}: {}", msg.id, err),
                }
            } else {
                filtered.push("sds");
            }
        }

        if dapnet.forward_callout {
            if ric_allowed(&dapnet.callout_allowed_rics, msg) {
                match self.forward_callout(dapnet, msg) {
                    Ok(()) => paths.push("callout"),
                    Err(err) => tracing::warn!("DAPNET: Call-Out forward failed for id={}: {}", msg.id, err),
                }
            } else {
                filtered.push("callout");
            }
        }

        if dapnet.forward_telegram {
            if ric_allowed(&dapnet.telegram_allowed_rics, msg) {
                match self.forward_telegram(dapnet, msg) {
                    Ok(()) => paths.push("telegram"),
                    Err(err) => tracing::warn!("DAPNET: Telegram forward failed for id={}: {}", msg.id, err),
                }
            } else {
                filtered.push("telegram");
            }
        }

        if paths.is_empty() {
            if filtered.is_empty() {
                tracing::info!(
                    "DAPNET: received id={} recipient={} with no successful forwarding target",
                    msg.id,
                    msg.recipient
                );
            } else {
                tracing::debug!(
                    "DAPNET: received id={} recipient={} filtered out for paths={}",
                    msg.id,
                    msg.recipient,
                    filtered.join(",")
                );
            }
        } else {
            tracing::info!(
                "DAPNET: forwarded id={} recipient={} callsign={} paths={} timestamp={} priority={:?}",
                msg.id,
                msg.recipient,
                msg.callsign,
                paths.join(","),
                msg.timestamp,
                msg.priority
            );
        }
        if let Some(sink) = &self.telemetry_sink {
            sink.send(TelemetryEvent::DapnetLog {
                direction: "rx".to_string(),
                id: msg.id.clone(),
                callsign: msg.callsign.clone(),
                recipient: msg.recipient.clone(),
                text: msg.text.clone(),
                priority: msg.priority,
                paths: paths.into_iter().map(|p| p.to_string()).collect(),
            });
        }
    }

    fn forward_sds(&self, dapnet: &CfgDapnet, msg: &DapnetMessage) -> Result<(), String> {
        let (dest_ssi, dest_is_group, route_label) = resolve_sds_destination(dapnet, msg)?;
        let Some(tx) = &self.cmce_cmd_tx else {
            return Err("CMCE control sender unavailable".to_string());
        };
        let text = format_plain_message(&msg.callsign, &msg.text);
        let (len_bits, payload) = build_sds_text_payload(&text);
        tracing::debug!(
            "DAPNET: SDS route id={} {} dest={} group={}",
            msg.id,
            route_label,
            dest_ssi,
            dest_is_group
        );
        tx.send(ControlCommand::SendSds {
            handle: 0,
            source_ssi: dapnet.sds_source_issi,
            dest_ssi,
            dest_is_group,
            len_bits,
            payload,
        })
        .map_err(|e| format!("send to CMCE failed: {}", e))
    }

    fn forward_callout(&mut self, dapnet: &CfgDapnet, msg: &DapnetMessage) -> Result<(), String> {
        if dapnet.callout_dest_issi == 0 {
            return Err("callout_dest_issi is 0".to_string());
        }
        let Some(tx) = self.cmce_cmd_tx.clone() else {
            return Err("CMCE control sender unavailable".to_string());
        };
        let incident = self.next_incident();
        let callout_text = prefixed_text(&dapnet.callout_text_prefix, &msg.text);
        let (callout_text, truncated) = truncate_chars(&callout_text, CALLOUT_TEXT_MAX_CHARS);
        if truncated {
            tracing::warn!(
                "DAPNET: TPG2200 Call-Out text for id={} truncated to {} chars",
                msg.id,
                CALLOUT_TEXT_MAX_CHARS
            );
        }
        let payload = build_tpg2200_callout_payload(incident, &callout_text);
        if payload.len() > (u16::MAX as usize / 8) {
            return Err(format!("payload too large ({} bytes)", payload.len()));
        }
        tracing::debug!(
            "DAPNET: TPG2200 Call-Out id={} incident={} dest={} payload=[{}]",
            msg.id,
            incident,
            dapnet.callout_dest_issi,
            format_hex_bytes(&payload)
        );
        tx.send(ControlCommand::SendRawSdsType4 {
            handle: 0,
            source_ssi: dapnet.callout_source_issi,
            dest_ssi: dapnet.callout_dest_issi,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        })
        .map_err(|e| format!("send to CMCE failed: {}", e))
    }

    fn forward_telegram(&self, dapnet: &CfgDapnet, msg: &DapnetMessage) -> Result<(), String> {
        let Some(sink) = &self.telegram_sink else {
            return Err("Telegram alerter unavailable".to_string());
        };
        sink.send_dapnet(dapnet.telegram_prefix.clone(), msg.callsign.clone(), msg.text.clone());
        Ok(())
    }

    fn next_incident(&mut self) -> u16 {
        let incident = self.next_callout_incident.clamp(1, 256);
        self.next_callout_incident = if incident >= 256 { 1 } else { incident + 1 };
        incident
    }
}

fn write_wire(stream: &mut TcpStream, text: &str) -> Result<(), String> {
    stream
        .write_all(text.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write failed: {}", e))
}

fn dapnet_version(version: &str) -> String {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        "v1.0".to_string()
    } else if trimmed.starts_with('v') || trimmed.starts_with('V') {
        trimmed.to_string()
    } else {
        format!("v{trimmed}")
    }
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn rwth_line_id(line: &str) -> Option<u8> {
    let id = line.get(1..3)?;
    u8::from_str_radix(id, 16).ok()
}

fn rwth_ack_line(msg_id: u8, ok: bool) -> String {
    let ack_id = msg_id.wrapping_add(1);
    let status = if ok { "+" } else { "-" };
    format!("#{ack_id:02x} {status}\r\n")
}

fn resolve_sds_destination(dapnet: &CfgDapnet, msg: &DapnetMessage) -> Result<(u32, bool, String), String> {
    if let Some(ric) = msg.ric {
        if let Some(gssi) = dapnet.ric_gssi_routes.get(&ric) {
            return Ok((
                *gssi,
                true,
                format!("ric-group:{}", tetra_config::bluestation::format_ric_route_key(ric)),
            ));
        }
        if let Some(issi) = dapnet.ric_issi_routes.get(&ric) {
            return Ok((
                *issi,
                false,
                format!("ric:{}", tetra_config::bluestation::format_ric_route_key(ric)),
            ));
        }
    }
    if dapnet.sds_dest_issi != 0 {
        return Ok((dapnet.sds_dest_issi, dapnet.sds_dest_is_group, "default".to_string()));
    }
    if let Some(ric) = msg.ric {
        Err(format!(
            "no ISSI route for RIC {} and sds_dest_issi is 0",
            tetra_config::bluestation::format_ric_route_key(ric)
        ))
    } else {
        Err("message has no RIC and sds_dest_issi is 0".to_string())
    }
}

fn ric_allowed(allowed_rics: &std::collections::BTreeSet<u32>, msg: &DapnetMessage) -> bool {
    if allowed_rics.is_empty() {
        return true;
    }
    match msg.ric {
        Some(ric) => allowed_rics.contains(&ric),
        None => false,
    }
}

fn parse_rwth_message(line: &str) -> Result<DapnetMessage, String> {
    let msg_id = rwth_line_id(line).ok_or_else(|| "invalid message id".to_string())?;
    let body = line.get(4..).ok_or_else(|| "message line too short".to_string())?;
    let parts: Vec<&str> = body.splitn(5, ':').collect();
    if parts.len() != 5 {
        return Err("expected five colon-separated fields".to_string());
    }
    let msg_type = parts[0].parse::<u8>().map_err(|_| format!("invalid message type '{}'", parts[0]))?;
    let speed = parts[1].parse::<u8>().ok();
    let ric = u32::from_str_radix(parts[2], 16).ok();
    let function = parts[3].parse::<u8>().ok();
    let text = decode_dapnet_text(&normalize_text(parts[4]));
    if text.is_empty() {
        return Err("empty message text".to_string());
    }
    let recipient = match (ric, function) {
        (Some(ric), Some(function)) => {
            format!("RIC {} / func {}", tetra_config::bluestation::format_ric_route_key(ric), function)
        }
        (Some(ric), None) => {
            format!("RIC {}", tetra_config::bluestation::format_ric_route_key(ric))
        }
        _ => parts[2].to_string(),
    };
    let callsign = extract_callsign(&text).unwrap_or_default();
    let id = format!("rwth:{msg_id:02X}:{}", stable_hash_hex(body));
    Ok(DapnetMessage {
        id,
        callsign,
        recipient,
        text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        priority: None,
        msg_type,
        speed,
        ric,
        function,
    })
}

fn stable_hash_hex(input: &str) -> String {
    let digest = md5::compute(input.as_bytes());
    format!("{digest:x}")
}

fn normalize_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\t'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn decode_dapnet_text(text: &str) -> String {
    let decoded = rot1_decode(text);
    if should_decode_rot1(text, &decoded) {
        clean_rot1_text(&decoded)
    } else {
        text.to_string()
    }
}

fn rot1_decode(text: &str) -> String {
    text.chars()
        .map(|c| if ('!'..='~').contains(&c) { ((c as u8) - 1) as char } else { c })
        .collect()
}

fn clean_rot1_text(decoded: &str) -> String {
    let stripped = strip_skyper_rubric_prefix(decoded.trim()).unwrap_or_else(|| decoded.trim());
    skyper_charset_to_unicode(stripped.trim())
}

fn strip_skyper_rubric_prefix(text: &str) -> Option<&str> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    let (_, second) = chars.next()?;
    let (third_idx, third) = chars.next()?;
    let after_third_idx = third_idx + third.len_utf8();

    if first.is_ascii_digit() && third == ')' {
        let rest = &text[after_third_idx..];
        if rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '[' | '\\' | ']' | '{' | '|' | '}' | '~'))
        {
            return Some(rest);
        }
    }

    if !third.is_ascii_alphanumeric() {
        return None;
    }

    if first == ':' && second == ' ' {
        return Some(&text[third_idx..]);
    }
    if first.is_ascii_punctuation() && (second.is_ascii_punctuation() || second.is_ascii_whitespace() || second.is_ascii_digit()) {
        return Some(&text[third_idx..]);
    }
    if matches!(first, 'Q' | 'q' | 'Y' | 'y' | 'N' | 'n') && (second.is_ascii_punctuation() || second.is_ascii_whitespace()) {
        return Some(&text[third_idx..]);
    }
    None
}

fn skyper_charset_to_unicode(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '[' => 'Ä',
            '\\' => 'Ö',
            ']' => 'Ü',
            '{' => 'ä',
            '|' => 'ö',
            '}' => 'ü',
            '~' => 'ß',
            _ => c,
        })
        .collect()
}

fn should_decode_rot1(raw: &str, decoded: &str) -> bool {
    if raw.is_empty() {
        return false;
    }
    if strip_skyper_rubric_prefix(decoded).is_some() {
        return true;
    }
    let raw_spaces = raw.chars().filter(|&c| c == ' ').count();
    let encoded_spaces = raw.chars().filter(|&c| c == '!').count();
    let encoded_punctuation = raw.chars().filter(|&c| matches!(c, ';' | '/' | '{' | '}')).count();
    let decoded_spaces = decoded.chars().filter(|&c| c == ' ').count();

    // DAPNET/Skyper rubrics are ROT-1 encrypted: spaces appear as '!', ':' as ';',
    // and '.' as '/'. Plain DAPNET messages from the core already contain normal spaces
    // and must stay untouched.
    encoded_spaces >= 2 && encoded_spaces > raw_spaces && decoded_spaces >= encoded_spaces && encoded_punctuation > 0
}

fn extract_callsign(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '/');
        if cleaned.len() < 3 || cleaned.len() > 12 {
            continue;
        }
        let has_alpha = cleaned.chars().any(|c| c.is_ascii_alphabetic());
        let has_digit = cleaned.chars().any(|c| c.is_ascii_digit());
        let valid = cleaned.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/');
        if has_alpha && has_digit && valid {
            return Some(cleaned.to_ascii_uppercase());
        }
    }
    None
}

fn format_plain_message(callsign: &str, text: &str) -> String {
    let callsign = callsign.trim();
    let text = text.trim();
    if callsign.is_empty() {
        text.to_string()
    } else {
        format!("{callsign} - {text}")
    }
}

fn prefixed_text(prefix: &str, text: &str) -> String {
    let prefix = prefix.trim();
    let text = text.trim();
    if prefix.is_empty() {
        text.to_string()
    } else if text.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {text}")
    }
}

fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((idx, _)) => (text[..idx].to_string(), true),
        None => (text.to_string(), false),
    }
}

fn sanitize_log_line(line: &str) -> String {
    truncate_chars(line, 160).0
}

#[cfg(test)]
mod tests {
    use super::{
        dapnet_version, decode_dapnet_text, extract_callsign, format_plain_message, parse_rwth_message, prefixed_text,
        resolve_sds_destination, ric_allowed, rwth_ack_line, truncate_chars,
    };
    use tetra_config::bluestation::CfgDapnet;

    #[test]
    fn parse_rwth_text_message_normalizes_fields() {
        let msg = parse_rwth_message("#00 6:1:3EC:3:5357.0 EA5FIV von DL4MFF um 1933z").unwrap();
        assert_eq!(msg.msg_type, 6);
        assert_eq!(msg.speed, Some(1));
        assert_eq!(msg.ric, Some(0x3EC));
        assert_eq!(msg.function, Some(3));
        assert_eq!(msg.callsign, "EA5FIV");
        assert_eq!(msg.recipient, "RIC 0001004 / func 3");
        assert!(msg.id.starts_with("rwth:00:"));
    }

    #[test]
    fn parse_rwth_message_keeps_colons_in_text() {
        let msg = parse_rwth_message("#01 6:1:3EC:3:Alarm: Pumpe: Test").unwrap();
        assert_eq!(msg.text, "Alarm: Pumpe: Test");
    }

    #[test]
    fn parse_rwth_message_does_not_copy_recipient_into_callsign() {
        let msg = parse_rwth_message("#02 6:1:E0:3:YYYYMMDDHHMMSS260626185400").unwrap();
        assert_eq!(msg.recipient, "RIC 0000224 / func 3");
        assert_eq!(msg.callsign, "");
    }

    #[test]
    fn dapnet_text_decodes_skyper_rot1_but_keeps_plain_text() {
        let darc_70mhz = ";#EBSD;!Cvoeftsbut.Esvdltbdif!efgjojfsu!jo!Gvopuf!Sfdiutsbinfo!g~s!81.NI{.Cfusjfc";
        let nordsee = "\\&Opsetff;!33/17/37!27;41!Ifmhpmboe!Cjoofoibgfo!679/1dn!NOX;vocflboou";

        assert_eq!(decode_dapnet_text("Tfu!pg!JTT!bu!19;67!VUD/"), "Set of ISS at 08:56 UTC.");
        assert_eq!(decode_dapnet_text("R#Tfu!pg!JTT!bu!19;67!VUD/"), "Set of ISS at 08:56 UTC.");
        assert_eq!(decode_dapnet_text(";!EBSD;!SUUZ.Uifnfobcfoe"), "DARC: RTTY-Themenabend");
        assert_eq!(
            decode_dapnet_text(darc_70mhz),
            "DARC: Bundesrats-Drucksache definiert in Funote Rechtsrahmen für 70-MHz-Betrieb"
        );
        assert_eq!(
            decode_dapnet_text(nordsee),
            "Nordsee: 22.06.26 16:30 Helgoland Binnenhafen 568.0cm MNW:unbekannt"
        );
        assert_eq!(decode_dapnet_text("1r*Gfjotubvc"), "Feinstaub");
        assert_eq!(decode_dapnet_text("1R*Tbufmmjufo"), "Satelliten");
        assert_eq!(decode_dapnet_text("1K*IBNOFU"), "HAMNET");
        assert_eq!(decode_dapnet_text("1_*XY.Mplbm"), "WX-Lokal");
        assert_eq!(decode_dapnet_text("1J*HPF!Opugvol"), "GOE Notfunk");
        assert_eq!(decode_dapnet_text("1D*Hf{fjufo"), "Gezeiten");
        assert_eq!(
            decode_dapnet_text("5357.0 EA5FIV von DL4MFF um 1933z"),
            "5357.0 EA5FIV von DL4MFF um 1933z"
        );
        assert_eq!(decode_dapnet_text("XTIME=1702220626"), "XTIME=1702220626");
    }

    #[test]
    fn helpers_are_stable() {
        assert_eq!(dapnet_version("1.0"), "v1.0");
        assert_eq!(dapnet_version("v2"), "v2");
        assert_eq!(extract_callsign("foo dl1abc-9 bar"), Some("DL1ABC-9".to_string()));
        assert_eq!(format_plain_message("DL1ABC", "Hallo"), "DL1ABC - Hallo");
        assert_eq!(prefixed_text("DAPNET", "Alarm"), "DAPNET Alarm");
        assert_eq!(truncate_chars("äöü", 2), ("äö".to_string(), true));
    }

    #[test]
    fn rwth_ack_line_increments_8bit_counter_and_uses_wire_format() {
        assert_eq!(rwth_ack_line(0x00, true), "#01 +\r\n");
        assert_eq!(rwth_ack_line(0x09, true), "#0a +\r\n");
        assert_eq!(rwth_ack_line(0xff, true), "#00 +\r\n");
        assert_eq!(rwth_ack_line(0x09, false), "#0a -\r\n");
    }

    #[test]
    fn sds_destination_prefers_ric_route_over_static_destination() {
        let msg = parse_rwth_message("#00 6:1:9A709:3:Alarm DJ2TH").unwrap();
        let mut dapnet = CfgDapnet::default();
        dapnet.sds_dest_issi = 9999999;
        dapnet.sds_dest_is_group = true;
        dapnet.ric_issi_routes.insert(632585, 2632585);

        let (dest, is_group, route) = resolve_sds_destination(&dapnet, &msg).unwrap();
        assert_eq!(dest, 2632585);
        assert!(!is_group);
        assert_eq!(route, "ric:0632585");
    }

    #[test]
    fn sds_destination_can_route_dapnet_ric_to_tetra_group() {
        let msg = parse_rwth_message("#00 6:1:11A8:3:Rubric alarm").unwrap();
        let mut dapnet = CfgDapnet::default();
        dapnet.sds_dest_issi = 9999999;
        dapnet.ric_gssi_routes.insert(4520, 80);
        dapnet.sds_allowed_rics.insert(4520);

        let (dest, is_group, route) = resolve_sds_destination(&dapnet, &msg).unwrap();
        assert_eq!(dest, 80);
        assert!(is_group);
        assert_eq!(route, "ric-group:0004520");
        assert!(ric_allowed(&dapnet.sds_allowed_rics, &msg));

        let other = parse_rwth_message("#01 6:1:D0:3:Time sync").unwrap();
        assert!(!ric_allowed(&dapnet.sds_allowed_rics, &other));
    }
}
