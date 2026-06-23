//! Telegram alerts.
//!
//! Sends professionally-formatted notifications to the BTS owner's Telegram bot when notable
//! things happen on the station: a radio attaches or detaches, a radio is dropped for not
//! answering the periodic registration (T351), the Brew/TetraPack backhaul goes up or down, a
//! radio beacons its position over LIP/APRS, or the stack logs a WARN/ERROR.
//!
//! Design:
//! - The owner configures everything from the dashboard "Telegram Alerts" menu (bot token,
//!   detected chat IDs, per-category toggles). Settings are persisted to the TOML and applied
//!   live via [`tetra_config::bluestation::SharedConfig::effective_telegram`].
//! - [`TelegramAlerter`] runs on its own thread, consuming [`TelegramAlertMsg`]s fed from the
//!   telemetry tee and the dashboard log thread, off the real-time stack path. All outbound
//!   HTTPS is blocking (`reqwest::blocking`), exactly like the built-in WX/METAR fetch.
//! - [`TelegramClient`] talks to the Telegram Bot API: validate a token, detect the chats that
//!   messaged the bot (so the owner just clicks a button), and send messages.

pub mod alerter;
pub mod client;
pub mod format;

pub use alerter::TelegramAlerter;
pub use client::{DetectedChat, TelegramClient};

use crate::net_telemetry::TelemetryEvent;

/// One item handed to the alerter: either a telemetry event or a captured WARN/ERROR log line.
#[derive(Debug, Clone)]
pub enum TelegramAlertMsg {
    /// A telemetry event from the stack (the alerter decides whether it's alert-worthy).
    Event(TelemetryEvent),
    /// A captured stack log line at WARN or ERROR level (the "critical status" catch-all).
    CriticalLog { level: String, message: String },
    /// A DAPNET message forwarded through the existing Telegram alert delivery path.
    Dapnet { prefix: String, callsign: String, text: String },
    /// A MeshCom text message forwarded through the existing Telegram alert delivery path.
    Meshcom { prefix: String, src: String, text: String },
    /// A GeoAlarm geofence event forwarded through the existing Telegram alert delivery path.
    Geoalarm { prefix: String, source: String, text: String },
}

/// Cloneable, push-only handle. Cloned into the telemetry-tee and dashboard-log threads.
/// Fire-and-forget: silently drops if the alerter has gone away.
#[derive(Clone)]
pub struct TelegramAlertSink {
    tx: crossbeam_channel::Sender<TelegramAlertMsg>,
}

impl TelegramAlertSink {
    /// Forward a telemetry event to the alerter.
    #[inline]
    pub fn send_event(&self, event: TelemetryEvent) {
        let _ = self.tx.send(TelegramAlertMsg::Event(event));
    }

    /// Forward a captured WARN/ERROR log line to the alerter.
    #[inline]
    pub fn send_log(&self, level: String, message: String) {
        let _ = self.tx.send(TelegramAlertMsg::CriticalLog { level, message });
    }

    /// Forward a DAPNET message to the alerter for Telegram delivery.
    #[inline]
    pub fn send_dapnet(&self, prefix: String, callsign: String, text: String) {
        let _ = self.tx.send(TelegramAlertMsg::Dapnet { prefix, callsign, text });
    }

    /// Forward a MeshCom message to the alerter for Telegram delivery.
    #[inline]
    pub fn send_meshcom(&self, prefix: String, src: String, text: String) {
        let _ = self.tx.send(TelegramAlertMsg::Meshcom { prefix, src, text });
    }

    /// Forward a GeoAlarm event to the alerter for Telegram delivery.
    #[inline]
    pub fn send_geoalarm(&self, prefix: String, source: String, text: String) {
        let _ = self.tx.send(TelegramAlertMsg::Geoalarm { prefix, source, text });
    }
}

/// Receive side, owned by the [`TelegramAlerter`].
pub struct TelegramAlertSource {
    rx: crossbeam_channel::Receiver<TelegramAlertMsg>,
}

impl TelegramAlertSource {
    /// Blocking receive with timeout. `Err(Timeout)` lets the alerter run its maintenance cycle
    /// (deferred disconnects, log batching); `Err(Disconnected)` means all sinks were dropped.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<TelegramAlertMsg, crossbeam_channel::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

/// Create a linked (sink, source) pair for the Telegram alerter.
pub fn telegram_alert_channel() -> (TelegramAlertSink, TelegramAlertSource) {
    let (tx, rx) = crossbeam_channel::unbounded();
    (TelegramAlertSink { tx }, TelegramAlertSource { rx })
}
