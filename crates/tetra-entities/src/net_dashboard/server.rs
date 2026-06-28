use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tungstenite::{
    Message, accept_hdr,
    handshake::server::{Request, Response},
};

use crate::net_control::commands::ControlCommand;
use crate::net_dashboard::html::DASHBOARD_HTML;
use crate::net_dashboard::state::{CallEntry, DashboardState, DashboardStateInner, MsEntry};
use crate::net_telemetry::TelemetryEvent;
use crate::tpg2200::build_tpg2200_callout_payload;

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
            .map(|d| d.as_nanos())
            .unwrap_or(0);
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
    for candidate in &["/opt/tetra-bluestation", "/opt/flowstation", "/opt/tetra-bs", "/opt/tetra"] {
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

/// Spawn an already-configured command, streaming its stdout+stderr into the update log line by
/// line (so a long `cargo build` shows live progress instead of looking hung — FH-BUG-035), and
/// return the collected stdout on success. On spawn/exit failure it logs an error, marks the update
/// `finish(false)`, and returns `None`.
fn stream_cmd(update: &SharedUpdateState, mut cmd: std::process::Command, label: String) -> Option<String> {
    use std::io::{BufRead, BufReader};
    update.lock().unwrap().append(&label);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut u = update.lock().unwrap();
            u.append(&format!("ERROR: failed to start '{}': {}", label.trim_start_matches("$ "), e));
            u.finish(false);
            return None;
        }
    };
    // stderr on a side thread (cargo writes its progress there), stdout collected on this one.
    let err_handle = child.stderr.take().map(|err| {
        let u = std::sync::Arc::clone(update);
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                u.lock().unwrap().append(&line);
            }
        })
    });
    let mut collected = String::new();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            update.lock().unwrap().append(&line);
            collected.push_str(&line);
            collected.push('\n');
        }
    }
    if let Some(h) = err_handle {
        let _ = h.join();
    }
    match child.wait() {
        Ok(s) if s.success() => Some(collected),
        Ok(s) => {
            let mut u = update.lock().unwrap();
            u.append(&format!("ERROR: exited with {}", s));
            u.finish(false);
            None
        }
        Err(e) => {
            let mut u = update.lock().unwrap();
            u.append(&format!("ERROR: wait failed: {}", e));
            u.finish(false);
            None
        }
    }
}

/// Locate the `cargo` binary. Under a systemd service the process PATH usually omits `~/.cargo/bin`
/// (where rustup installs cargo), so `Command::new("cargo")` fails with ENOENT even though an
/// interactive SSH shell finds it (FH-BUG-037). We resolve an absolute path from $CARGO, the user's
/// home, and well-known install locations, falling back to a bare PATH lookup.
fn find_cargo() -> std::path::PathBuf {
    use std::path::{Path, PathBuf};
    if let Ok(c) = std::env::var("CARGO") {
        let p = PathBuf::from(c);
        if p.is_file() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = Path::new(&home).join(".cargo/bin/cargo");
        if p.is_file() {
            return p;
        }
    }
    // Root/system-owned locations only — safe to run as a possibly-privileged service identity.
    for cand in [
        "/usr/local/cargo/bin/cargo",
        "/root/.cargo/bin/cargo",
        "/usr/local/bin/cargo",
        "/usr/bin/cargo",
    ] {
        let p = PathBuf::from(cand);
        if p.is_file() {
            return p;
        }
    }
    // Fall back to a bare PATH lookup. We deliberately do NOT scan other users' home directories:
    // running the first `~/.cargo/bin/cargo` found under /home as the service identity would let any
    // local user plant a binary we'd execute. If the service has no cargo on PATH, the operator can
    // point us at one via the $CARGO environment variable.
    PathBuf::from("cargo")
}

/// Whether the running binary was built from the repository's current commit. `binary_git_hash` is
/// the abbreviated hash baked into `tetra_core::STACK_VERSION` at build time; `repo_head` is the
/// full HEAD hash. Returns `None` when the build embedded no usable hash (so we can't tell). This is
/// the source of truth for "is the binary actually up to date" — comparing git HEAD vs origin alone
/// wrongly reports success after a merge that landed but whose build then failed (FH-BUG-035/037).
fn binary_built_from(binary_git_hash: &str, repo_head: &str) -> Option<bool> {
    let h = binary_git_hash.strip_suffix("-modified").unwrap_or(binary_git_hash);
    if h.is_empty() || h == "unknown" {
        return None;
    }
    Some(repo_head.starts_with(h))
}

/// Run git pull + cargo build --release in a background thread.
/// Steps:
///   1. Resolve source dir (config override -> walk-up -> well-known paths -> CWD)
///   2. Validate it is a git repository
///   3. git fetch + compare commits
///   4. If commits differ: backup config.toml -> config.toml.bak, then git merge --ff-only
///   5. Rebuild only if the merge landed OR the running binary's embedded git hash != repo HEAD
///      (so a previously-failed build is not mistaken for "already up to date")
///   6. cargo build --release (cargo resolved explicitly; output streamed live)
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

    /// Run a command, streaming stdout+stderr into the log; return collected stdout or None.
    fn run_cmd_output(update: &SharedUpdateState, program: &str, args: &[&str], dir: &std::path::Path) -> Option<String> {
        let label = format!("$ {} {}", program, args.join(" "));
        tracing::info!("UPDATE: {}", label);
        let mut cmd = std::process::Command::new(program);
        cmd.args(args).current_dir(dir);
        stream_cmd(update, cmd, label)
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
        if run_cmd_output(
            &update,
            "git",
            &["config", "--global", "--add", "safe.directory", src_str],
            &src_dir,
        )
        .is_none()
        {
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

    // Step 5: sync the working tree to origin/main when the commits differ.
    let mut merged = false;
    if local_commit != remote_commit {
        // Show what changed.
        let _ = run_cmd_output(&update, "git", &["-C", src_str, "log", "--oneline", "HEAD..origin/main"], &src_dir);

        // Backup config before touching anything.
        let backup_path = format!("{}.bak", config_path);
        match std::fs::copy(&config_path, &backup_path) {
            Ok(_) => log!(update, "Config backed up → {}", backup_path),
            Err(e) => log!(update, "WARNING: config backup failed: {} (continuing)", e),
        }

        // Fast-forward merge (only changed files are touched on disk).
        log!(update, "--- git merge (fast-forward only) ---");
        if run_cmd_output(&update, "git", &["-C", src_str, "merge", "--ff-only", "origin/main"], &src_dir).is_none() {
            return;
        }
        merged = true;
    }

    // Step 6: decide whether a rebuild is actually required. We compare the git hash BAKED INTO the
    // running binary (tetra_core::STACK_VERSION) against the repository HEAD — not merely git HEAD
    // vs origin. This is what makes a previously-FAILED build recoverable: after a merge lands but
    // the build fails, git already points at origin, yet the running binary is still the old one, so
    // "git up to date" must NOT be reported as "updated" (FH-BUG-035 / FH-BUG-037).
    let repo_head = if merged { remote_commit.as_str() } else { local_commit.as_str() };
    let binary_current = binary_built_from(tetra_core::GIT_HASH, repo_head);
    if !merged && binary_current != Some(false) {
        match binary_current {
            Some(true) => log!(
                update,
                "Already up to date — running {} matches the repository.",
                tetra_core::STACK_VERSION
            ),
            _ => log!(
                update,
                "Repository is up to date; running {} (build hash not verifiable).",
                tetra_core::STACK_VERSION
            ),
        }
        update.lock().unwrap().finish(true);
        return;
    }
    if !merged {
        log!(
            update,
            "Repository is current but the running binary ({}) predates it — rebuilding.",
            tetra_core::STACK_VERSION
        );
    }

    // Step 7: build. cargo lives in ~/.cargo/bin, which the systemd service PATH usually omits, so
    // resolve it explicitly and put its directory on PATH for the rustc/rustup shims (FH-BUG-037).
    // Output is streamed live so a long compile shows progress instead of looking hung (FH-BUG-035).
    log!(update, "--- cargo build --release ---");
    let cargo = find_cargo();
    log!(update, "Using cargo: {}", cargo.display());
    let mut build = std::process::Command::new(&cargo);
    build.args(["build", "--release"]).current_dir(&src_dir);
    // Put cargo's own directory on PATH so the rustc/rustup shims resolve under the service's minimal
    // PATH. Only for an ABSOLUTE cargo: a bare "cargo" has an empty parent, and prepending "" (or
    // appending to an empty PATH) injects an empty entry that Unix treats as the current directory —
    // which would let a planted `rustc`/`cc` in the source tree run during the build.
    if cargo.is_absolute() {
        if let Some(bin) = cargo.parent().filter(|p| !p.as_os_str().is_empty()) {
            let new_path = match std::env::var("PATH") {
                Ok(p) if !p.is_empty() => format!("{}:{}", bin.display(), p),
                _ => bin.display().to_string(),
            };
            build.env("PATH", new_path);
        }
    }
    if stream_cmd(&update, build, "$ cargo build --release".to_string()).is_none() {
        return;
    }

    // Step 8: done — schedule restart.
    log!(update, "--- Build successful. Restarting service in 2s... ---");
    update.lock().unwrap().finish(true);

    crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, std::time::Duration::from_secs(2));
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
    /// When true AND `auth` is Some, anonymous visitors get a read-only public overview instead of
    /// being bounced to /login. Inert without auth. (FH-FEAT-033)
    public_overview: bool,
    /// In-memory session store backing the cookie auth.
    sessions: SharedSessionStore,
    /// Last time a ts_voice WS message was broadcast per carrier/timeslot.
    ts_last_broadcast: std::sync::Mutex<HashMap<(u16, u8), std::time::Instant>>,
    /// On-demand RadioID callsign resolver (ISSI → indicativ), cached locally.
    radioid: crate::net_dashboard::radioid::RadioIdCache,
}

impl DashboardServer {
    pub fn new(config_path: String) -> Self {
        // RadioID callsign cache lives next to the active config file.
        let radioid_path = std::path::Path::new(&config_path)
            .parent()
            .map(|d| d.join("radioid_cache.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("radioid_cache.json"));
        Self {
            state: Arc::new(RwLock::new(DashboardStateInner::new(config_path.clone()))),
            clients: Arc::new(Mutex::new(Vec::new())),
            config_path,
            shared_config: None,
            cmd_tx: None,
            update_state: Arc::new(Mutex::new(UpdateState::new())),
            source_dir_override: None,
            auth: None,
            public_overview: false,
            sessions: Arc::new(Mutex::new(SessionStore::new())),
            ts_last_broadcast: std::sync::Mutex::new(HashMap::new()),
            radioid: crate::net_dashboard::radioid::RadioIdCache::new(radioid_path),
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

    /// Enable the anonymous read-only public overview (only effective when auth is set).
    /// Must be called BEFORE `start()`, which captures the flag into the server thread.
    pub fn set_public_overview(&mut self, on: bool) {
        self.public_overview = on;
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
        let cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.cmd_tx.take()));
        let update_state = Arc::clone(&self.update_state);
        let source_dir_override = self.source_dir_override.clone();
        let auth = self.auth.clone();
        let public_overview = self.public_overview;
        let shared_config = self.shared_config.clone();
        let sessions = Arc::clone(&self.sessions);
        let radioid = self.radioid.clone();

        std::thread::Builder::new()
            .name("dashboard-server".into())
            .spawn(move || {
                // Retry the bind instead of giving up after a single failure (FH-BUG-043).
                // On a cold boot the configured bind address may not be assigned yet — DHCP
                // lease still pending, or a VPN/wg/tun interface that comes up after the
                // service — so the first bind can fail with EADDRNOTAVAIL even with
                // After=network-online.target. Previously the thread logged once and exited,
                // leaving the dashboard permanently down (while the RF stack ran fine) until a
                // manual stop/start. Retrying lets it self-heal once the address appears. This
                // loop runs only on the dashboard thread, so it can never block the PHY/main loop.
                let listener = loop {
                    match TcpListener::bind(&addr) {
                        Ok(l) => {
                            tracing::info!("Dashboard listening on http://{}", addr);
                            break l;
                        }
                        Err(e) => {
                            tracing::error!(
                                "Dashboard failed to bind {}: {} — retrying in 5s (interface/IP may not be ready yet)",
                                addr,
                                e
                            );
                            std::thread::sleep(std::time::Duration::from_secs(5));
                        }
                    }
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
                    let radioid = radioid.clone();
                    std::thread::Builder::new()
                        .name("dashboard-conn".into())
                        .spawn(move || {
                            handle_connection(
                                stream,
                                state,
                                clients,
                                config_path,
                                cmd_tx,
                                update_state,
                                source_dir_override,
                                auth,
                                shared_config,
                                sessions,
                                radioid,
                                public_overview,
                            )
                        })
                        .ok();
                }
            })
            .expect("failed to spawn dashboard thread");
    }

    pub fn handle_telemetry(&self, event: TelemetryEvent) {
        let mut msg = event_to_ws_msg(&event);
        // Emergency banner add/remove broadcasts are transition-gated (only on enter/clear, not on
        // every re-send), so they can't ride the generic `event_to_ws_msg` path. Collect them under
        // the state lock, then flush after it drops.
        let mut extra_broadcasts: Vec<String> = Vec::new();
        {
            let mut s = self.state.write().unwrap();
            match &event {
                TelemetryEvent::MsRegistration { issi } => {
                    s.ms_map.insert(
                        *issi,
                        MsEntry {
                            issi: *issi,
                            groups: Vec::new(),
                            selected_group: None,
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
                TelemetryEvent::MsTimeoutDrop { issi } => {
                    // Same UI effect as a deregistration (the MS is gone from the cell); the
                    // distinct event only matters to alert consumers that report the reason.
                    s.ms_map.remove(issi);
                    s.push_log("WARN", format!("MS {} dropped (no response to T351)", issi));
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
                        // If the previously-selected TG is no longer affiliated, drop the
                        // pointer so the dashboard doesn't carry a stale ▶ marker into the
                        // next render (or, worse, fail to re-render anything because the
                        // selected GSSI is missing from the groups list).
                        if let Some(sel) = e.selected_group
                            && !e.groups.contains(&sel)
                        {
                            e.selected_group = None;
                        }
                    }
                }
                TelemetryEvent::MsGroupDetach { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.groups.retain(|g| !gssis.contains(g));
                        // Same stale-pointer guard as the snapshot path above.
                        if let Some(sel) = e.selected_group
                            && gssis.contains(&sel)
                        {
                            e.selected_group = None;
                        }
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
                    carrier_num,
                    ts,
                    priority,
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
                            carrier_num: *carrier_num,
                            ts: *ts,
                            peer_carrier_num: None,
                            peer_ts: None,
                            priority: *priority,
                        },
                    );
                    // The caller keyed up on this GSSI, so it's their actively-selected TG (vs the
                    // other scanned/affiliated groups). The browser derives the same thing from the
                    // call_started message; this keeps the snapshot sent to new clients in sync.
                    if let Some(e) = s.ms_map.get_mut(caller_issi) {
                        e.selected_group = Some(*gssi);
                    }
                    s.push_last_heard(*caller_issi, "call_group", *gssi);
                    // priority 15 = emergency (ETSI clause 14.8). Flag it in the live log. (The
                    // persistent emergency banner + Telegram are driven by the emergency-status
                    // alarm; an emergency-priority CALL is surfaced in the Active Calls table.)
                    if *priority >= 15 {
                        s.push_log(
                            "WARN",
                            format!(
                                "EMERGENCY group call {} started: {} -> GSSI {} (priority {})",
                                call_id, caller_issi, gssi, priority
                            ),
                        );
                    } else {
                        s.push_log("INFO", format!("Group call {} started: {} -> GSSI {}", call_id, caller_issi, gssi));
                    }
                }
                TelemetryEvent::GroupCallEnded { call_id, gssi: _ } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("Group call {} ended", call_id));
                }
                TelemetryEvent::CallSpeakerChanged {
                    call_id,
                    is_group,
                    dest_addr,
                    speaker_issi,
                    carrier_num,
                    ts,
                } => {
                    if let Some(c) = s.calls.get_mut(call_id) {
                        c.speaker_issi = Some(*speaker_issi);
                    }
                    // Whoever is speaking has this TG/peer selected.
                    if let Some(e) = s.ms_map.get_mut(speaker_issi) {
                        e.selected_group = if *is_group { Some(*dest_addr) } else { None };
                    }
                    s.push_last_heard(*speaker_issi, if *is_group { "call_group" } else { "call_individual" }, *dest_addr);
                    if let Ok(json) = serde_json::to_string(&serde_json::json!({
                        "type":"speaker_changed",
                        "call_id":call_id,
                        "speaker_issi":speaker_issi,
                        "carrier_num":carrier_num,
                        "ts":ts,
                        "last_heard":{
                            "issi":speaker_issi,
                            "activity":if *is_group { "call_group" } else { "call_individual" },
                            "dest":dest_addr
                        }
                    })) {
                        msg = Some(json);
                    }
                }
                TelemetryEvent::IndividualCallStarted {
                    call_id,
                    calling_issi,
                    called_issi,
                    simplex,
                    carrier_num,
                    ts,
                    peer_carrier_num,
                    peer_ts,
                    priority,
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
                            carrier_num: *carrier_num,
                            ts: *ts,
                            peer_carrier_num: *peer_carrier_num,
                            peer_ts: *peer_ts,
                            priority: *priority,
                        },
                    );
                    s.push_last_heard(*calling_issi, "call_individual", *called_issi);
                    // priority 15 = emergency (ETSI clause 14.8). Flag it in the live log. (The
                    // persistent emergency banner + Telegram are driven by the emergency-status
                    // alarm; an emergency-priority CALL is surfaced in the Active Calls table.)
                    if *priority >= 15 {
                        s.push_log(
                            "WARN",
                            format!(
                                "EMERGENCY P2P call {} started: {} -> {} (priority {})",
                                call_id, calling_issi, called_issi, priority
                            ),
                        );
                    } else {
                        s.push_log("INFO", format!("P2P call {} started: {} -> {}", call_id, calling_issi, called_issi));
                    }
                }
                TelemetryEvent::IndividualCallEnded { call_id } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("P2P call {} ended", call_id));
                }
                TelemetryEvent::BrewConnected { connected, server_version } => {
                    s.brew_online = *connected;
                    // Version is monotonic within a run (FH-BUG: brew shown as v0). The transport
                    // reports 0 ("unknown") on every (re)connect and v1 is only learned later
                    // (lazily, from a v1-flavoured group call); an unconditional assignment let a
                    // reconnect DOWNGRADE a confirmed v1 back to v0. Only ever raise it.
                    if *connected {
                        s.brew_version = s.brew_version.max(*server_version);
                    }
                }
                TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
                    s.push_last_heard(*source_issi, "sds", *dest_issi);
                }
                TelemetryEvent::SdsLog {
                    direction,
                    source_issi,
                    dest_issi,
                    is_group,
                    protocol_id,
                    text,
                } => {
                    s.push_sds_log(direction, *source_issi, *dest_issi, *is_group, *protocol_id, text.clone());
                }
                TelemetryEvent::TsVoiceActivity { .. } => {
                    // Handled below with rate limiting — no state update needed
                }
                TelemetryEvent::TxVisual {
                    sample_rate,
                    center_freq_hz,
                    carriers,
                    constellation_carrier,
                    rms_dbfs,
                    peak_dbfs,
                    spectrum_db_tenths,
                    constellation_iq,
                } => {
                    // Cache the visual snapshot so newly-connected dashboard clients
                    // see something on the RF page before the next ~200 ms emit cycle.
                    s.last_tx_visual = Some(crate::net_dashboard::state::TxVisualSnapshot {
                        sample_rate: *sample_rate,
                        center_freq_hz: *center_freq_hz,
                        carriers: carriers.clone(),
                        constellation_carrier: *constellation_carrier,
                        rms_dbfs: *rms_dbfs,
                        peak_dbfs: *peak_dbfs,
                        spectrum_db_tenths: spectrum_db_tenths.clone(),
                        constellation_iq: constellation_iq.clone(),
                    });
                }
                TelemetryEvent::TxQuality {
                    papr_db,
                    evm_pct,
                    evm_carrier,
                    dc_offset_i,
                    dc_offset_q,
                    iq_amplitude_imbalance_db,
                    iq_phase_imbalance_deg,
                    carrier_leakage_db,
                    occupied_bandwidth_hz,
                } => {
                    // Cache the quality numbers so late-joining clients get them
                    // straight away rather than waiting up to a second.
                    s.last_tx_quality = Some(crate::net_dashboard::state::TxQualitySnapshot {
                        papr_db: *papr_db,
                        evm_pct: *evm_pct,
                        evm_carrier: *evm_carrier,
                        dc_offset_i: *dc_offset_i,
                        dc_offset_q: *dc_offset_q,
                        iq_amplitude_imbalance_db: *iq_amplitude_imbalance_db,
                        iq_phase_imbalance_deg: *iq_phase_imbalance_deg,
                        carrier_leakage_db: *carrier_leakage_db,
                        occupied_bandwidth_hz: *occupied_bandwidth_hz,
                    });
                }
                TelemetryEvent::SdrHealth {
                    temperature_c,
                    tx_gains,
                    rx_gains,
                } => {
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
                TelemetryEvent::HealthSnapshot(h) => {
                    // Log only when the overall level changes — not on every periodic sample.
                    if s.last_health.as_ref().map(|p| p.overall) != Some(h.overall) {
                        s.push_log("INFO", format!("Health: overall {}", h.overall.as_str()));
                    }
                    s.last_health = Some(h.clone());
                }
                TelemetryEvent::EmergencyAlarm { source_issi, dest_ssi } => {
                    // ENTER only — re-sends return false and produce no log/broadcast.
                    if s.emergency_enter(*source_issi, *dest_ssi) {
                        s.push_log("WARN", format!("EMERGENCY raised by ISSI {} (dest {})", source_issi, dest_ssi));
                        if let Ok(j) = serde_json::to_string(
                            &serde_json::json!({"type":"emergency_added","issi":source_issi,"dest_ssi":dest_ssi,"started_secs_ago":0}),
                        ) {
                            extra_broadcasts.push(j);
                        }
                    }
                }
                TelemetryEvent::EmergencyCancel { source_issi } => {
                    if s.emergency_clear(*source_issi) {
                        s.push_log("WARN", format!("EMERGENCY cleared for ISSI {}", source_issi));
                        if let Ok(j) = serde_json::to_string(&serde_json::json!({"type":"emergency_removed","issi":source_issi})) {
                            extra_broadcasts.push(j);
                        }
                    }
                }
                TelemetryEvent::DapnetLog {
                    direction,
                    id,
                    callsign,
                    recipient,
                    text,
                    priority,
                    paths,
                } => {
                    s.push_dapnet_log(
                        direction,
                        id.clone(),
                        callsign.clone(),
                        recipient.clone(),
                        text.clone(),
                        *priority,
                        paths.clone(),
                    );
                }
            }
        }
        if let Some(json) = msg {
            self.broadcast(&json);
        }
        for json in extra_broadcasts {
            self.broadcast(&json);
        }
        // TsVoiceActivity: rate-limit broadcasts to max 4/sec per carrier/timeslot (250ms cooldown)
        if let TelemetryEvent::TsVoiceActivity { carrier_num, ts, .. } = &event {
            let now = std::time::Instant::now();
            if let Ok(mut arr) = self.ts_last_broadcast.try_lock() {
                let key = (*carrier_num, *ts);
                let last = arr.entry(key).or_insert(now - std::time::Duration::from_secs(1));
                if now.duration_since(*last) >= std::time::Duration::from_millis(250) {
                    *last = now;
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
        TelemetryEvent::MsTimeoutDrop { issi } => serde_json::json!({"type":"ms_deregistered","issi":issi,"reason":"t351"}),
        TelemetryEvent::MsGroupAttach { issi, gssis } => serde_json::json!({"type":"ms_groups","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupDetach { issi, gssis } => serde_json::json!({"type":"ms_groups_detach","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupsSnapshot { issi, gssis } => serde_json::json!({"type":"ms_groups_all","issi":issi,"groups":gssis}),
        TelemetryEvent::MsRssi { issi, rssi_dbfs } => serde_json::json!({"type":"ms_rssi","issi":issi,"rssi_dbfs":rssi_dbfs}),
        TelemetryEvent::MsEnergySaving { issi, mode } => serde_json::json!({"type":"ms_energy_saving","issi":issi,"mode":mode}),
        TelemetryEvent::GroupCallStarted {
            call_id,
            gssi,
            caller_issi,
            carrier_num,
            ts,
            priority,
        } => {
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"group","gssi":gssi,"caller_issi":caller_issi,"carrier_num":carrier_num,"ts":ts,"priority":priority,"last_heard":{"issi":caller_issi,"activity":"call_group","dest":gssi}})
        }
        TelemetryEvent::GroupCallEnded { call_id, gssi: _ } => serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::CallSpeakerChanged { .. } => return None,
        TelemetryEvent::IndividualCallStarted {
            call_id,
            calling_issi,
            called_issi,
            simplex,
            carrier_num,
            ts,
            peer_carrier_num,
            peer_ts,
            priority,
        } => {
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"individual","caller_issi":calling_issi,"called_issi":called_issi,"simplex":simplex,"carrier_num":carrier_num,"ts":ts,"peer_carrier_num":peer_carrier_num,"peer_ts":peer_ts,"priority":priority,"last_heard":{"issi":calling_issi,"activity":"call_individual","dest":called_issi}})
        }
        TelemetryEvent::IndividualCallEnded { call_id } => serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::BrewConnected { connected, server_version } => {
            serde_json::json!({"type":"brew_status","connected":connected,"brew_version":server_version})
        }
        TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
            serde_json::json!({"type":"last_heard","issi":source_issi,"activity":"sds","dest":dest_issi})
        }
        TelemetryEvent::SdsLog {
            direction,
            source_issi,
            dest_issi,
            is_group,
            protocol_id,
            text,
        } => {
            serde_json::json!({"type":"sds_log","direction":direction,"source_issi":source_issi,"dest_issi":dest_issi,"is_group":is_group,"protocol_id":protocol_id,"text":text})
        }
        TelemetryEvent::TsVoiceActivity {
            carrier_num,
            ts,
            speaker_issi,
        } => serde_json::json!({"type":"ts_voice","carrier_num":carrier_num,"ts":ts,"speaker_issi":speaker_issi}),
        TelemetryEvent::TxVisual {
            sample_rate,
            center_freq_hz,
            carriers,
            constellation_carrier,
            rms_dbfs,
            peak_dbfs,
            spectrum_db_tenths,
            constellation_iq,
        } => serde_json::json!({
            "type": "tx_visual",
            "sample_rate": sample_rate,
            "center_freq_hz": center_freq_hz,
            "carriers": carriers,
            "constellation_carrier": constellation_carrier,
            "rms_dbfs": rms_dbfs,
            "peak_dbfs": peak_dbfs,
            "spectrum_db_tenths": spectrum_db_tenths,
            "constellation_iq": constellation_iq,
        }),
        TelemetryEvent::TxQuality {
            papr_db,
            evm_pct,
            evm_carrier,
            dc_offset_i,
            dc_offset_q,
            iq_amplitude_imbalance_db,
            iq_phase_imbalance_deg,
            carrier_leakage_db,
            occupied_bandwidth_hz,
        } => serde_json::json!({
            "type": "tx_quality",
            "papr_db": papr_db,
            "evm_pct": evm_pct,
            "evm_carrier": evm_carrier,
            "dc_offset_i": dc_offset_i,
            "dc_offset_q": dc_offset_q,
            "iq_amplitude_imbalance_db": iq_amplitude_imbalance_db,
            "iq_phase_imbalance_deg": iq_phase_imbalance_deg,
            "carrier_leakage_db": carrier_leakage_db,
            "occupied_bandwidth_hz": occupied_bandwidth_hz,
        }),
        TelemetryEvent::SdrHealth {
            temperature_c,
            tx_gains,
            rx_gains,
        } => serde_json::json!({
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
        TelemetryEvent::HealthSnapshot(h) => serde_json::json!({
            "type": "health",
            "overall": h.overall,
            "domains": h.domains,
            "last_action": h.last_action,
            "uptime_secs": h.uptime_secs,
        }),
        // Emergency add/remove are broadcast explicitly (transition-gated) from handle_telemetry,
        // so the generic path stays silent — otherwise every periodic re-send would re-broadcast.
        TelemetryEvent::EmergencyAlarm { .. } | TelemetryEvent::EmergencyCancel { .. } => return None,
        TelemetryEvent::DapnetLog {
            direction,
            id,
            callsign,
            recipient,
            text,
            priority,
            paths,
        } => {
            serde_json::json!({"type":"dapnet_log","direction":direction,"id":id,"callsign":callsign,"recipient":recipient,"text":text,"priority":priority,"paths":paths})
        }
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
                let decoded = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).ok()?;
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
    if a.len() != b.len() {
        return false;
    }
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
        body.len(),
        body
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

/// GET /api/sds-log — the persisted SDS Log as a JSON array, newest entry first.
fn serve_sds_log(stream: TcpStream, state: &DashboardState) {
    let body = {
        let s = state.read().unwrap();
        let list: Vec<_> = s.sds_log.iter().rev().cloned().collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    };
    http_json_response(stream, 200, &body);
}

/// Serialize the current live SDS queue to JSON and serve it.
fn serve_live_sds_list(mut stream: TcpStream, cfg: &Option<tetra_config::bluestation::SharedConfig>) {
    let items: Vec<serde_json::Value> = cfg
        .as_ref()
        .map(|c| {
            let state = c.state_read();
            state
                .live_sds_queue
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "text": m.text,
                        "protocol_id": m.protocol_id,
                        "source_issi": m.source_issi,
                        "repeat_count": m.repeat_count,
                        "sent_count": m.sent_count,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
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
    radioid: crate::net_dashboard::radioid::RadioIdCache,
    public_overview: bool,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    // ── Read the first 4KB of headers into a buffer, peek first for routing ──
    // We need to both route on the request line AND read the Authorization header,
    // so we collect all headers before dispatching.
    let mut header_buf = Vec::with_capacity(2048);
    {
        // peek for the request line (already works for routing)
        let mut peek_buf = [0u8; 4096];
        let n = match stream.peek(&mut peek_buf) {
            Ok(n) => n,
            Err(_) => return,
        };
        header_buf.extend_from_slice(&peek_buf[..n]);
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let req_line = header_str.lines().next().unwrap_or("").to_string();

    // Snom/desk-phone ActionURL endpoint. It has its own token and must work without the
    // dashboard cookie session, so handle it before the normal dashboard auth gate.
    if is_tpg2200_action_request(&req_line) {
        drain_http_headers(&mut stream);
        serve_tpg2200_action_url(stream, &req_line, &shared_config, &cmd_tx, &state);
        return;
    }

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
        let is_login_api = req_line.starts_with("POST /api/login ");

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
                if line == "\r\n" || line.is_empty() || line == "\n" {
                    break;
                }
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
            let mut body = vec![0u8; content_length.min(4096)];
            let _ = buf.read_exact(&mut body);
            let body_str = String::from_utf8_lossy(&body);

            let (user, pass) = parse_login_body(&body_str);
            let ok = timing_safe_eq(user.as_bytes(), expected_user.as_bytes()) && timing_safe_eq(pass.as_bytes(), expected_pass.as_bytes());

            if ok {
                let token = if let Ok(mut store) = sessions.lock() {
                    store.create()
                } else {
                    String::new()
                };
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
                if line == "\r\n" || line.is_empty() || line == "\n" {
                    break;
                }
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
                if line == "\r\n" || line.is_empty() || line == "\n" {
                    break;
                }
            }
            let inner = buf.into_inner();
            let is_root = req_line.starts_with("GET / ") || req_line.starts_with("GET /?") || req_line == "GET / HTTP/1.1";

            // Public overview (FH-FEAT-033): when enabled, an anonymous visitor may load the SPA
            // shell and read the narrow public snapshot — nothing else. Every other route (config,
            // controls, /ws, raw telemetry) still falls through to the redirect/401 below, so the
            // admin surface stays fully behind the session wall.
            if public_overview && is_root {
                serve_html(inner);
                return;
            }
            if public_overview
                && (req_line.starts_with("GET /api/public ")
                    || req_line.starts_with("GET /api/public?")
                    || req_line == "GET /api/public HTTP/1.1")
            {
                serve_public_snapshot(inner, &state);
                return;
            }

            // For GET / (the dashboard SPA): redirect to /login so the browser navigates.
            // For API requests: 401 so JS code can detect and refresh.
            if is_root {
                http_redirect(inner, "/login");
            } else {
                http_response(inner, 401, "Unauthorized — please log in");
            }
            return;
        }
    }

    if req_line.contains("/ws") {
        handle_ws(stream, state, clients, cmd_tx, update_state, auth);
    } else if req_line.contains("GET /api/system/brightness") {
        // Backlight status probe (FH-FEAT-008) — lets the UI hide the slider on a panel-less host.
        drain_http_headers(&mut stream);
        let st = crate::backlight::status();
        let body = serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string());
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/system/brightness") {
        // Set backlight brightness (FH-FEAT-008). Body: {"value": 0..=255}.
        let body = read_http_body(&mut stream);
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {e}"));
                return;
            }
        };
        let value = req.get("value").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
        if value > u64::from(crate::backlight::MAX_VALUE) {
            http_response(stream, 400, "value must be 0-255");
            return;
        }
        tracing::info!("Dashboard: set backlight brightness {}", value);
        let body = match crate::backlight::set_brightness(value as u32) {
            Ok(()) => serde_json::json!({ "ok": true }).to_string(),
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }).to_string(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("GET /api/system ") {
        // NOTE: trailing space is load-bearing — without it `contains` would also swallow
        // `GET /api/system/brightness`. The frontend's `fetch('/api/system')` still matches.
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
        let mut body = vec![0u8; content_length.min(512 * 1024)];
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
        // GET /api/configs/<name> — read a specific profile's content
        // GET /api/configs       — list all profiles
        let profile_name: Option<String> = req_line
            .split_whitespace()
            .nth(1)
            .and_then(|path| path.strip_prefix("/api/configs/"))
            .map(|n| n.to_string());
        if let Some(name) = profile_name {
            serve_config_profile_get(buf.into_inner(), &config_path, &name);
        } else {
            serve_config_list(buf.into_inner(), &config_path);
        }
    } else if req_line.contains("POST /api/configs/") {
        // POST /api/configs/<name> — save content to a specific profile (not activate)
        let profile_name: Option<String> = req_line
            .split_whitespace()
            .nth(1)
            .and_then(|path| path.strip_prefix("/api/configs/"))
            .map(|n| n.to_string());
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
        let mut body = vec![0u8; content_length.min(512 * 1024)];
        let _ = buf.read_exact(&mut body);
        match profile_name {
            None => http_response(buf.into_inner(), 400, "missing profile name"),
            Some(name) => match save_config_profile(&config_path, &name, &String::from_utf8_lossy(&body)) {
                Ok(_) => {
                    tracing::info!("Dashboard: saved profile '{}'", name);
                    http_response(buf.into_inner(), 200, "OK")
                }
                Err(e) => http_response(buf.into_inner(), 500, &e),
            },
        }
    } else if req_line.contains("GET /api/callsigns") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_callsigns(buf.into_inner(), &radioid, &req_line);
    } else if req_line.contains("GET /api/update/check") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_update_check(buf.into_inner());
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
        let mut body = vec![0u8; content_length.min(512 * 1024)];
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
    } else if req_line.contains("GET /api/btsinfo") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_bts_info(buf.into_inner(), &shared_config);
    } else if req_line.contains("GET /api/dualcarrier") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dual_carrier_get(s, &shared_config, &config_path);
    } else if req_line.contains("POST /api/dualcarrier") {
        let (inner, body_str) = read_post_body(stream);
        serve_dual_carrier_post(inner, &shared_config, &config_path, &body_str);
    } else if req_line.contains("GET /api/asterisk/status") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_asterisk_status(s, &shared_config);
    } else if req_line.contains("GET /api/snom-notify") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_snom_notify_get(s, &shared_config);
    } else if req_line.contains("POST /api/snom-notify") {
        let (inner, body_str) = read_post_body(stream);
        serve_snom_notify_post(inner, &shared_config, &config_path, &body_str);
    } else if req_line.contains("GET /api/whitelist") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_whitelist_get(buf.into_inner(), &shared_config);
    } else if req_line.contains("POST /api/whitelist") {
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
        let mut body = vec![0u8; content_length.min(512 * 1024)];
        let _ = buf.read_exact(&mut body);
        let body_str = String::from_utf8_lossy(&body);
        serve_whitelist_post(buf.into_inner(), &shared_config, &config_path, body_str.as_ref(), &cmd_tx);
    } else if req_line.contains("GET /api/wx") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_wx_get(buf.into_inner(), &shared_config);
    } else if req_line.contains("POST /api/wx") {
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
        let mut body = vec![0u8; content_length.min(512 * 1024)];
        let _ = buf.read_exact(&mut body);
        let body_str = String::from_utf8_lossy(&body);
        serve_wx_post(buf.into_inner(), &shared_config, &config_path, body_str.as_ref());
    } else if req_line.contains("POST /api/telegram/verify") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_verify(inner, &shared_config, &body_str);
    } else if req_line.contains("POST /api/telegram/detect") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_detect(inner, &shared_config, &body_str);
    } else if req_line.contains("POST /api/telegram/test") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_test(inner, &shared_config, &body_str);
    } else if req_line.contains("GET /api/telegram") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_telegram_get(s, &shared_config);
    } else if req_line.contains("POST /api/telegram") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_post(inner, &shared_config, &config_path, &body_str);
    } else if req_line.contains("DELETE /api/dapnet-log") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dapnet_log_clear(s, &state);
    } else if req_line.contains("GET /api/dapnet-log") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dapnet_log(s, &state);
    } else if req_line.contains("POST /api/dapnet/send") {
        let (inner, body_str) = read_post_body(stream);
        serve_dapnet_send(inner, &shared_config, &state, &clients, &body_str);
    } else if req_line.contains("GET /api/dapnet") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dapnet_get(s, &shared_config);
    } else if req_line.contains("POST /api/dapnet") {
        let (inner, body_str) = read_post_body(stream);
        serve_dapnet_post(inner, &shared_config, &config_path, &body_str);
    } else if req_line.contains("GET /api/geoalarm") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_geoalarm_get(s, &shared_config);
    } else if req_line.contains("POST /api/geoalarm") {
        let (inner, body_str) = read_post_body(stream);
        serve_geoalarm_post(inner, &shared_config, &config_path, &body_str);
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
    } else if req_line.contains("DELETE /api/sds-log") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_sds_log_clear(buf.into_inner(), &state);
    } else if req_line.contains("GET /api/sds-log") {
        // Return the persisted SDS Log (newest first) as JSON.
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_sds_log(buf.into_inner(), &state);
    } else if req_line.contains("GET /api/live-sds") {
        // Return current live SDS queue as JSON.
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_live_sds_list(buf.into_inner(), &shared_config);
    } else if req_line.contains("DELETE /api/live-sds/") {
        // DELETE /api/live-sds/<id>
        let id: u32 = req_line
            .split('/')
            .nth(3)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
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
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
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
                    send_control_cmd(
                        &cmd_tx,
                        ControlCommand::AddLiveSds {
                            text,
                            protocol_id,
                            source_issi,
                            repeat_count,
                        },
                    );
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
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {}", e));
                return;
            }
        };
        let uuid = match req.get("uuid").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => {
                http_response(stream, 400, "missing uuid");
                return;
            }
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
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {}", e));
                return;
            }
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
        }))
        .unwrap_or_default();
        http_json_response(stream, 200, &body);
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
    update_state: SharedUpdateState,
    _auth: Option<(String, String)>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));

    // Note: cookie-based auth is checked by handle_connection BEFORE we get here,
    // so we don't need to re-validate during the WS upgrade. The cookie travels on
    // the Upgrade request and was already verified against the session store.
    let callback = move |_req: &Request, res: Response| -> Result<Response, _> { Ok(res) };

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
        let emergencies = s.snapshot_emergencies();
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
        let last_health = s.last_health.clone();
        drop(s);
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "type": "snapshot", "ms": ms, "calls": calls, "emergencies": emergencies, "log": logs,
            "brew_online": brew_online, "brew_version": brew_version, "last_heard": last_heard,
            "fallback_config_active": fallback_active, "fallback_config_reason": fallback_reason,
            "last_tx_visual": last_tx_visual,
            "last_tx_quality": last_tx_quality,
            "last_sdr_health": last_sdr_health,
            "last_sys_health": last_sys_health,
            "health": last_health,
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
                handle_ws_command(&text, &state, &cmd_tx, &update_state);
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

fn handle_ws_command(text: &str, state: &DashboardState, cmd_tx: &Arc<Mutex<Option<CmdSender>>>, update_state: &SharedUpdateState) {
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

    match v.get("type").and_then(|t| t.as_str()) {
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
                let bytes: Vec<u8> = msg_text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
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
        Some("dgna") => {
            let issi = v.get("issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let gssi = v.get("gssi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let attach = v.get("attach").and_then(|a| a.as_bool()).unwrap_or(true);
            if issi == 0 || gssi == 0 {
                return;
            }
            let verb = if attach { "assign" } else { "deassign" };
            tracing::info!("Dashboard: DGNA {} GSSI {} on ISSI {}", verb, gssi, issi);
            if !send_cmd(ControlCommand::Dgna { issi, gssi, attach }) {
                tracing::warn!("Dashboard: no control dispatcher for DGNA");
            }
            let mut s = state.write().unwrap();
            s.push_log(
                "INFO",
                format!(
                    "DGNA {} requested: GSSI {} {} ISSI {}",
                    verb,
                    gssi,
                    if attach { "to" } else { "from" },
                    issi
                ),
            );
        }
        Some("emergency_clear") => {
            let issi = v.get("issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            if issi == 0 {
                return;
            }
            tracing::info!("Dashboard: operator clearing emergency for ISSI {}", issi);
            // Route to CMCE: it clears the source SDS session (so the emergency does not re-arm on
            // the radio's next status re-send) and emits EmergencyCancel, which clears the banner
            // for EVERY connected client via the telemetry round-trip. We deliberately do NOT mutate
            // dashboard state here — otherwise the round-trip would find nothing to clear and skip
            // the broadcast to other browsers.
            if !send_cmd(ControlCommand::ClearEmergency { issi }) {
                tracing::warn!("Dashboard: no control dispatcher for emergency_clear");
            }
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

/// GET /api/update/check — query GitHub for the latest release and report whether a newer
/// version than the running build exists. Best-effort; on any failure returns
/// check_failed=true so the dashboard simply hides the badge.
/// GET /api/callsigns?ids=1,2,3 — resolve ISSIs to RadioID callsigns ("indicative"). Returns a JSON
/// object `{ "<id>": {"cs":"CALLSIGN","fl":"🇷🇴"} }` for resolved IDs (`fl` is the country flag emoji
/// derived from the call-sign prefix, or empty if unknown) and `{ "<id>": "" }` for IDs confirmed
/// absent from RadioID. IDs still being fetched in the background are OMITTED, so the client retries
/// them on a later poll. Lookups are non-blocking — unknown IDs are queued for background resolution.
fn serve_callsigns(stream: TcpStream, radioid: &crate::net_dashboard::radioid::RadioIdCache, req_line: &str) {
    use crate::net_dashboard::radioid::Lookup;
    // Parse the `ids=` query parameter from "GET /api/callsigns?ids=1,2,3 HTTP/1.1".
    let ids: Vec<u32> = req_line
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.split('?').nth(1))
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|kv| kv.strip_prefix("ids="))
        .map(|v| {
            v.split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .take(256) // bound work per request
                .collect()
        })
        .unwrap_or_default();

    let mut map = serde_json::Map::new();
    for id in ids {
        match radioid.get(id) {
            Lookup::Found(cs) => {
                let flag = crate::net_dashboard::callsign::callsign_flag(&cs).unwrap_or_default();
                let mut entry = serde_json::Map::new();
                entry.insert("cs".to_string(), serde_json::Value::String(cs));
                entry.insert("fl".to_string(), serde_json::Value::String(flag));
                map.insert(id.to_string(), serde_json::Value::Object(entry));
            }
            Lookup::NotFound => {
                map.insert(id.to_string(), serde_json::Value::String(String::new()));
            }
            Lookup::Pending => {} // omit — client retries on a later poll
        }
    }
    http_json_response(stream, 200, &serde_json::Value::Object(map).to_string());
}

fn serve_update_check(mut stream: TcpStream) {
    let result = crate::net_dashboard::update_check::check_for_update(tetra_core::STACK_VERSION);
    let body = result.to_json();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// GET /api/whitelist — return the effective whitelist as JSON:
/// `{"issi_whitelist":[...], "source":"override"|"config", "enabled":bool}`.
/// `enabled` is false when the list is empty (open network).
/// GET /api/btsinfo — static cell + RF identity pulled from the running config, for the
/// "TETRA BTS Details" card on the dashboard. Read-only; non-sensitive scalars only.
fn serve_bts_info(mut stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let body = match shared_config {
        Some(cfg) => {
            // Whitelist status (runtime override beats config) — mirrors serve_whitelist_get.
            let wl = match cfg.state_read().issi_whitelist_override.clone() {
                Some(l) => l,
                None => cfg.config().security.issi_whitelist.clone(),
            };
            let restricted = !wl.is_empty();
            let wl_count = wl.len();

            let c = cfg.config();
            let soapy = c.phy_io.soapysdr.as_ref();
            let tx = soapy.map(|s| s.dl_freq); // downlink = BS transmit
            let rx = soapy.map(|s| s.ul_freq); // uplink   = BS receive
            let carriers = c
                .bs_phase_mod_carriers()
                .unwrap_or_default()
                .into_iter()
                .map(|(carrier_num, dl_freq_hz, ul_freq_hz)| {
                    serde_json::json!({
                        "carrier_num": carrier_num,
                        "tx_freq_hz": dl_freq_hz,
                        "rx_freq_hz": ul_freq_hz,
                    })
                })
                .collect::<Vec<_>>();
            // Duplex shift expressed relative to TX (offset to add to TX to reach RX).
            let shift = match (tx, rx) {
                (Some(t), Some(r)) => Some(r - t),
                _ => None,
            };

            serde_json::json!({
                "tx_freq_hz": tx,
                "rx_freq_hz": rx,
                "shift_hz": shift,
                "carriers": carriers,
                "mcc": c.net.mcc,
                "mnc": c.net.mnc,
                "main_carrier": c.cell.main_carrier,
                "neighbor_count": c.cell.neighbor_cells_ca.len(),
                "hangtime_secs": c.cell.hangtime_secs,
                "whitelist_restricted": restricted,
                "whitelist_count": wl_count,
            })
            .to_string()
        }
        None => "{}".to_string(),
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// GET /api/dualcarrier — current Dual-Carrier ON/OFF state for the first-page toggle.
/// Reads the switch + configured secondary carrier from the TOML (so the number is shown even while
/// off), plus the running effective state and the main carrier.
fn serve_dual_carrier_get(
    mut stream: TcpStream,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
) {
    let st = crate::net_dashboard::dual_carrier::read_dual_carrier(config_path);
    let main_carrier = shared_config.as_ref().map(|c| c.config().cell.main_carrier);
    // What the running stack is actually doing right now (may lag the file until the restart lands).
    let running_active = shared_config
        .as_ref()
        .map(|c| c.config().cell.secondary_carrier.is_some())
        .unwrap_or(false);

    let body = serde_json::json!({
        "enabled": st.enabled,
        "secondary_carrier": st.secondary_carrier,
        "active": st.active(),
        "running_active": running_active,
        "main_carrier": main_carrier,
    })
    .to_string();

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// POST /api/dualcarrier — toggle dual carrier. Body: {"enabled": bool, "secondary_carrier"?: u16}.
///
/// The secondary carrier cannot be reconfigured live, so this validates the resulting config (so we
/// never restart into something the BS would reject and loop on), writes the TOML, then schedules a
/// controlled service restart to apply the new carrier set.
fn serve_dual_carrier_post(
    stream: TcpStream,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    body: &str,
) {
    use crate::net_dashboard::dual_carrier;

    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return http_response(stream, 400, &format!("invalid JSON: {e}")),
    };
    let Some(enabled) = req.get("enabled").and_then(|v| v.as_bool()) else {
        return http_response(stream, 400, "missing boolean field 'enabled'");
    };

    let current = dual_carrier::read_dual_carrier(config_path);

    // When enabling, resolve which carrier to use: explicit from the request, else the one already
    // configured. Disabling keeps the configured number untouched (so it is remembered).
    let secondary = if enabled {
        // Validate the RAW value before the u16 cast: a carrier number is a 12-bit field (0..4095)
        // and the FreqInfo encoder only accepts < 4000, so reject out-of-range here rather than let
        // `as u16` silently truncate a huge value into a valid-looking carrier (e.g. 67057 -> 1521).
        let requested = match req.get("secondary_carrier").and_then(|v| v.as_u64()) {
            Some(n) if n >= 4000 => {
                return http_response(stream, 400, "secondary_carrier must be in 0..3999");
            }
            Some(n) => Some(n as u16),
            None => None,
        };
        match requested.or(current.secondary_carrier) {
            Some(n) => Some(n),
            None => {
                return http_response(
                    stream,
                    400,
                    "enabling dual carrier needs a secondary_carrier number (none configured yet)",
                );
            }
        }
    } else {
        None
    };

    // Dry-run the prospective config so a bad carrier (e.g. outside the SDR passband, or equal to the
    // main carrier) is rejected here instead of crash-looping the service after the restart.
    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => return http_response(stream, 500, &format!("cannot read config: {e}")),
    };
    let prospective = dual_carrier::compute_toml(&original, enabled, secondary);
    match tetra_config::bluestation::parsing::from_toml_str(&prospective) {
        Ok(cfg) => {
            if let Err(e) = cfg.validate() {
                return http_response(stream, 400, &format!("resulting config is invalid: {e}"));
            }
        }
        Err(e) => return http_response(stream, 400, &format!("resulting config does not parse: {e}")),
    }

    if let Err(e) = dual_carrier::write_dual_carrier(config_path, enabled, secondary) {
        return http_response(stream, 500, &format!("failed to write config: {e}"));
    }

    let _ = shared_config; // the new carrier set is picked up by the restart, not mutated live.

    tracing::info!(
        "Dashboard: Dual-Carrier set {} (secondary_carrier={:?}); scheduling restart",
        if enabled { "ON" } else { "OFF" },
        secondary
    );
    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::Restart,
        std::time::Duration::from_secs(2),
    );

    http_response(
        stream,
        200,
        if enabled {
            "Dual carrier enabled; the base station is restarting to apply it."
        } else {
            "Dual carrier disabled; the base station is restarting to apply it."
        },
    );
}

fn serve_whitelist_get(mut stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let (list, source): (Vec<u32>, &str) = match shared_config {
        Some(cfg) => {
            let override_list = cfg.state_read().issi_whitelist_override.clone();
            match override_list {
                Some(l) => (l, "override"),
                None => (cfg.config().security.issi_whitelist.clone(), "config"),
            }
        }
        None => (Vec::new(), "config"),
    };
    let items: Vec<String> = list.iter().map(|n| n.to_string()).collect();
    let body = format!(
        "{{\"issi_whitelist\":[{}],\"source\":\"{}\",\"enabled\":{}}}",
        items.join(","),
        source,
        !list.is_empty()
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// POST /api/whitelist — set the whitelist. Body: JSON array `[1,2,3]` or
/// `{"issi_whitelist":[1,2,3]}`. Applies immediately via the StackState override AND
/// rewrites the TOML so it survives a restart. An empty list = open network.
fn serve_whitelist_post(
    stream: TcpStream,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    body: &str,
    cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
) {
    use crate::net_dashboard::whitelist;

    let list = match whitelist::parse_whitelist_body(body) {
        Ok(l) => l,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid whitelist: {e}"));
            return;
        }
    };

    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    // 1) Apply at runtime immediately so the next registration sees it.
    {
        let mut state = cfg.state_write();
        state.issi_whitelist_override = Some(list.clone());
    }

    // 2) Enforce immediately on terminals that are ALREADY registered. The whitelist is
    //    only checked at registration time, so without this an enabling edit would leave
    //    disallowed radios connected (looks like access control never turned on) and a
    //    removal would only take effect when the terminal next re-registers — i.e. on a
    //    reboot. Kick every currently-registered ISSI the new list no longer allows; it
    //    re-registers and is then rejected by MM. Empty list = open network = kick nobody.
    if !list.is_empty() {
        let to_kick: Vec<u32> = {
            let state = cfg.state_read();
            state
                .subscribers
                .all_registered_issis()
                .filter(|issi| !list.contains(issi))
                .collect()
        };
        for issi in to_kick {
            tracing::info!("Dashboard: whitelist change — kicking non-whitelisted ISSI {}", issi);
            send_control_cmd(cmd_tx, ControlCommand::KickMs { issi });
        }
    }

    // 3) Persist to TOML so it survives a restart.
    if let Err(e) = whitelist::write_whitelist_to_toml(config_path, &list) {
        tracing::warn!("Dashboard: whitelist applied at runtime but failed to persist to TOML: {}", e);
        // Runtime change still took effect; report partial success so the operator knows
        // to check file permissions.
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!("Dashboard: ISSI whitelist updated ({} entries)", list.len());
    http_response(stream, 200, "OK");
}

// ---------------------------------------------------------------------------
// WX/METAR service config (dashboard-editable). See net_dashboard::wx_service.
// ---------------------------------------------------------------------------

/// GET /api/wx — return the effective WX service settings as JSON.
fn serve_wx_get(mut stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let wx = match shared_config {
        Some(cfg) => cfg.effective_wx_service(),
        None => tetra_config::bluestation::CfgWxService::default(),
    };
    let body = format!(
        "{{\"enabled\":{},\"service_issi\":{},\"periodic_enabled\":{},\"periodic_issi\":{},\"periodic_is_group\":{},\"periodic_icao\":\"{}\",\"periodic_interval_secs\":{}}}",
        wx.enabled,
        wx.service_issi,
        wx.periodic_enabled,
        wx.periodic_issi,
        wx.periodic_is_group,
        wx.periodic_icao.replace('\\', "\\\\").replace('"', "\\\""),
        wx.periodic_interval_secs
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// POST /api/wx — update WX service settings. Body: JSON object with the same fields as
/// GET. Applies immediately via the StackState override AND rewrites the TOML.
fn serve_wx_post(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
    use tetra_config::bluestation::WxRuntimeOverride;

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };

    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    // Start from the current effective values so a partial POST only changes what it sends.
    let cur = cfg.effective_wx_service();
    let as_u32 = |v: &serde_json::Value, k: &str, d: u32| v.get(k).and_then(|x| x.as_u64()).map(|n| n as u32).unwrap_or(d);
    let as_u64 = |v: &serde_json::Value, k: &str, d: u64| v.get(k).and_then(|x| x.as_u64()).unwrap_or(d);
    let as_bool = |v: &serde_json::Value, k: &str, d: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(d);
    let icao = json
        .get("periodic_icao")
        .and_then(|x| x.as_str())
        .map(|s| {
            s.trim()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(4)
                .collect::<String>()
                .to_uppercase()
        })
        .unwrap_or(cur.periodic_icao.clone());

    let ov = WxRuntimeOverride {
        enabled: as_bool(&json, "enabled", cur.enabled),
        service_issi: as_u32(&json, "service_issi", cur.service_issi),
        periodic_enabled: as_bool(&json, "periodic_enabled", cur.periodic_enabled),
        periodic_issi: as_u32(&json, "periodic_issi", cur.periodic_issi),
        periodic_is_group: as_bool(&json, "periodic_is_group", cur.periodic_is_group),
        periodic_icao: icao,
        periodic_interval_secs: as_u64(&json, "periodic_interval_secs", cur.periodic_interval_secs),
    };

    // 1) Apply at runtime.
    {
        let mut state = cfg.state_write();
        state.wx_override = Some(ov.clone());
    }

    // 2) Persist to TOML.
    if let Err(e) = crate::net_dashboard::wx_service::write_wx_to_toml(config_path, &ov) {
        tracing::warn!("Dashboard: WX applied at runtime but failed to persist to TOML: {}", e);
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!(
        "Dashboard: WX service updated (enabled={} svc_issi={} periodic={} -> {} icao={})",
        ov.enabled,
        ov.service_issi,
        ov.periodic_enabled,
        ov.periodic_issi,
        ov.periodic_icao
    );
    http_response(stream, 200, "OK");
}

/// Read an HTTP POST body off `stream`, returning the stream plus the body as a UTF-8 string.
fn read_post_body(mut stream: TcpStream) -> (TcpStream, String) {
    let body = read_http_body(&mut stream);
    let s = String::from_utf8_lossy(&body).into_owned();
    (stream, s)
}

/// Resolve the bot token to use for a verify/detect/test/save request: a freshly-typed token from
/// the body (never the masked placeholder, which contains '…'), else the currently-saved one.
fn telegram_resolve_token(json: &serde_json::Value, shared_config: &Option<tetra_config::bluestation::SharedConfig>) -> String {
    if let Some(t) = json.get("bot_token").and_then(|v| v.as_str()) {
        let t = t.trim();
        if !t.is_empty() && !t.contains('…') {
            return t.to_string();
        }
    }
    match shared_config {
        Some(cfg) => cfg.effective_telegram().bot_token.as_ref().to_string(),
        None => String::new(),
    }
}

/// Whether a bot token is safe to store. Empty is allowed (not yet configured). A real Telegram
/// token is `<bot-id>:<auth>` with no whitespace or control characters — rejecting anything else
/// keeps the token safe inside the config TOML (a stray newline would corrupt the file) and inside
/// the API URL path.
fn telegram_token_acceptable(t: &str) -> bool {
    t.is_empty() || (t.contains(':') && t.chars().all(|c| !c.is_whitespace() && !c.is_control()))
}

/// GET /api/telegram — return the effective Telegram settings as JSON. The token is masked and is
/// never echoed in the clear; `token_set` tells the UI whether one is stored.
fn serve_telegram_get(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let tg = match shared_config {
        Some(cfg) => cfg.effective_telegram(),
        None => tetra_config::bluestation::CfgTelegram::default(),
    };
    let masked = crate::net_dashboard::telegram::mask_token(tg.bot_token.as_ref());
    let token_set = !tg.bot_token.as_ref().trim().is_empty();
    let chat_ids = tg.chat_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let body = format!(
        "{{\"enabled\":{},\"bot_token_masked\":\"{}\",\"token_set\":{},\"chat_ids\":[{}],\"alert_connect\":{},\"alert_disconnect\":{},\"alert_t351\":{},\"alert_lip\":{},\"alert_backhaul\":{},\"alert_critical_logs\":{}}}",
        tg.enabled,
        crate::net_dashboard::telegram::json_escape(&masked),
        token_set,
        chat_ids,
        tg.alert_connect,
        tg.alert_disconnect,
        tg.alert_t351,
        tg.alert_lip,
        tg.alert_backhaul,
        tg.alert_critical_logs,
    );
    http_json_response(stream, 200, &body);
}

/// POST /api/telegram — save Telegram settings. Applies immediately via the StackState override
/// AND rewrites the TOML. The token is only changed when a fresh (non-masked) one is supplied.
fn serve_telegram_post(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
    use tetra_config::bluestation::TelegramRuntimeOverride;

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };
    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    let cur = cfg.effective_telegram();
    let as_bool = |k: &str, d: bool| json.get(k).and_then(|x| x.as_bool()).unwrap_or(d);

    let bot_token = telegram_resolve_token(&json, shared_config);
    if !telegram_token_acceptable(&bot_token) {
        http_response(stream, 400, "Invalid token: no spaces or control characters, and must contain ':'.");
        return;
    }
    let chat_ids = match json.get("chat_ids").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().filter_map(|v| v.as_i64()).collect::<Vec<i64>>(),
        None => cur.chat_ids.clone(),
    };

    let ov = TelegramRuntimeOverride {
        enabled: as_bool("enabled", cur.enabled),
        bot_token,
        chat_ids,
        alert_connect: as_bool("alert_connect", cur.alert_connect),
        alert_disconnect: as_bool("alert_disconnect", cur.alert_disconnect),
        alert_t351: as_bool("alert_t351", cur.alert_t351),
        alert_lip: as_bool("alert_lip", cur.alert_lip),
        alert_backhaul: as_bool("alert_backhaul", cur.alert_backhaul),
        alert_critical_logs: as_bool("alert_critical_logs", cur.alert_critical_logs),
    };

    {
        let mut state = cfg.state_write();
        state.telegram_override = Some(ov.clone());
    }

    if let Err(e) = crate::net_dashboard::telegram::write_telegram_to_toml(config_path, &ov) {
        tracing::warn!("Dashboard: Telegram applied at runtime but failed to persist to TOML: {}", e);
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!(
        "Dashboard: Telegram alerts updated (enabled={} chats={})",
        ov.enabled,
        ov.chat_ids.len()
    );
    http_response(stream, 200, "OK");
}

/// POST /api/telegram/verify — validate the token via getMe and return the bot @username.
fn serve_telegram_verify(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, body: &str) {
    let json: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null);
    let token = telegram_resolve_token(&json, shared_config);
    if token.is_empty() {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Niciun token setat\"}");
        return;
    }
    let client = crate::net_telegram::TelegramClient::new();
    match client.get_me(&token) {
        Ok(info) => {
            let body = format!(
                "{{\"ok\":true,\"username\":\"{}\"}}",
                crate::net_dashboard::telegram::json_escape(&info.username)
            );
            http_json_response(stream, 200, &body);
        }
        Err(e) => {
            let body = format!("{{\"ok\":false,\"error\":\"{}\"}}", crate::net_dashboard::telegram::json_escape(&e));
            http_json_response(stream, 200, &body);
        }
    }
}

/// POST /api/telegram/detect — return the chats that recently messaged the bot (getUpdates).
fn serve_telegram_detect(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, body: &str) {
    let json: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null);
    let token = telegram_resolve_token(&json, shared_config);
    if token.is_empty() {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Niciun token setat\"}");
        return;
    }
    let client = crate::net_telegram::TelegramClient::new();
    match client.get_updates(&token) {
        Ok(chats) => {
            let items = chats
                .iter()
                .map(|c| {
                    format!(
                        "{{\"id\":{},\"name\":\"{}\",\"kind\":\"{}\"}}",
                        c.id,
                        crate::net_dashboard::telegram::json_escape(&c.name),
                        crate::net_dashboard::telegram::json_escape(&c.kind)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let body = format!("{{\"ok\":true,\"chats\":[{}]}}", items);
            http_json_response(stream, 200, &body);
        }
        Err(e) => {
            let body = format!("{{\"ok\":false,\"error\":\"{}\"}}", crate::net_dashboard::telegram::json_escape(&e));
            http_json_response(stream, 200, &body);
        }
    }
}

/// POST /api/telegram/test — send a test alert to the configured (or body-supplied) chats.
fn serve_telegram_test(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, body: &str) {
    let json: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null);
    let Some(cfg) = shared_config else {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Config indisponibil\"}");
        return;
    };
    let token = telegram_resolve_token(&json, shared_config);
    let tg = cfg.effective_telegram();
    let chat_ids: Vec<i64> = match json.get("chat_ids").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().filter_map(|v| v.as_i64()).collect(),
        None => tg.chat_ids.clone(),
    };
    if token.is_empty() {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Niciun token setat\"}");
        return;
    }
    if chat_ids.is_empty() {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Niciun Chat ID setat\"}");
        return;
    }
    let station = crate::net_telegram::format::StationInfo::from_config(cfg);
    let html = crate::net_telegram::format::test_message(&station);
    let client = crate::net_telegram::TelegramClient::new();
    let mut sent = 0u32;
    let mut errors: Vec<String> = Vec::new();
    for id in &chat_ids {
        match client.send_message_html(&token, *id, &html) {
            Ok(_) => sent += 1,
            Err(e) => errors.push(format!("{id}: {e}")),
        }
    }
    let ok = errors.is_empty();
    let body = format!(
        "{{\"ok\":{},\"sent\":{},\"error\":\"{}\"}}",
        ok,
        sent,
        crate::net_dashboard::telegram::json_escape(&errors.join("; "))
    );
    http_json_response(stream, 200, &body);
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

    // CPU model — /proc/cpuinfo "model name" (x86) or "Model" (ARM/Pi)
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.to_lowercase().starts_with("model name") || l.to_lowercase().starts_with("hardware"))
                .and_then(|l| l.splitn(2, ':').nth(1).map(|v| v.trim().to_string()))
        })
        .unwrap_or_else(|| "unknown".to_string());

    // CPU core count
    let cpu_cores = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .map(|s| s.lines().filter(|l| l.starts_with("processor")).count())
        .unwrap_or(0);

    // CPU load — /proc/stat first line: user nice system idle iowait irq softirq
    // Take a 100ms sample for a meaningful reading
    fn read_cpu_stat() -> Option<(u64, u64)> {
        let s = std::fs::read_to_string("/proc/stat").ok()?;
        let line = s.lines().next()?;
        let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
        if nums.len() < 4 {
            return None;
        }
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0); // idle + iowait
        let total: u64 = nums.iter().sum();
        Some((total, idle))
    }
    let cpu_pct = if let (Some((t1, i1)), Some((t2, i2))) = (read_cpu_stat(), {
        std::thread::sleep(std::time::Duration::from_millis(100));
        read_cpu_stat()
    }) {
        let dt = t2.saturating_sub(t1);
        let di = i2.saturating_sub(i1);
        if dt > 0 { ((dt - di) * 100 / dt) as u8 } else { 0 }
    } else {
        0
    };

    // RAM — /proc/meminfo MemTotal and MemAvailable
    let (ram_total_mb, ram_used_mb) = std::fs::read_to_string("/proc/meminfo")
        .ok()
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
    ]
    .iter()
    .find_map(|path| {
        std::fs::read_to_string(path)
            .ok()
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
            let probe = std::process::Command::new(bin).arg("--info").output();
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
                    let info_summary: String = info
                        .lines()
                        .filter(|l| {
                            let ll = l.to_lowercase();
                            ll.contains("lib version")
                                || ll.contains("api version")
                                || ll.contains("abi version")
                                || ll.contains("install root")
                        })
                        .take(4)
                        .collect::<Vec<&str>>()
                        .join("\n");
                    return match find {
                        Some(text) if text.lines().any(|l| l.to_lowercase().contains("found device")) => {
                            // Keep only the useful per-device lines (driver/serial/label) to
                            // avoid dumping pages of advertising.
                            let lines: Vec<&str> = text
                                .lines()
                                .filter(|l| {
                                    let ll = l.to_lowercase();
                                    ll.contains("found device")
                                        || ll.contains("driver")
                                        || ll.contains("serial")
                                        || ll.contains("label")
                                        || ll.contains("name")
                                        || ll.contains("manufacturer")
                                })
                                .take(20)
                                .collect();
                            format!("{}\n{}", info_summary, lines.join("\n"))
                        }
                        Some(_) => format!("{}\nNo SDR device detected.", info_summary),
                        None => format!("{}\nSoapySDRUtil --find failed.", info_summary),
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
    let sdr_name = crate::phy::components::soapy_settings::detected_sdr_name().unwrap_or_else(|| "unknown".to_string());

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
    }))
    .unwrap_or_else(|_| "{}".to_string());

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

/// Read a specific config profile and serve its content as plain text.
fn serve_config_profile_get(stream: TcpStream, config_path: &str, profile_name: &str) {
    if profile_name.contains('/') || profile_name.contains('\\') || profile_name.contains("..") {
        return http_response(stream, 400, "invalid profile name");
    }
    if !profile_name.ends_with(".toml") {
        return http_response(stream, 400, "profile must be a .toml file");
    }
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
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
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(profile_name);

    // Refuse to overwrite the active config through this endpoint
    let active_name = std::path::Path::new(config_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if profile_name == active_name {
        return Err("cannot overwrite active config via profile editor — use the Config editor tab".to_string());
    }

    std::fs::write(&profile_path, content.as_bytes()).map_err(|e| format!("failed to write profile: {}", e))
}

/// GET /api/public — anonymous read-only overview (FH-FEAT-033). Projects ONLY non-sensitive,
/// already-public scalars from the dashboard's own state — never SharedConfig/StackState, and never
/// ISSIs/GSSIs, the whitelist, SDS contents or the log ring. The read lock is the dashboard's own
/// RwLock (the same one the WS snapshot takes), held only long enough to copy a handful of counts.
fn serve_public_snapshot(stream: TcpStream, state: &DashboardState) {
    let body = match state.read() {
        Ok(s) => {
            let active_calls = s.calls.len();
            let group_calls = s.calls.values().filter(|c| c.is_group).count();
            let individual_calls = active_calls - group_calls;
            let center_freq_hz = s.last_tx_visual.as_ref().map(|v| v.center_freq_hz);
            serde_json::json!({
                "registered_ms": s.ms_map.len(),
                "active_calls": active_calls,
                "group_calls": group_calls,
                "individual_calls": individual_calls,
                "center_freq_hz": center_freq_hz,
                "rf_active": s.last_tx_visual.is_some(),
                "brew_online": s.brew_online,
                "stack_version": tetra_core::STACK_VERSION,
            })
            .to_string()
        }
        Err(_) => "{}".to_string(),
    };
    http_json_response(stream, 200, &body);
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

/// Like `http_response` but serves JSON. Used by the WiFi management endpoints
/// which all return structured `{"ok": ..., ...}` payloads.
fn http_json_response(mut stream: TcpStream, code: u16, body: &str) {
    let status = if code == 200 { "OK" } else { "Error" };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code,
        status,
        body.len(),
        body
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
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        // Detect "\r\n\r\n" by sliding a 4-byte window.
        if prev3 == [b'\r', b'\n', b'\r'] && byte[0] == b'\n' {
            break;
        }
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
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            return Vec::new();
        }
        header_buf.push(byte[0]);
        if prev3 == [b'\r', b'\n', b'\r'] && byte[0] == b'\n' {
            break;
        }
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
    let mut body = vec![0u8; content_length.min(512 * 1024)];
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
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
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
        body.len(),
        token,
        body
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

// ===========================================================================
// Integration dashboard endpoints — DAPNET / GeoAlarm / Snom NOTIFY / Asterisk
// plus the TPG2200 ActionURL and DAPNET/SDS log helpers. Ported from the dj2th
// fork (echolink/meshcom routes intentionally excluded).
// ===========================================================================

/// DELETE /api/sds-log — clear the persisted SDS Log.
fn serve_sds_log_clear(stream: TcpStream, state: &DashboardState) {
    if let Ok(mut s) = state.write() {
        s.clear_sds_log();
    }
    http_json_response(stream, 200, "{\"ok\":true}");
}

/// GET /api/dapnet-log — the persisted DAPNET Log as a JSON array, newest entry first.
fn serve_dapnet_log(stream: TcpStream, state: &DashboardState) {
    let body = {
        match state.read() {
            Ok(s) => {
                let list: Vec<_> = s.dapnet_log.iter().rev().cloned().collect();
                serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
            }
            Err(_) => "[]".to_string(),
        }
    };
    http_json_response(stream, 200, &body);
}

/// DELETE /api/dapnet-log — clear the persisted DAPNET Log.
fn serve_dapnet_log_clear(stream: TcpStream, state: &DashboardState) {
    if let Ok(mut s) = state.write() {
        s.clear_dapnet_log();
    }
    http_json_response(stream, 200, "{\"ok\":true}");
}

fn request_path(req_line: &str) -> Option<&str> {
    req_line.split_whitespace().nth(1)
}

fn is_tpg2200_action_request(req_line: &str) -> bool {
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let route = path.split_once('?').map(|(route, _)| route).unwrap_or(path);
    matches!(method, "GET" | "POST") && route == "/api/action/tpg2200"
}

fn query_params(path: &str) -> HashMap<String, String> {
    let Some((_, query)) = path.split_once('?') else {
        return HashMap::new();
    };
    query
        .split('&')
        .filter_map(|part| {
            if part.is_empty() {
                return None;
            }
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((url_decode(key), url_decode(value)))
        })
        .collect()
}

fn truncate_action_text(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((idx, _)) => (text[..idx].to_string(), true),
        None => (text.to_string(), false),
    }
}

fn next_tpg2200_action_incident(cfg: &tetra_config::bluestation::SharedConfig, base: u16) -> u16 {
    let base = base.clamp(1, 256);
    let mut state = cfg.state_write();
    let incident = state.tpg2200_action_next_incident.unwrap_or(base).clamp(1, 256);
    state.tpg2200_action_next_incident = Some(if incident >= 256 { 1 } else { incident + 1 });
    incident
}

/// GET /api/action/tpg2200?token=...&text=...
///
/// Public-by-design ActionURL endpoint for phones that cannot hold the dashboard session cookie.
/// The dedicated token is mandatory and configured in `[tpg2200_action]`.
fn serve_tpg2200_action_url(
    stream: TcpStream,
    req_line: &str,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
    state: &DashboardState,
) {
    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };
    let action = cfg.config().tpg2200_action.clone();
    if !action.enabled {
        http_response(stream, 404, "TPG2200 ActionURL disabled");
        return;
    }
    let Some(path) = request_path(req_line) else {
        http_response(stream, 400, "Invalid request");
        return;
    };
    let params = query_params(path);
    let supplied_token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let expected_token = action.token.as_ref();
    if expected_token.trim().is_empty() || !timing_safe_eq(supplied_token.as_bytes(), expected_token.as_bytes()) {
        tracing::warn!("TPG2200 ActionURL rejected: invalid token");
        http_response(stream, 403, "Forbidden");
        return;
    }
    if action.dest_issi == 0 || action.source_issi == 0 {
        http_response(stream, 500, "TPG2200 ActionURL not fully configured");
        return;
    }

    let requested_text = params
        .get("text")
        .or_else(|| params.get("message"))
        .or_else(|| params.get("msg"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(action.default_text.trim());
    let message = if requested_text.is_empty() { "ALARM" } else { requested_text };
    let (message, truncated) = truncate_action_text(message, action.max_text_chars.max(1));
    if truncated {
        tracing::warn!("TPG2200 ActionURL text truncated to {} chars", action.max_text_chars);
    }

    let tx = match cmd_tx.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(tx) = tx else {
        http_response(stream, 503, "CMCE control channel unavailable");
        return;
    };

    let incident = next_tpg2200_action_incident(cfg, action.incident_base);
    let payload = build_tpg2200_callout_payload(incident, &message);
    if payload.len() > (u16::MAX as usize / 8) {
        http_response(stream, 500, "TPG2200 payload too large");
        return;
    }
    let len_bits = (payload.len() * 8) as u16;
    let cmd = ControlCommand::SendRawSdsType4 {
        handle: 0,
        source_ssi: action.source_issi,
        dest_ssi: action.dest_issi,
        dest_is_group: false,
        len_bits,
        payload,
    };
    if tx.send(cmd).is_err() {
        http_response(stream, 503, "CMCE control channel unavailable");
        return;
    }

    tracing::info!(
        "TPG2200 ActionURL sent: dest={} source={} incident={} text={:?}",
        action.dest_issi,
        action.source_issi,
        incident,
        message
    );
    if let Ok(mut s) = state.write() {
        s.push_log(
            "INFO",
            format!(
                "TPG2200 ActionURL sent to {}: incident {} text {}",
                action.dest_issi, incident, message
            ),
        );
    }
    http_response(stream, 200, &format!("OK incident={incident}"));
}

/// GET /api/asterisk/status — return Asterisk SIP/RTP config + runtime status.
fn serve_asterisk_status(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let body = match shared_config {
        Some(cfg) => {
            let c = cfg.config();
            let runtime = cfg.state_read().asterisk_status.clone();
            serde_json::json!({
                "config": {
                    "configured": true,
                    "enabled": c.asterisk.enabled,
                    "register": c.asterisk.register,
                    "sip_listen": format!("{}:{}", c.asterisk.bind_addr, c.asterisk.bind_port),
                    "remote": format!("{}:{}", c.asterisk.remote_host, c.asterisk.remote_port),
                    "rtp_port_range": format!("{}-{}", c.asterisk.rtp_port_min, c.asterisk.rtp_port_max),
                    "codec": c.asterisk.codec.clone(),
                    "outbound_prefix": c.asterisk.outbound_prefix.clone(),
                    "strip_outbound_prefix": c.asterisk.strip_outbound_prefix,
                    "service_numbers": c.asterisk.service_numbers.clone(),
                    "local_user": c.asterisk.local_user.clone(),
                    "auth_user": c.asterisk.auth_user.clone(),
                    "realm": c.asterisk.realm.clone(),
                },
                "runtime": {
                    "configured": runtime.configured,
                    "enabled": runtime.enabled,
                    "register_status": runtime.register_status,
                    "sip_listen": runtime.sip_listen,
                    "remote": runtime.remote,
                    "rtp_port_range": runtime.rtp_port_range,
                    "codec": runtime.codec,
                    "active_dialogs": runtime.active_dialogs,
                    "last_rx": runtime.last_rx,
                    "last_tx": runtime.last_tx,
                    "last_error": runtime.last_error,
                }
            })
        }
        None => serde_json::json!({
            "config": { "configured": false, "enabled": false },
            "runtime": {
                "configured": false,
                "enabled": false,
                "register_status": "disabled",
                "sip_listen": "",
                "remote": "",
                "rtp_port_range": "",
                "codec": "PCMU",
                "active_dialogs": 0,
                "last_rx": null,
                "last_tx": null,
                "last_error": null,
            }
        }),
    };
    http_json_response(stream, 200, &body.to_string());
}

/// GET /api/snom-notify — return effective Snom XML NOTIFY settings.
fn serve_snom_notify_get(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let snom = shared_config.as_ref().map(|cfg| cfg.effective_snom_notify()).unwrap_or_default();
    let password = snom.ami_password.as_ref();
    let body = serde_json::json!({
        "enabled": snom.enabled,
        "ami_host": snom.ami_host.clone(),
        "ami_port": snom.ami_port,
        "ami_username": snom.ami_username.clone(),
        "ami_password_masked": crate::net_dashboard::snom_notify::mask_secret(password),
        "ami_password_set": !password.trim().is_empty(),
        "endpoints": snom.endpoints.clone(),
        "notify_sds": snom.notify_sds,
        "notify_dapnet": snom.notify_dapnet,
        "notify_telegram": snom.notify_telegram,
        "sds_directions": snom.sds_directions.clone(),
        "dapnet_allowed_rics": dapnet_ric_set_as_json(&snom.dapnet_allowed_rics),
        "sds_allowed_issis": snom.sds_allowed_issis.iter().copied().collect::<Vec<u32>>(),
        "title_prefix": snom.title_prefix.clone(),
        "notify_event": snom.notify_event.clone(),
        "content_type": snom.content_type.clone(),
        "subscription_state": snom.subscription_state.clone(),
        "max_text_chars": snom.max_text_chars,
        "connect_timeout_secs": snom.connect_timeout_secs,
    });
    http_json_response(stream, 200, &body.to_string());
}

/// POST /api/snom-notify — update Snom XML NOTIFY settings live and persist to config.toml.
fn serve_snom_notify_post(
    stream: TcpStream,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    body: &str,
) {
    use tetra_config::bluestation::SnomNotifyRuntimeOverride;

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };
    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    let cur = cfg.effective_snom_notify();
    let dapnet_allowed_rics = match dapnet_ric_set_from_json(&json, "dapnet_allowed_rics", &cur.dapnet_allowed_rics) {
        Ok(rics) => rics,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid Snom DAPNET RIC filter: {err}"));
            return;
        }
    };
    let sds_allowed_issis = match snom_issi_set_from_json(&json, "sds_allowed_issis", &cur.sds_allowed_issis) {
        Ok(issis) => issis,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid Snom SDS ISSI filter: {err}"));
            return;
        }
    };

    let enabled = dapnet_as_bool(&json, "enabled", cur.enabled);
    let ami_host = snom_non_empty_or(dapnet_as_string(&json, "ami_host", &cur.ami_host), "127.0.0.1");
    let ami_port = dapnet_as_u16(&json, "ami_port", cur.ami_port);
    if ami_port == 0 {
        http_response(stream, 400, "Invalid Snom NOTIFY setting: AMI port cannot be 0");
        return;
    }
    if enabled && ami_host.trim().is_empty() {
        http_response(stream, 400, "Invalid Snom NOTIFY setting: AMI host is required when enabled");
        return;
    }

    let endpoints = snom_string_list(&json, "endpoints", &cur.endpoints);
    let sds_directions = snom_string_list(&json, "sds_directions", &cur.sds_directions)
        .into_iter()
        .map(|d| d.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let ov = SnomNotifyRuntimeOverride {
        enabled,
        ami_host,
        ami_port,
        ami_username: dapnet_as_string(&json, "ami_username", &cur.ami_username),
        ami_password: dapnet_resolve_secret(&json, "ami_password", cur.ami_password.as_ref()),
        endpoints,
        notify_sds: dapnet_as_bool(&json, "notify_sds", cur.notify_sds),
        notify_dapnet: dapnet_as_bool(&json, "notify_dapnet", cur.notify_dapnet),
        notify_telegram: dapnet_as_bool(&json, "notify_telegram", cur.notify_telegram),
        sds_directions,
        dapnet_allowed_rics,
        sds_allowed_issis,
        title_prefix: snom_non_empty_or(dapnet_as_string(&json, "title_prefix", &cur.title_prefix), "FlowStation"),
        notify_event: snom_non_empty_or(dapnet_as_string(&json, "notify_event", &cur.notify_event), "xml"),
        content_type: snom_non_empty_or(dapnet_as_string(&json, "content_type", &cur.content_type), "application/snomxml"),
        subscription_state: snom_non_empty_or(
            dapnet_as_string(&json, "subscription_state", &cur.subscription_state),
            "active;expires=30000",
        ),
        max_text_chars: dapnet_as_usize(&json, "max_text_chars", cur.max_text_chars).clamp(40, 2000),
        connect_timeout_secs: dapnet_as_u64(&json, "connect_timeout_secs", cur.connect_timeout_secs).clamp(1, 30),
    };

    let mut text_fields = vec![
        ov.ami_host.as_str(),
        ov.ami_username.as_str(),
        ov.ami_password.as_str(),
        ov.title_prefix.as_str(),
        ov.notify_event.as_str(),
        ov.content_type.as_str(),
        ov.subscription_state.as_str(),
    ];
    text_fields.extend(ov.endpoints.iter().map(String::as_str));
    text_fields.extend(ov.sds_directions.iter().map(String::as_str));
    if !text_fields.iter().all(|v| dapnet_text_acceptable(v)) {
        http_response(stream, 400, "Invalid Snom NOTIFY setting: control characters are not allowed");
        return;
    }

    {
        let mut state = cfg.state_write();
        state.snom_notify_override = Some(ov.clone());
    }

    if let Err(e) = crate::net_dashboard::snom_notify::write_snom_notify_to_toml(config_path, &ov) {
        tracing::warn!("Dashboard: Snom NOTIFY applied at runtime but failed to persist to TOML: {}", e);
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!(
        "Dashboard: Snom NOTIFY updated (enabled={} endpoints={} sds={} dapnet={} telegram={})",
        ov.enabled,
        ov.endpoints.len(),
        ov.notify_sds,
        ov.notify_dapnet,
        ov.notify_telegram
    );
    http_response(stream, 200, "OK");
}

fn snom_non_empty_or(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn dapnet_resolve_secret(json: &serde_json::Value, key: &str, current: &str) -> String {
    match json.get(key).and_then(|v| v.as_str()) {
        Some(v) if !v.contains('…') => v.trim().to_string(),
        _ => current.to_string(),
    }
}

fn dapnet_text_acceptable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control())
}

const DAPNET_API_TEXT_MAX_CHARS: usize = 80;

fn dapnet_as_bool(json: &serde_json::Value, key: &str, default: bool) -> bool {
    json.get(key).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn dapnet_as_u32(json: &serde_json::Value, key: &str, default: u32) -> u32 {
    json.get(key)
        .and_then(|x| x.as_u64())
        .map(|n| n.min(16_777_215) as u32)
        .unwrap_or(default)
}

fn dapnet_as_u64(json: &serde_json::Value, key: &str, default: u64) -> u64 {
    json.get(key).and_then(|x| x.as_u64()).unwrap_or(default)
}

fn dapnet_as_u16(json: &serde_json::Value, key: &str, default: u16) -> u16 {
    json.get(key)
        .and_then(|x| x.as_u64())
        .map(|n| n.min(u16::MAX as u64) as u16)
        .unwrap_or(default)
}

fn dapnet_as_f64(json: &serde_json::Value, key: &str, default: f64) -> f64 {
    json.get(key)
        .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().parse::<f64>().ok())))
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn dapnet_as_usize(json: &serde_json::Value, key: &str, default: usize) -> usize {
    json.get(key).and_then(|x| x.as_u64()).map(|n| n.max(1) as usize).unwrap_or(default)
}

fn dapnet_as_string(json: &serde_json::Value, key: &str, default: &str) -> String {
    json.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| default.to_string())
}

fn dapnet_ric_routes_as_json(routes: &BTreeMap<u32, u32>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (ric, issi) in routes {
        map.insert(tetra_config::bluestation::format_ric_route_key(*ric), serde_json::json!(issi));
    }
    serde_json::Value::Object(map)
}

fn dapnet_ric_set_as_json(rics: &BTreeSet<u32>) -> serde_json::Value {
    serde_json::Value::Array(
        rics.iter()
            .map(|ric| serde_json::Value::String(tetra_config::bluestation::format_ric_route_key(*ric)))
            .collect(),
    )
}

fn dapnet_parse_ric_json_value(value: &serde_json::Value, label: &str) -> Result<u32, String> {
    if let Some(s) = value.as_str() {
        return tetra_config::bluestation::parse_ric_route_key(s);
    }
    if let Some(n) = value.as_u64() {
        if n <= u32::MAX as u64 {
            return Ok(n as u32);
        }
    }
    Err(format!("{label}: RIC must be a string or positive integer"))
}

fn dapnet_ric_set_from_json(json: &serde_json::Value, key: &str, current: &BTreeSet<u32>) -> Result<BTreeSet<u32>, String> {
    let Some(value) = json.get(key) else {
        return Ok(current.clone());
    };
    let mut rics = BTreeSet::new();
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                rics.insert(dapnet_parse_ric_json_value(item, key)?);
            }
        }
        serde_json::Value::String(text) => {
            for line_raw in text.lines() {
                let line = line_raw.split('#').next().unwrap_or("").trim();
                if line.is_empty() {
                    continue;
                }
                for part in line.split(|c: char| c == ',' || c.is_whitespace()) {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    rics.insert(tetra_config::bluestation::parse_ric_route_key(part)?);
                }
            }
        }
        _ => return Err(format!("{key} must be an array or text list")),
    }
    Ok(rics)
}

fn dapnet_ric_routes_from_json_key(
    json: &serde_json::Value,
    key: &str,
    current: &BTreeMap<u32, u32>,
) -> Result<BTreeMap<u32, u32>, String> {
    let Some(value) = json.get(key) else {
        return Ok(current.clone());
    };
    let mut routes = BTreeMap::new();
    match value {
        serde_json::Value::Object(map) => {
            for (raw_ric, raw_issi) in map {
                let ric = tetra_config::bluestation::parse_ric_route_key(raw_ric)?;
                let Some(issi) = raw_issi.as_u64() else {
                    return Err(format!("RIC route {raw_ric}: ISSI must be a number"));
                };
                if issi == 0 || issi > 16_777_215 {
                    return Err(format!("RIC route {raw_ric}: ISSI out of range"));
                }
                routes.insert(ric, issi as u32);
            }
        }
        serde_json::Value::String(text) => {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((raw_ric, raw_issi)) = line.split_once('=') else {
                    return Err(format!("RIC route line '{line}' must be RIC=ISSI"));
                };
                let ric = tetra_config::bluestation::parse_ric_route_key(raw_ric)?;
                let issi = raw_issi
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("RIC route line '{line}' has invalid ISSI"))?;
                if issi == 0 || issi > 16_777_215 {
                    return Err(format!("RIC route line '{line}' has ISSI out of range"));
                }
                routes.insert(ric, issi);
            }
        }
        _ => return Err(format!("{key} must be an object or text lines")),
    }
    Ok(routes)
}

fn dapnet_ric_routes_from_json(json: &serde_json::Value, current: &BTreeMap<u32, u32>) -> Result<BTreeMap<u32, u32>, String> {
    dapnet_ric_routes_from_json_key(json, "ric_issi_routes", current)
}

fn dapnet_validate_route_conflicts(issi_routes: &BTreeMap<u32, u32>, gssi_routes: &BTreeMap<u32, u32>) -> Result<(), String> {
    for ric in issi_routes.keys() {
        if gssi_routes.contains_key(ric) {
            return Err(format!(
                "RIC {} is configured as both ISSI and GSSI route",
                tetra_config::bluestation::format_ric_route_key(*ric)
            ));
        }
    }
    Ok(())
}

/// GET /api/dapnet — return effective DAPNET settings as JSON. Secrets are masked and are never
/// echoed in the clear.
fn serve_dapnet_get(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let (dapnet, runtime) = match shared_config {
        Some(cfg) => (cfg.effective_dapnet(), cfg.state_read().dapnet_status.clone()),
        None => (
            tetra_config::bluestation::CfgDapnet::default(),
            tetra_config::bluestation::DapnetRuntimeStatus::default(),
        ),
    };
    let password = dapnet.password.as_ref();
    let authkey = dapnet.rwth_core_authkey.as_ref();
    let runtime_body = serde_json::json!({
        "configured": runtime.configured,
        "enabled": runtime.enabled,
        "rwth_core_enabled": runtime.rwth_core_enabled,
        "rwth_core_status": runtime.rwth_core_status,
        "endpoint": runtime.endpoint,
        "callsign": runtime.callsign,
        "forward_sds": runtime.forward_sds,
        "forward_callout": runtime.forward_callout,
        "forward_telegram": runtime.forward_telegram,
        "seen_messages": runtime.seen_messages,
        "last_rx": runtime.last_rx,
        "last_error": runtime.last_error,
    });
    let mut body = serde_json::json!({
        "enabled": dapnet.enabled,
        "api_url": dapnet.api_url.clone(),
        "username": dapnet.username.clone(),
        "password_masked": crate::net_dashboard::dapnet::mask_secret(password),
        "password_set": !password.trim().is_empty(),
        "poll_interval_secs": dapnet.poll_interval_secs,
        "forward_sds": dapnet.forward_sds,
        "forward_callout": dapnet.forward_callout,
        "forward_telegram": dapnet.forward_telegram,
        "sds_source_issi": dapnet.sds_source_issi,
        "sds_dest_issi": dapnet.sds_dest_issi,
        "sds_dest_is_group": dapnet.sds_dest_is_group,
        "ric_issi_routes": dapnet_ric_routes_as_json(&dapnet.ric_issi_routes),
        "ric_gssi_routes": dapnet_ric_routes_as_json(&dapnet.ric_gssi_routes),
        "sds_allowed_rics": dapnet_ric_set_as_json(&dapnet.sds_allowed_rics),
        "callout_allowed_rics": dapnet_ric_set_as_json(&dapnet.callout_allowed_rics),
        "telegram_allowed_rics": dapnet_ric_set_as_json(&dapnet.telegram_allowed_rics),
        "callout_source_issi": dapnet.callout_source_issi,
        "callout_dest_issi": dapnet.callout_dest_issi,
        "callout_incident_base": dapnet.callout_incident_base,
        "callout_text_prefix": dapnet.callout_text_prefix.clone(),
        "telegram_prefix": dapnet.telegram_prefix.clone(),
        "rwth_core_enabled": dapnet.rwth_core_enabled,
        "rwth_core_host": dapnet.rwth_core_host.clone(),
        "rwth_core_port": dapnet.rwth_core_port,
        "rwth_core_device": dapnet.rwth_core_device.clone(),
        "rwth_core_version": dapnet.rwth_core_version.clone(),
        "rwth_core_callsign": dapnet.rwth_core_callsign.clone(),
        "rwth_core_authkey_masked": crate::net_dashboard::dapnet::mask_secret(authkey),
        "rwth_core_authkey_set": !authkey.trim().is_empty(),
        "rwth_messages_limit": dapnet.rwth_messages_limit,
    });
    if let Some(obj) = body.as_object_mut() {
        obj.insert("runtime".to_string(), runtime_body);
    }
    http_json_response(stream, 200, &body.to_string());
}

/// POST /api/dapnet — update DAPNET settings. Applies immediately through StackState override
/// and rewrites `[dapnet]` in config.toml. Secrets are changed only when a fresh, non-masked
/// value is supplied.
fn serve_dapnet_post(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
    use tetra_config::bluestation::DapnetRuntimeOverride;

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };
    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    let cur = cfg.effective_dapnet();
    let password = dapnet_resolve_secret(&json, "password", cur.password.as_ref());
    let rwth_core_authkey = dapnet_resolve_secret(&json, "rwth_core_authkey", cur.rwth_core_authkey.as_ref());
    let ric_issi_routes = match dapnet_ric_routes_from_json(&json, &cur.ric_issi_routes) {
        Ok(routes) => routes,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid DAPNET RIC route: {err}"));
            return;
        }
    };
    let ric_gssi_routes = match dapnet_ric_routes_from_json_key(&json, "ric_gssi_routes", &cur.ric_gssi_routes) {
        Ok(routes) => routes,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid DAPNET group RIC route: {err}"));
            return;
        }
    };
    if let Err(err) = dapnet_validate_route_conflicts(&ric_issi_routes, &ric_gssi_routes) {
        http_response(stream, 400, &format!("Invalid DAPNET RIC route: {err}"));
        return;
    }
    let sds_allowed_rics = match dapnet_ric_set_from_json(&json, "sds_allowed_rics", &cur.sds_allowed_rics) {
        Ok(rics) => rics,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid SDS RIC filter: {err}"));
            return;
        }
    };
    let callout_allowed_rics = match dapnet_ric_set_from_json(&json, "callout_allowed_rics", &cur.callout_allowed_rics) {
        Ok(rics) => rics,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid Call-Out RIC filter: {err}"));
            return;
        }
    };
    let telegram_allowed_rics = match dapnet_ric_set_from_json(&json, "telegram_allowed_rics", &cur.telegram_allowed_rics) {
        Ok(rics) => rics,
        Err(err) => {
            http_response(stream, 400, &format!("Invalid Telegram RIC filter: {err}"));
            return;
        }
    };

    let ov = DapnetRuntimeOverride {
        enabled: dapnet_as_bool(&json, "enabled", cur.enabled),
        api_url: dapnet_as_string(&json, "api_url", &cur.api_url),
        username: dapnet_as_string(&json, "username", &cur.username),
        password,
        poll_interval_secs: dapnet_as_u64(&json, "poll_interval_secs", cur.poll_interval_secs).max(1),
        forward_sds: dapnet_as_bool(&json, "forward_sds", cur.forward_sds),
        forward_callout: dapnet_as_bool(&json, "forward_callout", cur.forward_callout),
        forward_telegram: dapnet_as_bool(&json, "forward_telegram", cur.forward_telegram),
        sds_source_issi: dapnet_as_u32(&json, "sds_source_issi", cur.sds_source_issi).max(1),
        sds_dest_issi: dapnet_as_u32(&json, "sds_dest_issi", cur.sds_dest_issi),
        sds_dest_is_group: dapnet_as_bool(&json, "sds_dest_is_group", cur.sds_dest_is_group),
        ric_issi_routes,
        ric_gssi_routes,
        sds_allowed_rics,
        callout_allowed_rics,
        telegram_allowed_rics,
        callout_source_issi: dapnet_as_u32(&json, "callout_source_issi", cur.callout_source_issi).max(1),
        callout_dest_issi: dapnet_as_u32(&json, "callout_dest_issi", cur.callout_dest_issi),
        callout_incident_base: dapnet_as_u16(&json, "callout_incident_base", cur.callout_incident_base).clamp(1, 256),
        callout_text_prefix: dapnet_as_string(&json, "callout_text_prefix", &cur.callout_text_prefix),
        telegram_prefix: dapnet_as_string(&json, "telegram_prefix", &cur.telegram_prefix),
        rwth_core_enabled: dapnet_as_bool(&json, "rwth_core_enabled", cur.rwth_core_enabled),
        rwth_core_host: dapnet_as_string(&json, "rwth_core_host", &cur.rwth_core_host),
        rwth_core_port: dapnet_as_u16(&json, "rwth_core_port", cur.rwth_core_port),
        rwth_core_device: dapnet_as_string(&json, "rwth_core_device", &cur.rwth_core_device),
        rwth_core_version: dapnet_as_string(&json, "rwth_core_version", &cur.rwth_core_version),
        rwth_core_callsign: dapnet_as_string(&json, "rwth_core_callsign", &cur.rwth_core_callsign),
        rwth_core_authkey,
        rwth_messages_limit: dapnet_as_usize(&json, "rwth_messages_limit", cur.rwth_messages_limit),
    };

    let text_fields = [
        ov.api_url.as_str(),
        ov.username.as_str(),
        ov.password.as_str(),
        ov.callout_text_prefix.as_str(),
        ov.telegram_prefix.as_str(),
        ov.rwth_core_host.as_str(),
        ov.rwth_core_device.as_str(),
        ov.rwth_core_version.as_str(),
        ov.rwth_core_callsign.as_str(),
        ov.rwth_core_authkey.as_str(),
    ];
    if !text_fields.iter().all(|v| dapnet_text_acceptable(v)) {
        http_response(stream, 400, "Invalid DAPNET setting: control characters are not allowed");
        return;
    }

    {
        let mut state = cfg.state_write();
        state.dapnet_override = Some(ov.clone());
    }

    if let Err(e) = crate::net_dashboard::dapnet::write_dapnet_to_toml(config_path, &ov) {
        tracing::warn!("Dashboard: DAPNET applied at runtime but failed to persist to TOML: {}", e);
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!(
        "Dashboard: DAPNET updated (enabled={} rwth_core={} routes=sds:{} callout:{} telegram:{})",
        ov.enabled,
        ov.rwth_core_enabled,
        ov.forward_sds,
        ov.forward_callout,
        ov.forward_telegram
    );
    http_response(stream, 200, "OK");
}

/// GET /api/geoalarm — return effective GeoAlarm settings and runtime status as JSON.
fn serve_geoalarm_get(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let (geoalarm, runtime) = match shared_config {
        Some(cfg) => (cfg.effective_geoalarm(), cfg.state_read().geoalarm_status.clone()),
        None => (
            tetra_config::bluestation::CfgGeoalarm::default(),
            tetra_config::bluestation::GeoalarmRuntimeStatus::default(),
        ),
    };
    let events = runtime
        .events
        .iter()
        .map(|event| {
            serde_json::json!({
                "ts": event.ts.clone(),
                "source": event.source.clone(),
                "device": event.device.clone(),
                "lat": event.lat,
                "lon": event.lon,
                "distance_m": event.distance_m,
                "inside_radius": event.inside_radius,
                "alarmed": event.alarmed,
                "paths": event.paths.clone(),
            })
        })
        .collect::<Vec<_>>();
    let runtime_body = serde_json::json!({
        "configured": runtime.configured,
        "enabled": runtime.enabled,
        "center": runtime.center,
        "radius_m": runtime.radius_m,
        "trigger_tetra": runtime.trigger_tetra,
        "trigger_meshcom": runtime.trigger_meshcom,
        "forward_tpg2200": runtime.forward_tpg2200,
        "forward_sds": runtime.forward_sds,
        "forward_sip": runtime.forward_sip,
        "forward_telegram": runtime.forward_telegram,
        "seen_positions": runtime.seen_positions,
        "alarm_count": runtime.alarm_count,
        "last_position": runtime.last_position,
        "last_alarm": runtime.last_alarm,
        "last_error": runtime.last_error,
    });
    let body = serde_json::json!({
        "enabled": geoalarm.enabled,
        "flowstation_lat": geoalarm.flowstation_lat,
        "flowstation_lon": geoalarm.flowstation_lon,
        "radius_m": geoalarm.radius_m,
        "cooldown_secs": geoalarm.cooldown_secs,
        "trigger_tetra": geoalarm.trigger_tetra,
        "trigger_meshcom": geoalarm.trigger_meshcom,
        "forward_tpg2200": geoalarm.forward_tpg2200,
        "forward_sds": geoalarm.forward_sds,
        "forward_sip": geoalarm.forward_sip,
        "forward_telegram": geoalarm.forward_telegram,
        "tetra_issi_whitelist": issi_set_as_json(&geoalarm.tetra_issi_whitelist),
        "tetra_issi_blacklist": issi_set_as_json(&geoalarm.tetra_issi_blacklist),
        "meshcom_source_whitelist": meshcom_source_list_as_json(&geoalarm.meshcom_source_whitelist),
        "meshcom_source_blacklist": meshcom_source_list_as_json(&geoalarm.meshcom_source_blacklist),
        "sds_source_issi": geoalarm.sds_source_issi,
        "sds_dest_issi": geoalarm.sds_dest_issi,
        "sds_dest_is_group": geoalarm.sds_dest_is_group,
        "tpg2200_source_issi": geoalarm.tpg2200_source_issi,
        "tpg2200_dest_issi": geoalarm.tpg2200_dest_issi,
        "tpg2200_incident_base": geoalarm.tpg2200_incident_base,
        "tpg2200_text_prefix": geoalarm.tpg2200_text_prefix.clone(),
        "tpg2200_max_text_chars": geoalarm.tpg2200_max_text_chars,
        "sip_title_prefix": geoalarm.sip_title_prefix.clone(),
        "telegram_prefix": geoalarm.telegram_prefix.clone(),
        "runtime": runtime_body,
        "events": events,
    });
    http_json_response(stream, 200, &body.to_string());
}

/// POST /api/geoalarm — update GeoAlarm settings. Applies immediately through StackState
/// override and rewrites `[geoalarm]` in config.toml.
fn serve_geoalarm_post(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
    use tetra_config::bluestation::{CfgGeoalarmDto, GeoalarmRuntimeOverride, apply_geoalarm_patch};

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };
    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    let cur = cfg.effective_geoalarm();
    let tetra_issi_whitelist = match snom_issi_set_from_json(&json, "tetra_issi_whitelist", &cur.tetra_issi_whitelist) {
        Ok(v) => v,
        Err(err) => {
            http_response(stream, 400, &err);
            return;
        }
    };
    let tetra_issi_blacklist = match snom_issi_set_from_json(&json, "tetra_issi_blacklist", &cur.tetra_issi_blacklist) {
        Ok(v) => v,
        Err(err) => {
            http_response(stream, 400, &err);
            return;
        }
    };
    let meshcom_source_whitelist = match meshcom_source_list_from_json(&json, "meshcom_source_whitelist", &cur.meshcom_source_whitelist) {
        Ok(v) => v,
        Err(err) => {
            http_response(stream, 400, &err);
            return;
        }
    };
    let meshcom_source_blacklist = match meshcom_source_list_from_json(&json, "meshcom_source_blacklist", &cur.meshcom_source_blacklist) {
        Ok(v) => v,
        Err(err) => {
            http_response(stream, 400, &err);
            return;
        }
    };

    let dto = CfgGeoalarmDto {
        enabled: dapnet_as_bool(&json, "enabled", cur.enabled),
        flowstation_lat: dapnet_as_f64(&json, "flowstation_lat", cur.flowstation_lat),
        flowstation_lon: dapnet_as_f64(&json, "flowstation_lon", cur.flowstation_lon),
        radius_m: dapnet_as_f64(&json, "radius_m", cur.radius_m),
        cooldown_secs: dapnet_as_u64(&json, "cooldown_secs", cur.cooldown_secs),
        trigger_tetra: dapnet_as_bool(&json, "trigger_tetra", cur.trigger_tetra),
        trigger_meshcom: dapnet_as_bool(&json, "trigger_meshcom", cur.trigger_meshcom),
        forward_tpg2200: dapnet_as_bool(&json, "forward_tpg2200", cur.forward_tpg2200),
        forward_sds: dapnet_as_bool(&json, "forward_sds", cur.forward_sds),
        forward_sip: dapnet_as_bool(&json, "forward_sip", cur.forward_sip),
        forward_telegram: dapnet_as_bool(&json, "forward_telegram", cur.forward_telegram),
        tetra_issi_whitelist: tetra_issi_whitelist.iter().copied().collect(),
        tetra_issi_blacklist: tetra_issi_blacklist.iter().copied().collect(),
        meshcom_source_whitelist,
        meshcom_source_blacklist,
        sds_source_issi: dapnet_as_u32(&json, "sds_source_issi", cur.sds_source_issi),
        sds_dest_issi: dapnet_as_u32(&json, "sds_dest_issi", cur.sds_dest_issi),
        sds_dest_is_group: dapnet_as_bool(&json, "sds_dest_is_group", cur.sds_dest_is_group),
        tpg2200_source_issi: dapnet_as_u32(&json, "tpg2200_source_issi", cur.tpg2200_source_issi),
        tpg2200_dest_issi: dapnet_as_u32(&json, "tpg2200_dest_issi", cur.tpg2200_dest_issi),
        tpg2200_incident_base: dapnet_as_u16(&json, "tpg2200_incident_base", cur.tpg2200_incident_base),
        tpg2200_text_prefix: dapnet_as_string(&json, "tpg2200_text_prefix", &cur.tpg2200_text_prefix),
        tpg2200_max_text_chars: dapnet_as_usize(&json, "tpg2200_max_text_chars", cur.tpg2200_max_text_chars),
        sip_title_prefix: dapnet_as_string(&json, "sip_title_prefix", &cur.sip_title_prefix),
        telegram_prefix: dapnet_as_string(&json, "telegram_prefix", &cur.telegram_prefix),
        extra: HashMap::new(),
    };
    let normalized = match apply_geoalarm_patch(dto) {
        Ok(cfg) => cfg,
        Err(err) => {
            http_response(stream, 400, &err);
            return;
        }
    };
    let text_fields = [
        normalized.tpg2200_text_prefix.as_str(),
        normalized.sip_title_prefix.as_str(),
        normalized.telegram_prefix.as_str(),
    ];
    if !text_fields.iter().all(|v| dapnet_text_acceptable(v))
        || !normalized
            .meshcom_source_whitelist
            .iter()
            .chain(normalized.meshcom_source_blacklist.iter())
            .all(|v| dapnet_text_acceptable(v))
    {
        http_response(stream, 400, "Invalid GeoAlarm setting: control characters are not allowed");
        return;
    }

    let ov = GeoalarmRuntimeOverride {
        enabled: normalized.enabled,
        flowstation_lat: normalized.flowstation_lat,
        flowstation_lon: normalized.flowstation_lon,
        radius_m: normalized.radius_m,
        cooldown_secs: normalized.cooldown_secs,
        trigger_tetra: normalized.trigger_tetra,
        trigger_meshcom: normalized.trigger_meshcom,
        forward_tpg2200: normalized.forward_tpg2200,
        forward_sds: normalized.forward_sds,
        forward_sip: normalized.forward_sip,
        forward_telegram: normalized.forward_telegram,
        tetra_issi_whitelist: normalized.tetra_issi_whitelist,
        tetra_issi_blacklist: normalized.tetra_issi_blacklist,
        meshcom_source_whitelist: normalized.meshcom_source_whitelist,
        meshcom_source_blacklist: normalized.meshcom_source_blacklist,
        sds_source_issi: normalized.sds_source_issi,
        sds_dest_issi: normalized.sds_dest_issi,
        sds_dest_is_group: normalized.sds_dest_is_group,
        tpg2200_source_issi: normalized.tpg2200_source_issi,
        tpg2200_dest_issi: normalized.tpg2200_dest_issi,
        tpg2200_incident_base: normalized.tpg2200_incident_base,
        tpg2200_text_prefix: normalized.tpg2200_text_prefix,
        tpg2200_max_text_chars: normalized.tpg2200_max_text_chars,
        sip_title_prefix: normalized.sip_title_prefix,
        telegram_prefix: normalized.telegram_prefix,
    };

    {
        let mut state = cfg.state_write();
        state.geoalarm_override = Some(ov.clone());
    }

    if let Err(e) = crate::net_dashboard::geoalarm::write_geoalarm_to_toml(config_path, &ov) {
        tracing::warn!("Dashboard: GeoAlarm applied at runtime but failed to persist to TOML: {}", e);
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!(
        "Dashboard: GeoAlarm updated (enabled={} center={:.6},{:.6} radius={:.0}m routes=tpg2200:{} sds:{} sip:{} telegram:{})",
        ov.enabled,
        ov.flowstation_lat,
        ov.flowstation_lon,
        ov.radius_m,
        ov.forward_tpg2200,
        ov.forward_sds,
        ov.forward_sip,
        ov.forward_telegram
    );
    http_response(stream, 200, "OK");
}

fn snom_string_list(json: &serde_json::Value, key: &str, default: &[String]) -> Vec<String> {
    if let Some(arr) = json.get(key).and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Some(s) = json.get(key).and_then(|v| v.as_str()) {
        return s
            .split(|c: char| c == ',' || c == '\n' || c == '\r')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
    }
    default.to_vec()
}

fn meshcom_source_list_as_json(values: &BTreeSet<String>) -> serde_json::Value {
    serde_json::Value::Array(values.iter().map(|value| serde_json::Value::String(value.clone())).collect())
}

fn issi_set_as_json(values: &BTreeSet<u32>) -> serde_json::Value {
    serde_json::Value::Array(values.iter().map(|value| serde_json::json!(*value)).collect())
}

fn meshcom_source_list_from_json(json: &serde_json::Value, key: &str, current: &BTreeSet<String>) -> Result<Vec<String>, String> {
    let Some(value) = json.get(key) else {
        return Ok(current.iter().cloned().collect());
    };
    let mut out = Vec::new();
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(format!("{key}: source entries must be strings"));
                };
                push_meshcom_source_parts(key, text, &mut out)?;
            }
        }
        serde_json::Value::String(text) => {
            push_meshcom_source_parts(key, text, &mut out)?;
        }
        _ => return Err(format!("{key} must be an array or text list")),
    }
    Ok(out)
}

fn push_meshcom_source_parts(key: &str, text: &str, out: &mut Vec<String>) -> Result<(), String> {
    for line_raw in text.lines() {
        let line = line_raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        for part in line.split(|c: char| c == ',' || c.is_whitespace()) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if !dapnet_text_acceptable(part) {
                return Err(format!("{key}: source entries may not contain control characters"));
            }
            out.push(part.to_string());
        }
    }
    Ok(())
}

fn snom_issi_set_from_json(json: &serde_json::Value, key: &str, current: &BTreeSet<u32>) -> Result<BTreeSet<u32>, String> {
    let Some(value) = json.get(key) else {
        return Ok(current.clone());
    };
    let mut out = BTreeSet::new();
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                let issi = if let Some(n) = item.as_u64() {
                    n
                } else if let Some(s) = item.as_str() {
                    s.trim().parse::<u64>().map_err(|_| format!("{key}: ISSI must be numeric"))?
                } else {
                    return Err(format!("{key}: ISSI must be a positive integer"));
                };
                if issi > 16_777_215 {
                    return Err(format!("{key}: ISSI {} out of range", issi));
                }
                out.insert(issi as u32);
            }
        }
        serde_json::Value::String(text) => {
            for part in text.split(|c: char| c == ',' || c.is_whitespace()) {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let issi = part.parse::<u64>().map_err(|_| format!("{key}: ISSI must be numeric"))?;
                if issi > 16_777_215 {
                    return Err(format!("{key}: ISSI {} out of range", issi));
                }
                out.insert(issi as u32);
            }
        }
        _ => return Err(format!("{key} must be an array or text list")),
    }
    Ok(out)
}

fn dapnet_string_list(json: &serde_json::Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        if let Some(arr) = json.get(*key).and_then(|v| v.as_array()) {
            return arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(s) = json.get(*key).and_then(|v| v.as_str()) {
            return s
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
        }
    }
    Vec::new()
}

fn normalize_dapnet_api_url(api_url: &str) -> String {
    let mut url = api_url.trim().trim_end_matches('/').to_string();
    if let Some(rest) = url.strip_prefix("https://www.hampager.de") {
        url = format!("https://hampager.de{rest}");
    } else if let Some(rest) = url.strip_prefix("http://www.hampager.de") {
        url = format!("http://hampager.de{rest}");
    }
    if let Some(base) = url.strip_suffix("/api/messages") {
        return format!("{base}/api/calls");
    }
    if let Some(base) = url.strip_suffix("/messages")
        && base.ends_with("/api")
    {
        return format!("{base}/calls");
    }
    url
}

fn build_dapnet_call_payload(text: &str, callsigns: Vec<String>, groups: Vec<String>, emergency: bool) -> serde_json::Value {
    serde_json::json!({
        "text": text,
        "callSignNames": callsigns,
        "transmitterGroupNames": groups,
        "emergency": emergency,
    })
}

fn push_dapnet_log_and_broadcast(
    state: &DashboardState,
    clients: &WsClients,
    direction: &str,
    id: String,
    callsign: String,
    recipient: String,
    text: String,
    priority: Option<u8>,
    paths: Vec<String>,
) {
    {
        if let Ok(mut s) = state.write() {
            s.push_dapnet_log(
                direction,
                id.clone(),
                callsign.clone(),
                recipient.clone(),
                text.clone(),
                priority,
                paths.clone(),
            );
        }
    }
    if let Ok(json) = serde_json::to_string(&serde_json::json!({
        "type": "dapnet_log",
        "direction": direction,
        "id": id,
        "callsign": callsign,
        "recipient": recipient,
        "text": text,
        "priority": priority,
        "paths": paths,
    })) {
        if let Ok(mut clients) = clients.lock() {
            clients.retain(|tx| tx.send(json.clone()).is_ok());
        }
    }
}

/// POST /api/dapnet/send — send one outbound DAPNET message through the configured Hampager API.
fn serve_dapnet_send(
    stream: TcpStream,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    state: &DashboardState,
    clients: &WsClients,
    body: &str,
) {
    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_json_response(
                stream,
                400,
                &serde_json::json!({"ok":false,"error":format!("Invalid JSON: {e}")}).to_string(),
            );
            return;
        }
    };
    let Some(cfg) = shared_config else {
        http_json_response(stream, 503, "{\"ok\":false,\"error\":\"Config not available\"}");
        return;
    };
    let dapnet = cfg.effective_dapnet();
    if !dapnet.enabled {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"DAPNET is disabled\"}");
        return;
    }
    let text = json.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if text.is_empty() {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Message text is empty\"}");
        return;
    }
    if !dapnet_text_acceptable(&text) {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"Message contains control characters\"}");
        return;
    }
    if text.chars().count() > DAPNET_API_TEXT_MAX_CHARS {
        http_json_response(
            stream,
            200,
            "{\"ok\":false,\"error\":\"DAPNET message text exceeds 80 characters\"}",
        );
        return;
    }
    let api_url = normalize_dapnet_api_url(&dapnet.api_url);
    if api_url.is_empty() {
        http_json_response(stream, 200, "{\"ok\":false,\"error\":\"DAPNET api_url is empty\"}");
        return;
    }
    let callsigns = dapnet_string_list(&json, &["callSignNames", "callsigns", "call_signs"]);
    let groups = dapnet_string_list(&json, &["transmitterGroupNames", "transmitter_groups", "groups"]);
    if callsigns.is_empty() && groups.is_empty() {
        http_json_response(
            stream,
            200,
            "{\"ok\":false,\"error\":\"Set at least one callsign or transmitter group\"}",
        );
        return;
    }
    let emergency = json.get("emergency").and_then(|v| v.as_bool()).unwrap_or(false);
    let req_body = build_dapnet_call_payload(&text, callsigns, groups, emergency);

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            http_json_response(
                stream,
                200,
                &serde_json::json!({"ok":false,"error":format!("HTTP client error: {e}")}).to_string(),
            );
            return;
        }
    };
    let mut request = client.post(&api_url).json(&req_body);
    if !dapnet.username.trim().is_empty() {
        request = request.basic_auth(dapnet.username.trim().to_string(), Some(dapnet.password.as_ref().to_string()));
    }
    match request.send() {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                tracing::info!(
                    "Dashboard: DAPNET outbound sent via {} (callsigns={} groups={} emergency={})",
                    api_url,
                    req_body["callSignNames"].as_array().map(|a| a.len()).unwrap_or(0),
                    req_body["transmitterGroupNames"].as_array().map(|a| a.len()).unwrap_or(0),
                    emergency
                );
                push_dapnet_log_and_broadcast(
                    state,
                    clients,
                    "tx",
                    format!("api:{}", chrono::Utc::now().timestamp_millis()),
                    req_body["callSignNames"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    req_body["transmitterGroupNames"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(","))
                        .unwrap_or_default(),
                    text,
                    if emergency { Some(1) } else { None },
                    vec!["dapnet-api".to_string()],
                );
                http_json_response(stream, 200, "{\"ok\":true}");
            } else {
                let err = format!("DAPNET API returned HTTP {}", status.as_u16());
                tracing::warn!("Dashboard: {}", err);
                http_json_response(stream, 200, &serde_json::json!({"ok":false,"error":err}).to_string());
            }
        }
        Err(e) => {
            tracing::warn!("Dashboard: DAPNET outbound send failed: {}", e);
            http_json_response(
                stream,
                200,
                &serde_json::json!({"ok":false,"error":format!("Network error: {e}")}).to_string(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DashboardServer, binary_built_from};
    use crate::net_telemetry::TelemetryEvent;

    /// FH-BUG (brew shown as v0): the transport reports version 0 ("unknown") on every (re)connect
    /// and v1 is learned lazily from a v1 group call. A confirmed v1 must never be downgraded by a
    /// later 0-reporting (re)connect.
    #[test]
    fn brew_version_is_monotonic_across_reconnects() {
        let server = DashboardServer::new("/tmp/fs_brew_ver_test_config.toml".to_string());
        let v = || server.state.read().unwrap().brew_version;

        server.handle_telemetry(TelemetryEvent::BrewConnected {
            connected: true,
            server_version: 0,
        });
        assert_eq!(v(), 0, "initial connect reports unknown");

        server.handle_telemetry(TelemetryEvent::BrewConnected {
            connected: true,
            server_version: 1,
        });
        assert_eq!(v(), 1, "a v1 group call raises it to v1");

        // Disconnect then reconnect, transport again reports 0 — must NOT downgrade.
        server.handle_telemetry(TelemetryEvent::BrewConnected {
            connected: false,
            server_version: 0,
        });
        server.handle_telemetry(TelemetryEvent::BrewConnected {
            connected: true,
            server_version: 0,
        });
        assert_eq!(v(), 1, "reconnect reporting v0 must not downgrade a confirmed v1");
    }

    #[test]
    fn test_binary_built_from() {
        let head = "fcac34e2778658fd8a2c6767d54f6da6feaaa5fc";
        // Binary built from this commit (8-char abbrev) -> up to date.
        assert_eq!(binary_built_from("fcac34e2", head), Some(true));
        // Binary built from an older commit -> stale, needs rebuild (the FH-BUG-037 case).
        assert_eq!(binary_built_from("6abf749f", head), Some(false));
        // A "-modified" (dirty) build at the same commit still counts as that commit.
        assert_eq!(binary_built_from("fcac34e2-modified", head), Some(true));
        // No usable hash baked in -> cannot tell.
        assert_eq!(binary_built_from("unknown", head), None);
        assert_eq!(binary_built_from("", head), None);
    }
}
