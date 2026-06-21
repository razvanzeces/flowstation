use serde::Deserialize;

use crate::bluestation::SecretField;

/// Telegram alerts configuration.
///
/// The BTS owner creates a bot with @BotFather, pastes the bot token here, then registers one
/// or more chat IDs (their personal chat with the bot, or a group). When something notable
/// happens on the station, FlowStation sends a professionally-formatted alert to every chat ID.
///
/// The bot token is a secret and is wrapped in [`SecretField`] so it never leaks into logs.
/// Everything except the token can be toggled live from the dashboard without a restart
/// (see `effective_telegram` / `TelegramRuntimeOverride`); the new values are also written
/// back to the TOML so they persist.
#[derive(Debug, Clone)]
pub struct CfgTelegram {
    /// Master on/off for Telegram alerts.
    pub enabled: bool,
    /// Telegram Bot API token, obtained from @BotFather (e.g. "123456:ABC-DEF...").
    pub bot_token: SecretField,
    /// Destination chat IDs. Each receives every enabled alert. A negative value is a group/
    /// channel chat; a positive value is a private chat with the bot.
    pub chat_ids: Vec<i64>,

    /// Alert when a radio (MS) registers/attaches to the cell.
    pub alert_connect: bool,
    /// Alert when a radio deregisters/detaches.
    pub alert_disconnect: bool,
    /// Alert when a radio is dropped for not answering the periodic registration (T351).
    pub alert_t351: bool,
    /// Alert when a radio beacons its position over LIP/APRS.
    pub alert_lip: bool,
    /// Alert when the Brew/TetraPack backhaul connects or disconnects.
    pub alert_backhaul: bool,
    /// Forward the stack's own WARN/ERROR log lines as alerts (catch-all for critical status).
    pub alert_critical_logs: bool,
    /// Alert when the overall station-health level changes (Ok/Degraded/Critical transitions).
    pub alert_health: bool,
}

impl Default for CfgTelegram {
    fn default() -> Self {
        CfgTelegram {
            enabled: false,
            bot_token: SecretField::from(String::new()),
            chat_ids: Vec::new(),
            alert_connect: true,
            alert_disconnect: true,
            alert_t351: true,
            alert_lip: true,
            alert_backhaul: true,
            alert_critical_logs: true,
            alert_health: true,
        }
    }
}

impl CfgTelegram {
    /// True when alerts can actually be delivered: enabled, a token is set, and at least one
    /// recipient exists. The alerter short-circuits when this is false.
    pub fn is_deliverable(&self) -> bool {
        self.enabled && !self.bot_token.as_ref().trim().is_empty() && !self.chat_ids.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgTelegramDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub chat_ids: Vec<i64>,

    #[serde(default = "default_true")]
    pub alert_connect: bool,
    #[serde(default = "default_true")]
    pub alert_disconnect: bool,
    #[serde(default = "default_true")]
    pub alert_t351: bool,
    #[serde(default = "default_true")]
    pub alert_lip: bool,
    #[serde(default = "default_true")]
    pub alert_backhaul: bool,
    #[serde(default = "default_true")]
    pub alert_critical_logs: bool,
    #[serde(default = "default_true")]
    pub alert_health: bool,

    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, toml::Value>,
}

fn default_true() -> bool {
    true
}

pub fn apply_telegram_patch(dto: CfgTelegramDto) -> CfgTelegram {
    CfgTelegram {
        enabled: dto.enabled,
        bot_token: SecretField::from(dto.bot_token),
        chat_ids: dto.chat_ids,
        alert_connect: dto.alert_connect,
        alert_disconnect: dto.alert_disconnect,
        alert_t351: dto.alert_t351,
        alert_lip: dto.alert_lip,
        alert_backhaul: dto.alert_backhaul,
        alert_critical_logs: dto.alert_critical_logs,
        alert_health: dto.alert_health,
    }
}
