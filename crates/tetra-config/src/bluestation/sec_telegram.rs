use serde::{Deserialize, Serialize};

use crate::bluestation::SecretField;

/// A Telegram destination, optionally targeting a forum topic within the chat.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramRecipient {
    pub chat_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

impl TelegramRecipient {
    pub fn validate(&self) -> Result<(), String> {
        if self.chat_id == 0 || self.chat_id.unsigned_abs() > (1u64 << 52) - 1 {
            return Err("Chat ID must be a non-zero Telegram integer (at most 52 bits).".into());
        }
        if self.message_thread_id.is_some_and(|id| id <= 0) {
            return Err("Topic ID must be a positive integer.".into());
        }
        Ok(())
    }
}

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
    /// Additional destinations with optional forum topic IDs. Legacy `chat_ids` still work.
    pub recipients: Vec<TelegramRecipient>,

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
            recipients: Vec::new(),
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
        self.enabled && !self.bot_token.as_ref().trim().is_empty() && (!self.chat_ids.is_empty() || !self.recipients.is_empty())
    }

    /// Merge legacy chats and topic-aware recipients, avoiding duplicate deliveries to the
    /// same (chat, topic) pair while allowing several topics in a single group.
    pub fn destinations(&self) -> Vec<TelegramRecipient> {
        let mut destinations = Vec::new();
        for recipient in self
            .chat_ids
            .iter()
            .map(|&chat_id| TelegramRecipient {
                chat_id,
                message_thread_id: None,
            })
            .chain(self.recipients.iter().cloned())
        {
            if !destinations.contains(&recipient) {
                destinations.push(recipient);
            }
        }
        destinations
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
    #[serde(default)]
    pub recipients: Vec<TelegramRecipient>,

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
        recipients: dto.recipients,
        alert_connect: dto.alert_connect,
        alert_disconnect: dto.alert_disconnect,
        alert_t351: dto.alert_t351,
        alert_lip: dto.alert_lip,
        alert_backhaul: dto.alert_backhaul,
        alert_critical_logs: dto.alert_critical_logs,
        alert_health: dto.alert_health,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_deduplicate_pairs_without_merging_distinct_topics() {
        let normal = TelegramRecipient {
            chat_id: -100123,
            message_thread_id: None,
        };
        let topic = TelegramRecipient {
            chat_id: -100123,
            message_thread_id: Some(42),
        };
        let other_topic = TelegramRecipient {
            chat_id: -100123,
            message_thread_id: Some(99),
        };
        let cfg = CfgTelegram {
            chat_ids: vec![-100123, -100123],
            recipients: vec![normal.clone(), topic.clone(), topic.clone(), other_topic.clone()],
            ..Default::default()
        };
        assert_eq!(cfg.destinations(), vec![normal, topic, other_topic]);
    }
}
