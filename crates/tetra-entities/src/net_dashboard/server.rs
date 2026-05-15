use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tungstenite::{accept_hdr, handshake::server::{Request, Response}, Message};

use crate::net_dashboard::html::DASHBOARD_HTML;
use crate::net_dashboard::state::{DashboardState, DashboardStateInner, MsEntry, CallEntry};
use crate::net_telemetry::TelemetryEvent;
use crate::net_control::commands::ControlCommand;

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

// Each WS connection registers a Sender here.
// broadcast() sends to all of them; dead connections are pruned automatically.
type WsBroadcastTx = crossbeam_channel::Sender<String>;
type WsClients = Arc<Mutex<Vec<WsBroadcastTx>>>;

pub struct DashboardServer {
    pub state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Option<CmdSender>,
}

impl DashboardServer {
    pub fn new(config_path: String) -> Self {
        Self {
            state: Arc::new(RwLock::new(DashboardStateInner::new(config_path.clone()))),
            clients: Arc::new(Mutex::new(Vec::new())),
            config_path,
            cmd_tx: None,
        }
    }

    pub fn set_cmd_sender(&mut self, tx: CmdSender) {
        self.cmd_tx = Some(tx);
    }

    pub fn start(&mut self, bind: &str, port: u16) {
        let addr = format!("{}:{}", bind, port);
        let state = Arc::clone(&self.state);
        let clients = Arc::clone(&self.clients);
        let config_path = self.config_path.clone();
        let cmd_tx: Arc<Mutex<Option<CmdSender>>> =
            Arc::new(Mutex::new(self.cmd_tx.take()));

        std::thread::Builder::new()
            .name("dashboard-server".into())
            .spawn(move || {
                let listener = match TcpListener::bind(&addr) {
                    Ok(l) => { tracing::info!("Dashboard listening on http://{}", addr); l }
                    Err(e) => { tracing::error!("Dashboard failed to bind {}: {}", addr, e); return; }
                };
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let state = Arc::clone(&state);
                    let clients = Arc::clone(&clients);
                    let config_path = config_path.clone();
                    let cmd_tx = Arc::clone(&cmd_tx);
                    std::thread::Builder::new()
                        .name("dashboard-conn".into())
                        .spawn(move || handle_connection(stream, state, clients, config_path, cmd_tx))
                        .ok();
                }
            })
            .expect("failed to spawn dashboard thread");
    }

    pub fn handle_telemetry(&self, event: TelemetryEvent) {
        let msg = event_to_ws_msg(&event);
        {
            let mut s = self.state.write().unwrap();
            match &event {
                TelemetryEvent::MsRegistration { issi } => {
                    s.ms_map.insert(*issi, MsEntry {
                        issi: *issi, groups: Vec::new(),
                        rssi_dbfs: None, registered_at: Instant::now(), last_seen: Instant::now(),
                        energy_saving_mode: 0,
                    });
                    s.push_log("INFO", format!("MS {} registered", issi));
                }
                TelemetryEvent::MsDeregistration { issi } => {
                    s.ms_map.remove(issi);
                    s.push_log("INFO", format!("MS {} deregistered", issi));
                }
                TelemetryEvent::MsGroupAttach { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        for g in gssis { if !e.groups.contains(g) { e.groups.push(*g); } }
                    }
                }
                TelemetryEvent::MsGroupsSnapshot { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.groups = gssis.clone();
                    }
                }
                TelemetryEvent::MsGroupDetach { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.groups.retain(|g| !gssis.contains(g));
                    }
                }
                TelemetryEvent::MsRssi { issi, rssi_dbfs } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.rssi_dbfs = Some(*rssi_dbfs);
                        e.last_seen = Instant::now();
                    }
                }
                TelemetryEvent::MsEnergySaving { issi, mode } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.energy_saving_mode = *mode;
                    }
                }
                TelemetryEvent::GroupCallStarted { call_id, gssi, caller_issi } => {
                    s.calls.insert(*call_id, CallEntry {
                        call_id: *call_id, is_group: true, gssi: *gssi,
                        caller_issi: *caller_issi, called_issi: 0,
                        speaker_issi: Some(*caller_issi), started_at: Instant::now(), simplex: false,
                    });
                    s.push_last_heard(*caller_issi, "call_group", *gssi);
                    s.push_log("INFO", format!("Group call {} started: {} -> GSSI {}", call_id, caller_issi, gssi));
                }
                TelemetryEvent::GroupCallEnded { call_id, gssi: _ } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("Group call {} ended", call_id));
                }
                TelemetryEvent::GroupCallSpeakerChanged { call_id, gssi, speaker_issi } => {
                    if let Some(c) = s.calls.get_mut(call_id) { c.speaker_issi = Some(*speaker_issi); }
                    s.push_last_heard(*speaker_issi, "call_group", *gssi);
                }
                TelemetryEvent::IndividualCallStarted { call_id, calling_issi, called_issi, simplex } => {
                    s.calls.insert(*call_id, CallEntry {
                        call_id: *call_id, is_group: false, gssi: 0,
                        caller_issi: *calling_issi, called_issi: *called_issi,
                        speaker_issi: None, started_at: Instant::now(), simplex: *simplex,
                    });
                    s.push_last_heard(*calling_issi, "call_individual", *called_issi);
                    s.push_log("INFO", format!("P2P call {} started: {} -> {}", call_id, calling_issi, called_issi));
                }
                TelemetryEvent::IndividualCallEnded { call_id } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("P2P call {} ended", call_id));
                }
                TelemetryEvent::BrewConnected { connected } => {
                    s.brew_online = *connected;
                }
                TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
                    s.push_last_heard(*source_issi, "sds", *dest_issi);
                }
            }
        }
        if let Some(json) = msg {
            self.broadcast(&json);
        }
    }

    pub fn push_log(&self, level: &str, msg: String) {
        let entry = {
            let mut s = self.state.write().unwrap();
            s.push_log(level, msg);
            s.log_ring.back().cloned()
        };
        if let Some(entry) = entry {
            if let Ok(json) = serde_json::to_string(&serde_json::json!({
                "type": "log", "ts": entry.ts, "level": entry.level, "msg": entry.msg
            })) {
                self.broadcast(&json);
            }
        }
    }

    fn broadcast(&self, msg: &str) {
        let mut clients = self.clients.lock().unwrap();
        clients.retain(|tx| tx.send(msg.to_owned()).is_ok());
    }
}

fn event_to_ws_msg(event: &TelemetryEvent) -> Option<String> {
    let v = match event {
        TelemetryEvent::MsRegistration { issi } =>
            serde_json::json!({"type":"ms_registered","issi":issi}),
        TelemetryEvent::MsDeregistration { issi } =>
            serde_json::json!({"type":"ms_deregistered","issi":issi}),
        TelemetryEvent::MsGroupAttach { issi, gssis } =>
            serde_json::json!({"type":"ms_groups","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupDetach { issi, gssis } =>
            serde_json::json!({"type":"ms_groups_detach","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupsSnapshot { issi, gssis } =>
            serde_json::json!({"type":"ms_groups_all","issi":issi,"groups":gssis}),
        TelemetryEvent::MsRssi { issi, rssi_dbfs } =>
            serde_json::json!({"type":"ms_rssi","issi":issi,"rssi_dbfs":rssi_dbfs}),
        TelemetryEvent::MsEnergySaving { issi, mode } =>
            serde_json::json!({"type":"ms_energy_saving","issi":issi,"mode":mode}),
        TelemetryEvent::GroupCallStarted { call_id, gssi, caller_issi } =>
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"group","gssi":gssi,"caller_issi":caller_issi,"last_heard":{"issi":caller_issi,"activity":"call_group","dest":gssi}}),
        TelemetryEvent::GroupCallEnded { call_id, gssi: _ } =>
            serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::GroupCallSpeakerChanged { call_id, gssi, speaker_issi } =>
            serde_json::json!({"type":"speaker_changed","call_id":call_id,"speaker_issi":speaker_issi,"last_heard":{"issi":speaker_issi,"activity":"call_group","dest":gssi}}),
        TelemetryEvent::IndividualCallStarted { call_id, calling_issi, called_issi, simplex } =>
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"individual","caller_issi":calling_issi,"called_issi":called_issi,"simplex":simplex,"last_heard":{"issi":calling_issi,"activity":"call_individual","dest":called_issi}}),
        TelemetryEvent::IndividualCallEnded { call_id } =>
            serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::BrewConnected { connected } =>
            serde_json::json!({"type":"brew_status","connected":connected}),
        TelemetryEvent::SdsActivity { source_issi, dest_issi } =>
            serde_json::json!({"type":"last_heard","issi":source_issi,"activity":"sds","dest":dest_issi}),
    };
    serde_json::to_string(&v).ok()
}

fn handle_connection(
    stream: TcpStream,
    state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    let mut peek_buf = [0u8; 256];
    let n = match stream.peek(&mut peek_buf) { Ok(n) => n, Err(_) => return };
    let peek_str = String::from_utf8_lossy(&peek_buf[..n]);
    let req_line = peek_str.lines().next().unwrap_or("").to_string();

    if req_line.contains("/ws") {
        handle_ws(stream, state, clients, cmd_tx);
    } else if req_line.contains("GET /api/config") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        serve_config_get(buf.into_inner(), &config_path);
    } else if req_line.contains("POST /api/config") {
        let mut buf = BufReader::new(stream);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                content_length = lower.trim_start_matches("content-length:")
                    .trim().trim_end_matches("\r\n").trim_end_matches('\n')
                    .parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        let _ = buf.read_exact(&mut body);
        let body_str = String::from_utf8_lossy(&body);
        match std::fs::write(&config_path, body_str.as_ref()) {
            Ok(_) => http_response(buf.into_inner(), 200, "OK"),
            Err(e) => http_response(buf.into_inner(), 500, &e.to_string()),
        }
    } else {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        serve_html(buf.into_inner());
    }
}

fn handle_ws(stream: TcpStream, state: DashboardState, clients: WsClients,
             cmd_tx: Arc<Mutex<Option<CmdSender>>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));

    let callback = |_req: &Request, res: Response| Ok(res);
    let mut ws = match accept_hdr(stream, callback) {
        Ok(w) => w,
        Err(e) => { tracing::debug!("WS handshake failed: {}", e); return; }
    };

    // Register this connection for broadcasts
    let (broadcast_tx, broadcast_rx) = crossbeam_channel::unbounded::<String>();
    {
        let mut c = clients.lock().unwrap();
        c.push(broadcast_tx);
    }

    // Send initial snapshot
    {
        let s = state.read().unwrap();
        let ms = s.snapshot_ms();
        let calls = s.snapshot_calls();
        let logs: Vec<_> = s.log_ring.iter().cloned().collect();
        let last_heard: Vec<_> = s.last_heard.iter().cloned().collect();
        drop(s);
        let brew_online = state.read().unwrap().brew_online;
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "type": "snapshot", "ms": ms, "calls": calls, "log": logs,
            "brew_online": brew_online, "last_heard": last_heard
        })) {
            let _ = ws.send(Message::Text(json));
        }
    }

    let _ = ws.get_ref().set_read_timeout(Some(std::time::Duration::from_millis(20)));

    loop {
        // Drain outbound broadcast messages first
        while let Ok(msg) = broadcast_rx.try_recv() {
            if ws.send(Message::Text(msg)).is_err() { return; }
        }

        // Then check for inbound messages from browser
        match ws.read() {
            Ok(Message::Text(text)) => {
                handle_ws_command(&text, &state, &cmd_tx);
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(data)) => { let _ = ws.send(Message::Pong(data)); }
            Ok(_) => {}
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

fn handle_ws_command(text: &str, state: &DashboardState, cmd_tx: &Arc<Mutex<Option<CmdSender>>>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };

    let send_cmd = |cmd: ControlCommand| -> bool {
        if let Ok(guard) = cmd_tx.lock() {
            if let Some(ref tx) = *guard {
                return tx.send(cmd).is_ok();
            }
        }
        false
    };

    match v.get("type").and_then(|t| t.as_str()) {
        Some("kick") => {
            let issi = v.get("issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            if issi == 0 { return; }
            tracing::info!("Dashboard: kick ISSI {}", issi);
            if !send_cmd(ControlCommand::KickMs { issi }) {
                tracing::warn!("Dashboard: no control dispatcher for kick");
            }
            let mut s = state.write().unwrap();
            s.push_log("INFO", format!("Kick requested for ISSI {}", issi));
        }
        Some("restart") => {
            tracing::info!("Dashboard: restart service requested");
            send_cmd(ControlCommand::RestartService);
        }
        Some("shutdown") => {
            tracing::info!("Dashboard: shutdown service requested");
            send_cmd(ControlCommand::ShutdownService);
        }
        Some("sds") => {
            let dest = v.get("dest_issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let msg_text = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            if dest == 0 || msg_text.is_empty() { return; }
            tracing::info!("Dashboard: SDS to {} = {}", dest, msg_text);
            let payload = msg_text.as_bytes().to_vec();
            let len_bits = (payload.len() * 8) as u16;
            send_cmd(ControlCommand::SendSds {
                handle: 0,
                source_ssi: 9999,  // BS dispatcher ISSI
                dest_ssi: dest,
                dest_is_group: false,
                len_bits,
                payload,
            });
            let mut s = state.write().unwrap();
            s.push_log("INFO", format!("SDS sent to {}: {}", dest, msg_text));
        }
        _ => {}
    }
}

fn serve_html(mut stream: TcpStream) {
    let body = DASHBOARD_HTML.replace("{{STACK_VERSION}}", tetra_core::STACK_VERSION);
    let body = body.as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn serve_config_get(mut stream: TcpStream, config_path: &str) {
    match std::fs::read_to_string(config_path) {
        Ok(content) => {
            let body = content.as_bytes();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        }
        Err(e) => http_response(stream, 500, &e.to_string()),
    }
}

fn http_response(mut stream: TcpStream, code: u16, body: &str) {
    let status = if code == 200 { "OK" } else { "Error" };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code, status, body.len(), body
    );
    let _ = stream.write_all(resp.as_bytes());
}
