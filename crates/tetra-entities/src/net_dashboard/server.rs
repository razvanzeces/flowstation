use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tungstenite::{
    Message, accept_hdr,
    handshake::server::{Request, Response},
};

use tetra_config::bluestation::parsing;

use crate::net_control::commands::{ControlCommand, RfGainDirection};
use crate::net_dashboard::html::DASHBOARD_HTML;
use crate::net_dashboard::state::{CallEntry, DashboardState, DashboardStateInner, MsEntry, RfLoopbackFrame};
use crate::net_telemetry::TelemetryEvent;

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

// Each WS connection registers a Sender here.
// broadcast() sends to all of them; dead connections are pruned automatically.
type WsBroadcastTx = crossbeam_channel::Sender<String>;
type WsClients = Arc<Mutex<Vec<WsBroadcastTx>>>;

// ---------------------------------------------------------------------------
// OTA update state — shared between the HTTP handler and the update thread.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum UpdatePhase {
    Idle,
    Running,
    Done { success: bool },
}

struct UpdateState {
    phase: UpdatePhase,
    log: String,
}

impl UpdateState {
    fn new() -> Self {
        UpdateState {
            phase: UpdatePhase::Idle,
            log: String::new(),
        }
    }
    fn append(&mut self, line: &str) {
        self.log.push_str(line);
        self.log.push('\n');
    }
    fn start(&mut self) {
        self.phase = UpdatePhase::Running;
        self.log.clear();
    }
    fn finish(&mut self, success: bool) {
        self.phase = UpdatePhase::Done { success };
    }
}

type SharedUpdateState = Arc<Mutex<UpdateState>>;

/// Run git pull + cargo build --release in a background thread.
/// Steps:
///   1. Backup config.toml → config.toml.bak
///   2. git -C <src_dir> pull
///   3. cargo build --release
///   4. systemctl restart tetra   (after short delay, gives 200 OK time to reach browser)
///
/// src_dir is derived from the binary path: the directory containing the running binary's
/// parent (i.e. target/release is sibling of src root), so we go up two levels.
fn run_update(update: SharedUpdateState, config_path: String) {
    // Derive source root from the running binary's location.
    // Binary lives at  <src_root>/target/release/bluestation-bs
    // so src_root = binary_path.parent().parent().parent()
    let src_dir = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    macro_rules! log {
        ($update:expr, $($arg:tt)*) => {{
            let line = format!($($arg)*);
            tracing::info!("UPDATE: {}", line);
            $update.lock().unwrap().append(&line);
        }};
    }

    /// Run a command, stream stdout+stderr into the log, return Ok(stdout) or Err.
    fn run_cmd_output(update: &SharedUpdateState, program: &str, args: &[&str], dir: &std::path::Path) -> Option<String> {
        let line = format!("$ {} {}", program, args.join(" "));
        tracing::info!("UPDATE: {}", line);
        update.lock().unwrap().append(&line);

        match std::process::Command::new(program).args(args).current_dir(dir).output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                for l in stdout.lines() {
                    update.lock().unwrap().append(l);
                }
                for l in stderr.lines() {
                    update.lock().unwrap().append(l);
                }
                if out.status.success() {
                    Some(stdout)
                } else {
                    update.lock().unwrap().append(&format!("ERROR: exited with {}", out.status));
                    update.lock().unwrap().finish(false);
                    None
                }
            }
            Err(e) => {
                update.lock().unwrap().append(&format!("ERROR: failed to run '{}': {}", program, e));
                update.lock().unwrap().finish(false);
                None
            }
        }
    }

    log!(update, "=== FlowStation OTA Update ===");
    log!(update, "Source dir: {}", src_dir.display());

    let src_str = src_dir.to_str().unwrap_or(".");

    // Step 1: fetch remote without merging — just update refs
    log!(update, "--- Checking remote for updates ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "fetch", "origin", "main"], &src_dir).is_none() {
        return;
    }

    // Step 2: compare local HEAD with remote origin/main
    let local_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "HEAD"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if local_commit.is_empty() {
        return;
    }

    let remote_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "origin/main"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if remote_commit.is_empty() {
        return;
    }

    log!(update, "Local  commit: {}", &local_commit[..local_commit.len().min(12)]);
    log!(update, "Remote commit: {}", &remote_commit[..remote_commit.len().min(12)]);

    if local_commit == remote_commit {
        log!(update, "Already up to date — nothing to do.");
        update.lock().unwrap().finish(true);
        return;
    }

    // Step 3: show what changed
    let _ = run_cmd_output(
        &update,
        "git",
        &["-C", src_str, "log", "--oneline", &format!("HEAD..origin/main")],
        &src_dir,
    );

    // Step 4: backup config before touching anything
    let backup_path = format!("{}.bak", config_path);
    match std::fs::copy(&config_path, &backup_path) {
        Ok(_) => log!(update, "Config backed up → {}", backup_path),
        Err(e) => log!(update, "WARNING: config backup failed: {} (continuing)", e),
    }

    // Step 5: fast-forward merge (only changed files are touched on disk)
    log!(update, "--- git merge (fast-forward only) ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "merge", "--ff-only", "origin/main"], &src_dir).is_none() {
        return;
    }

    // Step 6: incremental build (cargo only recompiles changed crates)
    log!(update, "--- cargo build --release (incremental) ---");
    if run_cmd_output(&update, "cargo", &["build", "--release"], &src_dir).is_none() {
        return;
    }

    // Step 7: done — schedule restart
    log!(update, "--- Build successful. Restarting service in 2s... ---");
    update.lock().unwrap().finish(true);

    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = std::process::Command::new("systemctl").args(["restart", "tetra"]).status();
    });
}

pub struct DashboardServer {
    pub state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Option<CmdSender>,
    phy_cmd_tx: Option<CmdSender>,
    update_state: SharedUpdateState,
    /// Last time a ts_voice WS message was broadcast per TS (indexed 0..3 for TS1..TS4)
    ts_last_broadcast: std::sync::Mutex<[std::time::Instant; 4]>,
}

impl DashboardServer {
    pub fn new(config_path: String) -> Self {
        let now = std::time::Instant::now();
        Self {
            state: Arc::new(RwLock::new(DashboardStateInner::new(config_path.clone()))),
            clients: Arc::new(Mutex::new(Vec::new())),
            config_path,
            cmd_tx: None,
            phy_cmd_tx: None,
            update_state: Arc::new(Mutex::new(UpdateState::new())),
            ts_last_broadcast: std::sync::Mutex::new([now; 4]),
        }
    }

    pub fn set_cmd_sender(&mut self, tx: CmdSender) {
        self.cmd_tx = Some(tx);
    }

    pub fn set_phy_cmd_sender(&mut self, tx: CmdSender) {
        self.phy_cmd_tx = Some(tx);
    }

    pub fn start(&mut self, bind: &str, port: u16) {
        let addr = format!("{}:{}", bind, port);
        let state = Arc::clone(&self.state);
        let clients = Arc::clone(&self.clients);
        let config_path = self.config_path.clone();
        let cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.cmd_tx.take()));
        let phy_cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.phy_cmd_tx.take()));
        let update_state = Arc::clone(&self.update_state);

        std::thread::Builder::new()
            .name("dashboard-server".into())
            .spawn(move || {
                let listener = match TcpListener::bind(&addr) {
                    Ok(l) => {
                        tracing::info!("Dashboard listening on http://{}", addr);
                        l
                    }
                    Err(e) => {
                        tracing::error!("Dashboard failed to bind {}: {}", addr, e);
                        return;
                    }
                };
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let state = Arc::clone(&state);
                    let clients = Arc::clone(&clients);
                    let config_path = config_path.clone();
                    let cmd_tx = Arc::clone(&cmd_tx);
                    let phy_cmd_tx = Arc::clone(&phy_cmd_tx);
                    let update_state = Arc::clone(&update_state);
                    std::thread::Builder::new()
                        .name("dashboard-conn".into())
                        .spawn(move || handle_connection(stream, state, clients, config_path, cmd_tx, phy_cmd_tx, update_state))
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
                    s.ms_map.insert(
                        *issi,
                        MsEntry {
                            issi: *issi,
                            groups: Vec::new(),
                            rssi_dbfs: None,
                            registered_at: Instant::now(),
                            last_seen: Instant::now(),
                            energy_saving_mode: 0,
                        },
                    );
                    s.push_log("INFO", format!("MS {} registered", issi));
                }
                TelemetryEvent::MsDeregistration { issi } => {
                    s.ms_map.remove(issi);
                    s.push_log("INFO", format!("MS {} deregistered", issi));
                }
                TelemetryEvent::MsGroupAttach { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        for g in gssis {
                            if !e.groups.contains(g) {
                                e.groups.push(*g);
                            }
                        }
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
                TelemetryEvent::GroupCallStarted {
                    call_id,
                    gssi,
                    caller_issi,
                    ts,
                } => {
                    s.calls.insert(
                        *call_id,
                        CallEntry {
                            call_id: *call_id,
                            is_group: true,
                            gssi: *gssi,
                            caller_issi: *caller_issi,
                            called_issi: 0,
                            speaker_issi: Some(*caller_issi),
                            started_at: Instant::now(),
                            simplex: false,
                            ts: *ts,
                        },
                    );
                    s.push_last_heard(*caller_issi, "call_group", *gssi);
                    s.push_log("INFO", format!("Group call {} started: {} -> GSSI {}", call_id, caller_issi, gssi));
                }
                TelemetryEvent::GroupCallEnded { call_id, gssi: _ } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("Group call {} ended", call_id));
                }
                TelemetryEvent::GroupCallSpeakerChanged {
                    call_id,
                    gssi,
                    speaker_issi,
                } => {
                    if let Some(c) = s.calls.get_mut(call_id) {
                        c.speaker_issi = Some(*speaker_issi);
                    }
                    s.push_last_heard(*speaker_issi, "call_group", *gssi);
                }
                TelemetryEvent::IndividualCallStarted {
                    call_id,
                    calling_issi,
                    called_issi,
                    simplex,
                    ts,
                } => {
                    s.calls.insert(
                        *call_id,
                        CallEntry {
                            call_id: *call_id,
                            is_group: false,
                            gssi: 0,
                            caller_issi: *calling_issi,
                            called_issi: *called_issi,
                            speaker_issi: None,
                            started_at: Instant::now(),
                            simplex: *simplex,
                            ts: *ts,
                        },
                    );
                    s.push_last_heard(*calling_issi, "call_individual", *called_issi);
                    s.push_log("INFO", format!("P2P call {} started: {} -> {}", call_id, calling_issi, called_issi));
                }
                TelemetryEvent::IndividualCallEnded { call_id } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("P2P call {} ended", call_id));
                }
                TelemetryEvent::BrewConnected { connected, server_version } => {
                    s.brew_online = *connected;
                    if *connected {
                        s.brew_version = *server_version;
                    }
                }
                TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
                    s.push_last_heard(*source_issi, "sds", *dest_issi);
                }
                TelemetryEvent::TsVoiceActivity { .. } => {
                    // Handled below with rate limiting — no state update needed
                }
                TelemetryEvent::TxMonitor { .. } => {
                    // Stateless live RF monitor payload.
                }
                TelemetryEvent::RfLoopbackMonitor {
                    sample_rate,
                    center_freq_hz,
                    tone_hz,
                    amplitude,
                    rms_dbfs,
                    peak_dbfs,
                    spectrum_db_tenths,
                    constellation_iq,
                } => {
                    s.last_rf_loopback = Some(RfLoopbackFrame {
                        sample_rate: *sample_rate,
                        center_freq_hz: *center_freq_hz,
                        tone_hz: *tone_hz,
                        amplitude: *amplitude,
                        rms_dbfs: *rms_dbfs,
                        peak_dbfs: *peak_dbfs,
                        spectrum_db_tenths: spectrum_db_tenths.clone(),
                        constellation_iq: constellation_iq.clone(),
                    });
                }
            }
        }
        if let Some(json) = msg {
            self.broadcast(&json);
        }
        // TsVoiceActivity: rate-limit broadcasts to max 4/sec per TS (250ms cooldown)
        if let TelemetryEvent::TsVoiceActivity { ts } = &event {
            let idx = (ts.saturating_sub(1) as usize).min(3);
            let now = std::time::Instant::now();
            if let Ok(mut arr) = self.ts_last_broadcast.try_lock() {
                if now.duration_since(arr[idx]) >= std::time::Duration::from_millis(250) {
                    arr[idx] = now;
                    drop(arr);
                    if let Some(json) = event_to_ws_msg(&event) {
                        self.broadcast(&json);
                    }
                }
            }
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
        TelemetryEvent::MsRegistration { issi } => serde_json::json!({"type":"ms_registered","issi":issi}),
        TelemetryEvent::MsDeregistration { issi } => serde_json::json!({"type":"ms_deregistered","issi":issi}),
        TelemetryEvent::MsGroupAttach { issi, gssis } => serde_json::json!({"type":"ms_groups","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupDetach { issi, gssis } => serde_json::json!({"type":"ms_groups_detach","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupsSnapshot { issi, gssis } => serde_json::json!({"type":"ms_groups_all","issi":issi,"groups":gssis}),
        TelemetryEvent::MsRssi { issi, rssi_dbfs } => serde_json::json!({"type":"ms_rssi","issi":issi,"rssi_dbfs":rssi_dbfs}),
        TelemetryEvent::MsEnergySaving { issi, mode } => serde_json::json!({"type":"ms_energy_saving","issi":issi,"mode":mode}),
        TelemetryEvent::GroupCallStarted {
            call_id,
            gssi,
            caller_issi,
            ts,
        } => {
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"group","gssi":gssi,"caller_issi":caller_issi,"ts":ts,"last_heard":{"issi":caller_issi,"activity":"call_group","dest":gssi}})
        }
        TelemetryEvent::GroupCallEnded { call_id, gssi: _ } => serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::GroupCallSpeakerChanged {
            call_id,
            gssi,
            speaker_issi,
        } => {
            serde_json::json!({"type":"speaker_changed","call_id":call_id,"speaker_issi":speaker_issi,"last_heard":{"issi":speaker_issi,"activity":"call_group","dest":gssi}})
        }
        TelemetryEvent::IndividualCallStarted {
            call_id,
            calling_issi,
            called_issi,
            simplex,
            ts,
        } => {
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"individual","caller_issi":calling_issi,"called_issi":called_issi,"simplex":simplex,"ts":ts,"last_heard":{"issi":calling_issi,"activity":"call_individual","dest":called_issi}})
        }
        TelemetryEvent::IndividualCallEnded { call_id } => serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::BrewConnected { connected, server_version } => {
            serde_json::json!({"type":"brew_status","connected":connected,"brew_version":server_version})
        }
        TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
            serde_json::json!({"type":"last_heard","issi":source_issi,"activity":"sds","dest":dest_issi})
        }
        TelemetryEvent::TsVoiceActivity { ts } => serde_json::json!({"type":"ts_voice","ts":ts}),
        TelemetryEvent::TxMonitor {
            sample_rate,
            center_freq_hz,
            rms_dbfs,
            peak_dbfs,
            spectrum_db_tenths,
            constellation_iq,
        } => serde_json::json!({
            "type":"tx_monitor",
            "sample_rate":sample_rate,
            "center_freq_hz":center_freq_hz,
            "rms_dbfs":rms_dbfs,
            "peak_dbfs":peak_dbfs,
            "spectrum_db_tenths":spectrum_db_tenths,
            "constellation_iq":constellation_iq,
        }),
        TelemetryEvent::RfLoopbackMonitor {
            sample_rate,
            center_freq_hz,
            tone_hz,
            amplitude,
            rms_dbfs,
            peak_dbfs,
            spectrum_db_tenths,
            constellation_iq,
        } => serde_json::json!({
            "type":"rf_loopback_monitor",
            "sample_rate":sample_rate,
            "center_freq_hz":center_freq_hz,
            "tone_hz":tone_hz,
            "amplitude":amplitude,
            "rms_dbfs":rms_dbfs,
            "peak_dbfs":peak_dbfs,
            "spectrum_db_tenths":spectrum_db_tenths,
            "constellation_iq":constellation_iq,
        }),
    };
    serde_json::to_string(&v).ok()
}

fn handle_connection(
    stream: TcpStream,
    state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    phy_cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    let mut peek_buf = [0u8; 256];
    let n = match stream.peek(&mut peek_buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let peek_str = String::from_utf8_lossy(&peek_buf[..n]);
    let req_line = peek_str.lines().next().unwrap_or("").to_string();

    if req_line.contains("/ws") {
        handle_ws(stream, state, clients, cmd_tx, phy_cmd_tx, update_state);
    } else if req_line.contains("GET /api/system") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_system_info(buf.into_inner(), &config_path);
    } else if req_line.contains("POST /api/configs/activate") {
        let mut buf = BufReader::new(stream);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                content_length = lower
                    .trim_start_matches("content-length:")
                    .trim()
                    .trim_end_matches("\r\n")
                    .trim_end_matches('\n')
                    .parse()
                    .unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        let _ = buf.read_exact(&mut body);
        let profile = String::from_utf8_lossy(&body).trim().to_string();
        match activate_config_profile(&config_path, &profile) {
            Ok(_) => {
                tracing::info!("Dashboard: activated config profile '{}'", profile);
                http_response(buf.into_inner(), 200, "OK")
            }
            Err(e) => http_response(buf.into_inner(), 500, &e),
        }
    } else if req_line.contains("GET /api/configs") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_config_list(buf.into_inner(), &config_path);
    } else if req_line.contains("GET /api/update/status") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_update_status(buf.into_inner(), &update_state);
    } else if req_line.contains("POST /api/update") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        {
            let mut u = update_state.lock().unwrap();
            if u.phase == UpdatePhase::Running {
                http_response(buf.into_inner(), 409, "Update already in progress");
                return;
            }
            u.start();
        }
        tracing::info!("Dashboard: OTA update triggered");
        let update_clone = Arc::clone(&update_state);
        let cfg_clone = config_path.clone();
        std::thread::Builder::new()
            .name("ota-update".into())
            .spawn(move || run_update(update_clone, cfg_clone))
            .ok();
        http_response(buf.into_inner(), 200, "OK");
    } else if req_line.contains("GET /api/config/backup") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        let backup_path = format!("{}.bak", config_path);
        serve_config_get(buf.into_inner(), &backup_path);
    } else if req_line.contains("POST /api/config/restore") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        let backup_path = format!("{}.bak", config_path);
        match std::fs::copy(&backup_path, &config_path) {
            Ok(_) => {
                tracing::info!("Dashboard: config restored from backup");
                http_response(buf.into_inner(), 200, "OK")
            }
            Err(e) => http_response(buf.into_inner(), 500, &e.to_string()),
        }
    } else if req_line.contains("POST /api/config") {
        let mut buf = BufReader::new(stream);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                content_length = lower
                    .trim_start_matches("content-length:")
                    .trim()
                    .trim_end_matches("\r\n")
                    .trim_end_matches('\n')
                    .parse()
                    .unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        let _ = buf.read_exact(&mut body);
        let body_str = String::from_utf8_lossy(&body);
        // Write backup of current config before overwriting
        let backup_path = format!("{}.bak", config_path);
        if let Err(e) = std::fs::copy(&config_path, &backup_path) {
            tracing::warn!("Dashboard: failed to write config backup: {}", e);
        }
        match std::fs::write(&config_path, body_str.as_ref()) {
            Ok(_) => http_response(buf.into_inner(), 200, "OK"),
            Err(e) => http_response(buf.into_inner(), 500, &e.to_string()),
        }
    } else if req_line.contains("GET /api/config") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_config_get(buf.into_inner(), &config_path);
    } else {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_html(buf.into_inner());
    }
}

fn handle_ws(
    stream: TcpStream,
    state: DashboardState,
    clients: WsClients,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    phy_cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));

    let callback = |_req: &Request, res: Response| Ok(res);
    let mut ws = match accept_hdr(stream, callback) {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!("WS handshake failed: {}", e);
            return;
        }
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
        let last_rf_loopback = s.last_rf_loopback.clone();
        let config_path = s.config_path.clone();
        drop(s);
        let brew_online = state.read().unwrap().brew_online;
        let brew_version = state.read().unwrap().brew_version;
        let rf_gains = rf_gain_snapshot(&config_path);
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "type": "snapshot", "ms": ms, "calls": calls, "log": logs,
            "brew_online": brew_online, "brew_version": brew_version, "last_heard": last_heard,
            "last_rf_loopback": last_rf_loopback, "rf_gains": rf_gains
        })) {
            let _ = ws.send(Message::Text(json));
        }
    }

    let _ = ws.get_ref().set_read_timeout(Some(std::time::Duration::from_millis(20)));

    loop {
        // Drain outbound broadcast messages first
        while let Ok(msg) = broadcast_rx.try_recv() {
            if ws.send(Message::Text(msg)).is_err() {
                return;
            }
        }

        // Then check for inbound messages from browser
        match ws.read() {
            Ok(Message::Text(text)) => {
                handle_ws_command(&text, &state, &cmd_tx, &phy_cmd_tx, &update_state);
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(data)) => {
                let _ = ws.send(Message::Pong(data));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

fn rf_gain_snapshot(config_path: &str) -> Vec<serde_json::Value> {
    let Ok(config) = parsing::from_file(config_path) else {
        return Vec::new();
    };
    let Some(soapy) = config.phy_io.soapysdr.as_ref() else {
        return Vec::new();
    };
    let mut gains = Vec::new();
    for (name, value) in &soapy.rx_gains {
        gains.push(serde_json::json!({"direction":"rx","name":name.to_ascii_uppercase(),"value":value}));
    }
    for (name, value) in &soapy.tx_gains {
        gains.push(serde_json::json!({"direction":"tx","name":name.to_ascii_uppercase(),"value":value}));
    }
    gains
}

fn handle_ws_command(
    text: &str,
    state: &DashboardState,
    cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
    phy_cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
    update_state: &SharedUpdateState,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    let send_cmd = |cmd: ControlCommand| -> bool {
        if let Ok(guard) = cmd_tx.lock() {
            if let Some(ref tx) = *guard {
                return tx.send(cmd).is_ok();
            }
        }
        false
    };
    let send_phy_cmd = |cmd: ControlCommand| -> bool {
        if let Ok(guard) = phy_cmd_tx.lock() {
            if let Some(ref tx) = *guard {
                return tx.send(cmd).is_ok();
            }
        }
        false
    };

    match v.get("type").and_then(|t| t.as_str()) {
        Some("set_rf_gain") => {
            let Some(direction) = v.get("direction").and_then(|d| d.as_str()) else {
                return;
            };
            let direction = match direction {
                "rx" | "RX" => RfGainDirection::Rx,
                "tx" | "TX" => RfGainDirection::Tx,
                _ => return,
            };
            let Some(name) = v.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()) else {
                return;
            };
            let Some(value) = v.get("value").and_then(|g| g.as_f64()).filter(|g| g.is_finite()) else {
                return;
            };
            if !send_phy_cmd(ControlCommand::SetRfGain {
                direction,
                name: name.to_string(),
                value,
            }) {
                tracing::warn!("Dashboard: no PHY control dispatcher for RF gain");
            }
            let mut s = state.write().unwrap();
            let dir = match direction {
                RfGainDirection::Rx => "RX",
                RfGainDirection::Tx => "TX",
            };
            s.push_log("INFO", format!("RF gain set requested: {} {} {:.1}", dir, name, value));
        }
        Some("kick") => {
            let issi = v.get("issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            if issi == 0 {
                return;
            }
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
        Some("update") => {
            let mut u = update_state.lock().unwrap();
            if u.phase == UpdatePhase::Running {
                tracing::warn!("Dashboard: update already in progress, ignoring");
                return;
            }
            u.start();
            drop(u);
            tracing::info!("Dashboard: OTA update triggered via WS");
            // config_path not available here; caller must use POST /api/update instead
            // This WS variant is for UI convenience — it signals the browser to poll /api/update/status
            // The actual update must be triggered via POST /api/update from JS first.
            // Here we just ack that status polling should begin.
            let mut s = state.write().unwrap();
            s.push_log("INFO", "OTA update started — check /api/update/status for progress".to_string());
        }
        Some("sds") => {
            let dest = v.get("dest_issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let msg_text = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            if dest == 0 || msg_text.is_empty() {
                return;
            }
            tracing::info!("Dashboard: SDS to {} = {}", dest, msg_text);
            let payload = msg_text.as_bytes().to_vec();
            let len_bits = (payload.len() * 8) as u16;
            send_cmd(ControlCommand::SendSds {
                handle: 0,
                source_ssi: 9999, // BS dispatcher ISSI
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

fn serve_update_status(mut stream: TcpStream, update_state: &SharedUpdateState) {
    let (phase_str, success, log) = {
        let u = update_state.lock().unwrap();
        let phase_str = match &u.phase {
            UpdatePhase::Idle => "idle",
            UpdatePhase::Running => "running",
            UpdatePhase::Done { success: true } => "done_ok",
            UpdatePhase::Done { success: false } => "done_err",
        };
        let success = matches!(u.phase, UpdatePhase::Done { success: true });
        (phase_str, success, u.log.clone())
    };
    let body = format!(
        "{{\"status\":\"{}\",\"success\":{},\"log\":{}}}",
        phase_str,
        success,
        serde_json::to_string(&log).unwrap_or_else(|_| "\"\"".into())
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn serve_system_info(mut stream: TcpStream, config_path: &str) {
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let uptime_secs: u64 = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(|n| n.parse::<f64>().ok()))
        .flatten()
        .map(|f| f as u64)
        .unwrap_or(0);

    let os_info = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("PRETTY_NAME="))
                .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "Linux".to_string());

    let config_dir = std::path::Path::new(config_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let body = format!(
        "{{\"hostname\":{},\"uptime_secs\":{},\"os\":{},\"config_path\":{},\"config_dir\":{},\"stack_version\":{}}}",
        serde_json::to_string(&hostname).unwrap_or_default(),
        uptime_secs,
        serde_json::to_string(&os_info).unwrap_or_default(),
        serde_json::to_string(config_path).unwrap_or_default(),
        serde_json::to_string(&config_dir).unwrap_or_default(),
        serde_json::to_string(tetra_core::STACK_VERSION).unwrap_or_default(),
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn serve_config_list(mut stream: TcpStream, config_path: &str) {
    let active_name = std::path::Path::new(config_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));

    let mut profiles: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(config_dir) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Include .toml files, exclude backups (.bak)
                if name.ends_with(".toml") && !name.ends_with(".bak") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        for name in names {
            profiles.push(serde_json::json!({
                "name": name,
                "active": name == active_name,
            }));
        }
    }

    let body = serde_json::to_string(&profiles).unwrap_or_else(|_| "[]".to_string());
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// Copy selected profile over the active config_path, preserving a backup.
fn activate_config_profile(config_path: &str, profile_name: &str) -> Result<(), String> {
    // Security: profile_name must be a plain filename with no path separators
    if profile_name.contains('/') || profile_name.contains('\\') || profile_name.contains("..") {
        return Err("invalid profile name".to_string());
    }
    if !profile_name.ends_with(".toml") {
        return Err("profile must be a .toml file".to_string());
    }

    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(profile_name);

    if !profile_path.exists() {
        return Err(format!("profile '{}' not found", profile_name));
    }

    // Backup current config before switching
    let backup_path = format!("{}.bak", config_path);
    if let Err(e) = std::fs::copy(config_path, &backup_path) {
        tracing::warn!("Dashboard: failed to backup config before profile switch: {}", e);
    }

    std::fs::copy(&profile_path, config_path)
        .map(|_| ())
        .map_err(|e| format!("failed to copy profile: {}", e))
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
        code,
        status,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}
