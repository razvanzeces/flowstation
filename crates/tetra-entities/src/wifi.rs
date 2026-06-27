//! WiFi management via NetworkManager (`nmcli`).
//!
//! Wraps a small subset of `nmcli` to power the dashboard WiFi tab: scan
//! visible networks, list saved profiles, connect/disconnect, forget a saved
//! network. We chose nmcli because:
//!
//!   * It's the default network stack on Raspberry Pi OS Bookworm / Trixie,
//!     Debian 12+, Ubuntu 18+, and most modern desktop distros.
//!   * It speaks both Wi-Fi and Ethernet so users can swap interfaces
//!     without us having to re-implement the same logic twice.
//!   * It survives reboots — saved profiles persist in `/etc/NetworkManager/`.
//!
//! On hosts without NetworkManager (older RPi OS Buster, custom builds with
//! plain wpa_supplicant) `nmcli` is absent and every function returns the
//! sentinel error `WifiError::NotAvailable`. The dashboard surfaces that as a
//! clear "Not supported on this system — install NetworkManager" message
//! rather than failing silently.
//!
//! Safety considerations:
//!
//!   * `connect_new` builds the profile via `nmcli device wifi connect`,
//!     which validates the password against the AP before saving. A wrong
//!     password leaves no garbage profile behind.
//!   * Every operation has a 15 s timeout so a hung nmcli (rare but possible
//!     when the radio module is wedged) doesn't lock up the HTTP handler.
//!   * We never touch wpa_supplicant directly. NetworkManager and
//!     wpa_supplicant on the same interface fight; if the user has a custom
//!     wpa_supplicant.conf, NetworkManager is configured to leave it alone
//!     and our code respects that.

use std::process::{Command, Stdio};
use std::time::Duration;

/// Max time we'll wait for any single nmcli invocation. nmcli normally returns
/// within 2-3 seconds; we cap at 15 s so a stuck driver can't wedge the HTTP
/// thread indefinitely. The `--wait` flag we pass to connect commands is set
/// slightly lower so nmcli itself bails before our outer timeout fires.
const NMCLI_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "msg")]
pub enum WifiError {
    /// nmcli binary is not installed (no NetworkManager on this host).
    NotAvailable,
    /// nmcli returned a non-zero status; payload is stderr verbatim, trimmed.
    Failed(String),
    /// Could not exec nmcli at all (binary present but blocked by sandbox etc).
    Io(String),
    /// nmcli took too long to respond.
    Timeout,
}

impl std::fmt::Display for WifiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WifiError::NotAvailable => write!(f, "NetworkManager (nmcli) not installed"),
            WifiError::Failed(s) => write!(f, "nmcli failed: {}", s),
            WifiError::Io(s) => write!(f, "nmcli exec error: {}", s),
            WifiError::Timeout => write!(f, "nmcli timed out"),
        }
    }
}

/// A network visible to the radio right now. Returned by `scan()`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WifiScanResult {
    pub ssid: String,
    /// Signal strength as percentage 0-100 (nmcli's SIGNAL field).
    pub signal: u8,
    /// e.g. "WPA2", "WPA3", "WPA1 WPA2", "--" for open networks.
    pub security: String,
    /// True if this SSID matches a currently-saved profile.
    pub saved: bool,
    /// True if this is the SSID we're currently connected to.
    pub active: bool,
}

/// A saved profile (whether or not the AP is currently in range).
#[derive(Debug, Clone, serde::Serialize)]
pub struct WifiSavedProfile {
    pub uuid: String,
    pub name: String,
    /// "802-11-wireless" for Wi-Fi profiles. We use this to filter out
    /// Ethernet / VPN / bridge profiles which `nmcli con` also returns.
    pub conn_type: String,
    /// True if this is the connection currently bringing up the Wi-Fi iface.
    pub active: bool,
}

/// Snapshot of the current Wi-Fi connection state. Returned by `status()`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WifiStatus {
    /// True if there's a Wi-Fi device the kernel can see (`wlan0` etc).
    pub device_present: bool,
    /// True if NetworkManager has Wi-Fi radio enabled.
    pub radio_enabled: bool,
    /// SSID currently connected to, if any.
    pub connected_ssid: Option<String>,
    /// Signal strength of the active connection, 0-100.
    pub signal: Option<u8>,
    /// IPv4 address of the wireless device, if connected.
    pub ip_address: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────

/// Cheap one-off probe: is nmcli installed and does it run?
/// The dashboard calls this once at page-load to decide whether to enable
/// the WiFi tab at all.
pub fn available() -> bool {
    match run_nmcli(&["--version"]) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Trigger a fresh scan and return what's visible. nmcli's `--rescan yes`
/// forces a new scan rather than returning cached results, which is what
/// the user expects after pressing "Refresh" in the UI.
pub fn scan() -> Result<Vec<WifiScanResult>, WifiError> {
    // Field order chosen so we can split on the terse colon separator nmcli
    // emits in `-t` (terse) mode. INUSE is '*' when active, blank otherwise.
    // `--rescan auto` lets NetworkManager decide: it forces a fresh scan only when its
    // cached results are stale, otherwise it returns the cache. `--rescan yes` forces a
    // scan every call and nmcli rejects it ("Scanning not allowed immediately following
    // previous scan") when called again within its rate-limit window — which is exactly
    // what made the scan fail every time the WiFi tab was reopened shortly after the first
    // scan. `auto` is reliable on repeated opens and still refreshes when genuinely stale.
    let out = run_nmcli(&[
        "-t",
        "-f",
        "IN-USE,SSID,SIGNAL,SECURITY",
        "device",
        "wifi",
        "list",
        "--rescan",
        "auto",
    ])?;

    // Build a set of saved SSIDs so we can mark them in the scan results.
    // We do this *after* the scan call so the scan can fail fast if nmcli is
    // gone; if listing saved profiles fails we just treat everything as
    // not-saved rather than aborting the whole scan.
    let saved_ssids: std::collections::HashSet<String> = list_saved().unwrap_or_default().into_iter().map(|p| p.name).collect();

    let mut results = Vec::new();
    for line in out.lines() {
        // nmcli's terse format escapes literal ':' inside fields as '\:', so
        // a naïve split(':') would mangle SSIDs containing colons. We do a
        // single-char state machine that unescapes as it goes.
        let fields = parse_terse_line(line);
        if fields.len() < 4 {
            continue;
        }
        let in_use = fields[0].as_str();
        let ssid = fields[1].clone();
        let signal: u8 = fields[2].parse().unwrap_or(0);
        let security = if fields[3].is_empty() {
            "--".to_string()
        } else {
            fields[3].clone()
        };
        // Hidden networks show up with empty SSID; skip them so the list
        // isn't polluted by anonymous duplicates. Hidden APs can still be
        // added via the manual "Connect to hidden network" UI path.
        if ssid.is_empty() {
            continue;
        }
        let saved = saved_ssids.contains(&ssid);
        let active = in_use == "*";
        results.push(WifiScanResult {
            ssid,
            signal,
            security,
            saved,
            active,
        });
    }

    // Deduplicate by SSID, keeping the strongest signal. APs broadcasting on
    // multiple channels (very common) otherwise show up as duplicates.
    results.sort_by(|a, b| a.ssid.cmp(&b.ssid).then(b.signal.cmp(&a.signal)));
    results.dedup_by(|a, b| a.ssid == b.ssid);
    // Now resort by signal for display.
    results.sort_by(|a, b| b.signal.cmp(&a.signal));
    Ok(results)
}

/// List Wi-Fi profiles saved by NetworkManager. Ethernet / VPN profiles are
/// filtered out — the UI only wants to show Wi-Fi.
pub fn list_saved() -> Result<Vec<WifiSavedProfile>, WifiError> {
    let out = run_nmcli(&["-t", "-f", "UUID,NAME,TYPE,ACTIVE", "connection", "show"])?;
    let mut profiles = Vec::new();
    for line in out.lines() {
        let fields = parse_terse_line(line);
        if fields.len() < 4 {
            continue;
        }
        let conn_type = fields[2].clone();
        if conn_type != "802-11-wireless" {
            continue;
        }
        profiles.push(WifiSavedProfile {
            uuid: fields[0].clone(),
            name: fields[1].clone(),
            conn_type,
            active: fields[3] == "yes",
        });
    }
    Ok(profiles)
}

/// Bring up an already-saved profile. Use this for "reconnect to a network
/// I've used before" — no password is required because nmcli has it stored.
pub fn connect_saved(uuid: &str) -> Result<(), WifiError> {
    // --wait gives nmcli up to 12 s to actually associate before returning;
    // without it nmcli returns immediately after starting the connection and
    // the dashboard never knows whether it worked.
    run_nmcli(&["--wait", "12", "connection", "up", "uuid", uuid])?;
    Ok(())
}

/// Connect to a brand-new SSID. If `psk` is empty we treat it as open Wi-Fi
/// (no password). nmcli validates the password against the AP before saving
/// the profile, so a wrong PSK doesn't leave junk behind.
pub fn connect_new(ssid: &str, psk: &str, hidden: bool) -> Result<(), WifiError> {
    // We build the argument vector dynamically because nmcli rejects an
    // empty --password value with a confusing error; open networks need the
    // flag entirely omitted.
    let mut args: Vec<&str> = vec!["--wait", "12", "device", "wifi", "connect", ssid];
    if !psk.is_empty() {
        args.push("password");
        args.push(psk);
    }
    if hidden {
        args.push("hidden");
        args.push("yes");
    }
    run_nmcli(&args)?;
    Ok(())
}

/// Disconnect the active Wi-Fi connection. This *deactivates* the profile but
/// keeps it saved — calling `connect_saved` later re-uses the stored PSK.
pub fn disconnect(iface: &str) -> Result<(), WifiError> {
    run_nmcli(&["device", "disconnect", iface])?;
    Ok(())
}

/// Delete a saved profile entirely. Use this for "forget this network".
pub fn forget(uuid: &str) -> Result<(), WifiError> {
    run_nmcli(&["connection", "delete", "uuid", uuid])?;
    Ok(())
}

/// Turn Wi-Fi radio on/off at the NetworkManager level. Useful when the
/// host is on Ethernet and the operator wants to silence Wi-Fi for power
/// reasons.
pub fn set_radio(enabled: bool) -> Result<(), WifiError> {
    run_nmcli(&["radio", "wifi", if enabled { "on" } else { "off" }])?;
    Ok(())
}

/// Current state: device present, radio state, connected SSID, IPv4.
pub fn status() -> Result<WifiStatus, WifiError> {
    // Two separate calls because `nmcli general` doesn't expose enough and
    // we'd rather fail one cheap call than try to parse a long composite.

    // 1) Radio enabled?
    let radio_enabled = run_nmcli(&["-t", "-f", "WIFI", "radio"])
        .map(|s| s.trim().eq_ignore_ascii_case("enabled"))
        .unwrap_or(false);

    // 2) Find the Wi-Fi device + its state + SSID in a single nmcli call.
    //    Output looks like:  wlan0:wifi:connected:MyNetwork
    let dev_out = run_nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"])?;
    let mut device_present = false;
    let mut connected_ssid: Option<String> = None;
    let mut wifi_dev: Option<String> = None;
    for line in dev_out.lines() {
        let fields = parse_terse_line(line);
        if fields.len() < 4 {
            continue;
        }
        if fields[1] == "wifi" {
            device_present = true;
            wifi_dev = Some(fields[0].clone());
            if fields[2] == "connected" && !fields[3].is_empty() && fields[3] != "--" {
                connected_ssid = Some(fields[3].clone());
            }
            break;
        }
    }

    // 3) If connected, fetch signal + IP. These are nice-to-have so we
    //    silently tolerate failure.
    let mut signal: Option<u8> = None;
    let mut ip_address: Option<String> = None;
    if let (Some(_ssid), Some(dev)) = (connected_ssid.as_ref(), wifi_dev.as_ref()) {
        // Active scan entry gives signal strength of the AP we're on.
        if let Ok(out) = run_nmcli(&["-t", "-f", "IN-USE,SIGNAL", "device", "wifi", "list", "ifname", dev]) {
            for line in out.lines() {
                let fields = parse_terse_line(line);
                if fields.len() >= 2 && fields[0] == "*" {
                    signal = fields[1].parse().ok();
                    break;
                }
            }
        }
        if let Ok(out) = run_nmcli(&["-t", "-f", "IP4.ADDRESS", "device", "show", dev]) {
            for line in out.lines() {
                // Format: IP4.ADDRESS[1]:192.168.1.42/24
                if let Some(rest) = line.split(':').nth(1) {
                    if !rest.is_empty() {
                        // strip CIDR suffix for prettier display
                        let ip = rest.split('/').next().unwrap_or(rest).to_string();
                        ip_address = Some(ip);
                        break;
                    }
                }
            }
        }
    }

    Ok(WifiStatus {
        device_present,
        radio_enabled,
        connected_ssid,
        signal,
        ip_address,
    })
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

/// Run nmcli with the given args, return stdout as a String, with timeout.
///
/// We spawn the child and poll `try_wait` so we can enforce a timeout — the
/// stdlib doesn't have a `wait_with_timeout` for `Child` directly. 50 ms
/// poll interval is short enough that human-perceived latency on success
/// (nmcli returns in 2-3 s typically) is unaffected.
fn run_nmcli(args: &[&str]) -> Result<String, WifiError> {
    let mut child = match Command::new("nmcli")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            // Distinguish "not installed" (ENOENT) from other IO errors so
            // the UI can show a helpful install hint instead of a generic
            // exec error.
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(WifiError::NotAvailable);
            }
            return Err(WifiError::Io(e.to_string()));
        }
    };

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().map_err(|e| WifiError::Io(e.to_string()))?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if status.success() {
                    return Ok(stdout);
                }
                return Err(WifiError::Failed(if stderr.is_empty() {
                    format!("exit code {}", status.code().unwrap_or(-1))
                } else {
                    stderr
                }));
            }
            Ok(None) => {
                if start.elapsed() > NMCLI_TIMEOUT {
                    // Best-effort cleanup. If kill() fails the child may
                    // linger but the OS will reap it eventually.
                    let _ = child.kill();
                    return Err(WifiError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(WifiError::Io(e.to_string())),
        }
    }
}

/// Parse one line of nmcli `-t` (terse) output into its colon-separated
/// fields, handling the `\:` escape that nmcli uses for literal colons
/// inside field values (most commonly in SSIDs).
fn parse_terse_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Backslash escapes the next char literally. nmcli only escapes
            // ':' and '\\' but we treat any escaped char as literal for
            // safety against unknown future escapes.
            if let Some(next) = chars.next() {
                out.last_mut().unwrap().push(next);
            }
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}
