use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use tungstenite::{
    Message, accept_hdr,
    handshake::server::{Request, Response},
};

use crate::net_control::commands::ControlCommand;
use crate::net_dashboard::conn_stream::{
    ensure_dashboard_tls, tls_dir_for_config, tls_status, ConnStream, PrefixedConn,
};
use crate::net_dashboard::dashboard_ports::{LEGACY_HTTP_PORT, LEGACY_HTTPS_PORT};
use crate::net_dashboard::html::DASHBOARD_HTML;
use crate::net_dashboard::state::{CallEntry, DashboardState, DashboardStateInner, MsEntry, MsGroupState};
use crate::net_telemetry::TelemetryEvent;
use crate::net_telemetry::events::CellRfEvent;
use crate::tpg2200::build_tpg2200_callout_payload;

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

// Each WS connection registers a Sender here.
// broadcast() sends to all of them; dead connections are pruned automatically.
type WsBroadcastTx = crossbeam_channel::Sender<String>;
type WsClients = Arc<Mutex<Vec<WsBroadcastTx>>>;

/// Per-client outbound queue depth. A client that stops reading its socket fills this; once full,
/// broadcast() drops the connection (its sender errors and is pruned) rather than buffering without
/// bound. Sized for a burst of telemetry while a browser is briefly busy.
const WS_CLIENT_QUEUE: usize = 256;
/// Hard cap on concurrent WS clients, so many idle/non-draining connections can't grow memory and
/// thread count without bound. New upgrades past this are refused.
const WS_MAX_CLIENTS: usize = 64;
/// Cap concurrent dashboard HTTP(S) handler threads. Each request still gets its own thread; without
/// a ceiling, aggressive browser polls (LST DL, retries while the stack is wedged) can exhaust the
/// Pi and take down the whole process — which surfaces as ERR_CONNECTION_RESET / empty responses.
const DASH_MAX_CONN: usize = 32;

fn dash_conn_slots() -> &'static std::sync::atomic::AtomicUsize {
    static SLOTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    &SLOTS
}

fn dash_try_acquire_conn() -> bool {
    use std::sync::atomic::Ordering;
    let slots = dash_conn_slots();
    loop {
        let cur = slots.load(Ordering::Relaxed);
        if cur >= DASH_MAX_CONN {
            return false;
        }
        if slots
            .compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
}

fn dash_release_conn() {
    dash_conn_slots().fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
}

struct DashConnGuard;
impl Drop for DashConnGuard {
    fn drop(&mut self) {
        dash_release_conn();
    }
}

fn dash_reject_busy_plain(mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.write_all(
        b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 19\r\nContent-Type: text/plain\r\n\r\nDashboard busy\n",
    );
}

/// A TETRA SSI is a 24-bit identity. The PDU serializers write it with `write_bits(ssi, 24)`
/// (MAC-RESOURCE, D-SDS-DATA) whose range assertion aborts the calling thread on a wider value —
/// and every entity shares one un-isolated stack thread, so a single crafted dashboard request
/// would take the whole cell down, then take it down again on every retry. The DGNA path already
/// funnels through the same guard in MM; the dashboard is the highest-privilege plane, so it must
/// reject at its own boundary too rather than trust what reaches the serializer.
const MAX_TETRA_SSI: u32 = 0xFF_FFFF;

/// True when `n` is a usable TETRA SSI: 24 bits wide, non-zero (0 is the "no identity" sentinel
/// used across the call/SDS/DGNA paths).
fn is_valid_ssi(n: u64) -> bool {
    (1..=MAX_TETRA_SSI as u64).contains(&n)
}

/// Read `key` from a WS command body as a TETRA SSI. `None` when the field is missing, not a
/// number, zero, or wider than 24 bits — every one of which the caller must refuse.
fn json_ssi(v: &serde_json::Value, key: &str) -> Option<u32> {
    v.get(key).and_then(|x| x.as_u64()).filter(|n| is_valid_ssi(*n)).map(|n| n as u32)
}

/// Report a refused SSI to the operator. The WS command channel has no per-command reply, so the
/// log ring — which is broadcast to every connected browser — is where the rejection surfaces.
fn reject_ws_ssi(state: &DashboardState, cmd: &str, field: &str, raw: Option<&serde_json::Value>) {
    let raw = raw.map(|r| r.to_string()).unwrap_or_else(|| "missing".to_string());
    tracing::warn!(
        "Dashboard: refusing {} — {} {} out of range (must be 1..={})",
        cmd,
        field,
        raw,
        MAX_TETRA_SSI
    );
    if let Ok(mut s) = state.write() {
        s.push_log(
            "WARN",
            format!("{cmd} rejected: {field} {raw} out of range (must be 1..={MAX_TETRA_SSI})"),
        );
    }
}

fn cp1252_byte(ch: char) -> Option<u8> {
    match ch {
        '\u{20AC}' => Some(0x80),
        '\u{201A}' => Some(0x82),
        '\u{0192}' => Some(0x83),
        '\u{201E}' => Some(0x84),
        '\u{2026}' => Some(0x85),
        '\u{2020}' => Some(0x86),
        '\u{2021}' => Some(0x87),
        '\u{02C6}' => Some(0x88),
        '\u{2030}' => Some(0x89),
        '\u{0160}' => Some(0x8A),
        '\u{2039}' => Some(0x8B),
        '\u{0152}' => Some(0x8C),
        '\u{017D}' => Some(0x8E),
        '\u{2018}' => Some(0x91),
        '\u{2019}' => Some(0x92),
        '\u{201C}' => Some(0x93),
        '\u{201D}' => Some(0x94),
        '\u{2022}' => Some(0x95),
        '\u{2013}' => Some(0x96),
        '\u{2014}' => Some(0x97),
        '\u{02DC}' => Some(0x98),
        '\u{2122}' => Some(0x99),
        '\u{0161}' => Some(0x9A),
        '\u{203A}' => Some(0x9B),
        '\u{0153}' => Some(0x9C),
        '\u{017E}' => Some(0x9E),
        '\u{0178}' => Some(0x9F),
        ch if (ch as u32) <= 0xFF => Some(ch as u8),
        _ => None,
    }
}

fn looks_mojibake_token(s: &str) -> bool {
    s.contains('Ã') || s.contains('Â') || s.contains('â') || s.contains("ðŸ") || s.contains("Ãƒ")
}

fn fix_mojibake_token_once(token: &str) -> Option<String> {
    if !looks_mojibake_token(token) {
        return None;
    }
    let mut bytes = Vec::with_capacity(token.len());
    for ch in token.chars() {
        bytes.push(cp1252_byte(ch)?);
    }
    let decoded = std::str::from_utf8(&bytes).ok()?.to_string();
    if decoded == token || looks_mojibake_token(&decoded) && decoded.matches('Ã').count() >= token.matches('Ã').count() {
        return None;
    }
    Some(decoded)
}

fn normalize_mojibake_html(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut token = String::new();
    let flush = |out: &mut String, token: &mut String| {
        if token.is_empty() {
            return;
        }
        let mut cur = std::mem::take(token);
        for _ in 0..3 {
            let Some(next) = fix_mojibake_token_once(&cur) else { break };
            cur = next;
        }
        out.push_str(&cur);
    };
    for ch in src.chars() {
        if ch.is_whitespace() {
            flush(&mut out, &mut token);
            out.push(ch);
        } else {
            token.push(ch);
        }
    }
    flush(&mut out, &mut token);
    out
}

fn dashboard_html_body() -> &'static str {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| {
        normalize_mojibake_html(
            &DASHBOARD_HTML
                .replace("{{PRODUCT_NAME}}", tetra_core::PRODUCT_NAME)
                .replace("{{PRODUCT_VERSION}}", tetra_core::PRODUCT_VERSION)
                .replace("{{STACK_VERSION}}", tetra_core::PRODUCT_VERSION)
                .replace("{{VERSION_BASED_ON}}", tetra_core::VERSION_BASED_ON)
                .replace("{{PRODUCT_REPO_URL}}", tetra_core::PRODUCT_REPO_URL)
                .replace("{{PRODUCT_REPO_LABEL}}", tetra_core::PRODUCT_REPO_LABEL),
        )
    })
}

fn login_html_body() -> &'static str {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| {
        normalize_mojibake_html(
            &crate::net_dashboard::html::LOGIN_HTML
                .replace("{{PRODUCT_NAME}}", tetra_core::PRODUCT_NAME)
                .replace("{{PRODUCT_VERSION}}", tetra_core::PRODUCT_VERSION)
                .replace("{{STACK_VERSION}}", tetra_core::PRODUCT_VERSION)
                .replace("{{VERSION_BASED_ON}}", tetra_core::VERSION_BASED_ON)
                .replace("{{PRODUCT_REPO_URL}}", tetra_core::PRODUCT_REPO_URL)
                .replace("{{PRODUCT_REPO_LABEL}}", tetra_core::PRODUCT_REPO_LABEL),
        )
    })
}

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

    /// Drop every session (e.g. after a password change). Callers must re-login.
    fn invalidate_all(&mut self) {
        self.sessions.clear();
    }

    fn prune(&mut self) {
        let now = std::time::Instant::now();
        let ttl = self.ttl;
        self.sessions.retain(|_, seen| now.duration_since(*seen) < ttl);
    }
}

type SharedSessionStore = Arc<Mutex<SessionStore>>;
/// Live dashboard credentials — shared so a System-tab password change takes effect
/// without restarting the service.
type SharedAuth = Arc<RwLock<Option<(String, String)>>>;

/// Only the socket's direct peer participates in private HTTP admission. Forwarded headers are
/// intentionally irrelevant: the operator explicitly trusts these source networks, not a proxy.
fn private_http_configured(shared_config: &Option<tetra_config::bluestation::SharedConfig>) -> bool {
    shared_config
        .as_ref()
        .and_then(|cfg| cfg.config().dashboard.as_ref().map(|d| !d.http_allowed_networks.is_empty()))
        .unwrap_or(false)
}

fn private_http_credentials(
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    auth: &SharedAuth,
    peer: Option<std::net::IpAddr>,
) -> Option<(String, String)> {
    let peer = peer?;
    let config = shared_config.as_ref()?.config();
    if !config.dashboard.as_ref()?.permits_http_peer(peer) {
        return None;
    }
    // Runtime auth may have changed since the immutable startup config was parsed. Never let an
    // admitted HTTP connection fall through to the normal no-credentials/open-dashboard mode.
    auth.read()
        .ok()?
        .as_ref()
        .filter(|(user, password)| !user.trim().is_empty() && !password.is_empty())
        .cloned()
}

/// None means redirect. Some(true) means a mandatory authenticated HTTP connection; Some(false)
/// retains the existing TLS-unavailable fallback only when no private HTTP policy is configured.
fn primary_http_admission(
    tls_available: bool,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    auth: &SharedAuth,
    peer: Option<std::net::IpAddr>,
) -> Option<bool> {
    let scoped_http = private_http_configured(shared_config);
    if private_http_credentials(shared_config, auth, peer).is_some() {
        Some(true)
    } else if tls_available || scoped_http {
        None
    } else {
        Some(false)
    }
}

fn credentials_unchanged(auth: &SharedAuth, expected: &(String, String)) -> bool {
    auth.read()
        .ok()
        .and_then(|current| {
            current.as_ref().map(|(user, password)| {
                timing_safe_eq(user.as_bytes(), expected.0.as_bytes()) && timing_safe_eq(password.as_bytes(), expected.1.as_bytes())
            })
        })
        .unwrap_or(false)
}

/// An HTTP WebSocket must not retain management access after logout or an auth change. HTTPS keeps
/// its existing session behaviour; this guard applies only to explicitly admitted private HTTP.
struct PrivateHttpSession {
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    auth: SharedAuth,
    sessions: SharedSessionStore,
    peer: Option<std::net::IpAddr>,
    credentials: (String, String),
    token: String,
}

impl PrivateHttpSession {
    fn is_authorized(&self) -> bool {
        self.with_authorization(|| ())
    }

    /// Keep credential changes and logout ordered against a control dispatch. A check followed by
    /// an unlocked dispatch could otherwise execute a queued command after revocation completed.
    fn with_authorization(&self, action: impl FnOnce()) -> bool {
        let Some(peer) = self.peer else {
            return false;
        };
        let Some(config) = self.shared_config.as_ref().map(|config| config.config()) else {
            return false;
        };
        if !config.dashboard.as_ref().is_some_and(|dashboard| dashboard.permits_http_peer(peer)) {
            return false;
        }
        let Ok(current) = self.auth.read() else {
            return false;
        };
        if !current.as_ref().is_some_and(|(user, password)| {
            timing_safe_eq(user.as_bytes(), self.credentials.0.as_bytes())
                && timing_safe_eq(password.as_bytes(), self.credentials.1.as_bytes())
        }) {
            return false;
        }
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        if !sessions.validate(&self.token) {
            return false;
        }
        action();
        true
    }
}

/// Failed-login tracking, shared across every dashboard connection.
///
/// The previous throttle was a `thread::sleep(500ms)` on the connection that failed — per-connection
/// and nothing else. An attacker guessing in parallel paid nothing: each guess got its own thread
/// and its own independent sleep, so throughput scaled with concurrency. A single hit hands over
/// SDS, kick, DGNA, config, OTA and restart, so a failure has to cost across connections.
///
/// Counted per source IP: escalating delay first, then a hard lockout. The map is deliberately
/// bounded — an attacker rotating source addresses must not be able to grow it without limit, that
/// would just be a different DoS. Idle entries are pruned on access; if the map is still full the
/// new address is simply not tracked (it still pays the base delay).
struct LoginThrottle {
    entries: HashMap<std::net::IpAddr, FailedLogins>,
}

struct FailedLogins {
    count: u32,
    last: Instant,
}

/// Consecutive failures tolerated before the address is locked out entirely.
const LOGIN_LOCKOUT_AFTER: u32 = 5;
/// First lockout length; doubles per further failure up to `LOGIN_LOCKOUT_MAX`.
const LOGIN_LOCKOUT_BASE: std::time::Duration = std::time::Duration::from_secs(30);
const LOGIN_LOCKOUT_MAX: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// No failure for this long and the address starts from a clean slate (also the prune horizon —
/// deliberately longer than `LOGIN_LOCKOUT_MAX` so pruning can never drop an active lockout).
const LOGIN_FAIL_RESET: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// Hard cap on tracked addresses. Sized well above any plausible operator count.
const LOGIN_MAX_TRACKED_IPS: usize = 1024;
/// Per-attempt delay ceiling. Each connection is its own thread, so the sleep must stay short
/// enough that a flood of failures cannot pin threads — the lockout does the real work.
const LOGIN_DELAY_MAX: std::time::Duration = std::time::Duration::from_secs(5);

impl LoginThrottle {
    fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Lockout window earned by `count` consecutive failures, if any.
    fn lockout(count: u32) -> Option<std::time::Duration> {
        let over = count.checked_sub(LOGIN_LOCKOUT_AFTER)?;
        let factor = 1u32.checked_shl(over.min(16)).unwrap_or(u32::MAX);
        Some(LOGIN_LOCKOUT_BASE.saturating_mul(factor).min(LOGIN_LOCKOUT_MAX))
    }

    /// `Some(remaining)` while `ip` is locked out, `None` when it may attempt a login.
    fn locked_for(&mut self, ip: &std::net::IpAddr) -> Option<std::time::Duration> {
        self.prune();
        let entry = self.entries.get(ip)?;
        let lock = Self::lockout(entry.count)?;
        let elapsed = entry.last.elapsed();
        (elapsed < lock).then(|| lock - elapsed)
    }

    /// Record a failed attempt and return how long this connection should stall before replying.
    fn record_failure(&mut self, ip: std::net::IpAddr) -> std::time::Duration {
        self.prune();
        let has_room = self.entries.len() < LOGIN_MAX_TRACKED_IPS;
        let count = match self.entries.get_mut(&ip) {
            Some(e) => {
                e.count = e.count.saturating_add(1);
                e.last = Instant::now();
                e.count
            }
            None if has_room => {
                self.entries.insert(ip, FailedLogins { count: 1, last: Instant::now() });
                1
            }
            // Map full even after pruning — don't grow it; the base delay still applies.
            None => 1,
        };
        LOGIN_DELAY_MAX.min(std::time::Duration::from_millis(500).saturating_mul(count))
    }

    /// A successful login clears the address' history.
    fn clear(&mut self, ip: &std::net::IpAddr) {
        self.entries.remove(ip);
    }

    fn prune(&mut self) {
        self.entries.retain(|_, e| e.last.elapsed() < LOGIN_FAIL_RESET);
    }
}

type SharedLoginThrottle = Arc<Mutex<LoginThrottle>>;

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

/// Resolve the product git source directory for OTA updates.
///
/// Resolution order (first match wins):
///   1. `override_dir` from config ([dashboard].source_dir) — explicit user choice.
///   2. Walk up from `current_exe()` looking for a `.git` directory. This handles
///      the development case where the binary lives at `<src>/target/release/...`.
///   3. Well-known install paths: `/opt/ptbs`, then `/opt/bost-flowstation`, then
///      legacy FlowStation paths (`/opt/tetra-bluestation`, `/opt/flowstation`, …).
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

    // 3. Well-known install paths (PTBS first, then Bost legacy, then FlowStation).
    for candidate in &[
        tetra_core::PRODUCT_SRC_DIR,
        tetra_core::PRODUCT_SRC_DIR_LEGACY,
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
        "OTA update needs the {} git source tree to be present on this machine, \
         but none was found. You have two options:\n\
         \n\
         1) Clone the sources next to your binary:\n\
            git clone {} -b {} {}\n\
            (legacy path {} also works during the PTBS migration)\n\
            Then either move the binary into that tree, or set source_dir in config:\n\
            [dashboard]\n\
            source_dir = \"{}\"\n\
         \n\
         2) If your platform can't compile (e.g. Pi Zero), update manually by downloading \
         the latest release binary from GitHub.\n\
         \n\
         Paths tried: {}",
        tetra_core::PRODUCT_NAME_NEXT,
        tetra_core::PRODUCT_REPO_GIT,
        tetra_core::PRODUCT_OTA_BRANCH,
        tetra_core::PRODUCT_SRC_DIR,
        tetra_core::PRODUCT_SRC_DIR_LEGACY,
        tetra_core::PRODUCT_SRC_DIR,
        if tried.is_empty() { "(none)".to_string() } else { tried.join("; ") }
    ))
}

/// Reject an OTA source tree that is not safe to build from. `cargo build` runs any `build.rs` /
/// proc-macro in the tree as the service identity (often root), so before building we require the
/// tree to be neither group- nor world-writable (`mode & 0o022 == 0`).
///
/// Ownership rules:
/// - Non-root: tree must be owned by us (euid) or by root.
/// - Root: may build from a tree owned by the install service user (e.g. `bts` on
///   `/opt/bost-flowstation`) because the installer chowns the checkout for `cargo` as that user
///   while the systemd unit runs as root. Group/world-writability is still refused.
///
/// Unix-only; returns `Ok(())` on platforms without the metadata.
#[cfg(unix)]
fn source_tree_is_trusted(dir: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(dir).map_err(|e| format!("cannot stat source tree {}: {e}", dir.display()))?;
    // SAFETY: geteuid() is always successful and has no preconditions.
    let euid = unsafe { libc::geteuid() };
    let owner = meta.uid();
    let owner_ok = owner == euid || owner == 0 || euid == 0;
    if !owner_ok {
        return Err(format!(
            "source tree {} is owned by uid {} (we run as uid {}); refusing to build from a tree we don't own",
            dir.display(),
            owner,
            euid
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!(
            "source tree {} is group/world-writable (mode {:o}); refusing to build from a tree others can modify",
            dir.display(),
            meta.mode() & 0o7777
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn source_tree_is_trusted(_dir: &std::path::Path) -> Result<(), String> {
    Ok(())
}

/// Resolve the login name for the owner of `dir` (Unix). Used so OTA can run `cargo` as that
/// user instead of root — matching the installer (`sudo -u bts cargo build`) and avoiding
/// root-owned `target/` files + OOM from over-parallel root builds on a Pi.
#[cfg(unix)]
fn source_tree_owner_name(dir: &std::path::Path) -> Option<String> {
    use std::ffi::CStr;
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(dir).ok()?;
    let uid = meta.uid();
    // SAFETY: getpwuid returns a pointer into a static buffer; we copy out immediately.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() || (*pw).pw_name.is_null() {
            return None;
        }
        CStr::from_ptr((*pw).pw_name).to_str().ok().map(|s| s.to_string())
    }
}

#[cfg(not(unix))]
fn source_tree_owner_name(_dir: &std::path::Path) -> Option<String> {
    None
}

/// Ensure the OTA source tree (including `Cargo.lock` and `target/`) is owned by `user`
/// before a non-root build. Git sync runs as the service identity (often root), so a
/// `reset --hard` can leave root-owned sources; cargo as `bts` then fails with
/// `Permission denied` on `Cargo.lock`.
fn fix_source_tree_ownership(src_dir: &std::path::Path, user: &str, update: &SharedUpdateState) {
    update.lock().unwrap().append(&format!(
        "Ensuring {} is owned by {} (avoids Permission denied on Cargo.lock / target after root git sync)",
        src_dir.display(),
        user
    ));
    let Some(path) = src_dir.to_str() else {
        update
            .lock()
            .unwrap()
            .append("WARNING: source dir path is not valid UTF-8; skipping chown");
        return;
    };
    let status = std::process::Command::new("chown")
        .args(["-R", user, path])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => update.lock().unwrap().append(&format!("WARNING: chown exited with {s}")),
        Err(e) => update.lock().unwrap().append(&format!("WARNING: chown failed: {e}")),
    }
}

fn log_host_memory(update: &SharedUpdateState) {
    if let Ok(mem) = std::fs::read_to_string("/proc/meminfo") {
        let mut total = None;
        let mut avail = None;
        for line in mem.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total = rest.split_whitespace().next().map(|s| s.to_string());
            }
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                avail = rest.split_whitespace().next().map(|s| s.to_string());
            }
        }
        if let (Some(t), Some(a)) = (total, avail) {
            update.lock().unwrap().append(&format!(
                "Host memory: {a} kB available / {t} kB total (OTA uses -j 1 to reduce OOM risk)"
            ));
        }
    }
}

/// Ensure outerplane `libtetra-codec` is installed (clone+cmake to /usr/local) so LST voice
/// and optional Asterisk SIP can link. Prefer the tree script; fall back to a minimal inline build.
/// Returns true when the library is linkable afterwards.
///
/// Note: cannot use the `log!` macro from `run_update` (it is scoped there); append to the
/// shared update log the same way `log_host_memory` does.
fn ensure_tetra_codec_installed(src_dir: &std::path::Path, update: &SharedUpdateState) -> bool {
    let ota_log = |msg: String| {
        tracing::info!("UPDATE: {}", msg);
        update.lock().unwrap().append(&msg);
    };

    if std::env::var("BOST_SKIP_TETRA_CODEC")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        ota_log("BOST_SKIP_TETRA_CODEC set — not installing libtetra-codec".into());
        return tetra_codec_lib_present();
    }
    if tetra_codec_lib_present() {
        ota_log("libtetra-codec already present — OK".into());
        return true;
    }

    ota_log("--- Installing tetra-codec (outerplane ACELP for LST voice) ---".into());
    let script = src_dir.join("contrib/install/install-tetra-codec.sh");
    if script.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&script) {
                let mut perms = meta.permissions();
                perms.set_mode(perms.mode() | 0o111);
                let _ = std::fs::set_permissions(&script, perms);
            }
        }
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(&script);
        cmd.env("DEBIAN_FRONTEND", "noninteractive");
        // Prefer single-job cmake on Pi during OTA (memory).
        if std::env::var_os("BOST_TETRA_CODEC_JOBS").is_none() {
            cmd.env("BOST_TETRA_CODEC_JOBS", "1");
        }
        let label = format!("$ bash {}", script.display());
        if stream_cmd(update, label, cmd).is_none() {
            ota_log(
                "WARN: install-tetra-codec.sh failed — LST voice will stay signalling-only".into(),
            );
            return false;
        }
        let ok = tetra_codec_lib_present();
        if ok {
            ota_log("tetra-codec installed — LST voice link enabled".into());
        } else {
            ota_log(
                "WARN: install-tetra-codec.sh finished but lib still not detected".into(),
            );
        }
        return ok;
    }

    ota_log(format!(
        "WARN: {} missing — cannot auto-install codec (re-run OTA after sources include the script)",
        script.display()
    ));
    false
}

/// True when `libtetra-codec` is linkable on this host (ACELP for LST / optional SIP).
/// Does **not** enable Asterisk SIP — only the Cargo feature that links the shared library.
fn tetra_codec_lib_present() -> bool {
    use std::path::Path;

    // Explicit override (path to .so) or force flag for OTA on hosts with unusual layouts.
    if std::env::var("BOST_OTA_FORCE_ASTERISK")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return true;
    }
    if let Ok(p) = std::env::var("BOST_TETRA_CODEC_LIB") {
        if !p.is_empty() && Path::new(&p).is_file() {
            return true;
        }
    }

    // Same discovery path as crates/tetra-entities/build.rs (preferred on Pi installs).
    if let Ok(st) = std::process::Command::new("pkg-config")
        .args(["--exists", "tetra-codec"])
        .status()
    {
        if st.success() {
            return true;
        }
    }
    if let Ok(out) = std::process::Command::new("pkg-config")
        .args(["--libs", "tetra-codec"])
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.contains("tetra-codec") || text.contains("-ltetra-codec") {
                return true;
            }
        }
    }

    const CANDIDATES: &[&str] = &[
        "/usr/local/lib/libtetra-codec.so",
        "/usr/lib/libtetra-codec.so",
        "/usr/lib/aarch64-linux-gnu/libtetra-codec.so",
        "/usr/lib/arm-linux-gnueabihf/libtetra-codec.so",
        "/opt/tetra/lib/libtetra-codec.so",
        "/opt/bost-flowstation/lib/libtetra-codec.so",
        "/usr/local/lib/libtetra-codec.so.1",
        "/usr/lib/libtetra-codec.so.1",
    ];
    if CANDIDATES.iter().any(|p| Path::new(p).is_file()) {
        return true;
    }
    // Last resort: ldconfig cache (Pi OS / Debian).
    if let Ok(out) = std::process::Command::new("ldconfig").args(["-p"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("libtetra-codec.so") {
            return true;
        }
    }
    false
}

/// Build `Command` for `cargo build --release -p bluestation-bs`, preferably as the
/// source-tree owner when we are root. When `with_tetra_codec`, adds `--features asterisk`
/// (links libtetra-codec only; does not enable SIP).
fn cargo_build_command(
    cargo: &std::path::Path,
    src_dir: &std::path::Path,
    update: &SharedUpdateState,
    with_tetra_codec: bool,
) -> std::process::Command {
    use std::path::PathBuf;

    let jobs = std::env::var("BOST_OTA_JOBS").unwrap_or_else(|_| "1".into());
    // Feature name is historical: links libtetra-codec for ACELP (LST + optional Asterisk SIP).
    let mut cargo_args: Vec<&str> = vec![
        "build",
        "--release",
        "-p",
        "bluestation-bs",
        "-j",
        jobs.as_str(),
    ];
    if with_tetra_codec {
        cargo_args.push("--features");
        cargo_args.push("asterisk");
    }

    let mut path_env = std::env::var("PATH").unwrap_or_default();
    let mut cargo_home: Option<PathBuf> = None;
    let mut rustup_home: Option<PathBuf> = None;
    let mut home_env: Option<PathBuf> = None;

    if cargo.is_absolute() {
        if let Some(bin) = cargo.parent().filter(|p| !p.as_os_str().is_empty()) {
            path_env = if path_env.is_empty() {
                bin.display().to_string()
            } else {
                format!("{}:{}", bin.display(), path_env)
            };
            if let Some(ch) = bin.parent() {
                cargo_home = Some(ch.to_path_buf());
                if let Some(home) = ch.parent() {
                    home_env = Some(home.to_path_buf());
                    let ru = home.join(".rustup");
                    if ru.is_dir() {
                        rustup_home = Some(ru);
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    let euid = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let euid = 0u32;

    let owner = source_tree_owner_name(src_dir);
    let run_as_owner = euid == 0 && owner.as_deref().is_some_and(|u| u != "root");

    let mut build = if run_as_owner {
        let user = owner.unwrap();
        fix_source_tree_ownership(src_dir, &user, update);
        update.lock().unwrap().append(&format!(
            "Running cargo as user '{}' (source tree owner)",
            user
        ));
        // runuser is present on Raspberry Pi OS / Debian; falls back to sudo -n -u.
        let mut cmd = if std::path::Path::new("/usr/sbin/runuser").is_file()
            || std::path::Path::new("/sbin/runuser").is_file()
        {
            let mut c = std::process::Command::new("runuser");
            c.args(["-u", &user, "--"]);
            c
        } else {
            // -E keeps CARGO_HOME / RUSTUP_HOME / PATH we set on this Command.
            let mut c = std::process::Command::new("sudo");
            c.args(["-n", "-E", "-u", &user, "--"]);
            c
        };
        cmd.arg(cargo);
        cmd.args(&cargo_args);
        cmd
    } else {
        let mut cmd = std::process::Command::new(cargo);
        cmd.args(&cargo_args);
        cmd
    };

    build.current_dir(src_dir);
    build.env("PATH", &path_env);
    if let Some(h) = home_env {
        build.env("HOME", &h);
        build.env("USER", h.file_name().and_then(|s| s.to_str()).unwrap_or("bts"));
    }
    if let Some(ch) = cargo_home {
        build.env("CARGO_HOME", ch);
    }
    if let Some(ru) = rustup_home {
        build.env("RUSTUP_HOME", ru);
    }
    // Avoid rustc waiting on a missing sccache / mold etc. under the service environment.
    build.env_remove("RUSTC_WRAPPER");
    build.env_remove("CARGO_TARGET_DIR");
    build
}

/// Remove zero-byte loose objects left by a killed/OOM mid-fetch. Those make the next
/// `git fetch` fail with "object file … is empty" / "invalid index-pack", which the UI
/// used to mislabel as network/TLS.
fn purge_empty_git_objects(src_dir: &std::path::Path) -> u32 {
    let objects = match std::fs::read_dir(src_dir.join(".git/objects")) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let mut purged = 0u32;
    for entry in objects.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if let Ok(inner) = std::fs::read_dir(entry.path()) {
            for obj in inner.flatten() {
                if obj.metadata().map(|m| m.len() == 0).unwrap_or(false) {
                    let _ = std::fs::remove_file(obj.path());
                    purged += 1;
                }
            }
        }
    }
    purged
}

fn update_log_looks_like_corrupt_git(log: &str) -> bool {
    let l = log.to_ascii_lowercase();
    (l.contains("object file") && l.contains("is empty"))
        || l.contains("invalid index-pack")
        || l.contains("corrupt") && l.contains(".git")
        || l.contains("zlib:")
}

/// Spawn `cmd`, stream stdout+stderr line-by-line into `update.log`, and return
/// collected stdout on success (or None after marking the update failed).
fn stream_cmd(
    update: &std::sync::Arc<std::sync::Mutex<UpdateState>>,
    label: String,
    mut cmd: std::process::Command,
) -> Option<String> {
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

    // Heartbeat while cargo/rustc is quiet for a long stretch (e.g. Compiling tetra-entities
    // on a Pi). Without this the dashboard freezes at ~50% and looks hung.
    let last_activity = Arc::new(AtomicU64::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    ));
    let stop_hb = Arc::new(AtomicBool::new(false));
    let hb = {
        let u = Arc::clone(update);
        let last = Arc::clone(&last_activity);
        let stop = Arc::clone(&stop_hb);
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(20));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let quiet = now.saturating_sub(last.load(Ordering::Relaxed));
                if quiet >= 25 {
                    let mins = started.elapsed().as_secs() / 60;
                    let secs = started.elapsed().as_secs() % 60;
                    u.lock().unwrap().append(&format!(
                        "… still working ({mins}m {secs:02}s elapsed — long compiles on Pi are normal)"
                    ));
                }
            }
        })
    };

    let touch = |last: &AtomicU64| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        last.store(now, Ordering::Relaxed);
    };

    // stderr on a side thread (cargo writes its progress there), stdout collected on this one.
    let err_handle = child.stderr.take().map(|err| {
        let u = Arc::clone(update);
        let last = Arc::clone(&last_activity);
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                touch(&last);
                u.lock().unwrap().append(&line);
            }
        })
    });
    let mut collected = String::new();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            touch(&last_activity);
            update.lock().unwrap().append(&line);
            collected.push_str(&line);
            collected.push('\n');
        }
    }
    if let Some(h) = err_handle {
        let _ = h.join();
    }
    stop_hb.store(true, Ordering::Relaxed);
    // Do not join the heartbeat thread — it may be mid-sleep; joining would stall
    // the success path for up to the heartbeat interval.
    let _ = hb;
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
/// interactive SSH shell finds it (FH-BUG-037).
///
/// Resolution order:
///   1. `$CARGO` (explicit)
///   2. `$HOME/.cargo/bin/cargo`
///   3. Home of `$BOST_SERVICE_USER` / `$FLOWSTATION_SERVICE_USER` (installer default: `bts`)
///   4. Home of the OTA source-tree owner (install chowns `/opt/bost-flowstation` to `bts`)
///   5. Well-known system / root paths
///   6. Bare `cargo` on PATH
///
/// We do **not** scan every `/home/*` directory — only the configured service user and the
/// already-trusted source tree owner.
fn find_cargo(src_dir: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Path, PathBuf};

    fn is_cargo(p: &Path) -> bool {
        p.is_file()
    }

    if let Ok(c) = std::env::var("CARGO") {
        let p = PathBuf::from(c);
        if is_cargo(&p) {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = Path::new(&home).join(".cargo/bin/cargo");
        if is_cargo(&p) {
            return p;
        }
    }

    for key in ["BOST_SERVICE_USER", "FLOWSTATION_SERVICE_USER"] {
        if let Ok(user) = std::env::var(key) {
            let user = user.trim();
            if !user.is_empty() {
                let p = PathBuf::from(format!("/home/{user}/.cargo/bin/cargo"));
                if is_cargo(&p) {
                    return p;
                }
            }
        }
    }

    #[cfg(unix)]
    {
        use std::ffi::CStr;
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(src_dir) {
            let uid = meta.uid();
            // SAFETY: getpwuid returns a pointer into a static buffer; we copy out immediately.
            let home = unsafe {
                let pw = libc::getpwuid(uid);
                if pw.is_null() || (*pw).pw_dir.is_null() {
                    None
                } else {
                    CStr::from_ptr((*pw).pw_dir).to_str().ok().map(|s| s.to_string())
                }
            };
            if let Some(home) = home {
                let p = Path::new(&home).join(".cargo/bin/cargo");
                if is_cargo(&p) {
                    return p;
                }
            }
        }
    }

    // Default Bost install user + root/system locations.
    for cand in [
        "/home/bts/.cargo/bin/cargo",
        "/usr/local/cargo/bin/cargo",
        "/root/.cargo/bin/cargo",
        "/usr/local/bin/cargo",
        "/usr/bin/cargo",
    ] {
        let p = PathBuf::from(cand);
        if is_cargo(&p) {
            return p;
        }
    }

    PathBuf::from("cargo")
}

/// Install the freshly built release binary over the running ExecStart path (typically
/// `/usr/local/bin/bluestation-bs`). Without this, `cargo build` updates `target/release/` but the
/// systemd unit keeps running the old installed copy after restart.
///
/// Bridge (PTBS): also refreshes `/usr/local/bin/ptbs` (copy or symlink) so future units can
/// ExecStart the new name while legacy `bluestation-bs` keeps working.
fn install_built_binary(src_dir: &std::path::Path, update: &SharedUpdateState) -> bool {
    let built = src_dir.join("target/release").join(tetra_core::PRODUCT_BIN_NAME_LEGACY);
    if !built.is_file() {
        update.lock().unwrap().append(&format!(
            "ERROR: built binary not found at {}",
            built.display()
        ));
        update.lock().unwrap().finish(false);
        return false;
    }

    let dest = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            update.lock().unwrap().append(&format!("ERROR: cannot resolve running binary path: {e}"));
            update.lock().unwrap().finish(false);
            return false;
        }
    };

    // Prefer the well-known install path when the running exe is already that path, or when
    // current_exe points inside target/ (dev runs). Always refresh /usr/local/bin when present.
    let legacy_install = std::path::PathBuf::from(format!(
        "/usr/local/bin/{}",
        tetra_core::PRODUCT_BIN_NAME_LEGACY
    ));
    let next_install =
        std::path::PathBuf::from(format!("/usr/local/bin/{}", tetra_core::PRODUCT_BIN_NAME));
    let mut targets: Vec<std::path::PathBuf> = if dest == legacy_install || dest == next_install {
        vec![dest.clone()]
    } else if legacy_install.exists() {
        vec![legacy_install.clone(), dest.clone()]
    } else {
        vec![dest.clone()]
    };
    // Always stage the future binary name alongside the legacy one when we can write /usr/local/bin.
    if !targets.iter().any(|p| p == &next_install)
        && (next_install.exists()
            || legacy_install.exists()
            || next_install.parent().is_some_and(|d| d.is_dir()))
    {
        targets.push(next_install.clone());
    }

    for dest in targets {
        let tmp = dest.with_extension("ota-new");
        update.lock().unwrap().append(&format!(
            "Installing {} → {}",
            built.display(),
            dest.display()
        ));
        if let Err(e) = std::fs::copy(&built, &tmp) {
            update.lock().unwrap().append(&format!("ERROR: copy to {} failed: {e}", tmp.display()));
            update.lock().unwrap().finish(false);
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
        }
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::remove_file(&tmp);
            update.lock().unwrap().append(&format!("ERROR: install to {} failed: {e}", dest.display()));
            update.lock().unwrap().finish(false);
            return false;
        }
    }
    true
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

/// Refuse to align an OTA checkout while tracked, staged, submodule, or untracked files
/// would be lost. Ignored build output is deliberately allowed.
fn ota_worktree_is_clean(src_dir: &std::path::Path) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(src_dir)
        .args(["status", "--porcelain=v1", "--untracked-files=all", "--ignore-submodules=none"])
        .output()
        .map_err(|e| format!("cannot inspect OTA source changes: {e}"))?;
    if !output.status.success() {
        return Err("cannot inspect OTA source changes; refusing to change the checkout".into());
    }
    if !output.stdout.is_empty() {
        return Err(
            "local source changes or untracked files exist; preserve and integrate them before OTA. No sources were changed and RF remains online"
                .into(),
        );
    }
    Ok(())
}

/// `checkout -B` also overwrites an existing channel branch, which may differ from HEAD.
/// Require both histories to be contained in the fetched channel before resetting either.
fn ota_local_commits_are_preserved(src_dir: &std::path::Path, remote_ref: &str, branch: &str) -> Result<(), String> {
    let target_branch = format!("refs/heads/{branch}");
    let target_exists = std::process::Command::new("git")
        .arg("-C")
        .arg(src_dir)
        .args(["show-ref", "--verify", "--quiet", &target_branch])
        .status()
        .map_err(|e| format!("cannot inspect OTA channel branch: {e}"))?;
    if !target_exists.success() && target_exists.code() != Some(1) {
        return Err("cannot inspect OTA channel branch; refusing to change the checkout".into());
    }
    let mut refs = vec!["HEAD"];
    if target_exists.success() {
        refs.push(&target_branch);
    }
    for local_ref in refs {
        let range = format!("{remote_ref}..{local_ref}");
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(src_dir)
            .args(["rev-list", "--count", &range])
            .output()
            .map_err(|e| format!("cannot compare OTA source history: {e}"))?;
        if !output.status.success() {
            return Err("cannot compare OTA source history; refusing to change the checkout".into());
        }
        let count = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .map_err(|_| "invalid OTA history comparison; refusing to change the checkout".to_string())?;
        if count != 0 {
            return Err(format!(
                "{local_ref} contains {count} local commit(s) absent from {remote_ref}; preserve and integrate them before OTA. No sources were changed and RF remains online"
            ));
        }
    }
    Ok(())
}

/// Git otherwise overwrites ignored files when the destination starts tracking their path.
fn ota_checkout_command(src_dir: &std::path::Path, branch: &str, remote_ref: &str) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(src_dir).args(["checkout", "--no-overwrite-ignore", "-B", branch, remote_ref]);
    command.current_dir(src_dir);
    command
}

/// Once the SDR has been closed, only a fresh service process can reopen it. Any early
/// return or unwind after that point must schedule recovery, including failed builds.
struct OtaRfRecoveryGuard<F: FnOnce()> {
    recover: Option<F>,
}

impl<F: FnOnce()> OtaRfRecoveryGuard<F> {
    fn new(recover: F) -> Self {
        Self { recover: Some(recover) }
    }

    fn disarm(&mut self) {
        self.recover = None;
    }
}

impl<F: FnOnce()> Drop for OtaRfRecoveryGuard<F> {
    fn drop(&mut self) {
        if let Some(recover) = self.recover.take() {
            recover();
        }
    }
}

/// Run git sync + cargo build --release in a background thread.
/// Steps:
///   1. Resolve source dir (config override -> walk-up -> well-known paths -> CWD)
///   2. Validate it is a git repository
///   3. Read OTA channel from config → branch (`stable`/`main`, `beta`/`beta`)
///   4. git fetch + `checkout -B` + `reset --hard origin/<branch>` (preserves `target/`)
///   5. Skip rebuild/install/restart when the running binary already matches HEAD
///   6. cargo build --release (incremental; never `cargo clean`)
///   7. install binary (+ future `ptbs` name) + systemctl restart
fn run_update(update: SharedUpdateState, config_path: String, source_dir_override: Option<String>) {
    macro_rules! log {
        ($update:expr, $($arg:tt)*) => {{
            let line = format!($($arg)*);
            tracing::info!("UPDATE: {}", line);
            $update.lock().unwrap().append(&line);
        }};
    }

    log!(update, "=== {} OTA Update ===", tetra_core::PRODUCT_NAME);
    log!(
        update,
        "Migration notice: {} ({}) is coming. This bridge tracks stable→git branch `main` \
         (legacy `bost` still receives this bridge so field units can update once).",
        tetra_core::PRODUCT_NAME_NEXT,
        tetra_core::PRODUCT_NAME_NEXT_LONG
    );

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

    // Refuse to build from a tree we don't own or that others can write into. `cargo build` runs any
    // build.rs / proc-macro in this tree as the service identity (often root), so an attacker-writable
    // or attacker-owned tree would mean arbitrary code execution. This applies equally to an
    // auto-detected clone and a dashboard-supplied source_dir.
    if let Err(e) = source_tree_is_trusted(&src_dir) {
        log!(update, "ERROR: {}", e);
        update.lock().unwrap().finish(false);
        return;
    }

    /// Run a command, streaming stdout+stderr into the log; return collected stdout or None.
    fn run_cmd_output(update: &SharedUpdateState, program: &str, args: &[&str], dir: &std::path::Path) -> Option<String> {
        let label = format!("$ {} {}", program, args.join(" "));
        tracing::info!("UPDATE: {}", label);
        let mut cmd = std::process::Command::new(program);
        cmd.args(args).current_dir(dir);
        stream_cmd(update, label, cmd)
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

    // Reject local edits before changing origin or fetching. A failed preflight must not
    // take a working cell off the air or hide experimental station changes.
    if let Err(e) = ota_worktree_is_clean(&src_dir) {
        log!(update, "ERROR: {}", e);
        update.lock().unwrap().finish(false);
        return;
    }

    // Step 3: channel → branch, point origin at the Bost repo, fetch.
    let channel = crate::net_dashboard::ota_channel::read_ota_channel(&config_path);
    let ota_branch = tetra_core::ota_branch_for_channel(&channel);
    let ota_url = tetra_core::PRODUCT_REPO_GIT;
    let remote_ref = format!("origin/{}", ota_branch);
    log!(
        update,
        "OTA channel={} → branch {}",
        channel,
        ota_branch
    );
    log!(
        update,
        "--- Ensuring OTA remote {} (branch {}) ---",
        ota_url,
        ota_branch
    );
    let current_origin = run_cmd_output(&update, "git", &["-C", src_str, "remote", "get-url", "origin"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if current_origin.is_empty() {
        if run_cmd_output(
            &update,
            "git",
            &["-C", src_str, "remote", "add", "origin", ota_url],
            &src_dir,
        )
        .is_none()
        {
            return;
        }
    } else if current_origin != ota_url && !tetra_core::is_product_repo_url(&current_origin) {
        log!(
            update,
            "origin was '{}' — switching to {}",
            current_origin,
            ota_url
        );
        if run_cmd_output(
            &update,
            "git",
            &["-C", src_str, "remote", "set-url", "origin", ota_url],
            &src_dir,
        )
        .is_none()
        {
            return;
        }
    } else if current_origin != ota_url {
        // Same product repo (bost-flowstation or future ptbs), different URL form — normalize.
        let _ = run_cmd_output(
            &update,
            "git",
            &["-C", src_str, "remote", "set-url", "origin", ota_url],
            &src_dir,
        );
    }

    log!(update, "--- Checking remote for updates ---");
    // Explicit refspec so `origin/<branch>` always exists. Plain `git fetch origin <branch>`
    // often only updates FETCH_HEAD (seen on Pi clones that never tracked `beta` before),
    // which then makes `rev-parse origin/beta` fail with exit 128.
    let purged0 = purge_empty_git_objects(&src_dir);
    if purged0 > 0 {
        log!(
            update,
            "Removed {purged0} empty .git object(s) from a previous interrupted fetch."
        );
    }
    // Retry a few times: Pi ↔ GitHub over HTTPS occasionally fails with GnuTLS handshake errors.
    // On "empty object" / index-pack corruption, purge again mid-loop before the next attempt.
    let fetch_refspec = format!("+refs/heads/{0}:refs/remotes/origin/{0}", ota_branch);
    let fetch_ok = {
        const FETCH_ATTEMPTS: u32 = 3;
        let mut ok = false;
        let mut saw_corrupt = false;
        for attempt in 1..=FETCH_ATTEMPTS {
            if attempt > 1 {
                let wait_s = 5 * (attempt - 1);
                log!(
                    update,
                    "Fetch failed — retrying in {wait_s}s (attempt {attempt}/{FETCH_ATTEMPTS})…"
                );
                std::thread::sleep(std::time::Duration::from_secs(wait_s as u64));
                let log_snap = update.lock().unwrap().log.clone();
                if update_log_looks_like_corrupt_git(&log_snap) {
                    saw_corrupt = true;
                    let n = purge_empty_git_objects(&src_dir);
                    if n > 0 {
                        log!(update, "Purged {n} empty .git object(s) before retry.");
                    }
                }
            }
            if run_cmd_output(
                &update,
                "git",
                &["-C", src_str, "fetch", "origin", &fetch_refspec],
                &src_dir,
            )
            .is_some()
            {
                ok = true;
                break;
            }
            let log_snap = update.lock().unwrap().log.clone();
            if update_log_looks_like_corrupt_git(&log_snap) {
                saw_corrupt = true;
            }
        }
        if !ok {
            if saw_corrupt {
                log!(
                    update,
                    "ERROR: git fetch failed — local .git looks corrupt (empty objects / bad pack), not a generic network error. Empty objects were purged automatically; tap Actualizar again. If it keeps failing, on the Pi run: find /opt/bost-flowstation/.git/objects -type f -empty -delete && git -C /opt/bost-flowstation fetch origin"
                );
            } else {
                log!(
                    update,
                    "ERROR: git fetch failed after {FETCH_ATTEMPTS} attempts (network, DNS, or GitHub TLS on the Pi). Check connectivity to github.com, then retry. If the log shows 'object file … is empty', the local .git is corrupt — see the corrupt-.git message above."
                );
            }
        }
        ok
    };
    if !fetch_ok {
        return;
    }

    let local_branch = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "--abbrev-ref", "HEAD"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let local_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "HEAD"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if local_commit.is_empty() {
        log!(update, "ERROR: cannot read local HEAD after fetch.");
        return;
    }
    let remote_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", &remote_ref], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if remote_commit.is_empty() {
        log!(
            update,
            "ERROR: {} missing after fetch (ref not updated). Retry OTA, or on the Pi: git -C /opt/bost-flowstation ls-remote origin {}",
            remote_ref,
            ota_branch
        );
        return;
    }

    log!(update, "Local  branch: {}", if local_branch.is_empty() { "?" } else { &local_branch });
    log!(update, "Local  commit: {}", &local_commit[..local_commit.len().min(12)]);
    log!(update, "Remote commit: {}", &remote_commit[..remote_commit.len().min(12)]);

    if let Err(e) = ota_local_commits_are_preserved(&src_dir, &remote_ref, ota_branch) {
        log!(update, "ERROR: {}", e);
        update.lock().unwrap().finish(false);
        return;
    }
    // Fetch can take time; refuse edits made while it was running too.
    if let Err(e) = ota_worktree_is_clean(&src_dir) {
        log!(update, "ERROR: {}", e);
        update.lock().unwrap().finish(false);
        return;
    }

    let tree_moved = local_commit != remote_commit || local_branch != ota_branch;
    if tree_moved {
        let range = format!("HEAD..{}", remote_ref);
        let _ = run_cmd_output(&update, "git", &["-C", src_str, "log", "--oneline", &range], &src_dir);

        let backup_path = format!("{}.bak", config_path);
        match atomic_copy(&config_path, &backup_path) {
            Ok(_) => log!(update, "Config backed up → {}", backup_path),
            Err(e) => log!(update, "WARNING: config backup failed: {} (continuing)", e),
        }
    }

    // Step 4: align only after verifying that no local edits or commits will be lost.
    // `target/` is never deleted; divergent history requires explicit manual integration.
    log!(
        update,
        "--- Aligning sources to {} (reset --hard; keeps target/ for incremental builds) ---",
        remote_ref
    );
    if stream_cmd(
        &update,
        format!("$ git -C {src_str} checkout --no-overwrite-ignore -B {ota_branch} {remote_ref}"),
        ota_checkout_command(&src_dir, ota_branch, &remote_ref),
    )
    .is_none()
    {
        return;
    }
    if run_cmd_output(
        &update,
        "git",
        &["-C", src_str, "reset", "--hard", &remote_ref],
        &src_dir,
    )
    .is_none()
    {
        return;
    }

    // Step 5: decide whether to rebuild.
    // Always try to install tetra-codec first — even when git HEAD already matches the binary,
    // an existing install may still be signalling-only (no ACELP link). Without this, a second
    // OTA would say "Already up to date" and never enable LST voice.
    let binary_has_codec = cfg!(feature = "asterisk");
    let skip_codec = std::env::var("BOST_SKIP_TETRA_CODEC")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    // Always attempt install when this binary lacks ACELP — even if git already matches.
    let codec_ok = if !binary_has_codec && !skip_codec {
        ensure_tetra_codec_installed(&src_dir, &update)
    } else {
        tetra_codec_lib_present() || binary_has_codec
    };
    let need_voice_rebuild = !binary_has_codec && !skip_codec && codec_ok;

    let repo_head = remote_commit.as_str();
    let binary_current = binary_built_from(tetra_core::GIT_HASH, repo_head);
    let sources_match_binary = matches!(binary_current, Some(true))
        || (matches!(binary_current, None) && !tree_moved);

    if sources_match_binary && !need_voice_rebuild {
        if !binary_has_codec && !skip_codec {
            log!(
                update,
                "WARN: libtetra-codec still missing after install attempt — LST voice unavailable. Retry OTA when the Pi has network access to github.com/outerplane/tetra-codec."
            );
        }
        log!(
            update,
            "Already up to date — running {} matches {}@{}{}.",
            tetra_core::STACK_VERSION,
            ota_branch,
            &repo_head[..repo_head.len().min(12)],
            if binary_has_codec {
                " (voice codec linked)"
            } else if skip_codec {
                " (BOST_SKIP_TETRA_CODEC)"
            } else {
                " (no libtetra-codec — signalling only)"
            }
        );
        update.lock().unwrap().finish(true);
        return;
    }

    if need_voice_rebuild && sources_match_binary {
        log!(
            update,
            "Sources match {}, but this binary has no ACELP link — rebuilding with --features asterisk for LST voice.",
            ota_branch
        );
    } else if matches!(binary_current, Some(false)) {
        log!(
            update,
            "Running binary ({}) does not match {} — rebuilding (incremental).",
            tetra_core::STACK_VERSION,
            ota_branch
        );
    } else if matches!(binary_current, None) && tree_moved {
        log!(
            update,
            "Sources moved on {} but build hash is not verifiable — rebuilding to be safe.",
            ota_branch
        );
    }

    // Step 6: build. cargo lives in ~/.cargo/bin, which the systemd service PATH usually omits, so
    // resolve it explicitly (FH-BUG-037). Run as the source-tree owner with -j 1 to avoid
    // root-owned target/ stalls and OOM on Pi (compiling tetra-entities html.rs is heavy).
    // Output is streamed live so a long compile shows progress instead of looking hung (FH-BUG-035).
    // Never run `cargo clean` — incremental `target/` is intentional.
    log!(update, "--- cargo build --release ---");
    let cargo = find_cargo(&src_dir);
    if cargo == std::path::Path::new("cargo") {
        log!(
            update,
            "ERROR: cargo not found. Install rustup for the service user, or set Environment=CARGO=/home/<user>/.cargo/bin/cargo on the systemd unit."
        );
        update.lock().unwrap().finish(false);
        return;
    }
    log!(update, "Using cargo: {}", cargo.display());
    log_host_memory(&update);
    let jobs = std::env::var("BOST_OTA_JOBS").unwrap_or_else(|_| "1".into());
    let with_codec = tetra_codec_lib_present();
    if with_codec {
        log!(
            update,
            "libtetra-codec / pkg-config tetra-codec found — building with --features asterisk (ACELP for LST; does not enable SIP)"
        );
        if let Ok(out) = std::process::Command::new("pkg-config")
            .args(["--libs", "tetra-codec"])
            .output()
        {
            if out.status.success() {
                log!(
                    update,
                    "pkg-config --libs tetra-codec: {}",
                    String::from_utf8_lossy(&out.stdout).trim()
                );
            }
        }
    } else {
        log!(
            update,
            "WARN: libtetra-codec not found — LST voice encode/decode disabled (signalling only). Next OTA will retry auto-install, or run: sudo bash /opt/bost-flowstation/contrib/install/install-tetra-codec.sh"
        );
    }
    let build = cargo_build_command(&cargo, &src_dir, &update, with_codec);
    let label = if with_codec {
        format!("$ cargo build --release -p bluestation-bs --features asterisk -j {jobs}")
    } else {
        format!("$ cargo build --release -p bluestation-bs -j {jobs}")
    };
    let recovery_update = Arc::clone(&update);
    let mut rf_recovery = OtaRfRecoveryGuard::new(move || {
        let message = "OTA failed after RF was taken offline; restarting the installed service to restore RF in 5s";
        tracing::warn!("UPDATE: {}", message);
        if let Ok(mut state) = recovery_update.lock() {
            state.append(message);
            state.finish(false);
        }
        crate::service_control::schedule_service_action(
            crate::service_control::ServiceAction::Restart,
            std::time::Duration::from_secs(5),
        );
    });
    // Only stop RF once all preflight and no-op checks have passed and compilation will run.
    crate::rf_status::request_ota_rf_off();
    log!(update, "RF: offline for OTA (SDR will reopen after service restart)");

    if stream_cmd(&update, label, build).is_none() {
        return;
    }

    // Step 7: install the new binary where systemd actually runs it.
    log!(update, "--- Installing release binary ---");
    if !install_built_binary(&src_dir, &update) {
        return;
    }

    // Host WiFi drop-in (powersave off) — no SSH; service/OTA run as root on Pi installs.
    log!(update, "--- Ensuring NetworkManager WiFi drop-in ---");
    match crate::wifi::install_host_wifi_dropin(Some(src_dir.as_path())) {
        Ok(msg) => log!(update, "{msg}"),
        Err(e) => log!(update, "WARN: WiFi host drop-in not applied: {e}"),
    }

    // Flush filesystem buffers so a hard power-loss mid-restart cannot leave a half-written binary.
    let _ = std::process::Command::new("sync").status();

    // Step 8: done — schedule restart. Give the browser a few seconds to observe done_ok
    // before the HTTP server dies (Pi OTA builds leave memory tight; restart can be slow).
    log!(update, "--- Build successful. Restarting service in 5s… ---");
    log!(
        update,
        "Dashboard will disconnect briefly — wait for automatic reload (do not power-cycle yet)."
    );
    update.lock().unwrap().finish(true);

    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::Restart,
        std::time::Duration::from_secs(5),
    );
    rf_recovery.disarm();
}

pub struct DashboardServer {
    pub state: DashboardState,
    clients: WsClients,
    config_path: String,
    /// Shared stack config — used to read live_sds_queue from StackState.
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    /// Direct command channel to CMCE for SDS/call/emergency operations.
    cmd_tx: Option<CmdSender>,
    /// Direct command channel to MM for mobility/security operations (DGNA and SCK OTAR).
    mm_cmd_tx: Option<CmdSender>,
    update_state: SharedUpdateState,
    /// Optional override for the OTA update source directory.
    /// If None, the update routine auto-detects.
    source_dir_override: Option<String>,
    /// Authentication credentials. None = no auth (open access). When set, requests
    /// must carry a valid `fs_session` cookie obtained from `POST /api/login`.
    /// Wrapped in `Arc<RwLock<_>>` so System-tab credential changes hot-update without restart.
    auth: SharedAuth,
    /// When true AND `auth` is Some, anonymous visitors get a read-only public overview instead of
    /// being bounced to /login. Inert without auth. (FH-FEAT-033)
    public_overview: bool,
    /// In-memory session store backing the cookie auth.
    sessions: SharedSessionStore,
    /// Cross-connection failed-login counter backing the brute-force lockout.
    login_throttle: SharedLoginThrottle,
    /// Last time a ts_voice WS message was broadcast per carrier/timeslot.
    ts_last_broadcast: std::sync::Mutex<HashMap<(u16, u8), std::time::Instant>>,
    /// On-demand RadioID callsign resolver (ISSI → indicativ), cached locally.
    radioid: crate::net_dashboard::radioid::RadioIdCache,
    /// LST Dispatch shared handle (None when profile not active).
    lst_handle: Option<crate::net_lst_dispatch::LstDispatchHandle>,
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
            mm_cmd_tx: None,
            update_state: Arc::new(Mutex::new(UpdateState::new())),
            source_dir_override: None,
            auth: Arc::new(RwLock::new(None)),
            public_overview: false,
            sessions: Arc::new(Mutex::new(SessionStore::new())),
            login_throttle: Arc::new(Mutex::new(LoginThrottle::new())),
            ts_last_broadcast: std::sync::Mutex::new(HashMap::new()),
            radioid: crate::net_dashboard::radioid::RadioIdCache::new(radioid_path),
            lst_handle: None,
        }
    }

    pub fn set_cmd_sender(&mut self, tx: CmdSender) {
        self.cmd_tx = Some(tx);
    }

    /// Provide the MM command channel for dashboard commands owned by Mobility Management.
    pub fn set_mm_cmd_sender(&mut self, tx: CmdSender) {
        self.mm_cmd_tx = Some(tx);
    }

    pub fn set_lst_handle(&mut self, handle: crate::net_lst_dispatch::LstDispatchHandle) {
        handle.attach_ws_clients(self.clients.clone());
        self.lst_handle = Some(handle);
    }

    /// Provide the SharedConfig so the dashboard can read live SDS queue state.
    pub fn set_shared_config(&mut self, cfg: tetra_config::bluestation::SharedConfig) {
        self.shared_config = Some(cfg);
    }

    /// Configure an explicit source directory for OTA updates.
    pub fn set_source_dir(&mut self, source_dir: Option<String>) {
        self.source_dir_override = source_dir;
    }

    /// Configure dashboard login credentials (cookie session; not browser Basic Auth).
    pub fn set_auth(&mut self, auth: Option<(String, String)>) {
        *self.auth.write().unwrap_or_else(|e| e.into_inner()) = auth;
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

    /// Record a decoded LIP fix for Geo UI (Home + LST). Skip 0,0 at the call site.
    pub fn note_lip_position(&self, issi: u32, lat: f64, lon: f64) {
        let mut s = self.state.write().unwrap_or_else(|e| e.into_inner());
        s.note_lip_position(issi, lat, lon);
    }

    pub fn start(&mut self, bind: &str, http_port: u16, https_port: u16) {
        let state = Arc::clone(&self.state);
        let clients = Arc::clone(&self.clients);
        let config_path = self.config_path.clone();
        let cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.cmd_tx.take()));
        let mm_cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.mm_cmd_tx.take()));
        let update_state = Arc::clone(&self.update_state);
        let source_dir_override = self.source_dir_override.clone();
        let auth: SharedAuth = Arc::clone(&self.auth);
        let public_overview = self.public_overview;
        let shared_config = self.shared_config.clone();
        let sessions = Arc::clone(&self.sessions);
        let login_throttle = Arc::clone(&self.login_throttle);
        let radioid = self.radioid.clone();
        let lst_handle = self.lst_handle.clone();

        // Always try to materialise self-signed certs before spawning listeners.
        let tls_dir = tls_dir_for_config(&config_path);
        let tls_cfg = ensure_dashboard_tls(&tls_dir, https_port).map(|(cfg, _fp)| cfg);

        let https_state = Arc::clone(&state);
        let https_clients = Arc::clone(&clients);
        let https_config_path = config_path.clone();
        let https_cmd_tx = Arc::clone(&cmd_tx);
        let https_mm_cmd_tx = Arc::clone(&mm_cmd_tx);
        let https_update_state = Arc::clone(&update_state);
        let https_source_dir_override = source_dir_override.clone();
        let https_auth: SharedAuth = Arc::clone(&auth);
        let https_public_overview = public_overview;
        let https_shared_config = shared_config.clone();
        let https_sessions = Arc::clone(&sessions);
        let https_login_throttle = Arc::clone(&login_throttle);
        let https_radioid = radioid.clone();
        let https_lst_handle = lst_handle.clone();

        // HTTP redirects by default when TLS is up. An explicit private-source allowlist may
        // admit authenticated management on this primary HTTP port (including native apps).
        // The historical TLS-unavailable fallback stays unchanged when no allowlist is configured.
        // port == 0 disables the HTTP listener (high-port / shared-host preset).
        let http_redirect_only = tls_cfg.is_some();
        let http_bind = bind.to_string();
        let http_port_owned = http_port;
        let https_port_for_redirect = https_port;

        if http_port_owned != 0 {
            std::thread::Builder::new()
                .name("dashboard-http".into())
                .spawn(move || {
                    crate::wifi::spawn_watchdog();
                    let addr = format!("{}:{}", http_bind, http_port_owned);
                    let Some(listener) = bind_tcp_listener(&addr, BindMode::Canonical) else {
                        return;
                    };
                    if private_http_configured(&shared_config) {
                        tracing::info!(
                            "Dashboard primary HTTP listener on http://{}: authenticated management for explicitly allowed private peers; all other peers redirect to HTTPS",
                            addr
                        );
                    } else if http_redirect_only {
                        tracing::info!(
                            "Dashboard HTTP redirect on http://{} → https (port {})",
                            addr,
                            https_port_for_redirect
                        );
                    } else {
                        tracing::warn!(
                            "Dashboard listening on http://{} (HTTPS unavailable — install openssl for TLS)",
                            addr
                        );
                    }
                    for stream in listener.incoming() {
                        let Ok(stream) = stream else { continue };
                        let peer_ip = stream.peer_addr().ok().map(|peer| peer.ip());
                        let Some(require_http_auth) = primary_http_admission(http_redirect_only, &shared_config, &auth, peer_ip) else {
                            serve_https_redirect_plain(stream, https_port_for_redirect);
                            continue;
                        };
                        if !dash_try_acquire_conn() {
                            tracing::warn!(
                                "Dashboard: connection cap ({DASH_MAX_CONN}) reached — rejecting HTTP"
                            );
                            dash_reject_busy_plain(stream);
                            continue;
                        }
                        let state = Arc::clone(&state);
                        let clients = Arc::clone(&clients);
                        let config_path = config_path.clone();
                        let cmd_tx = Arc::clone(&cmd_tx);
                        let mm_cmd_tx = Arc::clone(&mm_cmd_tx);
                        let update_state = Arc::clone(&update_state);
                        let source_dir_override = source_dir_override.clone();
                        let auth = Arc::clone(&auth);
                        let shared_config = shared_config.clone();
                        let sessions = Arc::clone(&sessions);
                        let login_throttle = Arc::clone(&login_throttle);
                        let radioid = radioid.clone();
                        let lst_handle = lst_handle.clone();
                        if std::thread::Builder::new()
                            .name("dashboard-conn".into())
                            .spawn(move || {
                                handle_connection(
                                    ConnStream::plain(stream),
                                    state,
                                    clients,
                                    config_path,
                                    cmd_tx,
                                    mm_cmd_tx,
                                    update_state,
                                    source_dir_override,
                                    auth,
                                    shared_config,
                                    sessions,
                                    login_throttle,
                                    radioid,
                                    public_overview,
                                    lst_handle,
                                    require_http_auth,
                                    Some(DashConnGuard),
                                )
                            })
                            .is_err()
                        {
                            dash_release_conn();
                        }
                    }
                })
                .expect("failed to spawn dashboard HTTP thread");
        } else {
            tracing::info!("Dashboard HTTP redirect disabled (port = 0)");
            crate::wifi::spawn_watchdog();
        }

        // Bookmark redirects 8080 → canonical HTTPS only in the standard :443 layout.
        // Fail-soft: if the port is busy/denied, warn once and stop (no ERROR spam).
        if http_redirect_only && https_port == 443 && http_port != LEGACY_HTTP_PORT {
            spawn_legacy_http_redirect(bind, LEGACY_HTTP_PORT, https_port);
        }

        // Canonical HTTPS dashboard.
        if let Some(tls_config) = tls_cfg {
            let https_addr = format!("{}:{}", bind, https_port);
            let state = https_state;
            let clients = https_clients;
            let config_path = https_config_path;
            let cmd_tx = https_cmd_tx;
            let update_state = https_update_state;
            let source_dir_override = https_source_dir_override;
            let auth = https_auth;
            let public_overview = https_public_overview;
            let shared_config = https_shared_config;
            let sessions = https_sessions;
            let login_throttle = https_login_throttle;
            let radioid = https_radioid;
            let lst_handle = https_lst_handle;
            let tls_for_legacy = Arc::clone(&tls_config);
            let spawn_legacy_tls = https_port == 443;

            std::thread::Builder::new()
                .name("dashboard-https".into())
                .spawn(move || {
                    let Some(listener) = bind_tcp_listener(&https_addr, BindMode::Canonical) else {
                        return;
                    };
                    tracing::info!(
                        "Dashboard HTTPS listening on https://{} (self-signed; accept the browser warning)",
                        https_addr
                    );
                    for stream in listener.incoming() {
                        let Ok(tcp) = stream else { continue };
                        let peer = tcp
                            .peer_addr()
                            .map(|a| a.to_string())
                            .unwrap_or_else(|_| "unknown".into());
                        if !dash_try_acquire_conn() {
                            tracing::warn!(
                                "Dashboard: connection cap ({DASH_MAX_CONN}) reached — rejecting HTTPS from {peer}"
                            );
                            dash_reject_busy_plain(tcp);
                            continue;
                        }
                        let tls_config = Arc::clone(&tls_config);
                        let state = Arc::clone(&state);
                        let clients = Arc::clone(&clients);
                        let config_path = config_path.clone();
                        let cmd_tx = Arc::clone(&cmd_tx);
                        let mm_cmd_tx = Arc::clone(&https_mm_cmd_tx);
                        let update_state = Arc::clone(&update_state);
                        let source_dir_override = source_dir_override.clone();
                        let auth = Arc::clone(&auth);
                        let shared_config = shared_config.clone();
                        let sessions = Arc::clone(&sessions);
                        let login_throttle = Arc::clone(&login_throttle);
                        let radioid = radioid.clone();
                        let lst_handle = lst_handle.clone();
                        if std::thread::Builder::new()
                            .name("dashboard-https-conn".into())
                            .spawn(move || {
                                let guard = DashConnGuard;
                                let conn = match ConnStream::from_tls_handshake(tcp, tls_config) {
                                    Ok(c) => c,
                                    Err(e) => {
                                        tracing::debug!("[{}] TLS session init failed: {}", peer, e);
                                        return;
                                    }
                                };
                                handle_connection(
                                    conn,
                                    state,
                                    clients,
                                    config_path,
                                    cmd_tx,
                                    mm_cmd_tx,
                                    update_state,
                                    source_dir_override,
                                    auth,
                                    shared_config,
                                    sessions,
                                    login_throttle,
                                    radioid,
                                    public_overview,
                                    lst_handle,
                                    false,
                                    Some(guard),
                                )
                            })
                            .is_err()
                        {
                            dash_release_conn();
                        }
                    }
                })
                .expect("failed to spawn dashboard HTTPS thread");

            if spawn_legacy_tls {
                spawn_legacy_https_redirect(bind, LEGACY_HTTPS_PORT, https_port, tls_for_legacy);
            }
        }

        let http_note = if http_port == 0 {
            "HTTP redirect off".to_string()
        } else {
            format!("HTTP :{http_port} redirects")
        };
        tracing::info!(
            "Dashboard ready: https://{}:{}/ ({}; legacy bookmark redirects {})",
            bind,
            https_port,
            http_note,
            if https_port == 443 {
                "fail-soft on :8080/:8443"
            } else {
                "off"
            }
        );
    }

    pub fn handle_telemetry(&self, event: TelemetryEvent) {
        if let TelemetryEvent::CellRf { cell, event: rf } = &event {
            return self.handle_cell_rf(*cell, rf);
        }
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
                            group_catalog: Vec::new(),
                            selected_group: None,
                            rssi_dbfs: None,
                            registered_at: Instant::now(),
                            last_seen: Instant::now(),
                            energy_saving_mode: 0,
                            cell: None,
                        },
                    );
                    s.push_log("INFO", format!("MS {} registered", issi));
                }
                TelemetryEvent::MsCell { issi, cell } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.cell = Some(*cell);
                    }
                }
                TelemetryEvent::CellsSnapshot { .. } => {}
                TelemetryEvent::MsSecurity { issi, authenticated, encrypting } => {
                    let flags = s.ms_security.entry(*issi).or_default();
                    if let Some(a) = authenticated {
                        flags.0 = *a;
                    }
                    if let Some(e) = encrypting {
                        flags.1 = *e;
                    }
                    if *authenticated == Some(true) {
                        s.push_log("INFO", format!("MS {} authenticated", issi));
                    }
                    if *encrypting == Some(true) {
                        s.push_log("INFO", format!("MS {} is encrypting (SCK)", issi));
                    }
                }
                TelemetryEvent::OtarSck { issi, sckn, sck_vn, status } => {
                    let level = if status == "accepted" || status == "sent" { "INFO" } else { "WARN" };
                    s.push_log(level, format!("OTAR SCK {} v{} to ISSI {}: {}", sckn, sck_vn, issi, status));
                }
                TelemetryEvent::MsMovedAway { issi, cell } => {
                    // Moved to a sibling cell: its MsCell already put the row on the new cell, so
                    // it stays listed. A row still on the cell it left means the radio went to
                    // another station and is no longer attached here.
                    if s.ms_map.get(issi).is_some_and(|e| e.cell.unwrap_or(0) == *cell) {
                        s.ms_map.remove(issi);
                        s.ms_security.remove(issi);
                        s.push_log("INFO", format!("MS {} moved to another station", issi));
                    } else {
                        s.push_log("INFO", format!("MS {} moved to another cell", issi));
                    }
                }
                TelemetryEvent::MsDeregistration { issi } => {
                    s.ms_map.remove(issi);
                    s.ms_security.remove(issi);
                    s.push_log("INFO", format!("MS {} deregistered", issi));
                }
                TelemetryEvent::MsTimeoutDrop { issi } => {
                    // Same UI effect as a deregistration (the MS is gone from the cell); the
                    // distinct event only matters to alert consumers that report the reason.
                    s.ms_map.remove(issi);
                    s.ms_security.remove(issi);
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
                        for group in &mut e.group_catalog {
                            group.is_attached = e.groups.contains(&group.gssi);
                        }
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
                TelemetryEvent::MsGroupCatalogSnapshot { issi, groups } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.group_catalog = groups
                            .iter()
                            .map(|group| MsGroupState {
                                gssi: group.gssi,
                                mnemonic: group.mnemonic.clone(),
                                attachment_mode: group.attachment_mode,
                                is_dynamic: group.is_dynamic,
                                is_attached: group.is_attached,
                            })
                            .collect();
                        e.groups = e
                            .group_catalog
                            .iter()
                            .filter(|group| group.is_attached)
                            .map(|group| group.gssi)
                            .collect();
                        if let Some(sel) = e.selected_group
                            && !e.groups.contains(&sel)
                        {
                            e.selected_group = None;
                        }
                    }
                }
                TelemetryEvent::DgnaStatus(status) => {
                    s.push_dgna_log(status.clone());
                    s.push_log(
                        if status.accepted { "INFO" } else { "WARN" },
                        format!(
                            "DGNA {} ISSI {} GSSI {} [{}]: {}",
                            if status.attach { "assign" } else { "deassign" },
                            status.issi,
                            status.gssi,
                            status.source,
                            status.detail
                        ),
                    );
                }
                TelemetryEvent::MsGroupDetach { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.groups.retain(|g| !gssis.contains(g));
                        for group in &mut e.group_catalog {
                            if gssis.contains(&group.gssi) {
                                group.is_attached = false;
                            }
                        }
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
                    message_ref,
                } => {
                    s.push_sds_log(direction, *source_issi, *dest_issi, *is_group, *protocol_id, text.clone(), *message_ref);
                }
                TelemetryEvent::SdsReport {
                    source_issi,
                    dest_issi,
                    message_ref,
                    delivered,
                } => {
                    s.push_last_heard(*source_issi, "sds", *dest_issi);
                    if s.mark_sds_delivered(*source_issi, *dest_issi, *message_ref, *delivered).is_none() {
                        tracing::debug!(
                            "Dashboard: delivery report from {} for console {} MR={} matched no sent SDS",
                            source_issi,
                            dest_issi,
                            message_ref
                        );
                    }
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
                TelemetryEvent::CellRf { .. } => {} // handled above
                TelemetryEvent::StationVersion { .. } | TelemetryEvent::SiteLocation { .. } => {}
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

    /// An additional cell's RF event: the same message as the primary's, tagged with `cell`.
    fn handle_cell_rf(&self, cell: u8, rf: &CellRfEvent) {
        let Some(v) = cell_rf_ws_value(cell, rf) else { return };
        let kind = match rf {
            CellRfEvent::TxVisual { .. } => "tx_visual",
            CellRfEvent::TxQuality { .. } => "tx_quality",
            CellRfEvent::SdrHealth { .. } => "sdr_health",
        };
        self.state.write().unwrap().cell_rf.insert((cell, kind), v.clone());
        if let Ok(json) = serde_json::to_string(&v) {
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
        // try_send, not send: a client whose bounded queue is full (it stopped reading its socket) is
        // dropped here instead of letting an unbounded backlog grow. A closed receiver also errors and
        // is pruned, same as before.
        clients.retain(|tx| tx.try_send(msg.to_owned()).is_ok());
    }
}

/// WebSocket message for an additional cell's RF event: the primary's message plus `"cell"`.
fn cell_rf_ws_value(cell: u8, rf: &CellRfEvent) -> Option<serde_json::Value> {
    let mut v: serde_json::Value = serde_json::from_str(&event_to_ws_msg(&rf.to_event())?).ok()?;
    v.as_object_mut()?.insert("cell".into(), cell.into());
    Some(v)
}

/// The cell's security posture for the dashboard: class 1 (clear) or class 2 with the SCK in
/// use, and the authentication mode (EN 300 392-7 clauses 4.4 and 6.5).
fn cell_security_json(sec: &tetra_config::bluestation::sec_security::CfgSecurity) -> serde_json::Value {
    use tetra_config::bluestation::sec_security::AuthenticationMode;
    let authentication = match sec.authentication {
        AuthenticationMode::Off => "off",
        AuthenticationMode::Optional => "optional",
        AuthenticationMode::Required => "required",
    };
    match &sec.aie {
        Some(a) if a.enabled => serde_json::json!({
            "class": 2, "ksg": format!("TEA{}", a.ksg), "sckn": a.sckn, "sck_vn": a.sck_vn,
            "weak": a.ksg == 1, "clear_groups": a.clear_groups,
            "authentication": authentication, "mutual": sec.mutual_authentication,
            "subscribers": sec.subscribers.len(),
        }),
        staged => serde_json::json!({
            "class": 1,
            "staged": staged.as_ref().map(|a| serde_json::json!({"ksg": format!("TEA{}", a.ksg), "sckn": a.sckn, "sck_vn": a.sck_vn})),
            "authentication": authentication, "mutual": sec.mutual_authentication,
            "subscribers": sec.subscribers.len(),
        }),
    }
}

/// GET /api/security — running and saved security posture, keys masked.
fn serve_security_get(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str) {
    use crate::net_dashboard::security;
    let running = match shared_config {
        Some(cfg) => cfg.config().security.clone(),
        None => tetra_config::bluestation::sec_security::CfgSecurity::default(),
    };
    http_json_response(stream, 200, &security::page_json(&running, security::load_saved(config_path)));
}

/// POST /api/security — validate and write the settings; they apply on the next restart.
fn serve_security_post(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
    use crate::net_dashboard::security;
    let current = match security::load_saved(config_path) {
        Ok(s) => s,
        Err(e) => {
            http_response(stream, 500, &e);
            return;
        }
    };
    let edit = match security::parse_edit(body, &current) {
        Ok(e) => e,
        Err(e) => {
            http_response(stream, 400, &e);
            return;
        }
    };
    if let Err(e) = security::write_to_toml(config_path, &edit) {
        http_response(stream, 500, &format!("Could not write config: {e}"));
        return;
    }
    tracing::info!(
        "Dashboard: security settings saved (authentication {:?}, {} subscriber key(s), encryption {}) — restart to apply",
        edit.authentication,
        edit.subscribers.len(),
        edit.aie.as_ref().map(|a| format!("TEA{} SCK {} v{}", a.ksg, a.sckn, a.sck_vn)).unwrap_or_else(|| "off".into())
    );
    let running = match shared_config {
        Some(cfg) => cfg.config().security.clone(),
        None => tetra_config::bluestation::sec_security::CfgSecurity::default(),
    };
    http_json_response(stream, 200, &security::page_json(&running, security::load_saved(config_path)));
}

/// POST /api/security/generate {"what":"sck"|"k"} — a fresh random key for the form.
fn serve_security_generate(stream: PrefixedConn, body: &str) {
    let what = serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|v| v.get("what").and_then(|w| w.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    let n = match what.as_str() {
        "sck" => 10,
        "k" => 16,
        _ => {
            http_response(stream, 400, "what must be sck or k");
            return;
        }
    };
    match crate::net_dashboard::security::random_hex(n) {
        Ok(h) => http_json_response(stream, 200, &format!("{{\"hex\":\"{h}\"}}")),
        Err(e) => http_response(stream, 500, &format!("no random source: {e}")),
    }
}


fn event_to_ws_msg(event: &TelemetryEvent) -> Option<String> {
    let v = match event {
        TelemetryEvent::MsRegistration { issi } => serde_json::json!({"type":"ms_registered","issi":issi}),
        TelemetryEvent::MsCell { issi, cell } => serde_json::json!({"type":"ms_cell","issi":issi,"cell":cell}),
        TelemetryEvent::CellRf { cell, event } => return cell_rf_ws_value(*cell, event).and_then(|v| serde_json::to_string(&v).ok()),
        TelemetryEvent::MsDeregistration { issi } => serde_json::json!({"type":"ms_deregistered","issi":issi}),
        TelemetryEvent::MsTimeoutDrop { issi } => serde_json::json!({"type":"ms_deregistered","issi":issi,"reason":"t351"}),
        TelemetryEvent::MsGroupAttach { issi, gssis } => serde_json::json!({"type":"ms_groups","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupDetach { issi, gssis } => serde_json::json!({"type":"ms_groups_detach","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupsSnapshot { issi, gssis } => serde_json::json!({"type":"ms_groups_all","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupCatalogSnapshot { issi, groups } => {
            serde_json::json!({"type":"ms_group_catalog","issi":issi,"groups":groups})
        }
        TelemetryEvent::DgnaStatus(status) => serde_json::json!({
            "type":"dgna_status",
            "issi":status.issi,
            "gssi":status.gssi,
            "attach":status.attach,
            "accepted":status.accepted,
            "source":status.source,
            "detail":status.detail,
        }),
        TelemetryEvent::MsRssi { issi, rssi_dbfs } => serde_json::json!({"type":"ms_rssi","issi":issi,"rssi_dbfs":rssi_dbfs}),
        TelemetryEvent::MsEnergySaving { issi, mode } => serde_json::json!({"type":"ms_energy_saving","issi":issi,"mode":mode}),
        TelemetryEvent::MsSecurity { issi, authenticated, encrypting } => {
            serde_json::json!({"type":"ms_security","issi":issi,"authenticated":authenticated,"encrypting":encrypting})
        }
        TelemetryEvent::OtarSck { issi, sckn, sck_vn, status } => {
            serde_json::json!({"type":"otar_sck","issi":issi,"sckn":sckn,"sck_vn":sck_vn,"status":status})
        }
        TelemetryEvent::MsMovedAway { issi, cell } => serde_json::json!({"type":"ms_moved_away","issi":issi,"cell":cell}),
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
            message_ref,
        } => {
            serde_json::json!({"type":"sds_log","direction":direction,"source_issi":source_issi,"dest_issi":dest_issi,"is_group":is_group,"protocol_id":protocol_id,"text":text,"message_ref":message_ref})
        }
        TelemetryEvent::SdsReport {
            source_issi,
            dest_issi,
            message_ref,
            delivered,
        } => {
            serde_json::json!({"type":"sds_report","radio_issi":source_issi,"console_issi":dest_issi,"message_ref":message_ref,"delivered":delivered})
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
        // The Cells card polls /api/cells; this snapshot is for remote telemetry consumers.
        TelemetryEvent::CellsSnapshot { .. } | TelemetryEvent::StationVersion { .. } | TelemetryEvent::SiteLocation { .. } => return None,
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
/// Returns true iff `supplied == expected` in length and content.
///
/// Bailing out early on a length mismatch — as this used to — leaks the *expected* secret's length:
/// an attacker times candidates of increasing length and reads off how long the dashboard password
/// or the TPG2200 ActionURL token is before guessing a single byte of it. Instead we always walk
/// the supplied input end to end, wrapping around `expected`, and fold the length difference into
/// the accumulator. The work now depends only on the attacker's own input length, never on the
/// secret's, and the result is still exact equality.
fn timing_safe_eq(supplied: &[u8], expected: &[u8]) -> bool {
    let mut diff: u32 = (supplied.len() ^ expected.len()) as u32;
    let mut j = 0usize;
    for x in supplied {
        let y = expected.get(j).copied().unwrap_or(0);
        diff |= (*x ^ y) as u32;
        j += 1;
        if j >= expected.len() {
            j = 0;
        }
    }
    diff == 0
}

/// Send an HTTP 401 Unauthorized response that triggers the browser's native
/// Basic Auth dialog. Unused since the switch to cookie sessions.
#[allow(dead_code)]
fn http_response_401(mut stream: PrefixedConn) {
    let body = "Unauthorized";
    let resp = format!(
        "HTTP/1.1 401 Unauthorized\r\n\
         WWW-Authenticate: Basic realm=\"Bost FlowStation\", charset=\"UTF-8\"\r\n\
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
fn serve_sds_log(stream: PrefixedConn, state: &DashboardState) {
    let body = {
        let s = state.read().unwrap();
        let list: Vec<_> = s.sds_log.iter().rev().cloned().collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    };
    http_json_response(stream, 200, &body);
}

/// GET /api/dgna-log — the persisted DGNA activity log as a JSON array, newest entry first.
fn serve_dgna_log(stream: PrefixedConn, state: &DashboardState) {
    let body = {
        let s = state.read().unwrap();
        let list: Vec<_> = s.dgna_log.iter().rev().cloned().collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    };
    http_json_response(stream, 200, &body);
}

/// Serialize the current live SDS queue to JSON and serve it.
fn serve_live_sds_list(mut stream: PrefixedConn, cfg: &Option<tetra_config::bluestation::SharedConfig>) {
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
    mut stream: ConnStream,
    state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    mm_cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
    source_dir_override: Option<String>,
    auth: SharedAuth,
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    sessions: SharedSessionStore,
    login_throttle: SharedLoginThrottle,
    radioid: crate::net_dashboard::radioid::RadioIdCache,
    public_overview: bool,
    lst_handle: Option<crate::net_lst_dispatch::LstDispatchHandle>,
    require_http_auth: bool,
    // Held for short HTTP work; dropped before long-lived WebSocket loops so WS does not
    // permanently occupy a DASH_MAX_CONN slot (that starved polls and wedged the Pi UI).
    mut conn_guard: Option<DashConnGuard>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    // Captured before the stream is wrapped — the login throttle keys on it.
    let peer_ip = stream.peer_addr().ok().map(|a| a.ip());

    // ── Read the first 4KB for routing (TLS has no TcpStream::peek) ──
    // Bytes are re-queued via PrefixedConn so drain_http_headers / BufReader / WS
    // handshake still see the full request from the start.
    let mut peek_buf = [0u8; 4096];
    let n = match stream.read(&mut peek_buf) {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let header_buf = peek_buf[..n].to_vec();
    let header_str = String::from_utf8_lossy(&header_buf).into_owned();
    let req_line = header_str.lines().next().unwrap_or("").to_string();
    let mut stream = PrefixedConn::with_prefix(header_buf, stream);
    let auth_creds = if require_http_auth {
        match private_http_credentials(&shared_config, &auth, peer_ip) {
            Some(credentials) => Some(credentials),
            None => {
                http_response(stream, 401, "Private HTTP management requires configured credentials and an allowed direct peer");
                return;
            }
        }
    } else {
        auth.read().unwrap_or_else(|e| e.into_inner()).clone()
    };

    // Snom/desk-phone ActionURL endpoint. It has its own token and must work without the
    // dashboard cookie session, so handle it before the normal dashboard auth gate.
    if !require_http_auth && is_tpg2200_action_request(&req_line) {
        drain_http_headers(&mut stream);
        serve_tpg2200_action_url(stream, &req_line, &shared_config, &cmd_tx, &state);
        return;
    }

    // Public TLS status (no auth) so the UI can advertise the canonical HTTPS URL.
    if !require_http_auth
        && (req_line.starts_with("GET /api/dashboard/tls ")
            || req_line.starts_with("GET /api/dashboard/tls?")
            || req_line.starts_with("GET /api/dashboard/tls HTTP"))
    {
        drain_http_headers(&mut stream);
        serve_dashboard_tls_status(stream);
        return;
    }

    // ── Cookie-session auth ──────────────────────────────────────────────────
    // We replaced the browser-native Basic Auth dialog with a form-based login at
    // /login that issues an fs_session cookie. The native dialog has well-known
    // mobile usability issues (iOS Safari prompts 2-3 times, forgets credentials
    // between WebSocket reconnects, etc.). With cookies we control the UX fully.
    //
    // Public routes (no auth required): GET /login, POST /api/login, favicon, static assets.
    // Every other route is checked here against the session store.
    // Favicon must stay public so the browser can load it on /login before auth.
    // Match path prefix carefully: browsers request /favicon.ico, /favicon.png, /favicon.svg.
    if req_line.starts_with("GET /favicon.")
        || req_line.starts_with("GET /favicon.ico")
        || req_line.starts_with("GET /apple-touch-icon")
    {
        drain_http_headers(&mut stream);
        let want_svg = req_line.contains("/favicon.svg");
        serve_favicon(stream, want_svg);
        return;
    }

    // Vendored static assets (public, no auth): Leaflet for the offline LIP/location map, so the
    // map works on a station with no internet. Pinned and immutable.
    if req_line.starts_with("GET /assets/leaflet.js") {
        drain_http_headers(&mut stream);
        serve_static_asset(stream, include_bytes!("assets/leaflet.js"), "application/javascript; charset=utf-8");
        return;
    }
    if req_line.starts_with("GET /assets/leaflet.css") {
        drain_http_headers(&mut stream);
        serve_static_asset(stream, include_bytes!("assets/leaflet.css"), "text/css; charset=utf-8");
        return;
    }

    // Credentials were captured before public routing; scoped HTTP cannot become anonymous if
    // runtime auth is removed between listener admission and this request's cookie gate.
    if let Some((ref expected_user, ref expected_pass)) = auth_creds {
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

            // Shared lockout, checked before the credentials are even looked at: an attacker
            // opening N sockets in parallel used to sidestep the throttle entirely, because each
            // connection only slept on itself.
            if let Some(ip) = peer_ip {
                let locked = login_throttle
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .locked_for(&ip);
                if let Some(remaining) = locked {
                    let secs = remaining.as_secs().max(1);
                    tracing::warn!("Dashboard: login from {} locked out for another {}s", ip, secs);
                    http_response(
                        buf.into_inner(),
                        429,
                        &format!("Too many failed logins — try again in {}s", secs),
                    );
                    return;
                }
            }

            let (user, pass) = parse_login_body(&body_str);
            let ok = timing_safe_eq(user.as_bytes(), expected_user.as_bytes()) && timing_safe_eq(pass.as_bytes(), expected_pass.as_bytes());

            if ok {
                // Keep runtime credentials stable while an admitted HTTP login issues its cookie.
                // Without this guard an in-flight old-password login could outlive auth removal.
                let private_auth_guard = if require_http_auth { auth.read().ok() } else { None };
                if require_http_auth
                    && !private_auth_guard.as_ref().is_some_and(|current| {
                        current.as_ref().is_some_and(|(user, password)| {
                            timing_safe_eq(user.as_bytes(), expected_user.as_bytes())
                                && timing_safe_eq(password.as_bytes(), expected_pass.as_bytes())
                        })
                    })
                {
                    http_response(buf.into_inner(), 401, "Dashboard authentication changed; log in again");
                    return;
                }
                // Recover from a poisoned lock instead of issuing an empty cookie: an empty token
                // would "log in" the browser yet store no session, so every later request would fail
                // the gate (logged in, but bounced). Poisoning only happens after a panic elsewhere;
                // the session store itself stays usable.
                let token = sessions.lock().unwrap_or_else(|e| e.into_inner()).create();
                tracing::info!("Dashboard: login OK (user: {})", user);
                if let Some(ip) = peer_ip {
                    login_throttle.lock().unwrap_or_else(|e| e.into_inner()).clear(&ip);
                }
                serve_login_success(buf.into_inner(), &token);
            } else {
                tracing::warn!("Dashboard: login FAILED (user attempt: {})", user);
                // Escalating, cross-connection cost: the delay grows with the failure count for
                // this source IP, and past LOGIN_LOCKOUT_AFTER the address is locked out above.
                let delay = match peer_ip {
                    Some(ip) => login_throttle.lock().unwrap_or_else(|e| e.into_inner()).record_failure(ip),
                    None => std::time::Duration::from_millis(500),
                };
                std::thread::sleep(delay);
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
            if !require_http_auth && public_overview && is_root {
                serve_html(inner);
                return;
            }
            if !require_http_auth && public_overview
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

    // Auth may have changed while this request was being read. Scoped HTTP fails closed instead
    // of inheriting the ordinary dashboard's no-auth mode; upgraded sessions also keep this check.
    if require_http_auth
        && !auth_creds.as_ref().is_some_and(|expected| credentials_unchanged(&auth, expected))
    {
        http_response(stream, 401, "Dashboard authentication changed; log in again");
        return;
    }
    if require_http_auth && is_tpg2200_action_request(&req_line) {
        drain_http_headers(&mut stream);
        serve_tpg2200_action_url(stream, &req_line, &shared_config, &cmd_tx, &state);
        return;
    }
    if require_http_auth && (req_line.starts_with("GET /api/dashboard/tls ")
        || req_line.starts_with("GET /api/dashboard/tls?")
        || req_line.starts_with("GET /api/dashboard/tls HTTP"))
    {
        drain_http_headers(&mut stream);
        serve_dashboard_tls_status(stream);
        return;
    }

    // Access control is the cookie gate above: with credentials configured the dashboard requires a
    // login for everything; without credentials it is fully open (the home-network default). We do
    // NOT additionally restrict control by the peer's address — "is this localhost?" can't tell a
    // browser running on the BTS that connects via the box's LAN IP from a remote one, so it wrongly
    // blocked operators doing DGNA/SDS from the BTS itself. Set a username/password to lock it down.
    if req_line.contains("/ws") {
        // Release the HTTP concurrency slot before entering the WS read loop.
        drop(conn_guard.take());
        let private_http_session = if require_http_auth {
            Some(PrivateHttpSession {
                shared_config: shared_config.clone(),
                auth: Arc::clone(&auth),
                sessions: Arc::clone(&sessions),
                peer: peer_ip,
                credentials: auth_creds.expect("scoped HTTP credentials checked above"),
                token: parse_session_cookie(&header_str).expect("scoped HTTP session checked above"),
            })
        } else {
            None
        };
        handle_ws(
            stream,
            state,
            clients,
            cmd_tx,
            mm_cmd_tx,
            update_state,
            shared_config.clone(),
            lst_handle,
            private_http_session,
        );
    } else if req_line.contains("GET /api/service/status") {
        let mut s = stream;
        drain_http_headers(&mut s);
        let state = if crate::service_control::is_standby() {
            "standby"
        } else {
            "running"
        };
        http_json_response(s, 200, &format!(r#"{{"state":"{state}"}}"#));
    } else if req_line.contains("POST /api/service/start") {
        let mut s = stream;
        drain_http_headers(&mut s);
        if !crate::service_control::is_standby() {
            http_json_response(s, 200, r#"{"ok":true,"already":"running"}"#);
        } else {
            tracing::info!("Dashboard: service start requested (exit 75)");
            crate::service_control::request_start();
            http_json_response(s, 200, r#"{"ok":true,"action":"start"}"#);
        }
    } else if req_line.contains("POST /api/service/restart") {
        let mut s = stream;
        drain_http_headers(&mut s);
        tracing::info!("Dashboard: service restart requested via HTTP");
        crate::service_control::schedule_service_action(
            crate::service_control::ServiceAction::Restart,
            std::time::Duration::from_millis(500),
        );
        http_json_response(s, 200, r#"{"ok":true,"action":"restart"}"#);
    } else if req_line.contains("POST /api/service/shutdown") {
        let mut s = stream;
        drain_http_headers(&mut s);
        tracing::info!("Dashboard: soft shutdown requested via HTTP");
        crate::service_control::schedule_service_action(
            crate::service_control::ServiceAction::Stop,
            std::time::Duration::from_millis(500),
        );
        http_json_response(s, 200, r#"{"ok":true,"action":"shutdown"}"#);
    } else if req_line.contains("POST /api/system/reboot") {
        let mut s = stream;
        drain_http_headers(&mut s);
        tracing::warn!("Dashboard: HOST reboot requested via HTTP (µCell overlay / driver install)");
        crate::service_control::schedule_host_reboot(std::time::Duration::from_millis(800));
        http_json_response(s, 200, r#"{"ok":true,"action":"reboot"}"#);
    } else if req_line.contains("POST /api/system/poweroff") {
        let mut s = stream;
        drain_http_headers(&mut s);
        tracing::warn!("Dashboard: HOST poweroff requested via HTTP (full system shutdown)");
        crate::service_control::schedule_host_poweroff(std::time::Duration::from_millis(800));
        http_json_response(s, 200, r#"{"ok":true,"action":"poweroff"}"#);
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
    } else if req_line.contains("GET /api/system ") || req_line.contains("GET /api/system?") {
        // Trailing space / `?` avoid swallowing `/api/system/brightness`.
        // `?probe=1` runs SoapySDRUtil (slow); default path stays cheap for boot/polling.
        let probe = req_line.contains("probe=1") || req_line.contains("probe=true");
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_system_info(buf.into_inner(), &config_path, probe);
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
    } else if req_line.contains("GET /api/radio-names") {
        let mut s = stream;
        drain_http_headers(&mut s);
        http_json_response(s, 200, &radio_names_json(&radioid));
    } else if req_line.contains("POST /api/radio-names") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => {
                let issi = v.get("issi").and_then(|i| i.as_u64()).unwrap_or(0);
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if issi == 0 || issi > 16_777_214 {
                    http_response(inner, 400, "invalid issi");
                } else {
                    radioid.set_local_name(issi as u32, name);
                    tracing::info!("Dashboard: radio name for ISSI {} set to {:?}", issi, name.trim());
                    http_json_response(inner, 200, &radio_names_json(&radioid));
                }
            }
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
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
    } else if req_line.contains("GET /api/update/channel") {
        let mut s = stream;
        drain_http_headers(&mut s);
        let body = crate::net_dashboard::ota_channel::channel_json(&config_path);
        http_json_response(s, 200, &body);
    } else if req_line.contains("POST /api/update/channel") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => {
                let channel = v
                    .get("channel")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .trim();
                let channel = tetra_core::normalize_ota_channel(channel);
                match crate::net_dashboard::ota_channel::write_ota_channel(&config_path, channel) {
                    Ok(()) => {
                        tracing::info!("Dashboard: OTA channel set to {}", channel);
                        let body = crate::net_dashboard::ota_channel::channel_json(&config_path);
                        http_json_response(inner, 200, &body);
                    }
                    Err(e) => http_response(inner, 500, &format!("failed to save ota_channel: {e}")),
                }
            }
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if req_line.contains("GET /api/update/check") {
        let refresh = req_line.contains("refresh=1") || req_line.contains("refresh=true");
        let with_notes = req_line.contains("notes=1") || req_line.contains("notes=true");
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_update_check(buf.into_inner(), &config_path, refresh, with_notes);
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
        match restore_config_from_backup(&config_path) {
            Ok(()) => {
                tracing::info!("Dashboard: config restored from backup");
                http_response(buf.into_inner(), 200, "OK")
            }
            Err((code, msg)) => {
                tracing::warn!("Dashboard: config restore refused: {}", msg);
                http_response(buf.into_inner(), code, &msg)
            }
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
        match write_config_validated(&config_path, body_str.as_ref()) {
            Ok(()) => http_response(buf.into_inner(), 200, "OK"),
            Err((code, msg)) => http_response(buf.into_inner(), code, &msg),
        }
    } else if req_line.contains("GET /api/setup/status") {
        let mut s = stream;
        drain_http_headers(&mut s);
        let body = crate::net_dashboard::setup::status_payload(&config_path).to_string();
        http_json_response(s, 200, &body);
    } else if req_line.contains("POST /api/setup/scan-sdr") {
        let mut s = stream;
        drain_http_headers(&mut s);
        let body = crate::net_dashboard::setup::scan_sdr_devices().to_string();
        http_json_response(s, 200, &body);
    } else if req_line.contains("POST /api/setup/install-driver") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => {
                let driver = v.get("driver").and_then(|d| d.as_str()).unwrap_or("").trim();
                if driver.is_empty() {
                    http_response(inner, 400, "missing driver (sx|lime)");
                } else {
                    match crate::net_dashboard::setup::install_driver(driver) {
                        Ok(msg) => http_json_response(
                            inner,
                            200,
                            &serde_json::json!({"ok":true,"output":msg}).to_string(),
                        ),
                        Err(e) => http_json_response(
                            inner,
                            500,
                            &serde_json::json!({"ok":false,"error":e}).to_string(),
                        ),
                    }
                }
            }
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if req_line.contains("POST /api/setup/apply") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<crate::net_dashboard::setup::SetupApplyRequest>(&body_str) {
            Ok(req) => match crate::net_dashboard::setup::apply_setup(&config_path, &req) {
                Ok(()) => {
                    tracing::info!(
                        "Dashboard setup: apply enable_rf={} restart={} device={:?}",
                        req.enable_rf,
                        req.restart,
                        req.device
                    );
                    http_json_response(inner, 200, r#"{"ok":true}"#);
                }
                Err(e) => http_response(inner, 400, &e),
            },
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if req_line.contains("POST /api/setup/complete") {
        let (inner, body_str) = read_post_body(stream);
        let skipped = serde_json::from_str::<serde_json::Value>(&body_str)
            .ok()
            .and_then(|v| v.get("skip").or_else(|| v.get("skipped")).and_then(|x| x.as_bool()))
            .unwrap_or(false);
        match crate::net_dashboard::setup::mark_complete(&config_path, skipped) {
            Ok(state) => http_json_response(
                inner,
                200,
                &serde_json::to_string(&serde_json::json!({"ok":true,"setup":state}))
                    .unwrap_or_else(|_| r#"{"ok":true}"#.into()),
            ),
            Err(e) => http_response(inner, 500, &e),
        }
    } else if req_line.contains("POST /api/setup/systemd") {
        let (inner, body_str) = read_post_body(stream);
        let action = serde_json::from_str::<serde_json::Value>(&body_str)
            .ok()
            .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "status".into());
        match crate::net_dashboard::setup::systemd_action(&action) {
            Ok(msg) => {
                // status returns JSON string; others a plain message
                if action == "status" {
                    http_json_response(inner, 200, &msg);
                } else {
                    http_json_response(
                        inner,
                        200,
                        &serde_json::json!({"ok":true,"output":msg}).to_string(),
                    );
                }
            }
            Err(e) => http_json_response(
                inner,
                500,
                &serde_json::json!({"ok":false,"error":e}).to_string(),
            ),
        }
    } else if req_line.contains("GET /api/visual-config") {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::visual_config_from_toml(&config_path) {
            Ok(v) => http_json_response(s, 200, &v.to_string()),
            Err(e) => http_response(s, 500, &e),
        }
    } else if req_line.contains("POST /api/visual-config") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => match crate::net_dashboard::profiles::write_visual_config(&config_path, &v) {
                Ok(()) => {
                    tracing::info!("Dashboard: visual config saved");
                    http_json_response(inner, 200, r#"{"ok":true}"#);
                }
                Err(e) => http_response(inner, 400, &e),
            },
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if req_line.contains("GET /api/profiles/active") {
        let mut s = stream;
        drain_http_headers(&mut s);
        let _ = crate::net_dashboard::profiles::ensure_seeded(&config_path);
        let active = crate::net_dashboard::profiles::read_active(&config_path);
        http_json_response(s, 200, &serde_json::to_string(&active).unwrap_or_else(|_| "{}".into()));
    } else if req_line.contains("POST /api/profiles/apply") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => {
                let cell = v.get("cell").and_then(|c| c.as_str()).unwrap_or("").trim();
                if cell.is_empty() {
                    http_response(inner, 400, "missing cell profile name");
                } else {
                    let brew = match v.get("brew") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(serde_json::Value::String(s)) if s.trim().is_empty() => None,
                        Some(serde_json::Value::String(s)) => Some(s.as_str()),
                        _ => {
                            http_response(inner, 400, "brew must be a string or null");
                            return;
                        }
                    };
                    match crate::net_dashboard::profiles::apply_profiles(&config_path, cell, brew) {
                        Ok(()) => {
                            tracing::info!(
                                "Dashboard: applied cell='{}' brew={:?}",
                                cell,
                                brew
                            );
                            // Sync live whitelist to the Cell being applied (missing = open).
                            if let Ok(cell_json) =
                                crate::net_dashboard::profiles::get_cell_profile(&config_path, cell)
                            {
                                let list = crate::net_dashboard::profiles::extract_issi_whitelist(
                                    &cell_json,
                                )
                                .unwrap_or_default();
                                let _ = apply_issi_whitelist_live(
                                    &shared_config,
                                    &config_path,
                                    &list,
                                    &cmd_tx,
                                );
                            }
                            http_json_response(inner, 200, r#"{"ok":true}"#);
                        }
                        Err(e) => http_response(inner, 400, &e),
                    }
                }
            }
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if req_line.contains("GET /api/profiles/cell ")
        || req_line.contains("GET /api/profiles/cell?")
        || req_line.starts_with("GET /api/profiles/cell HTTP")
    {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::list_cell_profiles(&config_path) {
            Ok(list) => http_json_response(s, 200, &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())),
            Err(e) => http_response(s, 500, &e),
        }
    } else if req_line.contains("GET /api/profiles/brew ")
        || req_line.contains("GET /api/profiles/brew?")
        || req_line.starts_with("GET /api/profiles/brew HTTP")
    {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::list_brew_profiles(&config_path) {
            Ok(list) => http_json_response(s, 200, &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())),
            Err(e) => http_response(s, 500, &e),
        }
    } else if let Some(name) = profile_path_name(&req_line, "GET /api/profiles/cell/") {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::get_cell_profile(&config_path, &name) {
            Ok(v) => http_json_response(s, 200, &v.to_string()),
            Err(e) => http_response(s, 404, &e),
        }
    } else if let Some(name) = profile_path_name(&req_line, "GET /api/profiles/brew/") {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::get_brew_profile(&config_path, &name, true) {
            Ok(v) => http_json_response(s, 200, &v.to_string()),
            Err(e) => http_response(s, 404, &e),
        }
    } else if let Some(name) = profile_path_name(&req_line, "POST /api/profiles/cell/") {
        if let Some(src) = name.strip_suffix("/duplicate") {
            let (inner, body_str) = read_post_body(stream);
            let new_name = serde_json::from_str::<serde_json::Value>(&body_str)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            match crate::net_dashboard::profiles::duplicate_cell_profile(&config_path, src, &new_name) {
                Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                Err(e) => http_response(inner, 400, &e),
            }
        } else if let Some(src) = name.strip_suffix("/rename") {
            let (inner, body_str) = read_post_body(stream);
            let new_name = serde_json::from_str::<serde_json::Value>(&body_str)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            match crate::net_dashboard::profiles::rename_cell_profile(&config_path, src, &new_name) {
                Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                Err(e) => http_response(inner, 400, &e),
            }
        } else if let Some(cell) = name.strip_suffix("/whitelist") {
            let (inner, body_str) = read_post_body(stream);
            serve_cell_whitelist_post(
                inner,
                &shared_config,
                &config_path,
                cell,
                &body_str,
                &cmd_tx,
            );
        } else {
            let (inner, body_str) = read_post_body(stream);
            match serde_json::from_str::<serde_json::Value>(&body_str) {
                Ok(v) => match crate::net_dashboard::profiles::put_cell_profile(&config_path, &name, &v) {
                    Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                    Err(e) => http_response(inner, 400, &e),
                },
                Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
            }
        }
    } else if let Some(name) = profile_path_name(&req_line, "POST /api/profiles/brew/") {
        if let Some(src) = name.strip_suffix("/duplicate") {
            let (inner, body_str) = read_post_body(stream);
            let new_name = serde_json::from_str::<serde_json::Value>(&body_str)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            match crate::net_dashboard::profiles::duplicate_brew_profile(&config_path, src, &new_name) {
                Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                Err(e) => http_response(inner, 400, &e),
            }
        } else if let Some(src) = name.strip_suffix("/rename") {
            let (inner, body_str) = read_post_body(stream);
            let new_name = serde_json::from_str::<serde_json::Value>(&body_str)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            match crate::net_dashboard::profiles::rename_brew_profile(&config_path, src, &new_name) {
                Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                Err(e) => http_response(inner, 400, &e),
            }
        } else {
            let (inner, body_str) = read_post_body(stream);
            match serde_json::from_str::<serde_json::Value>(&body_str) {
                Ok(v) => match crate::net_dashboard::profiles::put_brew_profile(&config_path, &name, &v) {
                    Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                    Err(e) => http_response(inner, 400, &e),
                },
                Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
            }
        }
    } else if req_line.contains("POST /api/profiles/cell") {
        // Create: body { "name": "...", ...profile fields } or { "name", "from_visual": true, ... }
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => {
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                if name.is_empty() {
                    http_response(inner, 400, "missing name");
                } else if v.get("from_visual").and_then(|x| x.as_bool()).unwrap_or(false) {
                    match crate::net_dashboard::profiles::save_cell_from_visual(&config_path, &name, &v) {
                        Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                        Err(e) => http_response(inner, 400, &e),
                    }
                } else {
                    match crate::net_dashboard::profiles::put_cell_profile(&config_path, &name, &v) {
                        Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                        Err(e) => http_response(inner, 400, &e),
                    }
                }
            }
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if req_line.contains("POST /api/profiles/brew") {
        let (inner, body_str) = read_post_body(stream);
        match serde_json::from_str::<serde_json::Value>(&body_str) {
            Ok(v) => {
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                if name.is_empty() {
                    http_response(inner, 400, "missing name");
                } else if v.get("from_visual").and_then(|x| x.as_bool()).unwrap_or(false) {
                    match crate::net_dashboard::profiles::save_brew_from_visual(&config_path, &name, &v) {
                        Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                        Err(e) => http_response(inner, 400, &e),
                    }
                } else {
                    let brew = v.get("brew").cloned().unwrap_or(v);
                    match crate::net_dashboard::profiles::put_brew_profile(&config_path, &name, &brew) {
                        Ok(()) => http_json_response(inner, 200, r#"{"ok":true}"#),
                        Err(e) => http_response(inner, 400, &e),
                    }
                }
            }
            Err(e) => http_response(inner, 400, &format!("invalid JSON: {e}")),
        }
    } else if let Some(name) = profile_path_name(&req_line, "DELETE /api/profiles/cell/") {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::delete_cell_profile(&config_path, &name) {
            Ok(()) => http_json_response(s, 200, r#"{"ok":true}"#),
            Err(e) => http_response(s, 400, &e),
        }
    } else if let Some(name) = profile_path_name(&req_line, "DELETE /api/profiles/brew/") {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::profiles::delete_brew_profile(&config_path, &name) {
            Ok(()) => http_json_response(s, 200, r#"{"ok":true}"#),
            Err(e) => http_response(s, 400, &e),
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
    } else if req_line.contains("GET /api/cells") {
        let mut s = stream;
        drain_http_headers(&mut s);
        let body = match &shared_config {
            Some(cfg) => crate::net_dashboard::cells::cells_json(cfg).to_string(),
            None => "{\"cells\":[]}".to_string(),
        };
        http_json_response(s, 200, &body);
    } else if req_line.contains("GET /api/roaming") {
        let mut s = stream;
        drain_http_headers(&mut s);
        match crate::net_dashboard::roaming::roaming_json(&config_path, shared_config.as_ref()) {
            Ok(v) => http_json_response(s, 200, &v.to_string()),
            Err(e) => http_response(s, 500, &e),
        }
    } else if req_line.contains("POST /api/roaming") {
        let (inner, body_str) = read_post_body(stream);
        serve_roaming_save(inner, &config_path, &body_str);
    } else if req_line.contains("POST /api/cells/add") {
        let (inner, body_str) = read_post_body(stream);
        serve_cells_add(inner, &config_path, &body_str);
    } else if req_line.contains("POST /api/cells/remove") {
        let (inner, body_str) = read_post_body(stream);
        serve_cells_remove(inner, &config_path, &body_str);
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
    } else if req_line.contains("GET /api/dashboard-auth") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dashboard_auth_get(s, &auth);
    } else if req_line.contains("POST /api/dashboard-auth") {
        let (inner, body_str) = read_post_body(stream);
        serve_dashboard_auth_post(inner, &auth, &sessions, &config_path, &body_str);
    } else if req_line.contains("GET /api/dashboard-ports") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dashboard_ports_get(s, &shared_config);
    } else if req_line.contains("POST /api/dashboard-ports") {
        let (inner, body_str) = read_post_body(stream);
        serve_dashboard_ports_post(inner, &config_path, &body_str);
    } else if req_line.contains("GET /api/station/export") {
        drain_http_headers(&mut stream);
        serve_station_export(stream, &config_path);
    } else if req_line.contains("POST /api/station/import") {
        let body = read_http_body(&mut stream);
        serve_station_import(stream, &config_path, &body);
    } else if req_line.contains("GET /api/profiles/export") {
        drain_http_headers(&mut stream);
        serve_profiles_export(stream, &config_path);
    } else if req_line.contains("POST /api/profiles/import") {
        let body = read_http_body(&mut stream);
        serve_profiles_import(stream, &config_path, &body);
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
    } else if req_line.contains("GET /api/sds-commands") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_sds_commands_get(buf.into_inner(), &shared_config);
    } else if req_line.contains("POST /api/sds-commands") {
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
        serve_sds_commands_post(buf.into_inner(), &shared_config, &config_path, body_str.as_ref());
    } else if req_line.contains("POST /api/telegram/verify") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_verify(inner, &shared_config, &body_str);
    } else if req_line.contains("POST /api/telegram/detect") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_detect(inner, &shared_config, &body_str);
    } else if req_line.contains("POST /api/telegram/test") {
        let (inner, body_str) = read_post_body(stream);
        serve_telegram_test(inner, &shared_config, &body_str);
    } else if req_line.contains("POST /api/security/generate") {
        let (inner, body_str) = read_post_body(stream);
        serve_security_generate(inner, &body_str);
    } else if req_line.contains("POST /api/security/restart") {
        let (inner, _) = read_post_body(stream);
        tracing::info!("Dashboard: restart requested from the Security page");
        crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, std::time::Duration::from_secs(2));
        http_json_response(inner, 200, "{\"ok\":true}");
    } else if req_line.contains("GET /api/security") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_security_get(s, &shared_config, &config_path);
    } else if req_line.contains("POST /api/security") {
        let (inner, body_str) = read_post_body(stream);
        serve_security_post(inner, &shared_config, &config_path, &body_str);
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
    } else if req_line.contains("DELETE /api/dgna-log") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dgna_log_clear(s, &state);
    } else if req_line.contains("GET /api/dapnet-log") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dapnet_log(s, &state);
    } else if req_line.contains("GET /api/dgna-log") {
        let mut s = stream;
        drain_http_headers(&mut s);
        serve_dgna_log(s, &state);
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
                    // The broadcast is serialized with write_bits(source_ssi, 24). `as u32` on the
                    // raw JSON number let anything >= 2^24 through to that assertion, which aborts
                    // the single stack thread — one POST would crash-loop the cell.
                    let source_issi = v.get("source_issi").and_then(|s| s.as_u64()).unwrap_or(16_777_215);
                    if !is_valid_ssi(source_issi) {
                        http_response(buf.into_inner(), 400, "source_issi out of range (must be 1..=16777215)");
                        return;
                    }
                    let source_issi = source_issi as u32;
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
    // ── LST Dispatch ───────────────────────────────────────────────────
    } else if req_line.contains("GET /api/lst/status") {
        drain_http_headers(&mut stream);
        let body = match &lst_handle {
            Some(h) => h.status_json().to_string(),
            None => r#"{"enabled":false,"session_busy":false}"#.to_string(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("GET /api/lst/positions") {
        drain_http_headers(&mut stream);
        // Global LIP store on dashboard state — works with or without LST Dispatch.
        let body = state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .lip_positions_json()
            .to_string();
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/lst/claim") {
        drain_http_headers(&mut stream);
        let Some(h) = &lst_handle else {
            http_json_response(stream, 503, r#"{"ok":false,"error":"LST not active"}"#);
            return;
        };
        let label = peer_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "operator".into());
        match h.claim(label) {
            crate::net_lst_dispatch::ClaimResult::Ok { token } => {
                http_json_response(
                    stream,
                    200,
                    &serde_json::json!({"ok":true,"token":token.to_string()}).to_string(),
                );
            }
            crate::net_lst_dispatch::ClaimResult::Busy { holder } => {
                http_json_response(
                    stream,
                    409,
                    &serde_json::json!({"ok":false,"error":"busy","holder":holder}).to_string(),
                );
            }
        }
    } else if req_line.contains("POST /api/lst/release") {
        let body = read_http_body(&mut stream);
        let Some(h) = &lst_handle else {
            http_json_response(stream, 503, r#"{"ok":false,"error":"LST not active"}"#);
            return;
        };
        let token = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string()))
            .and_then(|s| uuid::Uuid::parse_str(&s).ok());
        let ok = token.map(|t| h.release(t)).unwrap_or(false);
        http_json_response(stream, 200, &format!(r#"{{"ok":{ok}}}"#));
    } else if req_line.contains("GET /api/lst/dl") {
        drain_http_headers(&mut stream);
        let token = req_line
            .split("token=")
            .nth(1)
            .and_then(|s| s.split(|c: char| c == ' ' || c == '&').next())
            .and_then(|s| uuid::Uuid::parse_str(s.trim()).ok());
        let Some(h) = &lst_handle else {
            let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
            return;
        };
        let Some(token) = token else {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
            return;
        };
        let chunks = h.take_dl_pcm(token, 8);
        let mut bytes = Vec::new();
        for c in chunks {
            for s in c {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
        }
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        let _ = stream.write_all(hdr.as_bytes());
        let _ = stream.write_all(&bytes);
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
    // ── Host network (Ethernet + overview) ─────────────────────────────
    // Complements /api/wifi/*: lists all host LAN ifaces and manages
    // ethernet profiles. Wi-Fi scan/connect stays under /api/wifi.
    } else if req_line.contains("GET /api/network/status") {
        drain_http_headers(&mut stream);
        let body = match crate::host_network::status() {
            Ok(s) => serde_json::to_string(&serde_json::json!({"ok": true, "status": s})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("GET /api/network/ethernet/saved") {
        drain_http_headers(&mut stream);
        let body = match crate::host_network::list_ethernet_saved() {
            Ok(profiles) => serde_json::to_string(&serde_json::json!({"ok": true, "profiles": profiles})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/network/ethernet/up") {
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
        tracing::info!("Dashboard: bringing ethernet profile up uuid={}", uuid);
        let body = match crate::host_network::ethernet_up(uuid) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if req_line.contains("POST /api/network/ethernet/down") {
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
        tracing::info!("Dashboard: bringing ethernet profile down uuid={}", uuid);
        let body = match crate::host_network::ethernet_down(uuid) {
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
    stream: PrefixedConn,
    state: DashboardState,
    clients: WsClients,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    mm_cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    lst_handle: Option<crate::net_lst_dispatch::LstDispatchHandle>,
    private_http_session: Option<PrivateHttpSession>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));
    if private_http_session.as_ref().is_some_and(|session| !session.is_authorized()) {
        http_response(stream, 401, "Dashboard authentication changed; log in again");
        return;
    }

    // Note: cookie-based auth is checked by handle_connection BEFORE we get here, so we don't
    // re-validate during the WS upgrade. The cookie travelled on the Upgrade request.
    let callback = move |_req: &Request, res: Response| -> Result<Response, _> { Ok(res) };

    let mut ws = match accept_hdr(stream, callback) {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!("WS handshake failed: {}", e);
            return;
        }
    };

    if private_http_session.as_ref().is_some_and(|session| !session.is_authorized()) {
        let _ = ws.close(None);
        return;
    }

    // Register this connection for broadcasts. The queue is bounded (a non-draining client is dropped
    // in broadcast() rather than buffered without bound), and total clients are capped so many idle
    // connections can't grow memory/threads without bound.
    let (broadcast_tx, broadcast_rx) = crossbeam_channel::bounded::<String>(WS_CLIENT_QUEUE);
    {
        let mut c = clients.lock().unwrap();
        if c.len() >= WS_MAX_CLIENTS {
            tracing::warn!("Dashboard: WS client cap ({}) reached, refusing new connection", WS_MAX_CLIENTS);
            let _ = ws.close(None);
            return;
        }
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
        let cell_rf: Vec<serde_json::Value> = s.cell_rf.values().cloned().collect();
        let last_sys_health = s.last_sys_health.clone();
        let last_health = s.last_health.clone();
        let dgna_log: Vec<_> = s.dgna_log.iter().rev().cloned().collect();
        let boot_id = s.boot_id.clone();
        let uptime_secs = s.started_at.elapsed().as_secs();
        drop(s);
        let (dgna_default_attachment_mode, dgna_attachment_mode_picker_enabled) = shared_config
            .as_ref()
            .map(|cfg| {
                (
                    cfg.config().cell.dgna_attachment_mode,
                    cfg.config()
                        .dashboard
                        .as_ref()
                        .map(|d| d.show_dgna_attachment_mode_picker)
                        .unwrap_or(false),
                )
            })
            .unwrap_or((0, false));
        let cell_security = shared_config.as_ref().map(|cfg| cell_security_json(&cfg.config().security));
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "type": "snapshot", "ms": ms, "calls": calls, "emergencies": emergencies, "log": logs,
            "cell_security": cell_security,
            "brew_online": brew_online, "brew_version": brew_version, "last_heard": last_heard,
            "fallback_config_active": fallback_active, "fallback_config_reason": fallback_reason,
            "last_tx_visual": last_tx_visual,
            "last_tx_quality": last_tx_quality,
            "last_sdr_health": last_sdr_health,
            "cell_rf": cell_rf,
            "last_sys_health": last_sys_health,
            "health": last_health,
            "dgna_log": dgna_log,
            "dgna_default_attachment_mode": dgna_default_attachment_mode,
            "dgna_attachment_mode_picker_enabled": dgna_attachment_mode_picker_enabled,
            "boot_id": boot_id,
            "uptime_secs": uptime_secs,
            "stack_version": tetra_core::STACK_VERSION,
        })) {
            let _ = ws.send(Message::Text(json));
        }
    }

    let _ = ws.get_ref().set_read_timeout(Some(std::time::Duration::from_millis(20)));
    let mut last_hello = std::time::Instant::now();

    loop {
        if private_http_session.as_ref().is_some_and(|session| !session.is_authorized()) {
            let _ = ws.close(None);
            return;
        }
        // Drain outbound broadcast messages first
        loop {
            match broadcast_rx.try_recv() {
                Ok(msg) => {
                    if ws.send(Message::Text(msg)).is_err() {
                        return;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                // broadcast() pruned this client (its queue filled up). Close the socket so the
                // browser reconnects and resyncs instead of sitting on a silent, "alive" link.
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return;
                }
            }
        }

        // Application-layer heartbeat so the browser can detect a dead/zombie link
        // and a process restart (boot_id change) without waiting for a failed POST.
        if last_hello.elapsed() >= std::time::Duration::from_secs(5) {
            last_hello = std::time::Instant::now();
            let (boot_id, uptime_secs) = {
                let s = state.read().unwrap();
                (s.boot_id.clone(), s.started_at.elapsed().as_secs())
            };
            if let Ok(json) = serde_json::to_string(&serde_json::json!({
                "type": "hello",
                "boot_id": boot_id,
                "uptime_secs": uptime_secs,
                "stack_version": tetra_core::STACK_VERSION,
            })) {
                if ws.send(Message::Text(json)).is_err() {
                    return;
                }
            }
        }

        // Then check for inbound messages from browser
        match ws.read() {
            Ok(Message::Text(text)) => {
                if private_http_session.as_ref().is_some_and(|session| !session.is_authorized()) {
                    let _ = ws.close(None);
                    return;
                }
                handle_ws_command(
                    &text,
                    &state,
                    &cmd_tx,
                    &mm_cmd_tx,
                    &update_state,
                    &shared_config,
                    &lst_handle,
                    private_http_session.as_ref(),
                );
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

fn handle_ws_command(
    text: &str,
    state: &DashboardState,
    cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
    mm_cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
    update_state: &SharedUpdateState,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    lst_handle: &Option<crate::net_lst_dispatch::LstDispatchHandle>,
    private_http_session: Option<&PrivateHttpSession>,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    let cmd_type = v.get("type").and_then(|t| t.as_str());

    let send_cmd = |cmd: ControlCommand| -> bool {
        // Dashboard commands enter the entity's own control channel directly. Keep this in
        // lockstep with ControlWorker routing: SCK OTAR and DGNA belong to MM; other current
        // dashboard commands belong to CMCE.
        let target_tx = match cmd.target_entity() {
            tetra_core::tetra_entities::TetraEntity::Mm => mm_cmd_tx,
            tetra_core::tetra_entities::TetraEntity::Cmce => cmd_tx,
            other => {
                tracing::warn!("Dashboard: no command channel wired for {:?}", other);
                return false;
            }
        };
        if let Some(session) = private_http_session {
            // Only the nonblocking enqueue is inside the auth/session critical section. Parsing,
            // state/log updates and dispatcher execution must never run with these locks held.
            let mut sent = false;
            return session.with_authorization(|| {
                if let Ok(guard) = target_tx.lock() {
                    if let Some(ref tx) = *guard {
                        sent = tx.try_send(cmd).is_ok();
                    }
                }
            }) && sent;
        }
        if let Ok(guard) = target_tx.lock() {
            if let Some(ref tx) = *guard {
                return tx.send(cmd).is_ok();
            }
        }
        false
    };

    match cmd_type {
        Some("otar_sck") => {
            let Some(issi) = json_ssi(&v, "issi") else {
                reject_ws_ssi(state, "otar_sck", "issi", v.get("issi"));
                return;
            };
            tracing::info!("Dashboard: OTAR SCK to ISSI {}", issi);
            if !send_cmd(ControlCommand::OtarSck { issi }) {
                tracing::warn!("Dashboard: no control dispatcher for otar_sck");
            }
            let mut s = state.write().unwrap();
            s.push_log("INFO", format!("OTAR: sending SCK to ISSI {}", issi));
        }
        Some("kick") => {
            let Some(issi) = json_ssi(&v, "issi") else {
                reject_ws_ssi(state, "kick", "issi", v.get("issi"));
                return;
            };
            tracing::info!("Dashboard: kick ISSI {}", issi);
            if !send_cmd(ControlCommand::KickMs { issi }) {
                tracing::warn!("Dashboard: no control dispatcher for kick");
            }
            let mut s = state.write().unwrap();
            s.push_log("INFO", format!("Kick requested for ISSI {}", issi));
        }
        Some("restart") => {
            tracing::info!("Dashboard: restart service requested");
            let schedule = || {
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Restart,
                    std::time::Duration::from_millis(500),
                );
            };
            if let Some(session) = private_http_session {
                session.with_authorization(schedule);
            } else {
                schedule();
            }
        }
        Some("shutdown") => {
            tracing::info!("Dashboard: soft shutdown (standby) requested");
            let schedule = || {
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Stop,
                    std::time::Duration::from_millis(500),
                );
            };
            if let Some(session) = private_http_session {
                session.with_authorization(schedule);
            } else {
                schedule();
            }
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
            // The destination lands in D-SDS-DATA's 24-bit address field. `as u32` silently
            // truncated a wider JSON value into an in-range-looking one only by accident — values
            // between 2^24 and 2^32 reached write_bits and aborted the stack thread.
            let Some(dest) = json_ssi(&v, "dest_issi") else {
                reject_ws_ssi(state, "sds", "dest_issi", v.get("dest_issi"));
                return;
            };
            let msg_text = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            if msg_text.is_empty() {
                return;
            }
            tracing::info!("Dashboard: SDS to {} = {}", dest, msg_text);

            // Bare text payload for ControlCommand::SendSds. CMCE wraps it in the SDS-TL header,
            // which already carries the ISO-8859-1 coding-scheme octet, so the payload must not
            // prepend its own — doing so put a spurious leading box in front of the text on the
            // receiving terminal (issue #55).
            let (len_bits, payload) = crate::tpg2200::build_sds_text_payload(&msg_text);

            send_cmd(ControlCommand::SendSds {
                handle: 0,
                source_ssi: v
                    .get("source_issi")
                    .and_then(|s| s.as_u64())
                    .map(|n| n as u32)
                    .filter(|&n| n >= 1 && n <= 16_777_214)
                    .unwrap_or(9999),
                dest_ssi: dest,
                dest_is_group: v.get("dest_is_group").and_then(|d| d.as_bool()).unwrap_or(false),
                len_bits,
                payload,
            });
            let mut s = state.write().unwrap();
            s.push_log("INFO", format!("SDS sent to {}: {}", dest, msg_text));
        }
        Some("dgna") => {
            let Some(issi) = json_ssi(&v, "issi") else {
                reject_ws_ssi(state, "dgna", "issi", v.get("issi"));
                return;
            };
            let Some(gssi) = json_ssi(&v, "gssi") else {
                reject_ws_ssi(state, "dgna", "gssi", v.get("gssi"));
                return;
            };
            let mnemonic = v
                .get("mnemonic")
                .and_then(|m| m.as_str())
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(|m| m.chars().take(15).collect::<String>());
            let attach = v.get("attach").and_then(|a| a.as_bool()).unwrap_or(true);
            let (default_attachment_mode, picker_enabled) = shared_config
                .as_ref()
                .map(|cfg| {
                    (
                        cfg.config().cell.dgna_attachment_mode,
                        cfg.config()
                            .dashboard
                            .as_ref()
                            .map(|d| d.show_dgna_attachment_mode_picker)
                            .unwrap_or(false),
                    )
                })
                .unwrap_or((0, false));
            let attachment_mode = if picker_enabled {
                v.get("attachment_mode")
                    .and_then(|m| m.as_u64())
                    .map(|m| m.min(5) as u8)
                    .unwrap_or(default_attachment_mode)
            } else {
                default_attachment_mode
            };
            let verb = if attach { "assign" } else { "deassign" };
            tracing::info!(
                "Dashboard: DGNA {} GSSI {} on ISSI {} (mnemonic={:?}, attachment_mode={})",
                verb,
                gssi,
                issi,
                mnemonic,
                attachment_mode
            );
            if !send_cmd(ControlCommand::Dgna {
                issi,
                gssi,
                mnemonic: if attach { mnemonic.clone() } else { None },
                attachment_mode,
                attach,
            }) {
                tracing::warn!("Dashboard: no control dispatcher for DGNA");
            }
            let mut s = state.write().unwrap();
            s.push_log(
                "INFO",
                format!(
                    "DGNA {} requested: GSSI {} {} ISSI {}{}",
                    verb,
                    gssi,
                    if attach { "to" } else { "from" },
                    issi,
                    format!(
                        "{} (mode: {})",
                        mnemonic.as_ref().map(|m| format!(" (name: {})", m)).unwrap_or_default(),
                        attachment_mode
                    )
                ),
            );
        }
        Some("dgna_bulk") => {
            let Some(gssi) = json_ssi(&v, "gssi") else {
                reject_ws_ssi(state, "dgna_bulk", "gssi", v.get("gssi"));
                return;
            };
            let mnemonic = v
                .get("mnemonic")
                .and_then(|m| m.as_str())
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(|m| m.chars().take(15).collect::<String>());
            let attach = v.get("attach").and_then(|a| a.as_bool()).unwrap_or(true);
            let all_radios = v.get("all_radios").and_then(|a| a.as_bool()).unwrap_or(false);
            let (default_attachment_mode, picker_enabled) = shared_config
                .as_ref()
                .map(|cfg| {
                    (
                        cfg.config().cell.dgna_attachment_mode,
                        cfg.config()
                            .dashboard
                            .as_ref()
                            .map(|d| d.show_dgna_attachment_mode_picker)
                            .unwrap_or(false),
                    )
                })
                .unwrap_or((0, false));
            let attachment_mode = if picker_enabled {
                v.get("attachment_mode")
                    .and_then(|m| m.as_u64())
                    .map(|m| m.min(5) as u8)
                    .unwrap_or(default_attachment_mode)
            } else {
                default_attachment_mode
            };
            let mut targets = Vec::<u32>::new();
            if all_radios {
                let mut issis: Vec<u32> = state
                    .read()
                    .unwrap()
                    .ms_map
                    .keys()
                    .copied()
                    .filter(|i| is_valid_ssi(*i as u64))
                    .collect();
                issis.sort_unstable();
                issis.dedup();
                targets = issis;
            } else if let Some(arr) = v.get("targets").and_then(|t| t.as_array()) {
                // Every target is addressed individually, so one out-of-range entry in the list is
                // enough to panic the serializer. Drop the bad ones and tell the operator, rather
                // than sending the good half and crashing on the rest.
                let mut refused = 0usize;
                for value in arr {
                    match value.as_u64() {
                        Some(n) if is_valid_ssi(n) => {
                            let issi = n as u32;
                            if !targets.contains(&issi) {
                                targets.push(issi);
                            }
                        }
                        _ => refused += 1,
                    }
                }
                targets.sort_unstable();
                if refused > 0 {
                    reject_ws_ssi(state, "dgna_bulk", "targets", v.get("targets"));
                }
            }
            if targets.is_empty() {
                return;
            }
            let verb = if attach { "assign" } else { "deassign" };
            tracing::info!(
                "Dashboard: DGNA bulk {} GSSI {} on {} radios (mnemonic={:?}, attachment_mode={})",
                verb,
                gssi,
                targets.len(),
                mnemonic,
                attachment_mode
            );
            let mut sent = 0usize;
            for issi in &targets {
                if send_cmd(ControlCommand::Dgna {
                    issi: *issi,
                    gssi,
                    mnemonic: if attach { mnemonic.clone() } else { None },
                    attachment_mode,
                    attach,
                }) {
                    sent += 1;
                }
            }
            if sent == 0 {
                tracing::warn!("Dashboard: no control dispatcher for DGNA bulk");
            }
            let mut s = state.write().unwrap();
            s.push_log(
                "INFO",
                format!(
                    "DGNA bulk {} requested: GSSI {} on {} radios{} (mode: {})",
                    verb,
                    gssi,
                    targets.len(),
                    mnemonic.as_ref().map(|m| format!(" (name: {m})")).unwrap_or_default(),
                    attachment_mode
                ),
            );
        }
        Some("emergency_clear") => {
            let Some(issi) = json_ssi(&v, "issi") else {
                reject_ws_ssi(state, "emergency_clear", "issi", v.get("issi"));
                return;
            };
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
        Some("lst_heartbeat") | Some("lst_join") | Some("lst_scan") | Some("lst_leave") | Some("lst_ptt")
        | Some("lst_private") | Some("lst_answer") | Some("lst_hangup") | Some("lst_set_issi")
        | Some("lst_ul_pcm") => {
            let Some(h) = lst_handle else {
                return;
            };
            let Some(token) = v
                .get("token")
                .and_then(|t| t.as_str())
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
            else {
                return;
            };
            use crate::net_lst_dispatch::LstUiCommand;
            match cmd_type {
                Some("lst_heartbeat") => {
                    let _ = h.heartbeat(token);
                }
                Some("lst_join") => {
                    if let Some(gssi) = json_ssi(&v, "gssi") {
                        let _ = h.push_cmd(token, LstUiCommand::JoinGroup { gssi });
                    }
                }
                Some("lst_scan") => {
                    let list = v
                        .get("list")
                        .and_then(|a| a.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_u64().map(|n| n as u32))
                                .filter(|n| *n > 0 && *n <= 16_777_214)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let tx = v
                        .get("tx")
                        .and_then(|t| t.as_u64())
                        .map(|n| n as u32)
                        .unwrap_or(0);
                    let _ = h.push_cmd(token, LstUiCommand::SetScanList { list, tx });
                }
                Some("lst_leave") => {
                    let _ = h.push_cmd(token, LstUiCommand::LeaveGroup);
                }
                Some("lst_ptt") => {
                    let down = v.get("down").and_then(|d| d.as_bool()).unwrap_or(false);
                    let _ = h.push_cmd(token, LstUiCommand::Ptt { down });
                }
                Some("lst_private") => {
                    if let Some(dest_issi) = json_ssi(&v, "issi") {
                        let duplex = v.get("duplex").and_then(|d| d.as_bool()).unwrap_or(false);
                        let _ = h.push_cmd(token, LstUiCommand::PrivateCall { dest_issi, duplex });
                    }
                }
                Some("lst_answer") => {
                    let _ = h.push_cmd(token, LstUiCommand::Answer);
                }
                Some("lst_hangup") => {
                    let _ = h.push_cmd(token, LstUiCommand::Hangup);
                }
                Some("lst_set_issi") => {
                    if let Some(issi) = json_ssi(&v, "issi") {
                        let _ = h.push_cmd(token, LstUiCommand::SetOperatorIssi { issi });
                    }
                }
                Some("lst_ul_pcm") => {
                    if let Some(b64) = v.get("pcm").and_then(|p| p.as_str()) {
                        if let Ok(raw) = lst_b64_decode(b64) {
                            if raw.len() >= 2 && raw.len() % 2 == 0 && raw.len() <= 16_000 {
                                let mut pcm = Vec::with_capacity(raw.len() / 2);
                                for chunk in raw.chunks_exact(2) {
                                    pcm.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                                }
                                let _ = h.push_ul_pcm(token, pcm);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn serve_update_status(mut stream: PrefixedConn, update_state: &SharedUpdateState) {
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

/// GET /api/callsigns?ids=1,2,3 — resolve ISSIs to RadioID callsigns ("indicative"). Returns a JSON
/// object `{ "<id>": {"cs":"CALLSIGN","fl":"🇷🇴"} }` for resolved IDs (`fl` is the country flag emoji
/// derived from the call-sign prefix, or empty if unknown) and `{ "<id>": "" }` for IDs confirmed
/// absent from RadioID. IDs still being fetched in the background are OMITTED, so the client retries
/// them on a later poll. Lookups are non-blocking — unknown IDs are queued for background resolution.
fn serve_callsigns(stream: PrefixedConn, radioid: &crate::net_dashboard::radioid::RadioIdCache, req_line: &str) {
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
        // An operator-assigned name wins over the RadioID callsign.
        if let Some(name) = radioid.local_name(id) {
            let mut entry = serde_json::Map::new();
            entry.insert("cs".to_string(), serde_json::Value::String(name));
            entry.insert("fl".to_string(), serde_json::Value::String(String::new()));
            entry.insert("local".to_string(), serde_json::Value::Bool(true));
            map.insert(id.to_string(), serde_json::Value::Object(entry));
            continue;
        }
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

/// `{"<issi>": "<name>", …}` for every operator-assigned radio name.
fn radio_names_json(radioid: &crate::net_dashboard::radioid::RadioIdCache) -> String {
    let mut map = serde_json::Map::new();
    for (issi, name) in radioid.local_names() {
        map.insert(issi.to_string(), serde_json::Value::String(name));
    }
    serde_json::Value::Object(map).to_string()
}

/// GET /api/update/check — compare the running build against the tip of the active OTA
/// channel branch on github.com/Aitorrio/bost-flowstation (fallback: latest GitHub Release).
/// Best-effort; on any failure returns check_failed=true so the dashboard hides the badge.
///
/// Query: `?refresh=1` bypasses success cache; `?notes=1` also fetches changelog text
/// (OTA modal). Boot/badge should omit notes to keep the Pi responsive.
fn serve_update_check(mut stream: PrefixedConn, config_path: &str, refresh: bool, with_notes: bool) {
    let channel = crate::net_dashboard::ota_channel::read_ota_channel(config_path);
    const CACHE_OK_SECS: u64 = 90;
    // Cache key includes notes so a light boot result does not starve the modal of changelog.
    type CacheEntry = (std::time::Instant, String, bool, String); // at, channel, with_notes, json
    static CACHE: std::sync::Mutex<Option<CacheEntry>> = std::sync::Mutex::new(None);
    // Coalesce concurrent identical GitHub checks (System open + boot + Update).
    type Slot = std::sync::Arc<(std::sync::Mutex<Option<String>>, std::sync::Condvar)>;
    static INFLIGHT: std::sync::Mutex<Option<(String, bool, Slot)>> = std::sync::Mutex::new(None);

    let cached = {
        let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if !refresh {
            if let Some((at, ch, notes, json)) = guard.as_ref() {
                if ch == &channel
                    && *notes == with_notes
                    && at.elapsed() < std::time::Duration::from_secs(CACHE_OK_SECS)
                    && !json.contains("\"check_failed\":true")
                {
                    Some(json.clone())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    let body = if let Some(json) = cached {
        json
    } else {
        let (is_leader, slot): (bool, Slot) = {
            let mut gate = INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((ch, notes, existing)) = gate.as_ref() {
                if ch == &channel && *notes == with_notes {
                    (false, existing.clone())
                } else {
                    // Different key already running — do not steal; run our own (rare).
                    let fresh: Slot = std::sync::Arc::new((
                        std::sync::Mutex::new(None),
                        std::sync::Condvar::new(),
                    ));
                    (true, fresh)
                }
            } else {
                let fresh: Slot = std::sync::Arc::new((
                    std::sync::Mutex::new(None),
                    std::sync::Condvar::new(),
                ));
                *gate = Some((channel.clone(), with_notes, fresh.clone()));
                (true, fresh)
            }
        };

        let json = if is_leader {
            let result = crate::net_dashboard::update_check::check_for_update(
                tetra_core::STACK_VERSION,
                &channel,
                with_notes,
            );
            let json = result.to_json();
            if !result.check_failed {
                if let Ok(mut guard) = CACHE.lock() {
                    *guard = Some((
                        std::time::Instant::now(),
                        channel.clone(),
                        with_notes,
                        json.clone(),
                    ));
                }
            } else if let Ok(mut guard) = CACHE.lock() {
                if guard
                    .as_ref()
                    .map(|(_, ch, _, _)| ch == &channel)
                    .unwrap_or(false)
                {
                    *guard = None;
                }
            }
            {
                let (lock, cv) = &*slot;
                let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
                *st = Some(json.clone());
                cv.notify_all();
            }
            if let Ok(mut gate) = INFLIGHT.lock() {
                if let Some((ch, notes, _)) = gate.as_ref() {
                    if ch == &channel && *notes == with_notes {
                        *gate = None;
                    }
                }
            }
            json
        } else {
            let (lock, cv) = &*slot;
            let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
            while st.is_none() {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let (guard, _) = cv
                    .wait_timeout(st, deadline.saturating_duration_since(now))
                    .unwrap_or_else(|e| e.into_inner());
                st = guard;
            }
            st.clone().unwrap_or_else(|| {
                crate::net_dashboard::update_check::check_for_update(
                    tetra_core::STACK_VERSION,
                    &channel,
                    with_notes,
                )
                .to_json()
            })
        };
        json
    };

    // Even when git HEAD matches, keep "update available" if this binary has no ACELP link —
    // so the GUI can install libtetra-codec without SSH.
    let body = enrich_update_check_for_voice(body);

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn enrich_update_check_for_voice(body: String) -> String {
    let skip = std::env::var("BOST_SKIP_TETRA_CODEC")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let voice_needed = cfg!(not(feature = "asterisk")) && !skip;
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return body;
    };
    v["voice_codec_linked"] = serde_json::json!(cfg!(feature = "asterisk"));
    v["voice_rebuild_needed"] = serde_json::json!(voice_needed);
    if voice_needed {
        v["update_available"] = serde_json::json!(true);
        let notes = v
            .get("release_notes")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if notes.is_empty() {
            v["release_notes"] = serde_json::json!(
                "- Install TETRA voice codec (libtetra-codec) and rebuild for LST Dispatch audio.\n- Confirm this update — no SSH required.\n- After restart, open https://IP/ so the browser allows the microphone."
            );
            v["notes_source"] = serde_json::json!("changelog");
        }
        if v.get("latest").and_then(|x| x.as_str()).is_none() {
            v["latest"] =
                serde_json::json!(format!("{}+voice", tetra_core::STACK_VERSION));
        }
    }
    v.to_string()
}

/// GET /api/whitelist — return the effective whitelist as JSON:
/// `{"issi_whitelist":[...], "source":"override"|"config", "enabled":bool}`.
/// `enabled` is false when the list is empty (open network).
/// GET /api/btsinfo — static cell + RF identity pulled from the running config, for the
/// "TETRA BTS Details" card on the dashboard. Read-only; non-sensitive scalars only.
fn serve_bts_info(mut stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
                "sample_rate_hz": c.phy_io.soapysdr.as_ref().and_then(|s| s.fs)
                    .unwrap_or(crate::net_dashboard::dual_carrier::DEFAULT_SAMPLE_RATE_HZ),
                "sample_rate_from_toml": c.phy_io.soapysdr.as_ref().and_then(|s| s.fs).is_some(),
                "dual_carrier_active": c.cell.secondary_carrier.is_some(),
                "secondary_carrier": c.cell.secondary_carrier,
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

/// Write a config produced by the cells editor and restart to apply it.
fn apply_cells_config(stream: PrefixedConn, config_path: &str, text: &str, what: String) {
    let backup = format!("{config_path}.cells.bak");
    let _ = std::fs::copy(config_path, &backup);
    if let Err(e) = atomic_write(config_path, text) {
        return http_response(stream, 500, &format!("failed to write config: {e}"));
    }
    tracing::info!("Dashboard: {what}; scheduling restart");
    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::Restart,
        std::time::Duration::from_secs(2),
    );
    http_response(stream, 200, &format!("{what}; the base station is restarting to apply it."));
}

/// POST /api/roaming — see `roaming::apply_roaming`; writes the config and restarts.
fn serve_roaming_save(stream: PrefixedConn, config_path: &str, body: &str) {
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return http_response(stream, 400, &format!("invalid JSON: {e}")),
    };
    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => return http_response(stream, 500, &format!("cannot read config: {e}")),
    };
    match crate::net_dashboard::roaming::apply_roaming(&original, &req) {
        Ok(text) => apply_cells_config(stream, config_path, &text, "roaming settings saved".to_string()),
        Err(e) => http_response(stream, 400, &e),
    }
}

/// POST /api/cells/add — `{"device": "...", "main_carrier": N, "colour_code": N?, "location_area": N?}`.
fn serve_cells_add(stream: PrefixedConn, config_path: &str, body: &str) {
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return http_response(stream, 400, &format!("invalid JSON: {e}")),
    };
    let device = req.get("device").and_then(|v| v.as_str()).unwrap_or("");
    let Some(main_carrier) = req.get("main_carrier").and_then(|v| v.as_u64()).filter(|n| *n < 4096) else {
        return http_response(stream, 400, "main_carrier (0-4095) is required");
    };
    let colour_code = req.get("colour_code").and_then(|v| v.as_u64()).map(|v| v.min(63) as u8);
    let location_area = match req.get("location_area").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_u64().filter(|n| *n <= 16383) {
            Some(n) => Some(n as u16),
            None => return http_response(stream, 400, "location_area must be 0-16383"),
        },
    };
    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => return http_response(stream, 500, &format!("cannot read config: {e}")),
    };
    match crate::net_dashboard::cells::add_cell_toml(&original, device, main_carrier as u16, colour_code, location_area) {
        Ok((text, id)) => apply_cells_config(stream, config_path, &text, format!("cell {id} added (carrier {main_carrier})")),
        Err(e) => http_response(stream, 400, &e),
    }
}

/// POST /api/cells/remove — `{"id": N}`.
fn serve_cells_remove(stream: PrefixedConn, config_path: &str, body: &str) {
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return http_response(stream, 400, &format!("invalid JSON: {e}")),
    };
    let Some(id) = req.get("id").and_then(|v| v.as_u64()).and_then(|v| u8::try_from(v).ok()) else {
        return http_response(stream, 400, "id is required");
    };
    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => return http_response(stream, 500, &format!("cannot read config: {e}")),
    };
    match crate::net_dashboard::cells::remove_cell_toml(&original, id) {
        Ok(text) => apply_cells_config(stream, config_path, &text, format!("cell {id} removed")),
        Err(e) => http_response(stream, 400, &e),
    }
}

/// GET /api/dualcarrier — Dual-Carrier state for BTS Details + Config form helpers.
fn serve_dual_carrier_get(mut stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str) {
    use crate::net_dashboard::dual_carrier;
    let st = dual_carrier::read_dual_carrier(config_path);
    let main_carrier = shared_config.as_ref().map(|c| c.config().cell.main_carrier);
    let running_active = shared_config
        .as_ref()
        .map(|c| c.config().cell.secondary_carrier.is_some())
        .unwrap_or(false);
    let fs_toml = dual_carrier::read_sample_rate_from_toml(config_path);
    let sample_rate_hz = dual_carrier::effective_sample_rate_hz(fs_toml);
    let max_delta = dual_carrier::max_carrier_delta(sample_rate_hz);
    let (sec_min, sec_max) = match main_carrier {
        Some(m) => {
            let lo = m.saturating_sub(max_delta);
            let hi = (m + max_delta).min(3999);
            (lo, hi)
        }
        None => (0u16, 3999u16),
    };

    let body = serde_json::json!({
        "enabled": st.enabled,
        "secondary_carrier": st.secondary_carrier,
        "active": st.active(),
        "running_active": running_active,
        "main_carrier": main_carrier,
        "sample_rate_hz": sample_rate_hz,
        "sample_rate_from_toml": fs_toml.is_some(),
        "passband_max_delta": max_delta,
        "secondary_min": sec_min,
        "secondary_max": sec_max,
    })
    .to_string();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// POST /api/dualcarrier — enable/disable dual carrier (restart). Prefer Config form; kept for API.
fn serve_dual_carrier_post(
    stream: PrefixedConn,
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
    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => return http_response(stream, 500, &format!("cannot read config: {e}")),
    };

    if !enabled {
        if let Err(e) = dual_carrier::write_dual_carrier(config_path, false, None) {
            return http_response(stream, 500, &format!("failed to write config: {e}"));
        }
        let _ = dual_carrier::sync_active_cell_profile(
            config_path,
            false,
            current.secondary_carrier,
            None,
            None,
            None,
        );
        tracing::info!("Dashboard: Dual-Carrier set OFF; scheduling restart");
        crate::service_control::schedule_service_action(
            crate::service_control::ServiceAction::Restart,
            std::time::Duration::from_secs(2),
        );
        return http_response(
            stream,
            200,
            "Dual carrier disabled; the base station is restarting to apply it.",
        );
    }

    let main = shared_config
        .as_ref()
        .map(|c| c.config().cell.main_carrier)
        .or_else(|| {
            tetra_config::bluestation::parsing::from_toml_str(&original)
                .ok()
                .map(|c| c.cell.main_carrier)
        });
    let Some(main) = main else {
        return http_response(stream, 400, "main_carrier unknown — cannot enable dual carrier");
    };

    let fs_hz = dual_carrier::effective_sample_rate_hz(
        req.get("sample_rate_hz")
            .and_then(|v| v.as_f64())
            .or_else(|| dual_carrier::read_sample_rate_from_toml(config_path)),
    );

    let requested = match req.get("secondary_carrier").and_then(|v| v.as_u64()) {
        Some(n) if n >= 4000 => {
            return http_response(stream, 400, "secondary_carrier must be in 0..3999");
        }
        Some(n) => Some(n as u16),
        None => current.secondary_carrier,
    };
    let want = requested.unwrap_or(main.saturating_add(1));
    let secondary = dual_carrier::clamp_secondary_carrier(main, want, fs_hz);
    if secondary == main {
        return http_response(stream, 400, "secondary_carrier must differ from main_carrier");
    }

    let probe = dual_carrier::compute_toml(&original, true, Some(secondary));
    let cfg = match tetra_config::bluestation::parsing::from_toml_str(&probe) {
        Ok(c) => c,
        Err(e) => return http_response(stream, 400, &format!("resulting config does not parse: {e}")),
    };
    let carriers = match cfg.bs_phase_mod_carriers() {
        Ok(c) => c,
        Err(e) => return http_response(stream, 400, &format!("invalid carrier frequencies: {e}")),
    };
    let (main_dl, main_ul) = match carriers.iter().find(|(n, _, _)| *n == main) {
        Some((_, d, u)) => (*d as f64, *u as f64),
        None => return http_response(stream, 400, "main carrier missing from frequency table"),
    };
    let (sec_dl, sec_ul) = match carriers.iter().find(|(n, _, _)| *n == secondary) {
        Some((_, d, u)) => (*d as f64, *u as f64),
        None => return http_response(stream, 400, "secondary carrier missing from frequency table"),
    };

    let prospective =
        dual_carrier::build_enabled_toml(&original, secondary, fs_hz, main_dl, main_ul, sec_dl, sec_ul);
    match tetra_config::bluestation::parsing::from_toml_str(&prospective) {
        Ok(cfg) => {
            if let Err(e) = cfg.validate() {
                return http_response(stream, 400, &format!("resulting config is invalid: {e}"));
            }
        }
        Err(e) => return http_response(stream, 400, &format!("resulting config does not parse: {e}")),
    }

    if let Err(e) = dual_carrier::write_toml_body(config_path, &prospective) {
        return http_response(stream, 500, &format!("failed to write config: {e}"));
    }

    let tx_c = (main_dl + sec_dl) / 2.0;
    let rx_c = (main_ul + sec_ul) / 2.0;
    if let Err(e) = dual_carrier::sync_active_cell_profile(
        config_path,
        true,
        Some(secondary),
        Some(fs_hz),
        Some(tx_c),
        Some(rx_c),
    ) {
        tracing::warn!("Dashboard: Dual-Carrier enabled but Cell profile sync failed: {e}");
    }

    let _ = shared_config;
    tracing::info!(
        "Dashboard: Dual-Carrier set ON (secondary_carrier={secondary}, fs={fs_hz}); scheduling restart"
    );
    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::Restart,
        std::time::Duration::from_secs(2),
    );
    http_response(
        stream,
        200,
        "Dual carrier enabled; the base station is restarting to apply it.",
    );
}


fn serve_whitelist_get(mut stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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

/// Apply ISSI whitelist at runtime (override + kick) and persist to live TOML.
/// Returns Ok(()) even if TOML write fails after runtime apply (warns).
fn apply_issi_whitelist_live(
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    list: &[u32],
    cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
) -> Result<(), String> {
    use crate::net_dashboard::whitelist;

    let Some(cfg) = shared_config else {
        return Err("Config not available".into());
    };

    {
        let mut state = cfg.state_write();
        state.issi_whitelist_override = Some(list.to_vec());
    }

    // Enforce on already-registered terminals (whitelist is checked at registration).
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

    if let Err(e) = whitelist::write_whitelist_to_toml(config_path, list) {
        tracing::warn!(
            "Dashboard: whitelist applied at runtime but failed to persist to TOML: {}",
            e
        );
        return Err(format!("runtime ok; TOML write failed: {e}"));
    }
    Ok(())
}

/// POST /api/whitelist — set the whitelist. Body: JSON array `[1,2,3]` or
/// `{"issi_whitelist":[1,2,3]}`. Applies immediately via the StackState override AND
/// rewrites the TOML so it survives a restart. An empty list = open network.
///
/// Prefer `POST /api/profiles/cell/{name}/whitelist` so the list is bound to a Cell profile.
fn serve_whitelist_post(
    stream: PrefixedConn,
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

    match apply_issi_whitelist_live(shared_config, config_path, &list, cmd_tx) {
        Ok(()) => {
            tracing::info!("Dashboard: ISSI whitelist updated ({} entries)", list.len());
            http_response(stream, 200, "OK");
        }
        Err(e) if e.contains("TOML write failed") => {
            http_response(
                stream,
                200,
                "Applied at runtime; failed to write config file (check permissions)",
            );
        }
        Err(e) => http_response(stream, 503, &e),
    }
}

/// POST /api/profiles/cell/{name}/whitelist — persist ISSI list on the Cell profile.
/// If that Cell is active, also apply live TOML + runtime override.
fn serve_cell_whitelist_post(
    stream: PrefixedConn,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    cell_name: &str,
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

    let is_active = match crate::net_dashboard::profiles::set_cell_whitelist(
        config_path,
        cell_name,
        &list,
    ) {
        Ok(active) => active,
        Err(e) => {
            http_response(stream, 400, &e);
            return;
        }
    };

    let mut applied_live = false;
    if is_active {
        match apply_issi_whitelist_live(shared_config, config_path, &list, cmd_tx) {
            Ok(()) => applied_live = true,
            Err(e) if e.contains("TOML write failed") => applied_live = true,
            Err(e) => {
                tracing::warn!(
                    "Dashboard: cell whitelist saved but live apply failed: {}",
                    e
                );
            }
        }
    }

    tracing::info!(
        "Dashboard: Cell '{}' whitelist updated ({} entries, applied_live={})",
        cell_name,
        list.len(),
        applied_live
    );
    http_json_response(
        stream,
        200,
        &format!(r#"{{"ok":true,"applied_live":{}}}"#, applied_live),
    );
}

// ---------------------------------------------------------------------------
// Dashboard account (single username/password in [dashboard]). System-tab GUI.
// ---------------------------------------------------------------------------

/// GET /api/dashboard-auth — `{auth_enabled, username}` (never the password).
fn serve_dashboard_auth_get(stream: PrefixedConn, auth: &SharedAuth) {
    let creds = auth.read().unwrap_or_else(|e| e.into_inner()).clone();
    let body = match creds {
        Some((user, _)) => {
            let escaped = user.replace('\\', "\\\\").replace('"', "\\\"");
            format!(r#"{{"auth_enabled":true,"username":"{escaped}"}}"#)
        }
        None => r#"{"auth_enabled":false,"username":null}"#.to_string(),
    };
    http_json_response(stream, 200, &body);
}

/// POST /api/dashboard-auth — enable auth or change username/password.
///
/// Body (JSON):
/// - Enable (auth currently off): `{username, new_password, confirm_password}`
/// - Change (auth on): `{current_password, username?, new_password?, confirm_password?}`
///   At least one of username / new_password must be present when auth is on.
///
/// Persists via validate-before-write, hot-updates SharedAuth, invalidates sessions.
fn serve_dashboard_auth_post(
    stream: PrefixedConn,
    auth: &SharedAuth,
    sessions: &SharedSessionStore,
    config_path: &str,
    body: &str,
) {
    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };

    let current = auth.read().unwrap_or_else(|e| e.into_inner()).clone();

    let json_str = |k: &str| {
        json.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };

    let (new_user, new_pass) = match &current {
        None => {
            // Enable auth from open access.
            let user = match json_str("username").or_else(|| json_str("new_username")) {
                Some(u) => u,
                None => {
                    http_response(stream, 400, "username required to enable dashboard auth");
                    return;
                }
            };
            let pass = match json_str("new_password") {
                Some(p) => p,
                None => {
                    http_response(stream, 400, "new_password required to enable dashboard auth");
                    return;
                }
            };
            let confirm = json_str("confirm_password").unwrap_or_default();
            if pass != confirm {
                http_response(stream, 400, "new_password and confirm_password do not match");
                return;
            }
            if user.trim().is_empty() || pass.is_empty() {
                http_response(stream, 400, "username and password cannot be empty");
                return;
            }
            (user.trim().to_string(), pass)
        }
        Some((cur_user, cur_pass)) => {
            let current_password = json_str("current_password").unwrap_or_default();
            if !timing_safe_eq(current_password.as_bytes(), cur_pass.as_bytes()) {
                http_response(stream, 403, "current_password is incorrect");
                return;
            }

            let next_user = json_str("username")
                .or_else(|| json_str("new_username"))
                .unwrap_or_else(|| cur_user.clone());
            let next_user = next_user.trim().to_string();
            if next_user.is_empty() {
                http_response(stream, 400, "username cannot be empty");
                return;
            }

            let next_pass = match json_str("new_password") {
                Some(p) => {
                    let confirm = json_str("confirm_password").unwrap_or_default();
                    if p != confirm {
                        http_response(stream, 400, "new_password and confirm_password do not match");
                        return;
                    }
                    if p.is_empty() {
                        http_response(stream, 400, "new_password cannot be empty");
                        return;
                    }
                    p
                }
                None => cur_pass.clone(),
            };

            if next_user == *cur_user && next_pass == *cur_pass {
                http_response(stream, 400, "no changes requested");
                return;
            }
            (next_user, next_pass)
        }
    };

    // Persist: patch TOML → parse+validate → atomic write (same safety as /api/config).
    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => {
            http_response(stream, 500, &format!("cannot read config: {e}"));
            return;
        }
    };
    let patched =
        crate::net_dashboard::dashboard_auth::patch_dashboard_credentials(&original, &new_user, &new_pass);
    if let Err((code, msg)) = write_config_validated(config_path, &patched) {
        http_response(stream, code, &msg);
        return;
    }

    // Hot-update runtime credentials and force re-login.
    *auth.write().unwrap_or_else(|e| e.into_inner()) = Some((new_user.clone(), new_pass));
    sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .invalidate_all();

    tracing::info!("Dashboard: account updated (user={})", new_user);
    http_json_response(
        stream,
        200,
        &format!(
            r#"{{"ok":true,"auth_enabled":true,"username":"{}","reauth":true}}"#,
            new_user.replace('\\', "\\\\").replace('"', "\\\"")
        ),
    );
}

/// GET /api/dashboard-ports — current listen ports + preset classification.
fn serve_dashboard_ports_get(
    stream: PrefixedConn,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
) {
    use crate::net_dashboard::dashboard_ports::preset_name;

    let (port, https_port) = match shared_config {
        Some(cfg) => cfg
            .config()
            .dashboard
            .as_ref()
            .map(|d| (d.port, d.https_port))
            .unwrap_or((80, 443)),
        None => (80, 443),
    };
    let preset = preset_name(port, https_port);
    let url_hint = if https_port == 443 {
        "https://<IP>/".to_string()
    } else {
        format!("https://<IP>:{https_port}/")
    };
    let body = format!(
        r#"{{"ok":true,"preset":"{preset}","port":{port},"https_port":{https_port},"url_hint":"{url_hint}"}}"#
    );
    http_json_response(stream, 200, &body);
}

/// POST /api/dashboard-ports — set preset `standard` | `high`, then restart the service.
///
/// Body: `{"preset":"standard"}` or `{"preset":"high"}`.
fn serve_dashboard_ports_post(stream: PrefixedConn, config_path: &str, body: &str) {
    use crate::net_dashboard::dashboard_ports::{patch_dashboard_ports, ports_for_preset, preset_name};

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };
    let preset = json
        .get("preset")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let Some((http_port, https_port)) = ports_for_preset(preset) else {
        http_response(
            stream,
            400,
            "preset must be \"standard\" (80→443) or \"high\" (HTTPS :8443 only)",
        );
        return;
    };

    let original = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => {
            http_response(stream, 500, &format!("Could not read config: {e}"));
            return;
        }
    };
    let patched = patch_dashboard_ports(&original, http_port, https_port);
    if let Err((code, msg)) = write_config_validated(config_path, &patched) {
        http_response(stream, code, &msg);
        return;
    }

    let url_hint = if https_port == 443 {
        "https://<IP>/".to_string()
    } else {
        format!("https://<IP>:{https_port}/")
    };
    tracing::info!(
        "Dashboard: ports preset '{preset}' applied (HTTP {http_port}, HTTPS {https_port}) — restarting"
    );
    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::Restart,
        std::time::Duration::from_millis(800),
    );
    let body = format!(
        r#"{{"ok":true,"preset":"{}","port":{http_port},"https_port":{https_port},"url_hint":"{url_hint}","restarting":true}}"#,
        preset_name(http_port, https_port)
    );
    http_json_response(stream, 200, &body);
}

fn serve_station_export(mut stream: PrefixedConn, config_path: &str) {
    match crate::net_dashboard::station_bundle::build_station_bptbs(config_path) {
        Ok(bytes) => {
            let fname = format!(
                "station-{}.bptbs",
                chrono::Local::now().format("%Y%m%d-%H%M")
            );
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"{fname}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&bytes);
        }
        Err(e) => http_response(stream, 500, &e),
    }
}

fn serve_station_import(stream: PrefixedConn, config_path: &str, body: &[u8]) {
    if body.is_empty() {
        http_response(stream, 400, "empty body — POST the .bptbs file");
        return;
    }
    if body.len() > 32 * 1024 * 1024 {
        http_response(stream, 413, "file too large (max 32 MiB)");
        return;
    }
    match crate::net_dashboard::station_bundle::import_station_bptbs(config_path, body) {
        Ok(res) => {
            tracing::info!(
                "Dashboard: station .bptbs imported ({} warning(s)) — restarting",
                res.warnings.len()
            );
            crate::service_control::schedule_service_action(
                crate::service_control::ServiceAction::Restart,
                std::time::Duration::from_millis(800),
            );
            let warn_json = serde_json::to_string(&res.warnings).unwrap_or_else(|_| "[]".into());
            http_json_response(
                stream,
                200,
                &format!(r#"{{"ok":true,"restarting":true,"warnings":{warn_json}}}"#),
            );
        }
        Err(e) => http_response(stream, 400, &e),
    }
}

fn serve_profiles_export(mut stream: PrefixedConn, config_path: &str) {
    match crate::net_dashboard::station_bundle::build_profiles_ptbs(config_path) {
        Ok(bytes) => {
            let fname = format!(
                "profiles-{}.ptbs",
                chrono::Local::now().format("%Y%m%d-%H%M")
            );
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"{fname}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&bytes);
        }
        Err(e) => http_response(stream, 500, &e),
    }
}

fn serve_profiles_import(stream: PrefixedConn, config_path: &str, body: &[u8]) {
    if body.is_empty() {
        http_response(stream, 400, "empty body — POST the .ptbs file");
        return;
    }
    if body.len() > 16 * 1024 * 1024 {
        http_response(stream, 413, "file too large (max 16 MiB)");
        return;
    }
    match crate::net_dashboard::station_bundle::import_profiles_ptbs(config_path, body) {
        Ok(res) => {
            tracing::info!("Dashboard: profiles .ptbs imported");
            let warn_json = serde_json::to_string(&res.warnings).unwrap_or_else(|_| "[]".into());
            http_json_response(
                stream,
                200,
                &format!(
                    r#"{{"ok":true,"restarting":false,"warnings":{warn_json},"hint":"Apply & Restart to put profiles on air"}}"#
                ),
            );
        }
        Err(e) => http_response(stream, 400, &e),
    }
}

// ---------------------------------------------------------------------------
// WX/METAR service config (dashboard-editable). See net_dashboard::wx_service.
// ---------------------------------------------------------------------------

/// GET /api/wx — return the effective WX service settings as JSON.
fn serve_wx_get(mut stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
fn serve_wx_post(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
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

    // 1) Apply at runtime — on every cell (each runs its own WX responder).
    {
        let mut state = cfg.state_write();
        state.wx_override = Some(ov.clone());
    }
    for (_, cell_cfg) in crate::net_site::extra_cells() {
        cell_cfg.state_write().wx_override = Some(ov.clone());
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

fn serve_sds_commands_get(
    mut stream: PrefixedConn,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
) {
    use crate::net_dashboard::sds_commands;
    let ov = match shared_config {
        Some(cfg) => {
            if let Some(o) = cfg.state_read().sds_command_override.clone() {
                o
            } else {
                sds_commands::from_cfg(cfg.config().cell.sds_command_control.as_ref())
            }
        }
        None => tetra_config::bluestation::SdsCommandRuntimeOverride::default(),
    };
    let body = sds_commands::override_to_json(&ov);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn serve_sds_commands_post(
    stream: PrefixedConn,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    body: &str,
) {
    use crate::net_dashboard::sds_commands;
    use tetra_config::bluestation::SdsCommandRuntimeOverride;

    let ov = match sds_commands::parse_body(body) {
        Ok(o) => o,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid sds-commands: {e}"));
            return;
        }
    };

    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    {
        let mut state = cfg.state_write();
        state.sds_command_override = Some(SdsCommandRuntimeOverride {
            enabled: ov.enabled,
            authorized_issis: ov.authorized_issis.clone(),
            commands: ov.commands.clone(),
        });
    }

    if let Err(e) = sds_commands::write_to_toml(config_path, &ov) {
        tracing::warn!(
            "Dashboard: SDS commands applied at runtime but failed to persist to TOML: {}",
            e
        );
        http_response(
            stream,
            200,
            "Applied at runtime; failed to write config file (check permissions)",
        );
        return;
    }

    tracing::info!(
        "Dashboard: SDS command control updated (enabled={}, {} ISSI(s), {} command(s))",
        ov.enabled,
        ov.authorized_issis.len(),
        ov.commands.len()
    );
    http_response(stream, 200, "OK");
}

/// Read an HTTP POST body off `stream`, returning the stream plus the body as a UTF-8 string.
fn read_post_body(mut stream: PrefixedConn) -> (PrefixedConn, String) {
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
fn serve_telegram_get(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
fn serve_telegram_post(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
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
fn serve_telegram_verify(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, body: &str) {
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
fn serve_telegram_detect(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, body: &str) {
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
fn serve_telegram_test(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, body: &str) {
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

fn serve_system_info(mut stream: PrefixedConn, config_path: &str, probe_sdr: bool) {
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

    // RF / SoapySDR — ONLY when explicitly probed. Spawning SoapySDRUtil --info/--find
    // on every boot /api/system (and System auto-refresh) stalls the Pi for seconds and
    // piles up dashboard-conn threads behind GitHub OTA checks.
    let soapy_info = if probe_sdr {
        (|| -> String {
            let candidates = ["SoapySDRUtil", "/usr/bin/SoapySDRUtil", "/usr/local/bin/SoapySDRUtil"];
            for bin in &candidates {
                let probe = std::process::Command::new(bin).arg("--info").output();
                if let Ok(out) = probe {
                    if out.status.success() {
                        let find = std::process::Command::new(bin)
                            .arg("--find")
                            .output()
                            .ok()
                            .filter(|o| o.status.success())
                            .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
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
            "SoapySDRUtil not installed (apt install soapysdr-tools).".to_string()
        })()
    } else {
        String::new()
    };

    // Auto-detected SDR name — set by `phy::components::soapy_settings::get_settings()`
    // at stack startup. None if no SoapySDR-backed phy is in use (file backend etc).
    let sdr_name = crate::phy::components::soapy_settings::detected_sdr_name().unwrap_or_else(|| "unknown".to_string());
    let rf_status = crate::rf_status::get();

    let body = serde_json::to_string(&serde_json::json!({
        "hostname": hostname,
        "uptime_secs": uptime_secs,
        "os": os_info,
        "config_path": config_path,
        "config_dir": config_dir,
        "stack_version": tetra_core::STACK_VERSION,
        "product_name": tetra_core::PRODUCT_NAME,
        "version_based_on": tetra_core::VERSION_BASED_ON,
        "product_repo": tetra_core::PRODUCT_REPO_URL,
        "cpu_model": cpu_model,
        "cpu_cores": cpu_cores,
        "cpu_pct": cpu_pct,
        "ram_total_mb": ram_total_mb,
        "ram_used_mb": ram_used_mb,
        "cpu_temp_c": cpu_temp_c,
        "soapy_info": soapy_info,
        "sdr_name": sdr_name,
        "rf_status": rf_status,
    }))
    .unwrap_or_else(|_| "{}".to_string());

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn serve_config_list(mut stream: PrefixedConn, config_path: &str) {
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
fn serve_config_profile_get(stream: PrefixedConn, config_path: &str, profile_name: &str) {
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

    // Secrets are served to the profile editor masked (serve_config_profile_get → mask_config_secrets).
    // If the operator saves without retyping a secret, the editor posts the mask (`••••`/`…`) back
    // verbatim; restore the real value from the profile's OWN on-disk content so we never persist the
    // placeholder over a real credential — the same corruption write_config_validated guards against
    // for the active config, which the profile path previously skipped.
    let content = match std::fs::read_to_string(&profile_path) {
        Ok(current) => unmask_config_secrets(content, &current),
        Err(_) => content.to_string(),
    };

    // Validate before writing. A malformed profile that is later Activated overwrites the live config
    // with an unparseable file and crash-loops the stack on the next restart (startup abort under
    // systemd). Reject it here, exactly like write_config_validated does for the active config.
    match tetra_config::bluestation::parsing::from_toml_str(&content) {
        Ok(cfg) => {
            if let Err(e) = cfg.validate() {
                return Err(format!("config is invalid: {e}"));
            }
        }
        Err(e) => return Err(format!("config does not parse: {e}")),
    }

    let profile_path = profile_path.to_str().ok_or_else(|| "invalid profile path".to_string())?;
    atomic_write(profile_path, &content).map_err(|e| format!("failed to write profile: {}", e))
}

/// GET /api/public — anonymous read-only overview (FH-FEAT-033). Projects ONLY non-sensitive,
/// already-public scalars from the dashboard's own state — never SharedConfig/StackState, and never
/// ISSIs/GSSIs, the whitelist, SDS contents or the log ring. The read lock is the dashboard's own
/// RwLock (the same one the WS snapshot takes), held only long enough to copy a handful of counts.
fn serve_public_snapshot(stream: PrefixedConn, state: &DashboardState) {
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

    // Validate the profile parses + passes validation before it becomes the live config. Activating an
    // unparseable/invalid profile bricks the cell on the next restart (startup abort under systemd →
    // crash-loop with no valid .fallback) — the exact failure write_config_validated exists to
    // prevent, so guard the activate path too rather than trusting whatever is on disk.
    let profile_content = std::fs::read_to_string(&profile_path).map_err(|e| format!("failed to read profile: {}", e))?;
    match tetra_config::bluestation::parsing::from_toml_str(&profile_content) {
        Ok(cfg) => {
            if let Err(e) = cfg.validate() {
                return Err(format!("profile '{}' is invalid: {e}", profile_name));
            }
        }
        Err(e) => return Err(format!("profile '{}' does not parse: {e}", profile_name)),
    }

    // Backup current config before switching
    let backup_path = format!("{}.bak", config_path);
    if let Err(e) = atomic_copy(config_path, &backup_path) {
        tracing::warn!("Dashboard: failed to backup config before profile switch: {}", e);
    }

    // Atomic: the live config is either the old one or the profile, never a truncated mix.
    atomic_write(config_path, &profile_content).map_err(|e| format!("failed to copy profile: {}", e))
}

fn serve_html(mut stream: PrefixedConn) {
    let body = dashboard_html_body().as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

/// Public brand favicon (antenna mark). PNG for Edge/Chrome tab compatibility; SVG optional.
fn serve_favicon(mut stream: PrefixedConn, svg: bool) {
    let (ctype, body): (&str, &[u8]) = if svg {
        ("image/svg+xml", crate::net_dashboard::html::FAVICON_SVG.as_bytes())
    } else {
        ("image/png", include_bytes!("favicon.png").as_slice())
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nCache-Control: public, max-age=3600\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

/// Serve a vendored, immutable static asset (e.g. the bundled Leaflet library) with a long cache.
fn serve_static_asset(mut stream: PrefixedConn, body: &[u8], ctype: &str) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nCache-Control: public, max-age=31536000, immutable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

/// The placeholder characters the secret maskers emit — U+2022 BULLET (`mask_secret`/`mask_token`
/// for short secrets) and U+2026 HORIZONTAL ELLIPSIS (the `head…tail` form). A posted secret value
/// containing either is the mask echoed back unchanged by the editor, not a freshly typed secret.
fn value_is_masked_secret(value: &str) -> bool {
    value.contains('\u{2022}') || value.contains('\u{2026}')
}

/// Is `key` one of the secrets that `mask_config_secrets` masks before sending config to the browser?
fn is_masked_secret_key(key: &str) -> bool {
    // `token` is the [tpg2200_action] ActionURL token — the sole credential guarding the pre-auth
    // public /api/action/tpg2200 endpoint. It is the only `token` key in the config schema, so a
    // bare-key match cannot collide with anything else.
    matches!(key, "password" | "bot_token" | "rwth_core_authkey" | "ami_password" | "token")
}

/// Split a `key = "value"` TOML line into `(key, inner)`, mirroring `mask_toml_secret_line`: only a
/// bare quoted-string assignment matches; comments and everything else return `None`. `inner` is the
/// raw (still TOML-escaped) text between the quotes, so it can be re-wrapped verbatim.
fn split_toml_str_assignment(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let eq = line.find('=')?;
    let key = line[..eq].trim();
    let rhs = line[eq + 1..].trim();
    let inner = rhs.strip_prefix('"').and_then(|s| s.strip_suffix('"'))?;
    Some((key, inner))
}

#[derive(serde::Deserialize, Default)]
struct ConfigE2eeSecrets {
    security: Option<ConfigE2eeSecurity>,
}

#[derive(serde::Deserialize)]
struct ConfigE2eeSecurity {
    e2ee: Option<ConfigE2eeKeys>,
}

#[derive(serde::Deserialize)]
struct ConfigE2eeKeys {
    #[serde(default)]
    teks: Vec<ConfigE2eeKey>,
}

#[derive(serde::Deserialize)]
struct ConfigE2eeKey {
    id: Option<u16>,
    key: Option<toml::Spanned<String>>,
}

/// Parse TOML value spans rather than matching lines: TEKs can be inline tables, use literal or
/// multiline strings, and put `id` after `key`. Spans let the editor retain comments and layout.
fn config_e2ee_keys(content: &str) -> Result<Vec<ConfigE2eeKey>, toml::de::Error> {
    let parsed: ConfigE2eeSecrets = toml::from_str(content)?;
    Ok(parsed.security.and_then(|s| s.e2ee).map(|e| e.teks).unwrap_or_default())
}

fn replace_config_spans(content: &str, mut edits: Vec<(std::ops::Range<usize>, String)>) -> String {
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut out = content.to_string();
    for (span, value) in edits {
        out.replace_range(span, &toml::Value::String(value).to_string());
    }
    out
}

/// Do not expose even a prefix/suffix of an encryption key. Failed TOML parsing cannot establish
/// where all key values end, so fail closed instead of returning possibly unredacted key material.
fn mask_e2ee_config_keys(content: &str) -> String {
    let Ok(keys) = config_e2ee_keys(content) else {
        return "# Configuration cannot be displayed safely because it is not valid TOML.\n".into();
    };
    let edits = keys
        .into_iter()
        .filter_map(|tek| tek.key.map(|key| (key.span(), "\u{2022}".repeat(8))))
        .collect();
    replace_config_spans(content, edits)
}

/// Repeated [[security.e2ee.teks]] tables share the same section name, so restoring by
/// `(section, key)` would overwrite every TEK with the last one. Resolve unique ids instead;
/// missing, changed or duplicate ids retain their mask and are rejected by config validation.
fn unmask_e2ee_config_keys(body: &str, current: &str) -> String {
    let (Ok(posted), Ok(stored)) = (config_e2ee_keys(body), config_e2ee_keys(current)) else {
        return body.to_string();
    };
    let mut stored_by_id: HashMap<u16, Option<String>> = HashMap::new();
    for tek in stored {
        if let (Some(id), Some(key)) = (tek.id, tek.key) {
            stored_by_id.entry(id).and_modify(|value| *value = None).or_insert_with(|| Some(key.into_inner()));
        }
    }
    let mut posted_counts = HashMap::<u16, usize>::new();
    for tek in &posted {
        if let Some(id) = tek.id {
            *posted_counts.entry(id).or_default() += 1;
        }
    }
    let edits = posted
        .into_iter()
        .filter_map(|tek| {
            let (id, key) = (tek.id?, tek.key?);
            if posted_counts.get(&id) != Some(&1) || !value_is_masked_secret(key.get_ref()) {
                return None;
            }
            let original = stored_by_id.get(&id)?.as_ref()?;
            Some((key.span(), original.clone()))
        })
        .collect();
    replace_config_spans(body, edits)
}

/// Restore secret values that the editor posted back still masked, from the currently-stored config.
///
/// `serve_config_get` masks secrets (`password`, `bot_token`, `rwth_core_authkey`, `ami_password`)
/// with bullets/ellipsis before serving the config file to the browser, so cleartext never leaves the
/// host. If the operator saves without retyping a secret, the editor posts that mask back verbatim —
/// writing it would overwrite the real credential with placeholder characters and lock the operator
/// out (the reported "passwords corrupted with random symbols" bug). Here we walk the posted `body`,
/// and for any secret line whose value is still the mask, substitute the plaintext from the same
/// `[section] key` in `current` (the on-disk config). Genuinely retyped secrets are left untouched.
///
/// Matching is section-aware so `[dashboard] password` and `[brew] password` — same bare key — are
/// never crossed. A masked secret with no stored counterpart is left as-is (no worse than before).
fn unmask_config_secrets(body: &str, current: &str) -> String {
    // (section header, key) -> stored raw (escaped) value, for masked secret keys only.
    let mut stored: std::collections::HashMap<(String, String), String> = std::collections::HashMap::new();
    let mut section = String::new();
    for line in current.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.to_string();
        } else if let Some((key, inner)) = split_toml_str_assignment(line) {
            if is_masked_secret_key(key) {
                stored.insert((section.clone(), key.to_string()), inner.to_string());
            }
        }
    }

    let mut out = String::with_capacity(body.len());
    let mut section = String::new();
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.to_string();
        } else if let Some((key, inner)) = split_toml_str_assignment(line) {
            if is_masked_secret_key(key) && value_is_masked_secret(inner) {
                if let Some(orig) = stored.get(&(section.clone(), key.to_string())) {
                    let indent_len = line.len() - line.trim_start().len();
                    out.push_str(&format!("{}{} = \"{}\"", &line[..indent_len], key, orig));
                    out.push('\n');
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    unmask_e2ee_config_keys(&out, current)
}

/// Write `content` to `path` atomically: temp file in the SAME directory, flushed, then renamed
/// over the target. `fs::write`/`fs::copy` truncate the destination first, so a crash (or a power
/// cut — this is a base station) between truncate and write leaves a half-written config.toml that
/// no longer parses, which aborts startup under systemd `Restart=` and crash-loops the cell. rename
/// within one filesystem is atomic, so the config is either the old one or the new one, never half.
pub(crate) fn atomic_write(path: &str, content: &str) -> std::io::Result<()> {
    let target = std::path::Path::new(path);
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("config.toml");
    let tmp = dir.join(format!(".{}.tmp{}", name, std::process::id()));

    let write_tmp = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Copy `src` over `dst` atomically (read + [`atomic_write`]), for the config backup/activate paths
/// that used `fs::copy` — same truncate-then-write exposure as `fs::write`.
fn atomic_copy(src: &str, dst: &str) -> std::io::Result<()> {
    let content = std::fs::read_to_string(src)?;
    atomic_write(dst, &content)
}

/// POST /api/config/restore — put `<config>.bak` back as the live config.
///
/// The backup is NOT trusted: it may be truncated (an old non-atomic write that was interrupted),
/// or written by an earlier schema that no longer parses. Copying it over the live config and
/// restarting — which is what this endpoint used to do, unconditionally — bricks the station with
/// no way back. So dry-run parse + `validate()` first, exactly like `write_config_validated`, and
/// snapshot the config we are about to replace to `<config>.prerestore` so the operator can undo.
fn restore_config_from_backup(config_path: &str) -> Result<(), (u16, String)> {
    let backup_path = format!("{}.bak", config_path);
    let content = std::fs::read_to_string(&backup_path).map_err(|e| (500, format!("cannot read backup: {e}")))?;
    match tetra_config::bluestation::parsing::from_toml_str(&content) {
        Ok(cfg) => {
            if let Err(e) = cfg.validate() {
                return Err((400, format!("backup is invalid, refusing to restore: {e}")));
            }
        }
        Err(e) => return Err((400, format!("backup does not parse, refusing to restore: {e}"))),
    }
    // Keep the config we are replacing — a restore is itself an operation the operator may regret.
    let snapshot_path = format!("{}.prerestore", config_path);
    if let Err(e) = atomic_copy(config_path, &snapshot_path) {
        tracing::warn!("Dashboard: failed to snapshot config before restore: {}", e);
    }
    atomic_write(config_path, &content).map_err(|e| (500, e.to_string()))
}

/// Persist a posted config.toml after a dry-run parse + validate.
///
/// Writing garbage here is what would brick the base station: the service restarts to apply config,
/// the new file fails to parse, and (with no valid `.fallback`) startup aborts under systemd
/// `Restart=` into a crash-loop. So parse + `validate()` exactly like `serve_dual_carrier_post`
/// before touching disk — a bad body is rejected and the running config is left untouched. On
/// success the previous config is backed up to `<path>.bak` and the new one written.
///
/// Returns `Ok(())` on success, or `Err((http_code, message))` for the caller to relay.
fn write_config_validated(config_path: &str, body: &str) -> Result<(), (u16, String)> {
    // Secrets are served to the editor masked; restore any still-masked value from the stored config
    // so a save that didn't retype a password keeps the real one instead of persisting the mask.
    let body = match std::fs::read_to_string(config_path) {
        Ok(current) => unmask_config_secrets(body, &current),
        Err(_) => body.to_string(),
    };
    let body = body.as_str();
    match tetra_config::bluestation::parsing::from_toml_str(body) {
        Ok(cfg) => {
            if let Err(e) = cfg.validate() {
                return Err((400, format!("config is invalid: {e}")));
            }
        }
        Err(e) => return Err((400, format!("config does not parse: {e}"))),
    }
    // Back up the current config before overwriting (best-effort; a missing source is not fatal).
    let backup_path = format!("{}.bak", config_path);
    if let Err(e) = atomic_copy(config_path, &backup_path) {
        tracing::warn!("Dashboard: failed to write config backup: {}", e);
    }
    atomic_write(config_path, body).map_err(|e| (500, e.to_string()))
}

fn serve_config_get(mut stream: PrefixedConn, config_path: &str) {
    match std::fs::read_to_string(config_path) {
        Ok(content) => {
            let masked = mask_config_secrets(&content);
            let body = masked.as_bytes();
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

/// Mask the secret values in a raw config.toml before it is sent to the browser.
///
/// The config holds plaintext credentials (Telegram bot token, DAPNET password / RWTH core authkey,
/// Asterisk AMI password). The per-field endpoints already mask these; the raw `/api/config` read
/// must do the same so the cleartext never leaves the host. We mask line-by-line, keying on the TOML
/// key name, and replace only the quoted value — comments and structure are preserved so the file
/// still reads naturally in the editor.
fn mask_config_secrets(content: &str) -> String {
    use crate::net_dashboard::dapnet::mask_secret;
    use crate::net_dashboard::telegram::mask_token;

    // key -> masker. `bot_token` keeps the "id:tail" shape; the rest use the generic head…tail mask.
    fn mask_for_key(key: &str, value: &str) -> Option<String> {
        match key {
            "bot_token" => Some(mask_token(value)),
            "password" | "rwth_core_authkey" | "ami_password" | "token" => Some(mask_secret(value)),
            _ => None,
        }
    }

    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        out.push_str(&mask_toml_secret_line(line, mask_for_key));
        out.push('\n');
    }
    mask_e2ee_config_keys(&out)
}

/// Mask the value of a single TOML `key = "value"` line when `key` is a known secret. Leaves
/// comments (lines whose first non-space char is `#`) and non-secret/non-matching lines untouched.
fn mask_toml_secret_line(line: &str, mask_for_key: fn(&str, &str) -> Option<String>) -> String {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return line.to_string();
    }
    let Some(eq) = line.find('=') else {
        return line.to_string();
    };
    let key = line[..eq].trim();
    // Only touch a bare quoted-string assignment. The value must be a `"..."` literal.
    let rhs = line[eq + 1..].trim();
    let Some(inner) = rhs.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return line.to_string();
    };
    match mask_for_key(key, inner) {
        Some(masked) => {
            let indent_len = line.len() - line.trim_start().len();
            format!("{}{} = \"{}\"", &line[..indent_len], key, masked)
        }
        None => line.to_string(),
    }
}

fn http_response(mut stream: PrefixedConn, code: u16, body: &str) {
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
fn http_json_response(mut stream: PrefixedConn, code: u16, body: &str) {
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

fn lst_b64_decode(s: &str) -> Result<Vec<u8>, ()> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s.trim()))
        .map_err(|_| ())
}

/// Consume and discard HTTP request headers up to the blank line. Use this
/// for GET-style endpoints that don't read a body — we still need to clear
/// the headers off the stream before responding, otherwise some clients
/// reuse the connection and get confused.
/// Extract `/api/profiles/{kind}/<name…>` from a request line after a fixed method+prefix.
/// Percent-decodes the name; returns None when the path does not match.
fn profile_path_name(req_line: &str, method_prefix: &str) -> Option<String> {
    let path = req_line.split_whitespace().nth(1)?;
    let method = req_line.split_whitespace().next()?;
    let want_method = method_prefix.split_whitespace().next()?;
    if !method.eq_ignore_ascii_case(want_method) {
        return None;
    }
    let prefix = method_prefix.split_whitespace().nth(1)?;
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() {
        return None;
    }
    Some(url_decode(rest))
}

fn drain_http_headers(stream: &mut PrefixedConn) {
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
fn read_http_body(stream: &mut PrefixedConn) -> Vec<u8> {
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
    if content_length == 0 {
        return Vec::new();
    }
    let mut body = vec![0u8; content_length.min(32 * 1024 * 1024)];
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

fn http_redirect(mut stream: PrefixedConn, location: &str) {
    let resp = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn request_target(req_line: &str) -> &str {
    let mut parts = req_line.split_whitespace();
    let _method = parts.next();
    match parts.next() {
        Some(t) if t.starts_with('/') => t,
        _ => "/",
    }
}

fn host_header<'a>(header_str: &'a str) -> Option<&'a str> {
    for line in header_str.lines().skip(1) {
        if line.is_empty() || line == "\r" {
            break;
        }
        let trimmed = line.trim_end_matches('\r');
        if let Some(rest) = trimmed
            .strip_prefix("Host:")
            .or_else(|| trimmed.strip_prefix("host:"))
        {
            return Some(rest.trim());
        }
    }
    None
}

fn host_without_port(host: &str) -> &str {
    if host.starts_with('[') {
        if let Some(end) = host.find(']') {
            return &host[..=end];
        }
        return host;
    }
    if let Some((h, p)) = host.rsplit_once(':') {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            return h;
        }
    }
    host
}

fn absolute_https_location(header_str: &str, req_line: &str, https_port: u16) -> String {
    let host = host_without_port(host_header(header_str).unwrap_or("localhost"));
    let path = request_target(req_line);
    if https_port == 443 {
        format!("https://{host}{path}")
    } else {
        format!("https://{host}:{https_port}{path}")
    }
}

#[derive(Clone, Copy)]
enum BindMode {
    /// Configured dashboard port — keep retrying; slow down on conflict.
    Canonical,
    /// Optional bookmark redirect — few attempts then give up.
    Legacy,
}

fn bind_is_conflict(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
    )
}

/// Bind a TCP listener with mode-specific retry policy.
/// Canonical: infinite retry (60s on conflict, 5s otherwise, rate-limited logs).
/// Legacy: up to 3 attempts then `None`.
fn bind_tcp_listener(addr: &str, mode: BindMode) -> Option<TcpListener> {
    let max_attempts = match mode {
        BindMode::Canonical => usize::MAX,
        BindMode::Legacy => 3,
    };
    let mut attempt = 0usize;
    let mut logged_conflict = false;
    loop {
        attempt = attempt.saturating_add(1);
        match TcpListener::bind(addr) {
            Ok(l) => return Some(l),
            Err(e) => {
                let conflict = bind_is_conflict(&e);
                match mode {
                    BindMode::Legacy => {
                        if attempt >= max_attempts || conflict {
                            tracing::warn!(
                                "Dashboard legacy bind {}: {} — giving up (bookmarks to this port will not redirect)",
                                addr,
                                e
                            );
                            return None;
                        }
                        tracing::warn!(
                            "Dashboard legacy bind {}: {} — retry {}/{}",
                            addr,
                            e,
                            attempt,
                            max_attempts
                        );
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                    BindMode::Canonical => {
                        if conflict {
                            if !logged_conflict {
                                tracing::error!(
                                    "Dashboard failed to bind {}: {} — check [dashboard] port/https_port, \
                                     other daemons (nginx/apache), or use the high-port preset (HTTPS :8443). \
                                     Retrying every 60s.",
                                    addr,
                                    e
                                );
                                logged_conflict = true;
                            }
                            std::thread::sleep(std::time::Duration::from_secs(60));
                        } else if attempt == 1 || attempt % 12 == 0 {
                            tracing::warn!(
                                "Dashboard failed to bind {}: {} — retrying in 5s (interface may not be ready)",
                                addr,
                                e
                            );
                            std::thread::sleep(std::time::Duration::from_secs(5));
                        } else {
                            std::thread::sleep(std::time::Duration::from_secs(5));
                        }
                    }
                }
            }
        }
    }
}

fn write_plain_redirect(mut stream: TcpStream, location: &str) {
    let resp = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_https_redirect_plain(mut stream: TcpStream, https_port: u16) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let header_str = String::from_utf8_lossy(&buf[..n]);
    let req_line = header_str.lines().next().unwrap_or("");
    let location = absolute_https_location(&header_str, req_line, https_port);
    write_plain_redirect(stream, &location);
}

fn spawn_legacy_http_redirect(bind: &str, listen_port: u16, https_port: u16) {
    let addr = format!("{}:{}", bind, listen_port);
    std::thread::Builder::new()
        .name("dashboard-http-legacy".into())
        .spawn(move || {
            let Some(listener) = bind_tcp_listener(&addr, BindMode::Legacy) else {
                return;
            };
            tracing::info!(
                "Dashboard legacy HTTP redirect on http://{} → https (port {})",
                addr,
                https_port
            );
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve_https_redirect_plain(stream, https_port);
            }
        })
        .ok();
}

fn spawn_legacy_https_redirect(
    bind: &str,
    listen_port: u16,
    https_port: u16,
    tls_config: Arc<rustls::ServerConfig>,
) {
    let addr = format!("{}:{}", bind, listen_port);
    std::thread::Builder::new()
        .name("dashboard-https-legacy".into())
        .spawn(move || {
            let Some(listener) = bind_tcp_listener(&addr, BindMode::Legacy) else {
                return;
            };
            tracing::info!(
                "Dashboard legacy HTTPS redirect on https://{} → canonical HTTPS :{}",
                addr,
                https_port
            );
            for stream in listener.incoming() {
                let Ok(tcp) = stream else { continue };
                let tls_config = Arc::clone(&tls_config);
                std::thread::Builder::new()
                    .name("dashboard-https-legacy-conn".into())
                    .spawn(move || {
                        let mut conn = match ConnStream::from_tls_handshake(tcp, tls_config) {
                            Ok(c) => c,
                            Err(_) => return,
                        };
                        let _ = conn.set_read_timeout(Some(std::time::Duration::from_millis(500)));
                        let mut buf = [0u8; 4096];
                        let n = match conn.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        let header_str = String::from_utf8_lossy(&buf[..n]);
                        let req_line = header_str.lines().next().unwrap_or("");
                        let location = absolute_https_location(&header_str, req_line, https_port);
                        let resp = format!(
                            "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            location
                        );
                        let _ = conn.write_all(resp.as_bytes());
                    })
                    .ok();
            }
        })
        .ok();
}

fn serve_login_success(mut stream: PrefixedConn, token: &str) {
    // Two cookies:
    //   fs_session: HttpOnly — the actual session token, inaccessible to JS.
    //   fs_auth: readable — a marker telling the dashboard JS "auth is on",
    //                       so it can decide to show the Logout button.
    // The marker carries no security value; the HttpOnly session is what's checked.
    // On TLS, add Secure so browsers won't send the cookie over plain HTTP.
    let secure = if stream.is_tls() { "; Secure" } else { "" };
    let body = "{\"ok\":true}";
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Set-Cookie: fs_session={}; Path=/; HttpOnly; SameSite=Lax{}; Max-Age=604800\r\n\
         Set-Cookie: fs_auth=1; Path=/; SameSite=Lax{}; Max-Age=604800\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        token,
        secure,
        secure,
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_logout(mut stream: PrefixedConn) {
    // Expire both cookies immediately; client navigates to /login next.
    let secure = if stream.is_tls() { "; Secure" } else { "" };
    let resp = format!(
        "HTTP/1.1 302 Found\r\n\
                Location: /login\r\n\
                Set-Cookie: fs_session=; Path=/; HttpOnly; SameSite=Lax{}; Max-Age=0\r\n\
                Set-Cookie: fs_auth=; Path=/; SameSite=Lax{}; Max-Age=0\r\n\
                Content-Length: 0\r\n\
                Connection: close\r\n\r\n",
        secure, secure
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// GET /api/dashboard/tls — HTTPS status for the UI (public, no auth).
fn serve_dashboard_tls_status(stream: PrefixedConn) {
    let st = tls_status();
    let body = format!(
        r#"{{"https_port":{},"enabled":{},"cert_fingerprint":"{}"}}"#,
        st.https_port,
        st.enabled,
        st.cert_fingerprint.replace('"', ""),
    );
    http_json_response(stream, 200, &body);
}

fn serve_login_page(mut stream: PrefixedConn) {
    let body = login_html_body();
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
fn serve_sds_log_clear(stream: PrefixedConn, state: &DashboardState) {
    if let Ok(mut s) = state.write() {
        s.clear_sds_log();
    }
    http_json_response(stream, 200, "{\"ok\":true}");
}

/// DELETE /api/dgna-log — clear the persisted DGNA activity log.
fn serve_dgna_log_clear(stream: PrefixedConn, state: &DashboardState) {
    if let Ok(mut s) = state.write() {
        s.clear_dgna_log();
    }
    http_json_response(stream, 200, "{\"ok\":true}");
}

/// GET /api/dapnet-log — the persisted DAPNET Log as a JSON array, newest entry first.
fn serve_dapnet_log(stream: PrefixedConn, state: &DashboardState) {
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
fn serve_dapnet_log_clear(stream: PrefixedConn, state: &DashboardState) {
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
    stream: PrefixedConn,
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
    // Both identities go into D-SDS-DATA's 24-bit address fields. A misconfigured (or hand-edited)
    // wider value would panic the stack thread on the first call-out, so refuse to send at all.
    if !is_valid_ssi(action.dest_issi as u64) || !is_valid_ssi(action.source_issi as u64) {
        http_response(
            stream,
            500,
            "TPG2200 ActionURL not fully configured (source/dest ISSI must be 1..=16777215)",
        );
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
fn serve_asterisk_status(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
fn serve_snom_notify_get(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
    stream: PrefixedConn,
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
fn serve_dapnet_get(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
fn serve_dapnet_post(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
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
fn serve_geoalarm_get(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
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
fn serve_geoalarm_post(stream: PrefixedConn, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
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
    stream: PrefixedConn,
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
    use super::{
        DashboardServer, LoginThrottle, MAX_TETRA_SSI, UpdateState, activate_config_profile, binary_built_from,
        handle_ws_command, is_masked_secret_key, mask_config_secrets, normalize_mojibake_html,
        restore_config_from_backup, save_config_profile, timing_safe_eq, write_config_validated,
    };
    use crate::net_control::commands::ControlCommand;
    use crate::net_dashboard::state::DashboardStateInner;
    use crate::net_telemetry::TelemetryEvent;
    use std::sync::{Arc, Mutex, RwLock};

    /// Exercise the production admission, HTTP handler and WebSocket loop over ephemeral loopback
    /// sockets. Do not start DashboardServer: these tests must not run host service/Wi-Fi helpers,
    /// materialise certificates, or open an RF backend.
    mod private_http {
        use super::super::*;
        use std::time::Duration;
        use tungstenite::client::IntoClientRequest;

        struct Fixture {
            server: DashboardServer,
            commands: crossbeam_channel::Receiver<ControlCommand>,
        }

        impl Fixture {
            fn new(networks: &[&str]) -> Self {
                let mut config = tetra_config::bluestation::from_toml_str(super::MINIMAL_CONFIG).unwrap();
                config.dashboard = Some(tetra_config::bluestation::CfgDashboard {
                    username: Some("operator".into()),
                    password: Some("test-password".into()),
                    http_allowed_networks: networks.iter().map(|network| network.parse().unwrap()).collect(),
                    public_overview: true,
                    ..Default::default()
                });
                let shared_config = tetra_config::bluestation::SharedConfig::from_parts(config, None);
                let mut server = DashboardServer::new("unused-private-http-test.toml".into());
                server.set_shared_config(shared_config);
                server.set_auth(Some(("operator".into(), "test-password".into())));
                server.set_public_overview(true);
                let (sender, commands) = crossbeam_channel::unbounded();
                server.set_cmd_sender(sender);
                Self { server, commands }
            }

            fn connect(&self) -> (TcpStream, std::thread::JoinHandle<()>) {
                let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback test listener");
                let address = listener.local_addr().unwrap();
                let state = Arc::clone(&self.server.state);
                let clients = Arc::clone(&self.server.clients);
                let config_path = self.server.config_path.clone();
                let cmd_tx = Arc::new(Mutex::new(self.server.cmd_tx.clone()));
                let mm_cmd_tx = Arc::new(Mutex::new(self.server.mm_cmd_tx.clone()));
                let update_state = Arc::clone(&self.server.update_state);
                let auth = Arc::clone(&self.server.auth);
                let shared_config = self.server.shared_config.clone();
                let sessions = Arc::clone(&self.server.sessions);
                let throttle = Arc::clone(&self.server.login_throttle);
                let radioid = self.server.radioid.clone();
                let (admitted, ready) = std::sync::mpsc::channel();
                let thread = std::thread::spawn(move || {
                    let (stream, peer) = listener.accept().unwrap();
                    let admission = primary_http_admission(true, &shared_config, &auth, Some(peer.ip()));
                    admitted.send(()).unwrap();
                    let Some(require_http_auth) = admission else {
                        serve_https_redirect_plain(stream, 443);
                        return;
                    };
                    assert!(require_http_auth);
                    handle_connection(
                        ConnStream::plain(stream),
                        state,
                        clients,
                        config_path,
                        cmd_tx,
                        mm_cmd_tx,
                        update_state,
                        None,
                        auth,
                        shared_config,
                        sessions,
                        throttle,
                        radioid,
                        true,
                        None,
                        require_http_auth,
                        None,
                    );
                });
                let stream = TcpStream::connect(address).expect("connect loopback client");
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                ready.recv_timeout(Duration::from_secs(2)).expect("listener admission");
                (stream, thread)
            }

            fn request(&self, method: &str, target: &str, headers: &str, body: &str) -> String {
                let (mut stream, thread) = self.connect();
                let request = format!(
                    "{method} {target} HTTP/1.1\r\nHost: station.test:80\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(request.as_bytes()).unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).expect("read HTTP response");
                thread.join().expect("HTTP handler completed");
                response
            }

            fn login(&self) -> String {
                let response = self.request(
                    "POST",
                    "/api/login",
                    "Content-Type: application/json\r\n",
                    r#"{"user":"operator","password":"test-password"}"#,
                );
                assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                let cookie = response.lines().find(|line| line.starts_with("Set-Cookie: fs_session=")).unwrap();
                assert!(cookie.contains("HttpOnly"));
                assert!(!cookie.contains("Secure"), "an HTTP session must be usable over HTTP");
                cookie.strip_prefix("Set-Cookie: ").unwrap().split(';').next().unwrap().into()
            }

            fn websocket(&self, cookie: &str) -> (tungstenite::WebSocket<TcpStream>, std::thread::JoinHandle<()>) {
                let (stream, thread) = self.connect();
                let mut request = "ws://station.test/ws".into_client_request().unwrap();
                request.headers_mut().insert("Cookie", cookie.parse().unwrap());
                let (mut websocket, response) = tungstenite::client(request, stream).expect("authenticated WS upgrade");
                assert_eq!(response.status(), 101);
                let snapshot: serde_json::Value = serde_json::from_str(websocket.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(snapshot["type"], "snapshot");
                (websocket, thread)
            }
        }

        #[test]
        fn private_http_default_and_denied_peer_redirect_even_with_forwarded_headers() {
            let headers = "X-Forwarded-For: 10.0.0.11\r\nForwarded: for=10.0.0.11\r\n";
            for fixture in [Fixture::new(&[]), Fixture::new(&["10.0.0.0/24"])] {
                let response = fixture.request("GET", "/api/service/status", headers, "");
                assert!(response.starts_with("HTTP/1.1 302"), "{response}");
                assert!(response.contains("Location: https://station.test/api/service/status"));
            }
            let fixture = Fixture::new(&[]);
            assert_eq!(
                primary_http_admission(
                    false,
                    &fixture.server.shared_config,
                    &fixture.server.auth,
                    Some("127.0.0.1".parse().unwrap())
                ),
                Some(false)
            );
            let fixture = Fixture::new(&["10.0.0.0/24"]);
            assert_eq!(
                primary_http_admission(
                    false,
                    &fixture.server.shared_config,
                    &fixture.server.auth,
                    Some("127.0.0.1".parse().unwrap())
                ),
                None
            );
        }

        #[test]
        fn private_http_requires_cookie_for_public_snapshot_api_and_websocket() {
            let fixture = Fixture::new(&["127.0.0.1"]);
            for target in ["/api/public", "/api/service/status", "/api/dashboard/tls", "/ws"] {
                let response = fixture.request("GET", target, "", "");
                assert!(response.starts_with("HTTP/1.1 401"), "{target}: {response}");
                assert!(!response.contains("\"type\":\"snapshot\""));
            }
            let root = fixture.request("GET", "/", "", "");
            assert!(root.contains("Location: /login"), "anonymous overview must stay gated");
            let basic = fixture.request(
                "GET",
                "/api/service/status",
                "Authorization: Basic b3BlcmF0b3I6dGVzdC1wYXNzd29yZA==\r\n",
                "",
            );
            assert!(
                basic.starts_with("HTTP/1.1 401"),
                "Basic credentials do not bypass the cookie session"
            );
            let cookie = fixture.login();
            let authenticated = fixture.request("GET", "/api/dashboard/tls", &format!("Cookie: {cookie}\r\n"), "");
            assert!(authenticated.starts_with("HTTP/1.1 200"), "{authenticated}");
            let (mut websocket, thread) = fixture.websocket(&cookie);
            websocket.close(None).unwrap();
            drop(websocket);
            thread.join().unwrap();
        }

        #[test]
        fn private_http_auth_removed_after_accept_never_falls_open() {
            let fixture = Fixture::new(&["127.0.0.1"]);
            let (mut stream, thread) = fixture.connect();
            *fixture.server.auth.write().unwrap() = None;
            stream.write_all(b"GET /api/public HTTP/1.1\r\nHost: station.test\r\n\r\n").unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            thread.join().unwrap();
            assert!(response.starts_with("HTTP/1.1 401"), "{response}");
            let next = fixture.request("GET", "/", "", "");
            assert!(next.starts_with("HTTP/1.1 302"));
            assert!(next.contains("Location: https://station.test/"));
            assert!(fixture.commands.try_recv().is_err());
        }

        #[test]
        fn private_http_logout_and_credential_removal_revoke_websocket_commands() {
            for remove_credentials in [false, true] {
                let fixture = Fixture::new(&["127.0.0.1"]);
                let cookie = fixture.login();
                let (mut websocket, thread) = fixture.websocket(&cookie);
                // An authenticated harmless command reaches the in-memory dispatcher, not RF.
                websocket.send(Message::Text(r#"{"type":"kick","issi":1234}"#.into())).unwrap();
                assert!(matches!(
                    fixture.commands.recv_timeout(Duration::from_secs(2)),
                    Ok(ControlCommand::KickMs { issi: 1234 })
                ));
                if remove_credentials {
                    *fixture.server.auth.write().unwrap() = None;
                } else {
                    let response = fixture.request("POST", "/api/logout", &format!("Cookie: {cookie}\r\n"), "");
                    assert!(response.starts_with("HTTP/1.1 302"));
                }
                let _ = websocket.send(Message::Text(r#"{"type":"kick","issi":4321}"#.into()));
                assert!(
                    matches!(websocket.read(), Ok(Message::Close(_)) | Err(_)),
                    "revoked socket must close"
                );
                drop(websocket);
                thread.join().unwrap();
                assert!(fixture.commands.try_recv().is_err(), "revoked command reached dispatcher");
            }
        }

        #[test]
        fn private_http_session_rejects_changed_credentials_missing_policy_and_expiry() {
            let fixture = Fixture::new(&["127.0.0.1"]);
            let token = fixture.server.sessions.lock().unwrap().create();
            let mut session = PrivateHttpSession {
                shared_config: fixture.server.shared_config.clone(),
                auth: Arc::clone(&fixture.server.auth),
                sessions: Arc::clone(&fixture.server.sessions),
                peer: Some("127.0.0.1".parse().unwrap()),
                credentials: ("operator".into(), "test-password".into()),
                token,
            };
            assert!(session.is_authorized());
            *fixture.server.auth.write().unwrap() = Some(("operator".into(), "new-password".into()));
            assert!(!session.is_authorized());
            *fixture.server.auth.write().unwrap() = Some(session.credentials.clone());
            session.shared_config = None;
            assert!(!session.is_authorized());
            session.shared_config = fixture.server.shared_config.clone();
            fixture.server.sessions.lock().unwrap().ttl = Duration::ZERO;
            assert!(!session.is_authorized());
        }
    }

    struct OtaTestRepository(std::path::PathBuf);

    impl OtaTestRepository {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "flowstation-ota-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("create OTA test repository");
            let repo = Self(path);
            repo.git(&["init", "--quiet", "--initial-branch=main"]);
            repo.git(&["config", "user.name", "Station Test"]);
            repo.git(&["config", "user.email", "station@example.invalid"]);
            std::fs::write(repo.0.join("source.txt"), "original\n").unwrap();
            std::fs::write(repo.0.join(".gitignore"), "/target/\n").unwrap();
            repo.git(&["add", "source.txt", ".gitignore"]);
            repo.git(&["commit", "--quiet", "--no-gpg-sign", "-m", "Initial source"]);
            repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(["-c", "core.hooksPath=/dev/null"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("run test git");
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }

        fn commit_change(&self, text: &str) {
            std::fs::write(self.0.join("source.txt"), text).unwrap();
            self.git(&["add", "source.txt"]);
            self.git(&["commit", "--quiet", "--no-gpg-sign", "-m", "Update source"]);
        }
    }

    impl Drop for OtaTestRepository {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ota_preflight_rejects_untracked_staged_and_unstaged_changes_without_mutation() {
        let repo = OtaTestRepository::new();
        let head = repo.git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.0.join("local-weather.rs"), "local extension\n").unwrap();
        assert!(super::ota_worktree_is_clean(&repo.0).unwrap_err().contains("untracked"));
        assert_eq!(std::fs::read_to_string(repo.0.join("local-weather.rs")).unwrap(), "local extension\n");
        std::fs::remove_file(repo.0.join("local-weather.rs")).unwrap();

        std::fs::write(repo.0.join("source.txt"), "station changes\n").unwrap();
        assert!(super::ota_worktree_is_clean(&repo.0).is_err(), "unstaged changes must block OTA");
        repo.git(&["add", "source.txt"]);
        assert!(super::ota_worktree_is_clean(&repo.0).is_err(), "staged changes must block OTA");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read_to_string(repo.0.join("source.txt")).unwrap(), "station changes\n");
    }

    #[test]
    fn ota_preflight_accepts_clean_sources_and_ignored_incremental_builds() {
        let repo = OtaTestRepository::new();
        std::fs::create_dir(repo.0.join("target")).unwrap();
        std::fs::write(repo.0.join("target/incremental.bin"), "cached build").unwrap();
        super::ota_worktree_is_clean(&repo.0).unwrap();
        super::ota_local_commits_are_preserved(&repo.0, "origin/main", "main").unwrap();
    }

    #[test]
    fn ota_preflight_accepts_fast_forward_and_refuses_local_commits() {
        let repo = OtaTestRepository::new();
        let original = repo.git(&["rev-parse", "HEAD"]);
        repo.commit_change("upstream update\n");
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["reset", "--hard", &original]);
        super::ota_local_commits_are_preserved(&repo.0, "origin/main", "main").unwrap();
        repo.commit_change("local station extension\n");
        let local_head = repo.git(&["rev-parse", "HEAD"]);
        let error = super::ota_local_commits_are_preserved(&repo.0, "origin/main", "main").unwrap_err();
        assert!(error.contains("local commit(s)"));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), local_head, "preflight must preserve local history");
        assert_eq!(std::fs::read_to_string(repo.0.join("source.txt")).unwrap(), "local station extension\n");
    }

    #[test]
    fn ota_preflight_protects_existing_target_branch_when_head_is_elsewhere() {
        let repo = OtaTestRepository::new();
        let original = repo.git(&["rev-parse", "HEAD"]);
        repo.commit_change("local channel customization\n");
        let local_main = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["checkout", "--quiet", "--detach", &original]);
        let error = super::ota_local_commits_are_preserved(&repo.0, "origin/main", "main").unwrap_err();
        assert!(error.contains("refs/heads/main"));
        assert_eq!(repo.git(&["rev-parse", "main"]), local_main);
    }

    #[test]
    fn ota_checkout_preserves_ignored_local_files_newly_tracked_upstream() {
        let repo = OtaTestRepository::new();
        std::fs::write(repo.0.join(".gitignore"), "/target/\n/station-local.toml\n").unwrap();
        repo.git(&["add", ".gitignore"]);
        repo.git(&["commit", "--quiet", "--no-gpg-sign", "-m", "Ignore station settings"]);
        let original = repo.git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.0.join("station-local.toml"), "upstream defaults\n").unwrap();
        repo.git(&["add", "--force", "station-local.toml"]);
        repo.git(&["commit", "--quiet", "--no-gpg-sign", "-m", "Add default station settings"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["reset", "--hard", &original]);
        std::fs::write(repo.0.join("station-local.toml"), "local station settings\n").unwrap();
        super::ota_worktree_is_clean(&repo.0).unwrap();
        super::ota_local_commits_are_preserved(&repo.0, "origin/main", "main").unwrap();
        let output = super::ota_checkout_command(&repo.0, "main", "origin/main").output().unwrap();
        assert!(!output.status.success(), "checkout must refuse to overwrite ignored station settings");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), original);
        assert_eq!(std::fs::read_to_string(repo.0.join("station-local.toml")).unwrap(), "local station settings\n");
    }

    #[test]
    fn ota_preflight_fails_closed_for_unavailable_git_history() {
        let repo = OtaTestRepository::new();
        assert!(super::ota_local_commits_are_preserved(&repo.0, "origin/missing", "main").is_err());
        std::fs::rename(repo.0.join(".git"), repo.0.join("saved-git")).unwrap();
        assert!(super::ota_worktree_is_clean(&repo.0).is_err());
    }

    #[test]
    fn ota_rf_recovery_runs_on_failure_and_unwind_but_not_after_success() {
        use std::cell::Cell;
        let recoveries = Cell::new(0);
        let fail_build = || -> Result<(), ()> {
            let _guard = super::OtaRfRecoveryGuard::new(|| recoveries.set(recoveries.get() + 1));
            Err(())
        };
        assert!(fail_build().is_err());
        assert_eq!(recoveries.get(), 1);
        {
            let mut guard = super::OtaRfRecoveryGuard::new(|| recoveries.set(recoveries.get() + 1));
            guard.disarm();
        }
        assert_eq!(recoveries.get(), 1, "success already schedules its own restart");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = super::OtaRfRecoveryGuard::new(|| recoveries.set(recoveries.get() + 1));
            panic!("simulated interrupted OTA build");
        }));
        assert!(panic.is_err());
        assert_eq!(recoveries.get(), 2, "unwind must also restore RF through a restart");
    }

    /// A minimal config.toml that parses + validates (mirrors tetra-config's own minimal fixture).
    const MINIMAL_CONFIG: &str = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
"#;

    /// A bad config body posted to /api/config must be rejected (400) and must NOT touch the file on
    /// disk — otherwise the next restart fails to parse it and the base station crash-loops.
    #[test]
    fn bad_config_post_is_rejected_and_file_untouched() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("fs_a2_config_{}.toml", std::process::id()));
        let path_str = path.to_str().unwrap();
        std::fs::write(&path, MINIMAL_CONFIG).expect("seed config");

        // Garbage that does not parse as TOML.
        let res = write_config_validated(path_str, "this is not = valid toml [[[");
        match res {
            Err((400, _)) => {}
            other => panic!("expected 400 rejection, got {other:?}"),
        }
        // The on-disk config is exactly what we seeded — nothing was written, no .bak made.
        let after = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(after, MINIMAL_CONFIG, "running config must be untouched after a rejected write");
        assert!(
            !std::path::Path::new(&format!("{path_str}.bak")).exists(),
            "no backup should be written on rejection"
        );

        // A valid body is accepted and persisted.
        let good = format!("{}\n# touched\n", MINIMAL_CONFIG);
        write_config_validated(path_str, &good).expect("valid config accepted");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good, "valid config is written");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path_str}.bak"));
    }

    /// Raw config reads must mask plaintext secrets line-by-line, leaving structure/comments intact.
    #[test]
    fn config_get_masks_secrets() {
        let raw = "\
[telegram_alerts]
bot_token = \"123456:ABCdefGHIjklMNOpqrsWXYZ\"
# password = \"commented-should-stay\"

[dapnet]
password = \"supersecretpw\"
rwth_core_authkey = \"another-secret-key\"
enabled = true
";
        let masked = mask_config_secrets(raw);
        assert!(!masked.contains("123456:ABCdefGHIjklMNOpqrsWXYZ"), "bot_token cleartext leaked");
        assert!(!masked.contains("supersecretpw"), "dapnet password cleartext leaked");
        assert!(!masked.contains("another-secret-key"), "authkey cleartext leaked");
        // Structure / non-secret values / comments survive untouched.
        assert!(masked.contains("[telegram_alerts]"));
        assert!(masked.contains("enabled = true"));
        assert!(masked.contains("# password = \"commented-should-stay\""), "comments untouched");
    }

    /// The [tpg2200_action] ActionURL token is the sole credential guarding the pre-auth public
    /// /api/action/tpg2200 endpoint. It must be masked in raw config reads/backups like every other
    /// secret — it was previously leaked in cleartext because `token` was not in the mask set.
    #[test]
    fn config_get_masks_tpg2200_token() {
        let raw = "\
[tpg2200_action]
enabled = true
token = \"long-random-secret-token\"
dest_issi = 2632585
";
        let masked = mask_config_secrets(raw);
        assert!(!masked.contains("long-random-secret-token"), "tpg2200 token cleartext leaked");
        assert!(is_masked_secret_key("token"), "token must be treated as a masked secret key");
        // Non-secret values / structure survive untouched.
        assert!(masked.contains("[tpg2200_action]"));
        assert!(masked.contains("dest_issi = 2632585"));
    }

    /// A malformed config profile must be rejected at save time and never written — otherwise it can
    /// later be Activated over the live config and crash-loop the stack on the next restart. A valid
    /// profile is accepted.
    #[test]
    fn profile_save_validates_before_writing() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let cfg = dir.join(format!("fs_prof_active_{pid}.toml"));
        std::fs::write(&cfg, MINIMAL_CONFIG).expect("seed active config");
        let cfg_str = cfg.to_str().unwrap();

        let bad_name = format!("fs_prof_bad_{pid}.toml");
        let bad_res = save_config_profile(cfg_str, &bad_name, "this is not = valid toml [[[");
        assert!(bad_res.is_err(), "malformed profile must be rejected");
        assert!(!dir.join(&bad_name).exists(), "rejected profile must not be written to disk");

        let ok_name = format!("fs_prof_ok_{pid}.toml");
        save_config_profile(cfg_str, &ok_name, MINIMAL_CONFIG).expect("valid profile accepted");
        assert!(dir.join(&ok_name).exists(), "valid profile is written");

        let _ = std::fs::remove_file(&cfg);
        let _ = std::fs::remove_file(dir.join(&ok_name));
    }

    /// Activating a profile that does not parse/validate must be refused and must leave the live
    /// config untouched — the whole point of the fallback design is defeated if a bad profile can be
    /// copied over config.toml unchecked.
    #[test]
    fn profile_activate_rejects_invalid_and_preserves_live_config() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let cfg = dir.join(format!("fs_actprof_active_{pid}.toml"));
        std::fs::write(&cfg, MINIMAL_CONFIG).expect("seed active config");
        let cfg_str = cfg.to_str().unwrap();

        // A pre-existing malformed profile sitting on disk next to the config.
        let bad_name = format!("fs_actprof_bad_{pid}.toml");
        std::fs::write(dir.join(&bad_name), "not = valid [[[").expect("seed bad profile");

        let res = activate_config_profile(cfg_str, &bad_name);
        assert!(res.is_err(), "activating an unparseable profile must be rejected");
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            MINIMAL_CONFIG,
            "live config must be untouched when activation is rejected"
        );

        let _ = std::fs::remove_file(&cfg);
        let _ = std::fs::remove_file(dir.join(&bad_name));
    }

    #[test]
    fn html_mojibake_normalizer_repairs_common_dashboard_tokens() {
        assert_eq!(
            normalize_mojibake_html("Open Ã¢â‚¬â€ all ISSI may register"),
            "Open — all ISSI may register"
        );
        assert_eq!(normalize_mojibake_html("waitingÃ¢â‚¬Â¦"), "waiting…");
    }

    /// Authentication completes before the registration that creates the MS row, and the
    /// encrypting flag arrives from the MAC later: both must end up on the radio's row, and
    /// a deregistration must forget them.
    #[test]
    fn security_flags_survive_registration_order() {
        let server = DashboardServer::new("/tmp/fs_sec_flags_test_config.toml".to_string());
        let snap = |issi: u32| {
            server.state.read().unwrap().snapshot_ms().into_iter().find(|m| m.issi == issi).map(|m| (m.authenticated, m.encrypting))
        };

        server.handle_telemetry(TelemetryEvent::MsSecurity { issi: 2358245, authenticated: Some(true), encrypting: None });
        assert_eq!(snap(2358245), None, "no row before registration");
        server.handle_telemetry(TelemetryEvent::MsRegistration { issi: 2358245 });
        assert_eq!(snap(2358245), Some((true, false)));
        server.handle_telemetry(TelemetryEvent::MsSecurity { issi: 2358245, authenticated: None, encrypting: Some(true) });
        assert_eq!(snap(2358245), Some((true, true)));
        server.handle_telemetry(TelemetryEvent::MsDeregistration { issi: 2358245 });
        server.handle_telemetry(TelemetryEvent::MsRegistration { issi: 2358245 });
        assert_eq!(snap(2358245), Some((false, false)), "flags are per registration");
    }

    /// Field case (M5a, H): the radio went to another station, the stack here dropped it (a kick
    /// found nothing), but the row stayed listed. A move to a sibling cell keeps the row.
    #[test]
    fn a_radio_that_moved_to_another_station_leaves_the_list() {
        let server = DashboardServer::new("/tmp/fs_moved_away_test_config.toml".to_string());
        let row = |issi: u32| server.state.read().unwrap().snapshot_ms().into_iter().find(|m| m.issi == issi).map(|m| m.cell);

        // Single-cell station: no MsCell, the radio leaves cell 0 for another station.
        server.handle_telemetry(TelemetryEvent::MsRegistration { issi: 100 });
        server.handle_telemetry(TelemetryEvent::MsMovedAway { issi: 100, cell: 0 });
        assert_eq!(row(100), None);

        // Multi-cell: registered on cell 1, then on sibling cell 2, then cell 1 drops it.
        server.handle_telemetry(TelemetryEvent::MsRegistration { issi: 200 });
        server.handle_telemetry(TelemetryEvent::MsCell { issi: 200, cell: 1 });
        server.handle_telemetry(TelemetryEvent::MsRegistration { issi: 200 });
        server.handle_telemetry(TelemetryEvent::MsCell { issi: 200, cell: 2 });
        server.handle_telemetry(TelemetryEvent::MsMovedAway { issi: 200, cell: 1 });
        assert_eq!(row(200), Some(Some(2)), "still listed on the sibling cell");

        // Then cell 2 loses it to another station.
        server.handle_telemetry(TelemetryEvent::MsMovedAway { issi: 200, cell: 2 });
        assert_eq!(row(200), None);
    }

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

    /// The reported "passwords corrupted with random symbols" bug: secrets are served masked, and a
    /// save that doesn't retype them must restore the stored plaintext instead of persisting the
    /// mask. This is the full round-trip: mask on read → post the masked body back (only a non-secret
    /// field changed) → the real secrets survive. Section-aware, so `[dashboard] password` and
    /// `[brew] password` (identical bare key) are never crossed.
    #[test]
    fn config_save_restores_unchanged_masked_secrets() {
        use super::{mask_config_secrets, unmask_config_secrets};

        let stored = "\
[telegram_alerts]
bot_token = \"123456:ABCdefGHIjklMNOpqrsWXYZ\"

[dashboard]
username = \"admin\"
password = \"dashRealPw\"

[brew]
password = \"brewRealPw!\"
enabled = true
";
        // What the browser's editor receives — cleartext never leaves the host.
        let served = mask_config_secrets(stored);
        assert!(!served.contains("dashRealPw"));
        assert!(!served.contains("brewRealPw!"));
        assert!(!served.contains("123456:ABCdefGHIjklMNOpqrsWXYZ"));

        // Operator flips a non-secret field and posts the (still-masked) body back.
        let posted = served.replace("enabled = true", "enabled = false");
        let resolved = unmask_config_secrets(&posted, stored);

        // Each real secret is restored to its own section — dashboard and brew not crossed.
        assert!(resolved.contains("password = \"dashRealPw\""), "dashboard password restored");
        assert!(resolved.contains("password = \"brewRealPw!\""), "brew password restored");
        assert!(
            resolved.contains("bot_token = \"123456:ABCdefGHIjklMNOpqrsWXYZ\""),
            "bot token restored"
        );
        assert!(resolved.contains("enabled = false"), "non-secret edit preserved");
        // No mask placeholder characters are ever written to disk.
        assert!(
            !resolved.contains('\u{2022}') && !resolved.contains('\u{2026}'),
            "mask characters must not be persisted"
        );
    }

    const E2EE_KEY_ONE: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const E2EE_KEY_TWO: &str = "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";

    fn e2ee_config_fixture() -> String {
        format!(
            "{MINIMAL_CONFIG}\n[dashboard]\nusername = \"operator\"\npassword = \"station-test-password\"\n\n\
             [security.e2ee]\nenabled = true\n\n\
             [[security.e2ee.teks]]\n# first key precedes its id\nkey = '{E2EE_KEY_ONE}' # keep this comment\nid = 7\n\n\
             [[security.e2ee.teks]]\nid = 8\nkey = \"{E2EE_KEY_TWO}\"\n"
        )
    }

    fn e2ee_key_values(content: &str) -> std::collections::BTreeMap<u16, String> {
        super::config_e2ee_keys(content)
            .expect("valid TOML")
            .into_iter()
            .map(|tek| (tek.id.unwrap(), tek.key.unwrap().into_inner()))
            .collect()
    }

    #[test]
    fn e2ee_config_masks_entire_keys_and_preserves_two_key_round_trip() {
        let stored = e2ee_config_fixture();
        let served = mask_config_secrets(&stored);
        assert!(!served.contains(E2EE_KEY_ONE) && !served.contains(E2EE_KEY_TWO));
        assert!(!served.contains("station-test-password"));
        assert!(e2ee_key_values(&served).values().all(|key| key == &"\u{2022}".repeat(8)));
        assert!(served.contains("# keep this comment"), "keep operator annotations");

        let posted = served.replace("# keep this comment", "# non-secret edit");
        let restored = super::unmask_config_secrets(&posted, &stored);
        assert_eq!(e2ee_key_values(&restored), e2ee_key_values(&stored));
        assert!(restored.contains("# non-secret edit"));
        assert!(restored.contains("station-test-password"), "legacy credentials still round-trip");
    }

    #[test]
    fn e2ee_config_restores_reordered_teks_by_id() {
        let stored = e2ee_config_fixture();
        let served = mask_config_secrets(&stored);
        let tables: Vec<&str> = served.split("[[security.e2ee.teks]]").collect();
        let reordered = format!("{}[[security.e2ee.teks]]{}[[security.e2ee.teks]]{}", tables[0], tables[2], tables[1]);
        let restored = super::unmask_config_secrets(&reordered, &stored);
        assert_eq!(e2ee_key_values(&restored), e2ee_key_values(&stored));
    }

    #[test]
    fn e2ee_config_masks_inline_and_multiline_toml_key_values() {
        for stored in [
            format!("security.e2ee.teks = [{{ key = '{E2EE_KEY_ONE}', id = 7 }}, {{ id = 8, key = \"{E2EE_KEY_TWO}\" }}]\n"),
            format!("[[security . e2ee . teks]] # table comment\n\"key\" = '''{E2EE_KEY_ONE}''' # key comment\n\"id\" = 7\n"),
        ] {
            let served = mask_config_secrets(&stored);
            assert!(!served.contains(E2EE_KEY_ONE) && !served.contains(E2EE_KEY_TWO));
            assert_eq!(e2ee_key_values(&super::unmask_config_secrets(&served, &stored)), e2ee_key_values(&stored));
        }
    }

    #[test]
    fn e2ee_config_preserves_rotated_key_without_changing_other_teks() {
        let stored = e2ee_config_fixture();
        let rotated = "ab".repeat(32);
        let posted = mask_config_secrets(&stored).replacen(&"\u{2022}".repeat(8), &rotated, 1);
        let values = e2ee_key_values(&super::unmask_config_secrets(&posted, &stored));
        assert_eq!(values[&7], rotated);
        assert_eq!(values[&8], E2EE_KEY_TWO);
    }

    #[test]
    fn e2ee_config_unmatched_or_duplicate_ids_do_not_copy_another_key() {
        let stored = e2ee_config_fixture();
        for posted in [
            mask_config_secrets(&stored).replace("id = 7", "id = 9"),
            mask_config_secrets(&stored).replace("id = 8", "id = 7"),
        ] {
            let restored = super::unmask_config_secrets(&posted, &stored);
            assert!(restored.contains('\u{2022}'), "unmatched/ambiguous keys must stay masked and fail validation");
            assert!(tetra_config::bluestation::parsing::from_toml_str(&restored).is_err());
        }
    }

    #[test]
    fn e2ee_config_masking_rejects_invalid_toml_without_leaking_key_text() {
        let invalid = format!("[[security.e2ee.teks]]\nid = 7\nkey = \"{E2EE_KEY_ONE}");
        let served = mask_config_secrets(&invalid);
        assert!(!served.contains(E2EE_KEY_ONE));
        assert!(served.contains("cannot be displayed safely"));
    }

    #[test]
    fn e2ee_config_validated_save_keeps_distinct_keys_and_rejects_unknown_masked_id() {
        let dir = std::env::temp_dir().join(format!("flowstation-e2ee-save-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("config.toml");
        let stored = e2ee_config_fixture();
        std::fs::write(&path, &stored).unwrap();
        let served = mask_config_secrets(&stored);
        write_config_validated(path.to_str().unwrap(), &served).expect("masked save succeeds");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(e2ee_key_values(&saved), e2ee_key_values(&stored));
        let cfg = tetra_config::bluestation::parsing::from_toml_str(&saved).unwrap();
        assert_eq!(cfg.security.e2ee.as_ref().unwrap().teks.len(), 2);

        let invalid = served.replace("id = 7", "id = 9");
        assert!(matches!(write_config_validated(path.to_str().unwrap(), &invalid), Err((400, _))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved, "failed edit leaves stored keys untouched");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Drive `handle_ws_command` with real CMCE and MM channels and report both routes.
    fn run_ws_command_routed(json: &str) -> (Vec<ControlCommand>, Vec<ControlCommand>) {
        let (cmce_tx, cmce_rx) = crossbeam_channel::unbounded();
        let (mm_tx, mm_rx) = crossbeam_channel::unbounded();
        let state = Arc::new(RwLock::new(DashboardStateInner::new("/tmp/fs_ws_cmd_test.toml".to_string())));
        let cmd_tx = Arc::new(Mutex::new(Some(cmce_tx)));
        let mm_cmd_tx = Arc::new(Mutex::new(Some(mm_tx)));
        let update_state = Arc::new(Mutex::new(UpdateState::new()));
        handle_ws_command(
            json,
            &state,
            &cmd_tx,
            &mm_cmd_tx,
            &update_state,
            &None,
            &None,
            None,
        );
        (cmce_rx.try_iter().collect(), mm_rx.try_iter().collect())
    }

    /// Compatibility helper for tests whose expected destination is CMCE.
    fn run_ws_command(json: &str) -> Vec<ControlCommand> {
        run_ws_command_routed(json).0
    }

    /// SHIP-BLOCKER regression: a TETRA SSI is 24 bits and the PDU serializers assert that range,
    /// so an out-of-range `dest_issi` on the dashboard SDS path used to reach `write_bits(ssi, 24)`
    /// and abort the single stack thread — one WS frame crash-looping the whole cell. It must be
    /// refused at the dashboard boundary instead, and an in-range one must still go through.
    #[test]
    fn ws_sds_rejects_out_of_range_ssi() {
        // 2^24 — the first value that no longer fits the 24-bit address field.
        let over = MAX_TETRA_SSI as u64 + 1;
        let sent = run_ws_command(&format!(r#"{{"type":"sds","dest_issi":{over},"message":"boom"}}"#));
        assert!(sent.is_empty(), "out-of-range dest_issi must never reach CMCE, got {sent:?}");

        // 2^32 + 1 — `as u32` would silently truncate this to a perfectly legal-looking ISSI 1.
        let wrapped = 0x1_0000_0001u64;
        let sent = run_ws_command(&format!(r#"{{"type":"sds","dest_issi":{wrapped},"message":"boom"}}"#));
        assert!(sent.is_empty(), "wrapping dest_issi must be refused, not truncated");

        // Zero (the "no identity" sentinel) stays refused too.
        assert!(run_ws_command(r#"{"type":"sds","dest_issi":0,"message":"x"}"#).is_empty());

        // The largest legal SSI is accepted and reaches CMCE unchanged.
        let sent = run_ws_command(&format!(
            r#"{{"type":"sds","dest_issi":{},"message":"hello"}}"#,
            MAX_TETRA_SSI
        ));
        match sent.as_slice() {
            [ControlCommand::SendSds { dest_ssi, .. }] => assert_eq!(*dest_ssi, MAX_TETRA_SSI),
            other => panic!("expected one SendSds for a valid SSI, got {other:?}"),
        }
    }

    #[test]
    fn ws_otar_sck_routes_to_mm_not_cmce() {
        let (cmce, mm) = run_ws_command_routed(r#"{"type":"otar_sck","issi":200}"#);
        assert!(cmce.is_empty(), "OTAR must not reach CMCE: {cmce:?}");
        assert!(matches!(mm.as_slice(), [ControlCommand::OtarSck { issi: 200 }]));
    }

    /// The same 24-bit guard on the other privileged WS commands — each one addresses a radio or a
    /// group and ends up in a serializer with the same assertion.
    #[test]
    fn ws_kick_and_dgna_reject_out_of_range_ssi() {
        let over = MAX_TETRA_SSI as u64 + 1;
        assert!(run_ws_command(&format!(r#"{{"type":"kick","issi":{over}}}"#)).is_empty());
        for json in [
            format!(r#"{{"type":"dgna","issi":{over},"gssi":1000}}"#),
            format!(r#"{{"type":"dgna","issi":1000,"gssi":{over}}}"#),
            format!(r#"{{"type":"dgna_bulk","gssi":{over},"targets":[1000]}}"#),
        ] {
            let (cmce, mm) = run_ws_command_routed(&json);
            assert!(
                cmce.is_empty() && mm.is_empty(),
                "invalid DGNA must not reach either entity: CMCE={cmce:?}, MM={mm:?}"
            );
        }
        // Out-of-range entries are dropped from a bulk target list; the valid ones still go.
        let (cmce, mm) = run_ws_command_routed(&format!(r#"{{"type":"dgna_bulk","gssi":1000,"targets":[{over},4242]}}"#));
        assert!(cmce.is_empty(), "DGNA must not be sent to CMCE: {cmce:?}");
        match mm.as_slice() {
            [ControlCommand::Dgna { issi, gssi, .. }] => {
                assert_eq!((*issi, *gssi), (4242, 1000), "only the in-range target is regrouped");
            }
            other => panic!("expected one Dgna on MM for the single valid target, got {other:?}"),
        }
    }

    /// A `.bak` that does not parse (truncated by an interrupted write, or written by an older
    /// schema) must NOT be copied over the live config: the restore endpoint used to do exactly
    /// that and then restart, leaving an unbootable station with nothing to fall back to.
    #[test]
    fn config_restore_refuses_invalid_backup_and_preserves_live_config() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let cfg = dir.join(format!("fs_restore_{pid}.toml"));
        let cfg_str = cfg.to_str().unwrap().to_string();
        std::fs::write(&cfg, MINIMAL_CONFIG).expect("seed active config");

        // A truncated backup — parses as far as it goes, then stops mid-table.
        std::fs::write(format!("{cfg_str}.bak"), "config_version = \"0.6\"\n[phy_io\n").expect("seed bad backup");
        match restore_config_from_backup(&cfg_str) {
            Err((400, _)) => {}
            other => panic!("expected a 400 refusal for an unparseable backup, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            MINIMAL_CONFIG,
            "live config must be untouched when the backup is refused"
        );

        // A good backup restores, and the config it replaced is snapshotted so the operator can undo.
        let good = format!("{}\n# from backup\n", MINIMAL_CONFIG);
        std::fs::write(format!("{cfg_str}.bak"), &good).expect("seed good backup");
        restore_config_from_backup(&cfg_str).expect("valid backup restores");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), good, "valid backup is restored");
        assert_eq!(
            std::fs::read_to_string(format!("{cfg_str}.prerestore")).unwrap(),
            MINIMAL_CONFIG,
            "the replaced config is kept as a pre-restore snapshot"
        );

        let _ = std::fs::remove_file(&cfg);
        let _ = std::fs::remove_file(format!("{cfg_str}.bak"));
        let _ = std::fs::remove_file(format!("{cfg_str}.prerestore"));
    }

    /// Comparing credentials must not leak the *expected* secret's length via an early return.
    #[test]
    fn timing_safe_eq_is_exact_and_length_agnostic() {
        assert!(timing_safe_eq(b"hunter2", b"hunter2"));
        assert!(timing_safe_eq(b"", b""));
        assert!(!timing_safe_eq(b"hunter2", b"hunter"), "a prefix is not a match");
        assert!(!timing_safe_eq(b"hunter", b"hunter2"), "a shorter supplied value is not a match");
        assert!(!timing_safe_eq(b"", b"secret"));
        assert!(!timing_safe_eq(b"secret", b""));
        // The wrap-around must not make a repeated candidate compare equal to a shorter secret.
        assert!(!timing_safe_eq(b"abab", b"ab"));
    }

    /// Failed logins must cost across connections (the old per-connection sleep did nothing against
    /// a parallel attacker), and the tracking map must stay bounded so it isn't a DoS of its own.
    #[test]
    fn login_throttle_locks_out_and_stays_bounded() {
        use std::net::{IpAddr, Ipv4Addr};
        let mut t = LoginThrottle::new();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7));

        // Below the threshold: delay grows, but the address may keep trying.
        for _ in 0..super::LOGIN_LOCKOUT_AFTER - 1 {
            t.record_failure(ip);
            assert!(t.locked_for(&ip).is_none(), "must not lock out before the threshold");
        }
        // Crossing it locks the address out for everyone, on every connection.
        t.record_failure(ip);
        let lock = t.locked_for(&ip).expect("must lock out at the threshold");
        assert!(lock > std::time::Duration::from_secs(1));
        // Further failures escalate.
        t.record_failure(ip);
        assert!(t.locked_for(&ip).unwrap() > lock, "backoff must escalate");
        // A successful login wipes the history.
        t.clear(&ip);
        assert!(t.locked_for(&ip).is_none(), "success clears the lockout");

        // Rotating source addresses cannot grow the map without bound.
        for i in 0..(super::LOGIN_MAX_TRACKED_IPS as u32 + 500) {
            t.record_failure(IpAddr::V4(Ipv4Addr::from(i)));
        }
        assert!(
            t.entries.len() <= super::LOGIN_MAX_TRACKED_IPS,
            "throttle map must stay capped, got {}",
            t.entries.len()
        );
    }

    /// A genuinely retyped secret (no mask characters) must overwrite the stored one — otherwise the
    /// operator could never change a password.
    #[test]
    fn config_save_keeps_freshly_typed_secret() {
        use super::unmask_config_secrets;
        let stored = "[dashboard]\npassword = \"old-pw\"\n";
        let posted = "[dashboard]\npassword = \"brandNewPw\"\n";
        let resolved = unmask_config_secrets(posted, stored);
        assert!(resolved.contains("password = \"brandNewPw\""), "new password overwrites old");
        assert!(!resolved.contains("old-pw"), "old password gone once retyped");
    }
}
