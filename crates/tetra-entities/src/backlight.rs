//! Display backlight brightness control for the dashboard (FH-FEAT-008).
//!
//! Writes an integer brightness to the kernel backlight sysfs node
//! (`/sys/class/backlight/<device>/brightness`). The node is auto-discovered, so the
//! feature is self-disabling on hosts with no backlight panel (e.g. a desktop dev
//! box): [`status`] reports `present: false` and the dashboard hides the slider.
//!
//! Writing the node needs privilege. We try, in order:
//!   1. A direct `std::fs::write` — succeeds when the service runs as root, or when a
//!      udev rule has made the node group-writable (the preferred deployment).
//!   2. `sudo -n tee <node>` — for hosts that grant a NOPASSWD sudoers entry for
//!      exactly that node. `-n` never prompts, so a missing rule fails fast.
//!
//! All paths are confined to `/sys/class/backlight` (from discovery, never user
//! input) and only ASCII digits ever reach `tee`'s stdin — no shell, no injection.
//! The child is timeout-guarded so a wedged `sudo`/`tee` cannot lock the HTTP handler
//! (a detached `dashboard-conn` thread, off the TETRA stack).
//!
//! Deployment note: prefer a udev rule that group-writes the node (no sudo needed):
//!   SUBSYSTEM=="backlight", ACTION=="add", \
//!     RUN+="/bin/chgrp video /sys/class/backlight/%k/brightness", \
//!     RUN+="/bin/chmod g+w /sys/class/backlight/%k/brightness"
//! Otherwise a narrowly-scoped sudoers entry (chmod 0440), and DO NOT broaden it:
//!   tetra ALL=(root) NOPASSWD: /usr/bin/tee /sys/class/backlight/*/brightness

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Upper bound on the brightness value accepted from the UI. The official RPi/DSI
/// panels FlowStation targets use a 0-255 range; the written value is additionally
/// clamped to the device's own `max_brightness`.
pub const MAX_VALUE: u32 = 255;

/// Cap on how long we wait for the privileged write before giving up — a wedged
/// `sudo`/`tee` must never lock the HTTP handler thread.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "msg")]
pub enum BacklightError {
    /// No backlight panel on this host (nothing under /sys/class/backlight).
    NotAvailable,
    /// Requested value outside 0..=MAX_VALUE.
    OutOfRange,
    /// The write (direct and the sudo fallback) failed; payload is the reason.
    Failed(String),
    /// Could not spawn the helper at all.
    Io(String),
    /// The privileged write took too long.
    Timeout,
}

impl std::fmt::Display for BacklightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BacklightError::NotAvailable => write!(f, "no backlight device on this host"),
            BacklightError::OutOfRange => write!(f, "brightness must be 0..={MAX_VALUE}"),
            BacklightError::Failed(s) => write!(f, "backlight write failed: {s}"),
            BacklightError::Io(s) => write!(f, "backlight exec error: {s}"),
            BacklightError::Timeout => write!(f, "backlight write timed out"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BacklightStatus {
    pub present: bool,
    pub device: Option<String>,
    pub brightness: Option<u32>,
    pub max_brightness: Option<u32>,
}

/// First device under /sys/class/backlight (alphabetical, deterministic) that exposes
/// a `brightness` node.
fn discover() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir("/sys/class/backlight")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.into_iter().find(|p| p.join("brightness").exists())
}

fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse::<u32>().ok()
}

/// Current backlight status. Never errors — an absent panel is reported as
/// `present: false` so the UI can hide the control gracefully.
pub fn status() -> BacklightStatus {
    match discover() {
        None => BacklightStatus {
            present: false,
            device: None,
            brightness: None,
            max_brightness: None,
        },
        Some(dir) => BacklightStatus {
            present: true,
            device: dir.file_name().map(|n| n.to_string_lossy().into_owned()),
            brightness: read_u32(&dir.join("brightness")),
            max_brightness: read_u32(&dir.join("max_brightness")),
        },
    }
}

/// Set the backlight brightness. `value` must be 0..=[`MAX_VALUE`]; it is additionally
/// clamped to the device's `max_brightness` (if readable) before writing. Tries a
/// direct sysfs write first, then `sudo -n tee`.
pub fn set_brightness(value: u32) -> Result<(), BacklightError> {
    if value > MAX_VALUE {
        return Err(BacklightError::OutOfRange);
    }
    let dir = discover().ok_or(BacklightError::NotAvailable)?;
    let node = dir.join("brightness");
    let clamped = match read_u32(&dir.join("max_brightness")) {
        Some(max) => value.min(max),
        None => value,
    };
    let payload = clamped.to_string();

    // 1) Direct write — works as root or with a udev-granted group write.
    if std::fs::write(&node, payload.as_bytes()).is_ok() {
        return Ok(());
    }

    // 2) sudo -n tee <node> — needs a NOPASSWD sudoers entry for exactly this node.
    write_via_sudo_tee(&node, &payload)
}

/// `<value> | sudo -n tee <node> >/dev/null`, timeout-guarded. Only ASCII digits reach
/// stdin and `node` is /sys-confined from discovery — no shell, no injection.
fn write_via_sudo_tee(node: &Path, payload: &str) -> Result<(), BacklightError> {
    let mut child = match Command::new("sudo")
        .arg("-n")
        .arg("tee")
        .arg(node)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(BacklightError::Failed(
                    "sudo not found and direct write denied — grant a udev group-write rule".into(),
                ));
            }
            return Err(BacklightError::Io(e.to_string()));
        }
    };

    // Send the value, then drop stdin to signal EOF so `tee` completes.
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(payload.as_bytes()) {
            let _ = child.kill();
            return Err(BacklightError::Io(e.to_string()));
        }
    }

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                let output = child.wait_with_output().map_err(|e| BacklightError::Io(e.to_string()))?;
                if exit.success() {
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(BacklightError::Failed(if stderr.is_empty() {
                    format!(
                        "exit code {} (no sudoers rule? prefer a udev group-write rule)",
                        exit.code().unwrap_or(-1)
                    )
                } else {
                    stderr
                }));
            }
            Ok(None) => {
                if start.elapsed() > WRITE_TIMEOUT {
                    let _ = child.kill();
                    return Err(BacklightError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(BacklightError::Io(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_before_touching_hardware() {
        // 256 is rejected purely on the value, regardless of host hardware.
        assert!(matches!(set_brightness(MAX_VALUE + 1), Err(BacklightError::OutOfRange)));
    }

    #[test]
    fn status_is_graceful_without_a_panel() {
        // On a host with no /sys/class/backlight (CI / dev mac) this must not panic and
        // must report present:false rather than erroring.
        let st = status();
        if !st.present {
            assert!(st.device.is_none());
            assert!(st.brightness.is_none());
            // A valid value on a panel-less host surfaces NotAvailable, never a panic.
            assert!(matches!(set_brightness(128), Err(BacklightError::NotAvailable)));
        }
    }
}
