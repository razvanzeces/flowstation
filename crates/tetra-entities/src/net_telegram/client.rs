//! Minimal Telegram Bot API client (blocking).
//!
//! Three operations are all we need:
//! - [`TelegramClient::get_me`] — validate a bot token and learn the bot's @username.
//! - [`TelegramClient::get_updates`] — list the chats that recently messaged the bot, so the
//!   owner can pick their chat ID with one click instead of hunting for it.
//! - [`TelegramClient::send_message_html`] — deliver an alert (or a test message).
//!
//! Blocking HTTP, exactly like the built-in WX/METAR fetch. Always call from a worker thread
//! (the alerter) or the dashboard's per-connection thread — never from the stack loop.

use std::time::Duration;

const API_BASE: &str = "https://api.telegram.org";
const USER_AGENT: &str = "FlowStation-TelegramAlerts";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Bot identity returned by `getMe`.
#[derive(Debug, Clone)]
pub struct BotInfo {
    /// Bot username without the leading '@' (e.g. "MyStationBot").
    pub username: String,
}

/// A chat that recently messaged the bot, surfaced by `getUpdates` for one-click pickup.
#[derive(Debug, Clone)]
pub struct DetectedChat {
    /// Telegram chat ID (negative for groups/channels, positive for private chats).
    pub id: i64,
    /// Friendly label: a person's name, or a group/channel title, or "@username".
    pub name: String,
    /// Chat kind reported by Telegram: "private", "group", "supergroup", or "channel".
    pub kind: String,
    /// Forum topic observed in a recent message, if any.
    pub message_thread_id: Option<i32>,
}

pub struct TelegramClient {
    http: reqwest::blocking::Client,
}

impl Default for TelegramClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegramClient {
    pub fn new() -> Self {
        // If the client fails to build (should never happen with rustls-tls), fall back to a
        // default client so callers still get a clean per-request error instead of a panic.
        let http = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_default();
        Self { http }
    }

    fn method_url(token: &str, method: &str) -> String {
        format!("{API_BASE}/bot{token}/{method}")
    }

    /// Validate `token` and return the bot's identity. Err carries a human-readable reason.
    pub fn get_me(&self, token: &str) -> Result<BotInfo, String> {
        let url = Self::method_url(token, "getMe");
        let json = self.get_json(&url)?;
        let result = ok_result(&json)?;
        let username = result.get("username").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Ok(BotInfo { username })
    }

    /// List distinct chat/topic destinations that have recently messaged the bot. Telegram buffers updates
    /// for ~24h when no webhook is set, so the owner messages the bot once, then clicks detect.
    pub fn get_updates(&self, token: &str) -> Result<Vec<DetectedChat>, String> {
        let url = format!("{}?timeout=0&limit=100", Self::method_url(token, "getUpdates"));
        let json = self.get_json(&url)?;
        let result = ok_result(&json)?;
        let updates = result.as_array().ok_or_else(|| "unexpected getUpdates response".to_string())?;

        Ok(detect_destinations(updates))
    }

    /// Send an HTML-formatted message to `chat_id`. Used for both alerts and the test button.
    pub fn send_message_html(&self, token: &str, chat_id: i64, message_thread_id: Option<i32>, html: &str) -> Result<(), String> {
        let url = Self::method_url(token, "sendMessage");
        let body = message_body(chat_id, message_thread_id, html);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| format!("request failed: {e}"))?;
        let json: serde_json::Value = resp.json().map_err(|e| format!("read failed: {e}"))?;
        ok_result(&json).map(|_| ())
    }

    fn get_json(&self, url: &str) -> Result<serde_json::Value, String> {
        self.http
            .get(url)
            .send()
            .map_err(|e| format!("request failed: {e}"))?
            .json::<serde_json::Value>()
            .map_err(|e| format!("read failed: {e}"))
    }
}

fn message_body(chat_id: i64, message_thread_id: Option<i32>, html: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "chat_id": chat_id,
        "text": html,
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    if let Some(id) = message_thread_id {
        body["message_thread_id"] = id.into();
    }
    body
}

fn detect_destinations(updates: &[serde_json::Value]) -> Vec<DetectedChat> {
    let mut seen: Vec<DetectedChat> = Vec::new();
    for upd in updates {
        for key in ["message", "edited_message", "channel_post", "edited_channel_post", "my_chat_member"] {
            if let Some(message) = upd.get(key)
                && let Some(chat) = message.get("chat")
                && let Some(mut detected) = chat_to_detected(chat)
            {
                detected.message_thread_id = message
                    .get("message_thread_id")
                    .and_then(|v| v.as_i64())
                    .and_then(|id| i32::try_from(id).ok())
                    .filter(|id| *id > 0);
                if !seen
                    .iter()
                    .any(|c| c.id == detected.id && c.message_thread_id == detected.message_thread_id)
                {
                    seen.push(detected);
                }
            }
        }
    }
    seen
}

/// Extract `result` from a `{ ok, result }` envelope, or turn `{ ok:false, description }` into an Err.
fn ok_result(json: &serde_json::Value) -> Result<serde_json::Value, String> {
    if json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        Ok(json.get("result").cloned().unwrap_or(serde_json::Value::Null))
    } else {
        let desc = json
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown Telegram API error");
        Err(desc.to_string())
    }
}

/// Build a `DetectedChat` from a Telegram `chat` object, deriving a friendly display name.
fn chat_to_detected(chat: &serde_json::Value) -> Option<DetectedChat> {
    let id = chat.get("id").and_then(|v| v.as_i64())?;
    let kind = chat.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let title = chat.get("title").and_then(|v| v.as_str());
    let first = chat.get("first_name").and_then(|v| v.as_str());
    let last = chat.get("last_name").and_then(|v| v.as_str());
    let username = chat.get("username").and_then(|v| v.as_str());

    let name = if let Some(t) = title {
        t.to_string()
    } else if first.is_some() || last.is_some() {
        [first.unwrap_or(""), last.unwrap_or("")].join(" ").trim().to_string()
    } else if let Some(u) = username {
        format!("@{u}")
    } else {
        format!("Chat {id}")
    };

    Some(DetectedChat {
        id,
        name,
        kind,
        message_thread_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_message_payload_and_normal_chat_payload() {
        let topic = message_body(-100123, Some(42), "<b>test</b>");
        assert_eq!(topic["chat_id"], -100123);
        assert_eq!(topic["message_thread_id"], 42);
        assert_eq!(topic["parse_mode"], "HTML");
        assert_eq!(topic["text"], "<b>test</b>");
        let normal = message_body(-100123, None, "test");
        assert!(normal.get("message_thread_id").is_none());
    }

    #[test]
    fn detect_keeps_distinct_topics_and_deduplicates_repeated_messages() {
        let updates = serde_json::json!([
            {"message": {"chat": {"id": -100123, "type": "supergroup", "title": "Ops"}, "message_thread_id": 42}},
            {"edited_message": {"chat": {"id": -100123, "type": "supergroup"}, "message_thread_id": 42}},
            {"message": {"chat": {"id": -100123, "type": "supergroup"}, "message_thread_id": 99}},
            {"my_chat_member": {"chat": {"id": -100123, "type": "supergroup"}}},
            {"channel_post": {"chat": {"id": -100999, "type": "channel"}}}
        ]);
        let found = detect_destinations(updates.as_array().unwrap());
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].message_thread_id, Some(42));
        assert_eq!(found[1].message_thread_id, Some(99));
        assert_eq!(found[2].message_thread_id, None);
        assert_eq!(found[3].kind, "channel");
    }

    #[test]
    fn method_url_builds() {
        assert_eq!(
            TelegramClient::method_url("123:ABC", "getMe"),
            "https://api.telegram.org/bot123:ABC/getMe"
        );
    }

    #[test]
    fn ok_result_extracts() {
        let j = serde_json::json!({"ok": true, "result": {"username": "Bot"}});
        let r = ok_result(&j).unwrap();
        assert_eq!(r.get("username").unwrap(), "Bot");
    }

    #[test]
    fn ok_result_surfaces_error_description() {
        let j = serde_json::json!({"ok": false, "description": "Unauthorized"});
        assert_eq!(ok_result(&j).unwrap_err(), "Unauthorized");
    }

    #[test]
    fn detect_private_chat_name() {
        let chat = serde_json::json!({"id": 42, "type": "private", "first_name": "Ana", "last_name": "Pop"});
        let d = chat_to_detected(&chat).unwrap();
        assert_eq!(d.id, 42);
        assert_eq!(d.name, "Ana Pop");
        assert_eq!(d.kind, "private");
    }

    #[test]
    fn detect_group_chat_title() {
        let chat = serde_json::json!({"id": -100123, "type": "supergroup", "title": "BTS Ops"});
        let d = chat_to_detected(&chat).unwrap();
        assert_eq!(d.id, -100123);
        assert_eq!(d.name, "BTS Ops");
    }

    #[test]
    fn get_updates_dedupes_by_chat() {
        // Validate the dedup/parse logic against a representative getUpdates result without network.
        let json = serde_json::json!({
            "ok": true,
            "result": [
                {"update_id": 1, "message": {"chat": {"id": 7, "type": "private", "first_name": "Ed"}}},
                {"update_id": 2, "message": {"chat": {"id": 7, "type": "private", "first_name": "Ed"}}},
                {"update_id": 3, "message": {"chat": {"id": -9, "type": "group", "title": "Net"}}}
            ]
        });
        let result = ok_result(&json).unwrap();
        let seen = detect_destinations(result.as_array().unwrap());
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].id, 7);
        assert_eq!(seen[1].id, -9);
    }
}
