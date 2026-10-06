//! The station's public IPv4 address, for the SDS `ip` and `info` replies.
//!
//! The replies run on the stack's thread and must not wait for the Internet: they read the last
//! address known. A thread of its own asks a plain-text echo service (with a timeout) every
//! [`FRESH`], and a minute after a lookup that failed, so a station that started before its
//! network, or whose router got a new address, has the right one without any radio asking.
//! Lookups only happen once the station turns them on ([`enable`]), which it does when an SDS
//! command answers `ip` or `info`.

use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Plain-text echo services, tried in turn; each answers the address alone.
const SERVICES: [&str; 2] = ["https://api.ipify.org", "https://checkip.amazonaws.com"];
const TIMEOUT: Duration = Duration::from_secs(5);
/// How often the address is asked again.
pub const FRESH: Duration = Duration::from_secs(10 * 60);
/// How soon a lookup that failed is tried again.
const RETRY: Duration = Duration::from_secs(60);
/// Age from which the replies mark the address as old: a lookup was due [`FRESH`] before and none
/// has answered since.
pub const OLD: Duration = Duration::from_secs(2 * FRESH.as_secs());
const USER_AGENT: &str = concat!("FlowStation/", env!("CARGO_PKG_VERSION"));

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The last address found and when.
static KNOWN: Mutex<Option<(Ipv4Addr, Instant)>> = Mutex::new(None);

/// Turn the lookups on: the first one starts now, so that the first reply already has the
/// address. Only the first call starts the thread.
pub fn enable() {
    if ENABLED.swap(true, Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new().name("public-ip".to_string()).spawn(|| {
        // Not with the real-time priority of the thread that started it.
        crate::sndcp::wapgw::leave_realtime_scheduling();
        loop {
            let pause = match lookup() {
                Some(ip) => {
                    *KNOWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((ip, Instant::now()));
                    FRESH
                }
                // The last address known stays meanwhile, getting older.
                None => RETRY,
            };
            std::thread::sleep(pause);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("SDS-CMD: no public IP lookups: {e}");
        ENABLED.store(false, Ordering::Relaxed);
    }
}

/// The last public address known and how long ago it was found, at once. `None` until a lookup
/// answered, or while the lookups are off.
pub fn get() -> Option<(Ipv4Addr, Duration)> {
    let known = *KNOWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    known.map(|(ip, found)| (ip, found.elapsed()))
}

/// How the replies give the public address: "unknown" until there is one, and "(old)" after it
/// when it was found [`OLD`] ago or more (the lookups have been failing since).
pub fn text(known: Option<(Ipv4Addr, Duration)>) -> String {
    match known {
        None => "unknown".to_string(),
        Some((ip, age)) if age >= OLD => format!("{ip} (old)"),
        Some((ip, _)) => ip.to_string(),
    }
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
        assert!(!ENABLED.load(Ordering::Relaxed));
    }

    #[test]
    fn an_address_no_lookup_confirmed_for_20_minutes_is_marked_old() {
        let ip = Ipv4Addr::new(84, 127, 123, 185);
        assert_eq!(text(None), "unknown");
        assert_eq!(text(Some((ip, Duration::ZERO))), "84.127.123.185");
        // One lookup missed (the router changing its address, the line down for a while).
        assert_eq!(text(Some((ip, FRESH + RETRY))), "84.127.123.185");
        assert_eq!(text(Some((ip, OLD - Duration::from_secs(1)))), "84.127.123.185");
        assert_eq!(text(Some((ip, OLD))), "84.127.123.185 (old)");
        assert_eq!(text(Some((ip, Duration::from_secs(86_400)))), "84.127.123.185 (old)");
    }
}
