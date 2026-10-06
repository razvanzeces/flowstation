//! The station's public IPv4 address, for the SDS `ip` and `info` replies.
//!
//! The replies run on the stack's thread and must not wait for the Internet: they read the last
//! address known, and a lookup (a plain-text echo service, with a timeout, on a thread of its own)
//! refreshes it when it is missing or older than [`FRESH`]. Lookups only happen once the station
//! turns them on ([`enable`]), which it does when an SDS command answers `ip` or `info`.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Plain-text echo services, tried in turn; each answers the address alone.
const SERVICES: [&str; 2] = ["https://api.ipify.org", "https://checkip.amazonaws.com"];
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long an address is used before the next reply asks again.
pub const FRESH: Duration = Duration::from_secs(10 * 60);
const USER_AGENT: &str = concat!("FlowStation/", env!("CARGO_PKG_VERSION"));

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<State> = Mutex::new(State {
    ip: None,
    asked: None,
    busy: false,
});

struct State {
    ip: Option<Ipv4Addr>,
    /// When the last lookup started; `None` until then, and again after a lookup that failed.
    asked: Option<Instant>,
    busy: bool,
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Turn the lookups on and ask now, so that the first reply already has the address.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    let _ = get();
}

/// The last public address known, at once. When it is missing or older than [`FRESH`] a lookup
/// starts in the background for the next reply. `None` until a lookup answered, or while the
/// lookups are off.
pub fn get() -> Option<Ipv4Addr> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let mut s = state();
    let stale = s.asked.is_none_or(|t| t.elapsed() >= FRESH);
    if stale && !s.busy {
        s.busy = true;
        s.asked = Some(Instant::now());
        let spawned = std::thread::Builder::new().name("public-ip".to_string()).spawn(|| {
            // Spawned from the stack's thread: not with its real-time priority.
            crate::sndcp::wapgw::leave_realtime_scheduling();
            let found = lookup();
            let mut s = state();
            s.busy = false;
            match found {
                Some(ip) => s.ip = Some(ip),
                // Asked again by the next reply; the last address known stays meanwhile.
                None => s.asked = None,
            }
        });
        if spawned.is_err() {
            s.busy = false;
            s.asked = None;
        }
    }
    s.ip
}

fn lookup() -> Option<Ipv4Addr> {
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .ok()?;
    let found = SERVICES.iter().find_map(|url| {
        let body = client.get(*url).send().ok()?.error_for_status().ok()?.text().ok()?;
        parse(&body)
    });
    if found.is_none() {
        tracing::info!("SDS-CMD: no public IP from {:?}", SERVICES);
    }
    found
}

/// The address in the answer of an echo service: the text alone, maybe with a line end.
pub fn parse(body: &str) -> Option<Ipv4Addr> {
    body.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_answers() {
        // api.ipify.org and checkip.amazonaws.com (this one ends the line).
        assert_eq!(parse("84.127.123.185"), Some(Ipv4Addr::new(84, 127, 123, 185)));
        assert_eq!(parse("84.127.123.185\n"), Some(Ipv4Addr::new(84, 127, 123, 185)));
        for bad in ["", "<html>error</html>", "2a02:9130::1", "84.127.123", "84.127.123.185 x"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn nothing_and_no_lookup_while_off() {
        // The tests never turn the lookups on: no thread, no network.
        assert_eq!(get(), None);
        assert!(!state().busy && state().asked.is_none());
    }
}
