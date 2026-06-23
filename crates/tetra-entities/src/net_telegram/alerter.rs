//! The alerter worker thread.
//!
//! Consumes [`TelegramAlertMsg`]s, decides what is worth notifying about, and sends formatted
//! messages via [`TelegramClient`]. All policy lives here: which events map to which category,
//! de-duplication, and rate-limiting so a busy or flapping station never spams the owner.
//!
//! Settings are read live through [`SharedConfig::effective_telegram`], so toggling alerts on/off
//! or editing recipients from the dashboard takes effect without a restart.
//!
//! Loop-safety: the dashboard log thread forwards WARN/ERROR lines here as the "critical status"
//! catch-all. To avoid an infinite loop, this module logs its own send failures at `debug!`
//! level (the log thread only forwards WARN/ERROR), so a failing send never re-enters as an alert.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use tetra_config::bluestation::{CfgTelegram, SharedConfig};

use super::client::TelegramClient;
use super::format::{self, StationInfo};
use super::{TelegramAlertMsg, TelegramAlertSource};
use crate::net_snom::SnomNotifySink;
use crate::net_telemetry::TelemetryEvent;

/// SDS protocol identifier for the Location Information Protocol (LIP / APRS-style beacons).
const LIP_PROTOCOL_ID: u8 = 10;

/// How long to wait on the channel before running the maintenance cycle.
const POLL: Duration = Duration::from_millis(500);
/// Hold a generic disconnect briefly so an accompanying T351 drop can suppress it.
const DISCONNECT_COALESCE: Duration = Duration::from_secs(2);
/// A disconnect arriving within this window of a T351 drop for the same ISSI is the same event.
const T351_WINDOW: Duration = Duration::from_secs(5);
/// Minimum spacing between LIP beacon alerts from the same radio (beacons can be frequent).
const LIP_DEBOUNCE: Duration = Duration::from_secs(60);
/// Coalescing window for critical-log alerts: collect lines for this long, then send one message.
const LOG_COALESCE: Duration = Duration::from_secs(5);
/// Maximum log lines shown per coalesced message; the rest are summarised as "+N more".
const LOG_MAX_LINES: usize = 5;
/// Hard cap on buffered log lines between flushes (excess is counted, not stored).
const LOG_BUFFER_CAP: usize = 50;

pub struct TelegramAlerter {
    cfg: SharedConfig,
    source: TelegramAlertSource,
    client: TelegramClient,
    station: StationInfo,

    /// ISSIs currently considered attached — used to alert only on the unknown→known transition
    /// (so periodic re-registrations don't fire a "connected" alert).
    known_issis: HashSet<u32>,
    /// ISSI → time a T351 drop fired, so the accompanying generic deregistration is suppressed.
    recent_t351: HashMap<u32, Instant>,
    /// ISSI → time a deregistration arrived, held briefly so a T351 drop can upgrade it.
    pending_disconnect: HashMap<u32, Instant>,
    /// ISSI → time of the last LIP beacon alert (debounce).
    last_lip_alert: HashMap<u32, Instant>,
    /// Last observed backhaul state; `None` until the first observation (which is not alerted,
    /// to avoid a "connected" alert on every restart — only genuine up/down changes alert).
    last_brew: Option<bool>,

    /// Buffered critical-log lines awaiting a coalesced flush.
    log_buffer: Vec<(String, String)>,
    /// Count of log lines dropped because the buffer was full since the last flush.
    log_dropped: usize,
    /// When the current (non-empty) log buffer started accumulating.
    log_window_start: Option<Instant>,

    /// Last observed overall health level; `None` until the first snapshot. Used to alert only on
    /// level transitions (the monitor samples every few seconds, but most samples don't change).
    last_health: Option<crate::health::HealthLevel>,

    /// ISSIs currently in active emergency — alert only on the idle→emergency transition so the
    /// radio's periodic emergency re-sends don't re-fire the alert.
    emergency_issis: HashSet<u32>,
    /// Optional Snom display fanout. Receives the same already-formatted alert once per alert.
    snom_sink: Option<SnomNotifySink>,
}

impl TelegramAlerter {
    pub fn new(cfg: SharedConfig, source: TelegramAlertSource) -> Self {
        let station = StationInfo::from_config(&cfg);
        Self {
            cfg,
            source,
            client: TelegramClient::new(),
            station,
            known_issis: HashSet::new(),
            recent_t351: HashMap::new(),
            pending_disconnect: HashMap::new(),
            last_lip_alert: HashMap::new(),
            last_brew: None,
            log_buffer: Vec::new(),
            log_dropped: 0,
            log_window_start: None,
            last_health: None,
            emergency_issis: HashSet::new(),
            snom_sink: None,
        }
    }

    pub fn with_snom_sink(mut self, sink: Option<SnomNotifySink>) -> Self {
        self.snom_sink = sink;
        self
    }

    pub fn run(&mut self) {
        tracing::info!("Telegram alerter started");
        loop {
            match self.source.recv_timeout(POLL) {
                Ok(TelegramAlertMsg::Event(event)) => self.handle_event(event),
                Ok(TelegramAlertMsg::CriticalLog { level, message }) => self.handle_log(level, message),
                Ok(TelegramAlertMsg::Dapnet { prefix, callsign, text }) => self.handle_dapnet(prefix, callsign, text),
                Ok(TelegramAlertMsg::Meshcom { prefix, src, text }) => self.handle_dapnet(prefix, src, text),
                Ok(TelegramAlertMsg::Geoalarm { prefix, source, text }) => self.handle_dapnet(prefix, source, text),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.maintenance();
        }
        tracing::info!("Telegram alerter exiting");
    }

    fn handle_event(&mut self, event: TelemetryEvent) {
        let tg = self.cfg.effective_telegram();
        match event {
            TelemetryEvent::MsRegistration { issi } => {
                self.recent_t351.remove(&issi);
                // A registration that arrives while a disconnect is pending is a flap / immediate
                // re-registration: cancel the pending disconnect and emit nothing.
                if self.pending_disconnect.remove(&issi).is_some() {
                    self.known_issis.insert(issi);
                    return;
                }
                let is_new = self.known_issis.insert(issi);
                if is_new && tg.alert_connect && tg.is_deliverable() {
                    let html = format::connect(&self.station, issi);
                    self.send_all(&tg, &html);
                }
            }
            TelemetryEvent::MsDeregistration { issi } => {
                // Defer: a T351 drop (emitted right after) should replace the generic disconnect.
                self.pending_disconnect.entry(issi).or_insert_with(Instant::now);
            }
            TelemetryEvent::MsTimeoutDrop { issi } => {
                self.recent_t351.insert(issi, Instant::now());
                self.pending_disconnect.remove(&issi);
                // Only alert on the transition from known→dropped. A radio that stays gone can
                // re-trigger T351 first-expiry on later intervals; once we've dropped it from
                // known_issis it won't re-alert until it reconnects (MsRegistration) and drops
                // again. (On a fresh boot an unseen radio's drop is silent — intended.)
                let was_known = self.known_issis.remove(&issi);
                if was_known && tg.alert_t351 && tg.is_deliverable() {
                    let html = format::t351_drop(&self.station, issi);
                    self.send_all(&tg, &html);
                }
            }
            TelemetryEvent::BrewConnected { connected, server_version } => {
                let changed = self.last_brew.is_some_and(|prev| prev != connected);
                self.last_brew = Some(connected);
                if changed && tg.alert_backhaul && tg.is_deliverable() {
                    let html = format::backhaul(&self.station, connected, server_version);
                    self.send_all(&tg, &html);
                }
            }
            // LIP/APRS position beacons: SDS protocol id 10, received over the air from a radio.
            TelemetryEvent::SdsLog {
                direction,
                source_issi,
                dest_issi,
                protocol_id,
                text,
                ..
            } if protocol_id == LIP_PROTOCOL_ID && direction == "rx" => {
                let debounced = self.last_lip_alert.get(&source_issi).is_some_and(|t| t.elapsed() < LIP_DEBOUNCE);
                if !debounced {
                    self.last_lip_alert.insert(source_issi, Instant::now());
                    if tg.alert_lip && tg.is_deliverable() {
                        let html = format::lip_beacon(&self.station, source_issi, dest_issi, &text);
                        self.send_all(&tg, &html);
                    }
                }
            }
            // Station-health level change. Alert only on transitions; the first observation is
            // silent unless it's already not-Ok (so a healthy boot doesn't spam an alert).
            TelemetryEvent::HealthSnapshot(snap) => {
                let first = self.last_health.is_none();
                let changed = self.last_health != Some(snap.overall);
                self.last_health = Some(snap.overall);
                let healthy = matches!(snap.overall, crate::health::HealthLevel::Ok);
                if changed && (!first || !healthy) && tg.alert_health && tg.is_deliverable() {
                    let html = format::health(&self.station, &snap);
                    self.send_all(&tg, &html);
                }
            }
            // Emergency raised by a radio (emergency status PDU or emergency-priority call).
            // Alert only on the idle→emergency transition; gated by the [emergency] telegram_alert
            // toggle (read live), so the radio's periodic re-sends don't spam.
            TelemetryEvent::EmergencyAlarm { source_issi, dest_ssi } => {
                let is_new = self.emergency_issis.insert(source_issi);
                if is_new && self.cfg.config().emergency.telegram_alert && tg.is_deliverable() {
                    let html = format::emergency(&self.station, source_issi, dest_ssi);
                    self.send_all(&tg, &html);
                }
            }
            TelemetryEvent::EmergencyCancel { source_issi } => {
                self.emergency_issis.remove(&source_issi);
            }
            _ => {}
        }
    }

    fn handle_log(&mut self, level: String, message: String) {
        if self.log_buffer.is_empty() {
            self.log_window_start = Some(Instant::now());
        }
        if self.log_buffer.len() < LOG_BUFFER_CAP {
            self.log_buffer.push((level, message));
        } else {
            self.log_dropped += 1;
        }
    }

    fn handle_dapnet(&self, prefix: String, callsign: String, text: String) {
        let tg = self.cfg.effective_telegram();
        if tg.is_deliverable() {
            let html = format::dapnet(&self.station, &prefix, &callsign, &text);
            self.send_all(&tg, &html);
        }
    }

    /// Per-tick maintenance: emit deferred disconnects and flush the coalesced log buffer.
    fn maintenance(&mut self) {
        let tg = self.cfg.effective_telegram();

        // Deferred disconnects whose coalesce window elapsed.
        let due: Vec<u32> = self
            .pending_disconnect
            .iter()
            .filter(|(_, t)| t.elapsed() >= DISCONNECT_COALESCE)
            .map(|(issi, _)| *issi)
            .collect();
        for issi in due {
            self.pending_disconnect.remove(&issi);
            self.known_issis.remove(&issi);
            let suppressed = self.recent_t351.get(&issi).is_some_and(|t| t.elapsed() < T351_WINDOW);
            if !suppressed && tg.alert_disconnect && tg.is_deliverable() {
                let html = format::disconnect(&self.station, issi);
                self.send_all(&tg, &html);
            }
        }

        // Prune stale T351 markers.
        self.recent_t351.retain(|_, t| t.elapsed() < T351_WINDOW);

        // Flush the coalesced log buffer once its window elapses.
        if let Some(start) = self.log_window_start
            && start.elapsed() >= LOG_COALESCE
        {
            self.flush_logs(&tg);
        }
    }

    fn flush_logs(&mut self, tg: &CfgTelegram) {
        let buffer = std::mem::take(&mut self.log_buffer);
        let dropped = std::mem::take(&mut self.log_dropped);
        self.log_window_start = None;
        if buffer.is_empty() {
            return;
        }
        if tg.alert_critical_logs && tg.is_deliverable() {
            let shown = buffer.len().min(LOG_MAX_LINES);
            let extra = (buffer.len() - shown) + dropped;
            let html = format::critical_logs(&self.station, &buffer[..shown], extra);
            self.send_all(tg, &html);
        }
    }

    /// Deliver one HTML message to every configured chat. Failures are logged at `debug!` (never
    /// WARN/ERROR) so the critical-log catch-all cannot re-feed them as alerts.
    fn send_all(&self, tg: &CfgTelegram, html: &str) {
        if let Some(sink) = &self.snom_sink {
            sink.send_telegram_html(html.to_string());
        }
        let token = tg.bot_token.as_ref();
        for &chat_id in &tg.chat_ids {
            if let Err(e) = self.client.send_message_html(token, chat_id, html) {
                tracing::debug!("Telegram: send to chat {} failed: {}", chat_id, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::SharedConfig;

    /// A SharedConfig with no `[telegram_alerts]` section → `effective_telegram()` is not
    /// deliverable, so `handle_event` exercises the dedup/coalescing state machine without ever
    /// touching the network.
    fn test_cfg() -> SharedConfig {
        let toml = r#"
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
        let cfg = tetra_config::bluestation::parsing::from_toml_str(toml).unwrap();
        SharedConfig::from_parts(cfg, None)
    }

    fn alerter() -> TelegramAlerter {
        let (_sink, source) = super::super::telegram_alert_channel();
        // Keep the sink alive for the alerter's lifetime is unnecessary here — we call
        // handle_event directly and never run the recv loop.
        TelegramAlerter::new(test_cfg(), source)
    }

    #[test]
    fn connect_is_transition_only() {
        let mut a = alerter();
        a.handle_event(TelemetryEvent::MsRegistration { issi: 100 });
        assert!(a.known_issis.contains(&100));
        // A periodic re-registration of a known ISSI must not grow the set (i.e. no re-alert).
        a.handle_event(TelemetryEvent::MsRegistration { issi: 100 });
        assert_eq!(a.known_issis.len(), 1);
    }

    #[test]
    fn disconnect_is_deferred_then_t351_suppresses_it() {
        let mut a = alerter();
        a.handle_event(TelemetryEvent::MsRegistration { issi: 100 });
        // Deregistration is held (not applied) so a T351 drop can upgrade it.
        a.handle_event(TelemetryEvent::MsDeregistration { issi: 100 });
        assert!(a.pending_disconnect.contains_key(&100));
        assert!(a.known_issis.contains(&100));
        // The T351 drop fires its own alert, clears the pending generic disconnect, and forgets
        // the ISSI.
        a.handle_event(TelemetryEvent::MsTimeoutDrop { issi: 100 });
        assert!(a.recent_t351.contains_key(&100));
        assert!(!a.pending_disconnect.contains_key(&100));
        assert!(!a.known_issis.contains(&100));
    }

    #[test]
    fn reregister_cancels_pending_disconnect() {
        let mut a = alerter();
        a.handle_event(TelemetryEvent::MsRegistration { issi: 7 });
        a.handle_event(TelemetryEvent::MsDeregistration { issi: 7 });
        assert!(a.pending_disconnect.contains_key(&7));
        // A flap (re-registration within the coalesce window) cancels the pending disconnect.
        a.handle_event(TelemetryEvent::MsRegistration { issi: 7 });
        assert!(!a.pending_disconnect.contains_key(&7));
        assert!(a.known_issis.contains(&7));
    }

    #[test]
    fn t351_drop_for_unknown_issi_is_a_noop() {
        // A T351 drop for a radio we never saw connect must not be tracked as a known drop
        // (so it can't later masquerade as a transition). known_issis stays empty.
        let mut a = alerter();
        a.handle_event(TelemetryEvent::MsTimeoutDrop { issi: 555 });
        assert!(!a.known_issis.contains(&555));
        assert!(a.known_issis.is_empty());
        // It is still recorded as a recent T351 so a trailing generic disconnect is coalesced.
        assert!(a.recent_t351.contains_key(&555));
    }

    #[test]
    fn backhaul_first_observation_is_not_an_event() {
        let mut a = alerter();
        // First observation only records state (no spurious "connected" alert on every restart).
        a.handle_event(TelemetryEvent::BrewConnected {
            connected: true,
            server_version: 1,
        });
        assert_eq!(a.last_brew, Some(true));
    }

    #[test]
    fn critical_log_buffers_until_flush_window() {
        let mut a = alerter();
        a.handle_log("WARN".to_string(), "something".to_string());
        assert_eq!(a.log_buffer.len(), 1);
        assert!(a.log_window_start.is_some());
    }
}
