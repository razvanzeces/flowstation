use std::collections::HashMap;
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
    fn new() -> Self { UpdateState { phase: UpdatePhase::Idle, log: String::new() } }
    fn append(&mut self, line: &str) { self.log.push_str(line); self.log.push('\n'); }
    fn start(&mut self) { self.phase = UpdatePhase::Running; self.log.clear(); }
    fn finish(&mut self, success: bool) { self.phase = UpdatePhase::Done { success }; }
}

type SharedUpdateState = Arc<Mutex<UpdateState>>;

/// In-memory session store for cookie-based authentication.
///
/// We deliberately don't use Basic Auth from the browser any more: on iOS Safari and
/// older mobile browsers the native Basic Auth dialog frequently asks for credentials
/// 2-3 times in a row, prompts on every WebSocket reconnect, or "forgets" credentials
/// after switching tabs. A cookie-backed session avoids all of that and lets us
/// design a proper login screen.
///
/// Tokens are random 32-byte hex strings. They expire after 7 days of inactivity.
/// The store is per-process (no on-disk persistence) — restarting FlowStation logs
/// every session out. That's fine: the dashboard is typically a single-operator tool.
pub struct SessionStore {
    sessions: HashMap<String, std::time::Instant>,
    /// Sessions older than this are pruned on access.
    ttl: std::time::Duration,
}

impl SessionStore {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            ttl: std::time::Duration::from_secs(7 * 24 * 60 * 60),
        }
    }

    /// Create a new session, return its token. Caller sets it as a cookie.
    fn create(&mut self) -> String {
        self.prune();
        let token = generate_session_token();
        self.sessions.insert(token.clone(), std::time::Instant::now());
        token
    }

    /// Return true if the token is known and not expired. Refreshes last-seen on hit.
    fn validate(&mut self, token: &str) -> bool {
        self.prune();
        if let Some(seen) = self.sessions.get_mut(token) {
            *seen = std::time::Instant::now();
            return true;
        }
        false
    }

    fn invalidate(&mut self, token: &str) {
        self.sessions.remove(token);
    }

    fn prune(&mut self) {
        let now = std::time::Instant::now();
        let ttl = self.ttl;
        self.sessions.retain(|_, seen| now.duration_since(*seen) < ttl);
    }
}

type SharedSessionStore = Arc<Mutex<SessionStore>>;

/// 32 bytes of entropy → 64-char hex string. Uses the OS RNG via `getrandom`-style
/// `/dev/urandom` read. Falls back to a time+pid mix if /dev/urandom is unavailable —
/// not cryptographically perfect, but adequate for a session token on a LAN-only
/// dashboard. Production-grade deployments behind a reverse proxy already get HTTPS
/// hardening from the proxy layer.
fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    } else {
        // Fallback: deterministic-ish entropy from time + pid + addr-of-self.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos()).unwrap_or(0);
        let pid = std::process::id() as u128;
        let mix = nanos.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(pid << 64);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((mix >> (i * 4)) & 0xff) as u8;
        }
    }
    let mut s = String::with_capacity(64);
    for b in &bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Extract `fs_session=<token>` from a Cookie header in the raw request.
fn parse_session_cookie(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("cookie:") {
            // Use original (non-lowered) line for the value to preserve case in token.
            let value_line = &line["cookie:".len()..];
            for kv in value_line.split(';') {
                let kv = kv.trim();
                if let Some(token) = kv.strip_prefix("fs_session=") {
                    return Some(token.to_string());
                }
            }
            let _ = rest;
        }
    }
    None
}

/// Resolve the FlowStation git source directory for OTA updates.
///
/// Resolution order (first match wins):
///   1. `override_dir` from config ([dashboard].source_dir) — explicit user choice.
///   2. Walk up from `current_exe()` looking for a `.git` directory. This handles
///      the development case where the binary lives at `<src>/target/release/...`.
///   3. Well-known install paths: `/opt/tetra-bluestation`, `/opt/flowstation`,
///      `/opt/tetra-bs`, `/opt/tetra`. Useful when the binary was deployed
///      separately from the source tree (e.g. binary in `/opt/tetra/`, sources
///      cloned in `/opt/tetra-bluestation/`).
///   4. `current_dir()` if it contains a `.git` directory.
///
/// Returns `Ok(path)` on success, or `Err(message)` listing all paths tried.
/// The returned path is guaranteed to contain a `.git` entry (file or directory —
/// `.git` can be a file in git worktrees).
fn resolve_source_dir(override_dir: Option<&str>) -> Result<std::path::PathBuf, String> {
    fn is_git_repo(p: &std::path::Path) -> bool {
        // `.git` is a directory in normal clones, but a file in git worktrees,
        // so check for existence of either form.
        p.join(".git").exists()
    }

    fn is_acceptable_path(p: &std::path::Path) -> bool {
        // Reject filesystem root, single-character paths, /usr, /bin, etc.
        // These are never valid source directories and would just produce confusing errors.
        let s = p.to_string_lossy();
        s != "/" && s.len() > 6 && !matches!(s.as_ref(), "/usr" | "/bin" | "/sbin" | "/etc" | "/var" | "/tmp")
    }

    let mut tried: Vec<String> = Vec::new();

    // 1. Explicit override from config.
    if let Some(dir) = override_dir {
        let path = std::path::PathBuf::from(dir);
        if is_git_repo(&path) && is_acceptable_path(&path) {
            return Ok(path);
        }
        tried.push(format!("{} (from config: not a git repo)", path.display()));
    }

    // 2. Walk up from the running binary path, up to 6 levels.
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..6 {
            let Some(p) = cur else { break };
            if !is_acceptable_path(&p) {
                tried.push(format!("{} (rejected: system path or too shallow)", p.display()));
                break;
            }
            if is_git_repo(&p) {
                return Ok(p);
            }
            tried.push(format!("{} (walked up from binary)", p.display()));
            cur = p.parent().map(|pp| pp.to_path_buf());
        }
    }

    // 3. Well-known install paths.
    for candidate in &[
        "/opt/tetra-bluestation",
        "/opt/flowstation",
        "/opt/tetra-bs",
        "/opt/tetra",
    ] {
        let p = std::path::PathBuf::from(candidate);
        if is_git_repo(&p) {
            return Ok(p);
        }
        if p.exists() {
            tried.push(format!("{} (well-known path: exists but not a git repo)", candidate));
        }
    }

    // 4. Current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        if is_git_repo(&cwd) && is_acceptable_path(&cwd) {
            return Ok(cwd);
        }
        tried.push(format!("{} (current working dir: not a git repo)", cwd.display()));
    }

    Err(format!(
        "OTA update needs the FlowStation git source tree to be present on this machine, \
         but none was found. You have two options:\n\
         \n\
         1) Clone the sources next to your binary:\n\
            git clone https://github.com/razvanzeces/flowstation.git /opt/tetra-bluestation\n\
            Then either move the binary into that tree, or set source_dir in config:\n\
            [dashboard]\n\
            source_dir = \"/opt/tetra-bluestation\"\n\
         \n\
         2) If your platform can't compile (e.g. Pi Zero), update manually by downloading \
         the latest release binary from GitHub.\n\
         \n\
         Paths tried: {}",
        if tried.is_empty() { "(none)".to_string() } else { tried.join("; ") }
    ))
}

/// Run git pull + cargo build --release in a background thread.
/// Steps:
///   1. Resolve source dir (config override -> walk-up -> well-known paths -> CWD)
///   2. Validate it is a git repository
///   3. Backup config.toml -> config.toml.bak
///   4. git fetch + compare commits
///   5. git merge --ff-only origin/main
///   6. cargo build --release
///   7. systemctl restart <service>  (after short delay)
fn run_update(update: SharedUpdateState, config_path: String, source_dir_override: Option<String>) {
    macro_rules! log {
        ($update:expr, $($arg:tt)*) => {{
            let line = format!($($arg)*);
            tracing::info!("UPDATE: {}", line);
            $update.lock().unwrap().append(&line);
        }};
    }

    log!(update, "=== FlowStation OTA Update ===");

    // Step 1: resolve source directory. Bail out cleanly if we can't find a git repo.
    let src_dir = match resolve_source_dir(source_dir_override.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            log!(update, "ERROR: {}", e);
            update.lock().unwrap().finish(false);
            return;
        }
    };

    log!(update, "Source dir: {}", src_dir.display());

    /// Run a command, stream stdout+stderr into the log, return Ok(stdout) or Err.
    fn run_cmd_output(
        update: &SharedUpdateState,
        program: &str,
        args: &[&str],
        dir: &std::path::Path,
    ) -> Option<String> {
        let line = format!("$ {} {}", program, args.join(" "));
        tracing::info!("UPDATE: {}", line);
        update.lock().unwrap().append(&line);

        match std::process::Command::new(program).args(args).current_dir(dir).output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                for l in stdout.lines() { update.lock().unwrap().append(l); }
                for l in stderr.lines() { update.lock().unwrap().append(l); }
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

    let src_str = src_dir.to_str().unwrap_or(".");

    // Step 2: explicit sanity check that this is a working git repo.
    // The .git existence check in resolve_source_dir() is necessary but not sufficient
    // (e.g. a corrupted repo). This catches edge cases with a clear error.
    //
    // Common edge case: FlowStation runs as root (e.g. via systemd) but the git clone
    // lives in a user's home directory (e.g. /home/pi/tetra-bluestation, owned by pi:pi).
    // Recent git versions refuse to operate on repos owned by a different user with
    // "dubious ownership" — fatal: detected dubious ownership in repository at '...'.
    // We try once first, and if we see that error, register the path as a safe.directory
    // via `git config --global --add safe.directory <path>` and retry.
    log!(update, "--- Verifying git repository ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "--is-inside-work-tree"], &src_dir).is_none() {
        // Check if the failure was specifically dubious ownership. The error went to the log
        // already; we look at the log content to decide whether to attempt the auto-fix.
        let saw_dubious_ownership = {
            let u = update.lock().unwrap();
            u.log.contains("dubious ownership")
        };
        if !saw_dubious_ownership {
            return;
        }
        log!(update, "");
        log!(update, "--- Detected dubious ownership — registering as safe.directory ---");
        if run_cmd_output(&update, "git", &["config", "--global", "--add", "safe.directory", src_str], &src_dir).is_none() {
            log!(update, "ERROR: could not register safe.directory automatically.");
            log!(update, "Manual fix: run this on the server as the user that runs FlowStation:");
            log!(update, "    git config --global --add safe.directory {}", src_str);
            return;
        }
        // Retry the verification.
        if run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "--is-inside-work-tree"], &src_dir).is_none() {
            log!(update, "ERROR: git verification still failing after safe.directory fix.");
            return;
        }
        log!(update, "✓ safe.directory registered, continuing.");
    }

    // Step 3: fetch remote without merging — just update refs
    log!(update, "--- Checking remote for updates ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "fetch", "origin", "main"], &src_dir).is_none() {
        return;
    }

    // Step 4: compare local HEAD with remote origin/main
    let local_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "HEAD"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if local_commit.is_empty() { return; }

    let remote_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "origin/main"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if remote_commit.is_empty() { return; }

    log!(update, "Local  commit: {}", &local_commit[..local_commit.len().min(12)]);
    log!(update, "Remote commit: {}", &remote_commit[..remote_commit.len().min(12)]);

    if local_commit == remote_commit {
        log!(update, "Already up to date — nothing to do.");
        update.lock().unwrap().finish(true);
        return;
    }

    // Step 5: show what changed
    let _ = run_cmd_output(&update, "git", &["-C", src_str, "log", "--oneline",
        &format!("HEAD..origin/main")], &src_dir);

    // Step 6: backup config before touching anything
    let backup_path = format!("{}.bak", config_path);
    match std::fs::copy(&config_path, &backup_path) {
        Ok(_)  => log!(update, "Config backed up → {}", backup_path),
        Err(e) => log!(update, "WARNING: config backup failed: {} (continuing)", e),
    }

    // Step 7: fast-forward merge (only changed files are touched on disk)
    log!(update, "--- git merge (fast-forward only) ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "merge", "--ff-only", "origin/main"], &src_dir).is_none() {
        return;
    }

    // Step 8: incremental build (cargo only recompiles changed crates)
    log!(update, "--- cargo build --release (incremental) ---");
    if run_cmd_output(&update, "cargo", &["build", "--release"], &src_dir).is_none() {
        return;
    }

    // Step 9: done — schedule restart
    log!(update, "--- Build successful. Restarting service in 2s... ---");
    update.lock().unwrap().finish(true);

    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::Restart,
        std::time::Duration::from_secs(2),
    );
}

pub struct DashboardServer {
    pub state: DashboardState,
    clients: WsClients,
    config_path: String,
    /// Shared stack config — used to read live_sds_queue from StackState.
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    cmd_tx: Option<CmdSender>,
    update_state: SharedUpdateState,
    /// Optional override for the OTA update source directory.
    /// If None, the update routine auto-detects.
    source_dir_override: Option<String>,
    /// Authentication credentials. None = no auth (open access). When set, requests
    /// must carry a valid `fs_session` cookie obtained from `POST /api/login`.
    auth: Option<(String, String)>,
    /// In-memory session store backing the cookie auth.
    sessions: SharedSessionStore,
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
            shared_config: None,
            cmd_tx: None,
            update_state: Arc::new(Mutex::new(UpdateState::new())),
            source_dir_override: None,
            auth: None,
            sessions: Arc::new(Mutex::new(SessionStore::new())),
            ts_last_broadcast: std::sync::Mutex::new([now; 4]),
        }
    }

    pub fn set_cmd_sender(&mut self, tx: CmdSender) {
        self.cmd_tx = Some(tx);
    }

    /// Provide the SharedConfig so the dashboard can read live SDS queue state.
    pub fn set_shared_config(&mut self, cfg: tetra_config::bluestation::SharedConfig) {
        self.shared_config = Some(cfg);
    }

    /// Configure an explicit source directory for OTA updates.
    pub fn set_source_dir(&mut self, source_dir: Option<String>) {
        self.source_dir_override = source_dir;
    }

    /// Configure HTTP Basic Auth credentials.
    pub fn set_auth(&mut self, auth: Option<(String, String)>) {
        self.auth = auth;
    }

    /// Mark that the stack started on the fallback config, with the reason why.
    /// The dashboard will display a persistent warning banner.
    pub fn set_fallback_config(&self, reason: String) {
        let mut s = self.state.write().unwrap();
        s.fallback_config_active = true;
        s.fallback_config_reason = reason;
    }

    pub fn start(&mut self, bind: &str, port: u16) {
        let addr = format!("{}:{}", bind, port);
        let state = Arc::clone(&self.state);
        let clients = Arc::clone(&self.clients);
        let config_path = self.config_path.clone();
        let cmd_tx: Arc<Mutex<Option<CmdSender>>> =
            Arc::new(Mutex::new(self.cmd_tx.take()));
        let update_state = Arc::clone(&self.update_state);
        let source_dir_override = self.source_dir_override.clone();
        let auth = self.auth.clone();
        let shared_config = self.shared_config.clone();
        let sessions = Arc::clone(&self.sessions);

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
                    let update_state = Arc::clone(&update_state);
                    let source_dir_override = source_dir_override.clone();
                    let auth = auth.clone();
                    let shared_config = shared_config.clone();
                    let sessions = Arc::clone(&sessions);
                    std::thread::Builder::new()
                        .name("dashboard-conn".into())
                        .spawn(move || handle_connection(stream, state, clients, config_path, cmd_tx, update_state, source_dir_override, auth, shared_config, sessions))
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
                TelemetryEvent::GroupCallStarted { call_id, gssi, caller_issi, ts } => {
                    s.calls.insert(*call_id, CallEntry {
                        call_id: *call_id, is_group: true, gssi: *gssi,
                        caller_issi: *caller_issi, called_issi: 0,
                        speaker_issi: Some(*caller_issi), started_at: Instant::now(), simplex: false, ts: *ts,
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
                TelemetryEvent::IndividualCallStarted { call_id, calling_issi, called_issi, simplex, ts } => {
                    s.calls.insert(*call_id, CallEntry {
                        call_id: *call_id, is_group: false, gssi: 0,
                        caller_issi: *calling_issi, called_issi: *called_issi,
                        speaker_issi: None, started_at: Instant::now(), simplex: *simplex, ts: *ts,
                    });
                    s.push_last_heard(*calling_issi, "call_individual", *called_issi);
                    s.push_log("INFO", format!("P2P call {} started: {} -> {}", call_id, calling_issi, called_issi));
                }
                TelemetryEvent::IndividualCallEnded { call_id } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("P2P call {} ended", call_id));
                }
                TelemetryEvent::BrewConnected { connected, server_version } => {
                    s.brew_online = *connected;
                    if *connected { s.brew_version = *server_version; }
                }
                TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
                    s.push_last_heard(*source_issi, "sds", *dest_issi);
                }
                TelemetryEvent::TsVoiceActivity { .. } => {
                    // Handled below with rate limiting — no state update needed
                }
                TelemetryEvent::TxVisual {
                    sample_rate, center_freq_hz, rms_dbfs, peak_dbfs,
                    spectrum_db_tenths, constellation_iq,
                } => {
                    // Cache the visual snapshot so newly-connected dashboard clients
                    // see something on the RF page before the next ~200 ms emit cycle.
                    s.last_tx_visual = Some(crate::net_dashboard::state::TxVisualSnapshot {
                        sample_rate: *sample_rate,
                        center_freq_hz: *center_freq_hz,
                        rms_dbfs: *rms_dbfs,
                        peak_dbfs: *peak_dbfs,
                        spectrum_db_tenths: spectrum_db_tenths.clone(),
                        constellation_iq: constellation_iq.clone(),
                    });
                }
                TelemetryEvent::TxQuality {
                    papr_db, evm_pct, dc_offset_i, dc_offset_q,
                    iq_amplitude_imbalance_db, iq_phase_imbalance_deg,
                    carrier_leakage_db, occupied_bandwidth_hz,
                } => {
                    // Cache the quality numbers so late-joining clients get them
                    // straight away rather than waiting up to a second.
                    s.last_tx_quality = Some(crate::net_dashboard::state::TxQualitySnapshot {
                        papr_db: *papr_db,
                        evm_pct: *evm_pct,
                        dc_offset_i: *dc_offset_i,
                        dc_offset_q: *dc_offset_q,
                        iq_amplitude_imbalance_db: *iq_amplitude_imbalance_db,
                        iq_phase_imbalance_deg: *iq_phase_imbalance_deg,
                        carrier_leakage_db: *carrier_leakage_db,
                        occupied_bandwidth_hz: *occupied_bandwidth_hz,
                    });
                }
                TelemetryEvent::SdrHealth { temperature_c, tx_gains, rx_gains } => {
                    s.last_sdr_health = Some(crate::net_dashboard::state::SdrHealthSnapshot {
                        temperature_c: *temperature_c,
                        tx_gains: tx_gains.clone(),
                        rx_gains: rx_gains.clone(),
                    });
                }
                TelemetryEvent::SysHealth { total_power_w, sensors } => {
                    s.last_sys_health = Some(crate::net_dashboard::state::SysHealthSnapshot {
                        total_power_w: *total_power_w,
                        sensors: sensors.clone(),
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
        TelemetryEvent::GroupCallStarted { call_id, gssi, caller_issi, ts } =>
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"group","gssi":gssi,"caller_issi":caller_issi,"ts":ts,"last_heard":{"issi":caller_issi,"activity":"call_group","dest":gssi}}),
        TelemetryEvent::GroupCallEnded { call_id, gssi: _ } =>
            serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::GroupCallSpeakerChanged { call_id, gssi, speaker_issi } =>
            serde_json::json!({"type":"speaker_changed","call_id":call_id,"speaker_issi":speaker_issi,"last_heard":{"issi":speaker_issi,"activity":"call_group","dest":gssi}}),
        TelemetryEvent::IndividualCallStarted { call_id, calling_issi, called_issi, simplex, ts } =>
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"individual","caller_issi":calling_issi,"called_issi":called_issi,"simplex":simplex,"ts":ts,"last_heard":{"issi":calling_issi,"activity":"call_individual","dest":called_issi}}),
        TelemetryEvent::IndividualCallEnded { call_id } =>
            serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::BrewConnected { connected, server_version } =>
            serde_json::json!({"type":"brew_status","connected":connected,"brew_version":server_version}),
        TelemetryEvent::SdsActivity { source_issi, dest_issi } =>
            serde_json::json!({"type":"last_heard","issi":source_issi,"activity":"sds","dest":dest_issi}),
        TelemetryEvent::TsVoiceActivity { ts } =>
            serde_json::json!({"type":"ts_voice","ts":ts}),
        TelemetryEvent::TxVisual {
            sample_rate, center_freq_hz, rms_dbfs, peak_dbfs,
            spectrum_db_tenths, constellation_iq,
        } => serde_json::json!({
            "type": "tx_visual",
            "sample_rate": sample_rate,
            "center_freq_hz": center_freq_hz,
            "rms_dbfs": rms_dbfs,
            "peak_dbfs": peak_dbfs,
            "spectrum_db_tenths": spectrum_db_tenths,
            "constellation_iq": constellation_iq,
        }),
        TelemetryEvent::TxQuality {
            papr_db, evm_pct, dc_offset_i, dc_offset_q,
            iq_amplitude_imbalance_db, iq_phase_imbalance_deg,
            carrier_leakage_db, occupied_bandwidth_hz,
        } => serde_json::json!({
            "type": "tx_quality",
            "papr_db": papr_db,
            "evm_pct": evm_pct,
            "dc_offset_i": dc_offset_i,
            "dc_offset_q": dc_offset_q,
            "iq_amplitude_imbalance_db": iq_amplitude_imbalance_db,
            "iq_phase_imbalance_deg": iq_phase_imbalance_deg,
            "carrier_leakage_db": carrier_leakage_db,
            "occupied_bandwidth_hz": occupied_bandwidth_hz,
        }),
        TelemetryEvent::SdrHealth { temperature_c, tx_gains, rx_gains } => serde_json::json!({
            "type": "sdr_health",
            "temperature_c": temperature_c,
            "tx_gains": tx_gains,
            "rx_gains": rx_gains,
        }),
        TelemetryEvent::SysHealth { total_power_w, sensors } => serde_json::json!({
            "type": "sys_health",
            "total_power_w": total_power_w,
            "sensors": sensors,
        }),
    };
    serde_json::to_string(&v).ok()
}

// ---------------------------------------------------------------------------
// HTTP Basic Auth helpers
// ---------------------------------------------------------------------------

/// Parse the `Authorization: Basic <base64>` header from raw HTTP headers string.
/// Returns `Some((username, password))` on success, `None` if absent or malformed.
///
/// Kept for potential future use (e.g. an opt-in scripting endpoint). The dashboard
/// now uses cookie-based sessions, so this is currently unreferenced.
#[allow(dead_code)]
fn parse_basic_auth(headers: &str) -> Option<(String, String)> {
    for line in headers.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("authorization:") {
            let value = line[14..].trim();
            if let Some(encoded) = value.strip_prefix("Basic ").or_else(|| value.strip_prefix("basic ")) {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(encoded.trim())
                    .ok()?;
                let s = String::from_utf8(decoded).ok()?;
                let mut parts = s.splitn(2, ':');
                let user = parts.next()?.to_string();
                let pass = parts.next().unwrap_or("").to_string();
                return Some((user, pass));
            }
        }
    }
    None
}

/// Constant-time byte slice comparison to mitigate timing attacks.
/// Returns true iff a == b in length and content.
fn timing_safe_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Send an HTTP 401 Unauthorized response that triggers the browser's native
/// Basic Auth dialog. Unused since the switch to cookie sessions.
#[allow(dead_code)]
fn http_response_401(mut stream: TcpStream) {
    let body = "Unauthorized";
    let resp = format!(
        "HTTP/1.1 401 Unauthorized\r\n\
         WWW-Authenticate: Basic realm=\"FlowStation Dashboard\", charset=\"UTF-8\"\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(), body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Send a ControlCommand through the dashboard → CMCE channel, best-effort.
fn send_control_cmd(cmd_tx: &Arc<Mutex<Option<CmdSender>>>, cmd: ControlCommand) {
    if let Ok(guard) = cmd_tx.lock() {
        if let Some(ref tx) = *guard {
            let _ = tx.send(cmd);
        }
    }
}

/// Serialize the current live SDS queue to JSON and serve it.
fn serve_live_sds_list(mut stream: TcpStream, cfg: &Option<tetra_config::bluestation::SharedConfig>) {
    let items: Vec<serde_json::Value> = cfg.as_ref().map(|c| {
        let state = c.state_read();
        state.live_sds_queue.iter().map(|m| serde_json::json!({
            "id": m.id,
            "text": m.text,
            "protocol_id": m.protocol_id,
            "source_issi": m.source_issi,
            "repeat_count": m.repeat_count,
            "sent_count": m.sent_count,
        })).collect()
    }).unwrap_or_default();
    let body = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn handle_connection(
    mut stream: TcpStream,
    state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
    source_dir_override: Option<String>,
    auth: Option<(String, String)>,
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    sessions: SharedSessionStore,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    // ── Read the first 4KB of headers into a buffer, peek first for routing ──
    // We need to both route on the request line AND read the Authorization header,
    // so we collect all headers before dispatching.
    let mut header_buf = Vec::with_capacity(2048);
    {
        // peek for the request line (already works for routing)
        let mut peek_buf = [0u8; 4096];
        let n = match stream.peek(&mut peek_buf) { Ok(n) => n, Err(_) => return };
        header_buf.extend_from_slice(&peek_buf[..n]);
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let req_line = header_str.lines().next().unwrap_or("").to_string();

    // ── Cookie-session auth ──────────────────────────────────────────────────
    // We replaced the browser-native Basic Auth dialog with a form-based login at
    // /login that issues an fs_session cookie. The native dialog has well-known
    // mobile usability issues (iOS Safari prompts 2-3 times, forgets credentials
    // between WebSocket reconnects, etc.). With cookies we control the UX fully.
    //
    // Public routes (no auth required): GET /login, POST /api/login, static assets.
    // Every other route is checked here against the session store.
    if let Some((ref expected_user, ref expected_pass)) = auth {
        // Login page and login API must remain reachable without a session.
        let is_login_page = req_line.starts_with("GET /login ") || req_line.starts_with("GET /login?");
        let is_login_api  = req_line.starts_with("POST /api/login ");

        // Validate session cookie when present. Note: validate() refreshes last-seen,
        // so active users effectively never time out.
        let session_ok = parse_session_cookie(&header_str)
            .and_then(|token| {
                let mut store = sessions.lock().ok()?;
                Some(store.validate(&token))
            })
            .unwrap_or(false);

        if is_login_page {
            let mut buf = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            }
            // If already logged in, send them straight to the dashboard.
            if session_ok {
                http_redirect(buf.into_inner(), "/");
            } else {
                serve_login_page(buf.into_inner());
            }
            return;
        }

        if is_login_api {
            // Body has form-encoded or JSON-encoded credentials.
            let mut buf = BufReader::new(stream);
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" { break; }
                let lower = line.to_lowercase();
                if lower.starts_with("content-length:") {
                    content_length = lower.trim_start_matches("content-length:").trim()
                        .trim_end_matches("\r\n").trim_end_matches('\n').parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length.min(4096)];
            let _ = buf.read_exact(&mut body);
            let body_str = String::from_utf8_lossy(&body);

            let (user, pass) = parse_login_body(&body_str);
            let ok = timing_safe_eq(user.as_bytes(), expected_user.as_bytes())
                  && timing_safe_eq(pass.as_bytes(), expected_pass.as_bytes());

            if ok {
                let token = if let Ok(mut store) = sessions.lock() { store.create() }
                            else { String::new() };
                tracing::info!("Dashboard: login OK (user: {})", user);
                serve_login_success(buf.into_inner(), &token);
            } else {
                tracing::warn!("Dashboard: login FAILED (user attempt: {})", user);
                // Small artificial delay to limit brute-force throughput.
                std::thread::sleep(std::time::Duration::from_millis(500));
                http_response(buf.into_inner(), 401, "Invalid credentials");
            }
            return;
        }

        // Logout: invalidate the cookie, then redirect to /login.
        if req_line.starts_with("POST /api/logout") || req_line.starts_with("GET /logout") {
            if let Some(token) = parse_session_cookie(&header_str) {
                if let Ok(mut store) = sessions.lock() {
                    store.invalidate(&token);
                }
            }
            let mut buf = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            }
            serve_logout(buf.into_inner());
            return;
        }

        // All other routes require a valid session.
        if !session_ok {
            let mut buf = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            }
            // For GET / (the dashboard SPA): redirect to /login so the browser navigates.
            // For API requests: 401 so JS code can detect and refresh.
            if req_line.starts_with("GET / ") || req_line.starts_with("GET /?") || req_line == "GET / HTTP/1.1" {
                http_redirect(buf.into_inner(), "/login");
            } else {
                http_response(buf.into_inner(), 401, "Unauthorized — please log in");
            }
            return;
        }
    }

    if req_line.contains("/ws") {
        handle_ws(stream, state, clients, cmd_tx, update_state, auth);
    } else if req_line.contains("GET /api/system") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        serve_system_info(buf.into_inner(), &config_path);
    } else if req_line.contains("POST /api/configs/activate") {
        let mut buf = BufReader::new(stream);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                content_length = lower.trim_start_matches("content-length:").trim()
                    .trim_end_matches("\r\n").trim_end_matches('\n').parse().unwrap_or(0);
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
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        // GET /api/configs/<name> — read a specific profile's content
        // GET /api/configs       — list all profiles
        let profile_name: Option<String> = req_line.split_whitespace().nth(1)
            .and_then(|path| path.strip_prefix("/api/configs/"))
            .map(|n| n.to_string());
        if let Some(name) = profile_name {
            serve_config_profile_get(buf.into_inner(), &config_path, &name);
        } else {
            serve_config_list(buf.into_inner(), &config_path);
        }
    } else if req_line.contains("POST /api/configs/") {
        // POST /api/configs/<name> — save content to a specific profile (not activate)
        let profile_name: Option<String> = req_line.split_whitespace().nth(1)
            .and_then(|path| path.strip_prefix("/api/configs/"))
            .map(|n| n.to_string());
        let mut buf = BufReader::new(stream);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                content_length = lower.trim_start_matches("content-length:").trim()
                    .trim_end_matches("\r\n").trim_end_matches('\n').parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length.min(512 * 1024)];
        let _ = buf.read_exact(&mut body);
        match profile_name {
            None => http_response(buf.into_inner(), 400, "missing profile name"),
            Some(name) => {
                match save_config_profile(&config_path, &name, &String::from_utf8_lossy(&body)) {
                    Ok(_) => {
                        tracing::info!("Dashboard: saved profile '{}'", name);
                        http_response(buf.into_inner(), 200, "OK")
                    }
                    Err(e) => http_response(buf.into_inner(), 500, &e),
                }
            }
        }
    } else if req_line.contains("GET /api/update/status") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        serve_update_status(buf.into_inner(), &update_state);
    } else if req_line.contains("POST /api/update") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
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
        let src_override = source_dir_override.clone();
        std::thread::Builder::new()
            .name("ota-update".into())
            .spawn(move || run_update(update_clone, cfg_clone, src_override))
            .ok();
        http_response(buf.into_inner(), 200, "OK");
    } else if req_line.contains("GET /api/config/backup") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        let backup_path = format!("{}.bak", config_path);
        serve_config_get(buf.into_inner(), &backup_path);
    } else if req_line.contains("POST /api/config/restore") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
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
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        serve_config_get(buf.into_inner(), &config_path);
    } else if req_line.contains("GET /api/live-sds") {
        // Return current live SDS queue as JSON.
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        serve_live_sds_list(buf.into_inner(), &shared_config);
    } else if req_line.contains("DELETE /api/live-sds/") {
        // DELETE /api/live-sds/<id>
        let id: u32 = req_line.split('/').nth(3)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        if id == 0 {
            http_response(buf.into_inner(), 400, "invalid id");
        } else {
            send_control_cmd(&cmd_tx, ControlCommand::DeleteLiveSds { id });
            http_response(buf.into_inner(), 200, "OK");
        }
    } else if req_line.contains("DELETE /api/live-sds") {
        // DELETE /api/live-sds  — clear all
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
        }
        send_control_cmd(&cmd_tx, ControlCommand::ClearLiveSds);
        http_response(buf.into_inner(), 200, "OK");
    } else if req_line.contains("POST /api/live-sds") {
        // POST /api/live-sds  body: JSON { "text": "...", "protocol_id": 220, "source_issi": 16777215, "repeat_count": 0 }
        let mut buf = BufReader::new(stream);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" { break; }
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:") {
                content_length = lower.trim_start_matches("content-length:").trim()
                    .trim_end_matches("\r\n").trim_end_matches('\n').parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length.min(4096)];
        let _ = buf.read_exact(&mut body);
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(v) => {
                let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
                if text.is_empty() || text.len() > 251 {
                    http_response(buf.into_inner(), 400, "text required, max 251 chars");
                } else {
                    let protocol_id = v.get("protocol_id").and_then(|p| p.as_u64()).unwrap_or(220) as u8;
                    let source_issi = v.get("source_issi").and_then(|s| s.as_u64()).unwrap_or(16777215) as u32;
                    let repeat_count = v.get("repeat_count").and_then(|r| r.as_u64()).unwrap_or(0) as u32;
                    tracing::info!("Dashboard: AddLiveSds text={:?} repeat={}", text, repeat_count);
                    send_control_cmd(&cmd_tx, ControlCommand::AddLiveSds {
                        text, protocol_id, source_issi, repeat_count,
                    });
                    http_response(buf.into_inner(), 200, "OK");
                }
            }
            Err(e) => http_response(buf.into_inner(), 400, &format!("invalid JSON: {}", e)),
        }
    // ── WiFi management endpoints ──────────────────────────────────────
    // All paths under /api/wifi/* are GET (read) or POST (mutate). We keep
    // the handlers small and delegate to the `wifi` module — see that for
    // docs on what each operation does. Responses are JSON.
    } else if req_line.contains("GET /api/wifi/status") {
        drain_http_headers(&mut stream);
        let body = match crate::wifi::status() {
            Ok(s) => serde_json::to_string(&serde_json::json!({"ok": true, "status": s})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("GET /api/wifi/scan") {
        drain_http_headers(&mut stream);
        let body = match crate::wifi::scan() {
            Ok(networks) => serde_json::to_string(&serde_json::json!({"ok": true, "networks": networks})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("GET /api/wifi/saved") {
        drain_http_headers(&mut stream);
        let body = match crate::wifi::list_saved() {
            Ok(profiles) => serde_json::to_string(&serde_json::json!({"ok": true, "profiles": profiles})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/wifi/connect") {
        // Body shape: {"ssid": "...", "psk": "...", "hidden": false} for a new
        // network, or {"uuid": "..."} to bring up a saved profile.
        let body = read_http_body(&mut stream);
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {}", e));
                return;
            }
        };
        let result = if let Some(uuid) = req.get("uuid").and_then(|v| v.as_str()) {
            tracing::info!("Dashboard: connecting saved WiFi profile uuid={}", uuid);
            crate::wifi::connect_saved(uuid)
        } else if let Some(ssid) = req.get("ssid").and_then(|v| v.as_str()) {
            let psk = req.get("psk").and_then(|v| v.as_str()).unwrap_or("");
            let hidden = req.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
            tracing::info!("Dashboard: connecting new WiFi ssid={} hidden={}", ssid, hidden);
            crate::wifi::connect_new(ssid, psk, hidden)
        } else {
            http_response(stream, 400, "missing uuid or ssid");
            return;
        };
        let body = match result {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/wifi/disconnect") {
        drain_http_headers(&mut stream);
        // Find the wireless device name and disconnect it. The body is empty.
        let iface = match crate::wifi::status() {
            Ok(s) if s.device_present => "wlan0".to_string(), // nmcli accepts any wifi dev name; wlan0 covers RPi
            _ => {
                http_response(stream, 400, "no wifi device");
                return;
            }
        };
        tracing::info!("Dashboard: disconnecting WiFi iface={}", iface);
        let body = match crate::wifi::disconnect(&iface) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/wifi/forget") {
        // Body: {"uuid": "..."}
        let body = read_http_body(&mut stream);
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => { http_response(stream, 400, &format!("invalid JSON: {}", e)); return; }
        };
        let uuid = match req.get("uuid").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => { http_response(stream, 400, "missing uuid"); return; }
        };
        tracing::info!("Dashboard: forgetting WiFi profile uuid={}", uuid);
        let body = match crate::wifi::forget(uuid) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/wifi/radio") {
        // Body: {"enabled": true|false}
        let body = read_http_body(&mut stream);
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => { http_response(stream, 400, &format!("invalid JSON: {}", e)); return; }
        };
        let enabled = req.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        tracing::info!("Dashboard: setting WiFi radio enabled={}", enabled);
        let body = match crate::wifi::set_radio(enabled) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("GET /api/wifi/available") {
        // Cheap probe used by the dashboard to decide whether to even show
        // the WiFi tab. Returns {"available": true|false}.
        drain_http_headers(&mut stream);
        let body = serde_json::to_string(&serde_json::json!({
            "available": crate::wifi::available()
        })).unwrap_or_default();
        http_json_response(stream, 200, &body);
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
             cmd_tx: Arc<Mutex<Option<CmdSender>>>, update_state: SharedUpdateState,
             _auth: Option<(String, String)>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));

    // Note: cookie-based auth is checked by handle_connection BEFORE we get here,
    // so we don't need to re-validate during the WS upgrade. The cookie travels on
    // the Upgrade request and was already verified against the session store.
    let callback = move |_req: &Request, res: Response| -> Result<Response, _> {
        Ok(res)
    };

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
        let brew_online = s.brew_online;
        let brew_version = s.brew_version;
        let fallback_active = s.fallback_config_active;
        let fallback_reason = s.fallback_config_reason.clone();
        let last_tx_visual = s.last_tx_visual.clone();
        let last_tx_quality = s.last_tx_quality.clone();
        let last_sdr_health = s.last_sdr_health.clone();
        let last_sys_health = s.last_sys_health.clone();
        drop(s);
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "type": "snapshot", "ms": ms, "calls": calls, "log": logs,
            "brew_online": brew_online, "brew_version": brew_version, "last_heard": last_heard,
            "fallback_config_active": fallback_active, "fallback_config_reason": fallback_reason,
            "last_tx_visual": last_tx_visual,
            "last_tx_quality": last_tx_quality,
            "last_sdr_health": last_sdr_health,
            "last_sys_health": last_sys_health,
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
                handle_ws_command(&text, &state, &cmd_tx, &update_state);
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

fn handle_ws_command(text: &str, state: &DashboardState, cmd_tx: &Arc<Mutex<Option<CmdSender>>>, update_state: &SharedUpdateState) {
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
            if dest == 0 || msg_text.is_empty() { return; }
            tracing::info!("Dashboard: SDS to {} = {}", dest, msg_text);

            // Encode text for SDS-TL TRANSFER:
            //   - If all characters are in ISO-8859-1 range → coding scheme 0x01 (LATIN), 1 byte/char
            //   - Otherwise → coding scheme 0x02 (UTF-16BE), 2 bytes/char (handles CJK, Arabic, etc.)
            // First byte of payload is the text coding scheme identifier per ETSI EN 300 392-2.
            let all_latin = msg_text.chars().all(|c| c as u32 <= 0xFF);
            let (coding_scheme, text_bytes): (u8, Vec<u8>) = if all_latin {
                let bytes: Vec<u8> = msg_text.chars().map(|c| c as u8).collect();
                (0x01, bytes)
            } else {
                // UTF-16BE encoding
                let bytes: Vec<u8> = msg_text.encode_utf16()
                    .flat_map(|u| u.to_be_bytes())
                    .collect();
                (0x02, bytes)
            };
            let mut payload = vec![coding_scheme];
            payload.extend_from_slice(&text_bytes);
            let len_bits = (payload.len() * 8) as u16;

            send_cmd(ControlCommand::SendSds {
                handle: 0,
                source_ssi: 9999,
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
        .output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let uptime_secs: u64 = std::fs::read_to_string("/proc/uptime").ok()
        .and_then(|s| s.split_whitespace().next().map(|n| n.parse::<f64>().ok()))
        .flatten().map(|f| f as u64).unwrap_or(0);

    let os_info = std::fs::read_to_string("/etc/os-release").ok()
        .and_then(|s| s.lines()
            .find(|l| l.starts_with("PRETTY_NAME="))
            .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string()))
        .unwrap_or_else(|| "Linux".to_string());

    let config_dir = std::path::Path::new(config_path)
        .parent().map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    // CPU model — /proc/cpuinfo "model name" (x86) or "Model" (ARM/Pi)
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo").ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.to_lowercase().starts_with("model name") || l.to_lowercase().starts_with("hardware"))
                .and_then(|l| l.splitn(2, ':').nth(1).map(|v| v.trim().to_string()))
        })
        .unwrap_or_else(|| "unknown".to_string());

    // CPU core count
    let cpu_cores = std::fs::read_to_string("/proc/cpuinfo").ok()
        .map(|s| s.lines().filter(|l| l.starts_with("processor")).count())
        .unwrap_or(0);

    // CPU load — /proc/stat first line: user nice system idle iowait irq softirq
    // Take a 100ms sample for a meaningful reading
    fn read_cpu_stat() -> Option<(u64, u64)> {
        let s = std::fs::read_to_string("/proc/stat").ok()?;
        let line = s.lines().next()?;
        let nums: Vec<u64> = line.split_whitespace().skip(1)
            .filter_map(|n| n.parse().ok()).collect();
        if nums.len() < 4 { return None; }
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0); // idle + iowait
        let total: u64 = nums.iter().sum();
        Some((total, idle))
    }
    let cpu_pct = if let (Some((t1, i1)), Some((t2, i2))) = (
        read_cpu_stat(),
        { std::thread::sleep(std::time::Duration::from_millis(100)); read_cpu_stat() }
    ) {
        let dt = t2.saturating_sub(t1);
        let di = i2.saturating_sub(i1);
        if dt > 0 { ((dt - di) * 100 / dt) as u8 } else { 0 }
    } else { 0 };

    // RAM — /proc/meminfo MemTotal and MemAvailable
    let (ram_total_mb, ram_used_mb) = std::fs::read_to_string("/proc/meminfo").ok()
        .map(|s| {
            let mut total = 0u64;
            let mut available = 0u64;
            for line in s.lines() {
                if line.starts_with("MemTotal:") {
                    total = line.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    available = line.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
                }
            }
            (total / 1024, (total.saturating_sub(available)) / 1024)
        })
        .unwrap_or((0, 0));

    // CPU temperature — try common Linux thermal zone paths
    let cpu_temp_c: Option<f32> = [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/thermal/thermal_zone1/temp",
        "/sys/devices/virtual/thermal/thermal_zone0/temp",
    ].iter().find_map(|path| {
        std::fs::read_to_string(path).ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .map(|t| t as f32 / 1000.0)
            .filter(|&t| t > 0.0 && t < 150.0) // sanity check
    });

    // RF / SoapySDR info — first check the binary exists at all, then run --find
    // (which is much cheaper than --probe and works even with no device attached;
    // --probe can hang for seconds on some drivers like HackRF/Pluto, and exits
    // with non-zero status when no device is found, which would have been
    // misreported as "not available").
    //
    // Try a couple of well-known install paths in addition to PATH because when
    // the stack runs under systemd it gets a minimal PATH that doesn't always
    // include /usr/local/bin where SoapySDR sometimes lands.
    let soapy_info = (|| -> String {
        let candidates = ["SoapySDRUtil", "/usr/bin/SoapySDRUtil", "/usr/local/bin/SoapySDRUtil"];
        for bin in &candidates {
            // First: does the binary respond to --info at all? That's the canonical
            // "is it installed?" check (`--info` is in every SoapySDR release and
            // returns the module summary regardless of attached hardware). We used
            // `--version` previously but on some SoapySDR builds it isn't a
            // recognised option, so the binary printed its help text with exit 0
            // and we misread that as "installed" → subsequently `--find` failed
            // and the user saw a confusing "SoapySDRUtil --find failed" with the
            // full help dump pasted in front of it. Thanks @shawnchain for the
            // PR comment.
            let probe = std::process::Command::new(bin)
                .arg("--info")
                .output();
            if let Ok(out) = probe {
                if out.status.success() {
                    // Now run --find to enumerate devices. Empty result means
                    // "no SDR connected" which is a valid, useful state to show.
                    let find = std::process::Command::new(bin)
                        .arg("--find")
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
                    // Keep only the first few lines of --info (banner + API/ABI
                    // version + module path). Beyond that it dumps a long module
                    // listing that isn't useful in the dashboard card.
                    let info = String::from_utf8_lossy(&out.stdout);
                    let info_summary: String = info.lines()
                        .filter(|l| {
                            let ll = l.to_lowercase();
                            ll.contains("lib version") || ll.contains("api version")
                            || ll.contains("abi version") || ll.contains("install root")
                        })
                        .take(4)
                        .collect::<Vec<&str>>()
                        .join("\n");
                    return match find {
                        Some(text) if text.lines().any(|l| l.to_lowercase().contains("found device")) => {
                            // Keep only the useful per-device lines (driver/serial/label) to
                            // avoid dumping pages of advertising.
                            let lines: Vec<&str> = text.lines()
                                .filter(|l| {
                                    let ll = l.to_lowercase();
                                    ll.contains("found device") || ll.contains("driver")
                                    || ll.contains("serial") || ll.contains("label")
                                    || ll.contains("name") || ll.contains("manufacturer")
                                })
                                .take(20)
                                .collect();
                            format!("{}\n{}", info_summary, lines.join("\n"))
                        }
                        Some(_) => format!("{}\nNo SDR device detected.", info_summary),
                        None    => format!("{}\nSoapySDRUtil --find failed.", info_summary),
                    };
                }
            }
        }
        // Falling through the loop without returning means no candidate path
        // successfully ran `--info` — the binary is genuinely missing.
        "SoapySDRUtil not installed (apt install soapysdr-tools).".to_string()
    })();

    // Auto-detected SDR name — set by `phy::components::soapy_settings::get_settings()`
    // at stack startup. None if no SoapySDR-backed phy is in use (file backend etc).
    let sdr_name = crate::phy::components::soapy_settings::detected_sdr_name()
        .unwrap_or_else(|| "unknown".to_string());

    let body = serde_json::to_string(&serde_json::json!({
        "hostname": hostname,
        "uptime_secs": uptime_secs,
        "os": os_info,
        "config_path": config_path,
        "config_dir": config_dir,
        "stack_version": tetra_core::STACK_VERSION,
        "cpu_model": cpu_model,
        "cpu_cores": cpu_cores,
        "cpu_pct": cpu_pct,
        "ram_total_mb": ram_total_mb,
        "ram_used_mb": ram_used_mb,
        "cpu_temp_c": cpu_temp_c,
        "soapy_info": soapy_info,
        "sdr_name": sdr_name,
    })).unwrap_or_else(|_| "{}".to_string());

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn serve_config_list(mut stream: TcpStream, config_path: &str) {
    let active_name = std::path::Path::new(config_path)
        .file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let config_dir = std::path::Path::new(config_path)
        .parent().unwrap_or(std::path::Path::new("."));

    let mut profiles: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(config_dir) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Include .toml files, exclude backups (.bak)
                if name.ends_with(".toml") && !name.ends_with(".bak") { Some(name) } else { None }
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

/// Read a specific config profile and serve its content as plain text.
fn serve_config_profile_get(stream: TcpStream, config_path: &str, profile_name: &str) {
    if profile_name.contains('/') || profile_name.contains('\\') || profile_name.contains("..") {
        return http_response(stream, 400, "invalid profile name");
    }
    if !profile_name.ends_with(".toml") {
        return http_response(stream, 400, "profile must be a .toml file");
    }
    let config_dir = std::path::Path::new(config_path)
        .parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(profile_name);
    serve_config_get(stream, &profile_path.to_string_lossy());
}

/// Save content to a specific config profile (not the active config).
/// The active config is identified by config_path; writing to it is rejected
/// (use POST /api/config for that).
fn save_config_profile(config_path: &str, profile_name: &str, content: &str) -> Result<(), String> {
    if profile_name.contains('/') || profile_name.contains('\\') || profile_name.contains("..") {
        return Err("invalid profile name".to_string());
    }
    if !profile_name.ends_with(".toml") {
        return Err("profile must be a .toml file".to_string());
    }
    let config_dir = std::path::Path::new(config_path)
        .parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(profile_name);

    // Refuse to overwrite the active config through this endpoint
    let active_name = std::path::Path::new(config_path)
        .file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if profile_name == active_name {
        return Err("cannot overwrite active config via profile editor — use the Config editor tab".to_string());
    }

    std::fs::write(&profile_path, content.as_bytes())
        .map_err(|e| format!("failed to write profile: {}", e))
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

    let config_dir = std::path::Path::new(config_path)
        .parent().unwrap_or(std::path::Path::new("."));
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
        code, status, body.len(), body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Like `http_response` but serves JSON. Used by the WiFi management endpoints
/// which all return structured `{"ok": ..., ...}` payloads.
fn http_json_response(mut stream: TcpStream, code: u16, body: &str) {
    let status = if code == 200 { "OK" } else { "Error" };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code, status, body.len(), body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Consume and discard HTTP request headers up to the blank line. Use this
/// for GET-style endpoints that don't read a body — we still need to clear
/// the headers off the stream before responding, otherwise some clients
/// reuse the connection and get confused.
fn drain_http_headers(stream: &mut TcpStream) {
    // We read byte-by-byte to find the \r\n\r\n delimiter. This is slower
    // than BufReader-line reads but doesn't consume bytes past the headers,
    // which matters for POST handlers that need to keep reading the body.
    let mut prev3 = [0u8; 3];
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte).unwrap_or(0) == 0 { break; }
        // Detect "\r\n\r\n" by sliding a 4-byte window.
        if prev3 == [b'\r', b'\n', b'\r'] && byte[0] == b'\n' { break; }
        prev3 = [prev3[1], prev3[2], byte[0]];
    }
}

/// Read an HTTP request body from the stream. Returns the body bytes.
/// We read headers first to extract Content-Length, then read exactly that
/// many bytes. Returns an empty vec if Content-Length is missing or 0.
fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
    // Read headers line-by-line. We can't use BufReader here because we'd
    // lose buffered bytes when we drop it; instead read one byte at a time
    // until we hit the header/body separator, accumulating into a String we
    // can scan for Content-Length.
    let mut header_buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    let mut prev3 = [0u8; 3];
    loop {
        if stream.read(&mut byte).unwrap_or(0) == 0 { return Vec::new(); }
        header_buf.push(byte[0]);
        if prev3 == [b'\r', b'\n', b'\r'] && byte[0] == b'\n' { break; }
        prev3 = [prev3[1], prev3[2], byte[0]];
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let mut content_length = 0usize;
    for line in header_str.lines() {
        let lower = line.to_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
            break;
        }
    }
    if content_length == 0 { return Vec::new(); }
    let mut body = vec![0u8; content_length];
    let _ = stream.read_exact(&mut body);
    body
}

// ── Login UI / session helpers ──────────────────────────────────────────────

/// Parse a login POST body. Accepts both `application/x-www-form-urlencoded`
/// (user=...&password=...) and a minimal JSON shape `{"user":"...","password":"..."}`.
/// This makes the endpoint trivially usable from both an HTML form and fetch().
fn parse_login_body(body: &str) -> (String, String) {
    let trimmed = body.trim();
    // JSON shape: look for "user":"..." and "password":"..." anywhere in the string.
    // We deliberately don't bring in a JSON parser for these two fields.
    if trimmed.starts_with('{') {
        let user = json_field(trimmed, "user").unwrap_or_default();
        let pass = json_field(trimmed, "password").unwrap_or_default();
        return (user, pass);
    }
    // Form-encoded.
    let mut user = String::new();
    let mut pass = String::new();
    for pair in trimmed.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        let decoded = url_decode(v);
        match k {
            "user" | "username" => user = decoded,
            "password" | "pass" => pass = decoded,
            _ => {}
        }
    }
    (user, pass)
}

fn json_field(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let idx = s.find(&needle)?;
    let after = &s[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]); i += 1;
                }
            }
            b => { out.push(b); i += 1; }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

fn http_redirect(mut stream: TcpStream, location: &str) {
    let resp = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_login_success(mut stream: TcpStream, token: &str) {
    // Two cookies:
    //   fs_session: HttpOnly — the actual session token, inaccessible to JS.
    //   fs_auth: readable — a marker telling the dashboard JS "auth is on",
    //                       so it can decide to show the Logout button.
    // The marker carries no security value; the HttpOnly session is what's checked.
    let body = "{\"ok\":true}";
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Set-Cookie: fs_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800\r\n\
         Set-Cookie: fs_auth=1; Path=/; SameSite=Lax; Max-Age=604800\r\n\
         Connection: close\r\n\r\n{}",
        body.len(), token, body
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_logout(mut stream: TcpStream) {
    // Expire both cookies immediately; client navigates to /login next.
    let resp = "HTTP/1.1 302 Found\r\n\
                Location: /login\r\n\
                Set-Cookie: fs_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0\r\n\
                Set-Cookie: fs_auth=; Path=/; SameSite=Lax; Max-Age=0\r\n\
                Content-Length: 0\r\n\
                Connection: close\r\n\r\n";
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_login_page(mut stream: TcpStream) {
    let body = crate::net_dashboard::html::LOGIN_HTML;
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}
