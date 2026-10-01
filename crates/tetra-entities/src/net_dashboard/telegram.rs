//! Dashboard-side persistence and helpers for Telegram alerts.
//!
//! Mirrors `wx_service.rs`: a surgical TOML writer that regenerates only the `[telegram_alerts]`
//! section (preserving the rest of the file and creating a backup), plus a helper to mask the
//! bot token before it is returned to the browser.

use tetra_config::bluestation::{CfgTelegram, TelegramRecipient, TelegramRuntimeOverride};

/// Parse and validate both destination formats before applying settings or sending tests.
/// Omitted fields keep their saved values; an explicit empty array clears them.
pub fn parse_destinations(json: &serde_json::Value, current: &CfgTelegram) -> Result<(Vec<i64>, Vec<TelegramRecipient>), String> {
    let chat_ids = match json.get("chat_ids") {
        Some(value) => {
            serde_json::from_value::<Vec<i64>>(value.clone()).map_err(|_| "chat_ids must be an array of integers.".to_string())?
        }
        None => current.chat_ids.clone(),
    };
    let recipients = match json.get("recipients") {
        Some(value) => serde_json::from_value::<Vec<TelegramRecipient>>(value.clone()).map_err(|e| format!("Invalid recipients: {e}"))?,
        None => current.recipients.clone(),
    };
    for recipient in chat_ids
        .iter()
        .map(|&chat_id| TelegramRecipient {
            chat_id,
            message_thread_id: None,
        })
        .chain(recipients.iter().cloned())
    {
        recipient.validate()?;
    }
    Ok((chat_ids, recipients))
}

/// Mask a bot token for display: keep the numeric bot id and the last few chars, hide the rest.
/// Returns an empty string for an empty token. e.g. "123456:ABCdef...WXYZ" → "123456:AB…WXYZ".
pub fn mask_token(token: &str) -> String {
    let token = token.trim();
    if token.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 10 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// JSON-escape a string for manual JSON assembly. Unlike a bare quote/backslash replace, this
/// also escapes control characters (newline, tab, etc.) — Telegram chat titles and reqwest error
/// strings are external data that can contain them, and a raw control char would make the
/// hand-assembled JSON response unparseable in the browser.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Rewrite (or insert) the `[telegram_alerts]` section in the TOML file, preserving everything
/// else. The whole section is regenerated from the override values; an existing block (from its
/// header until the next section header or EOF) is replaced. A `.telegram.bak` backup is made.
/// Mirrors [`crate::net_dashboard::wx_service::write_wx_to_toml`].
pub fn write_telegram_to_toml(config_path: &str, ov: &TelegramRuntimeOverride) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;

    let token_escaped = ov.bot_token.replace('\\', "\\\\").replace('"', "\\\"");
    let chat_ids = ov.chat_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
    // Inline tables keep destinations inside this section, so subsequent saves replace them.
    let recipients = ov
        .recipients
        .iter()
        .map(|r| match r.message_thread_id {
            Some(id) => format!("{{ chat_id = {}, message_thread_id = {} }}", r.chat_id, id),
            None => format!("{{ chat_id = {} }}", r.chat_id),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let section = format!(
        "[telegram_alerts]\n\
         enabled = {}\n\
         bot_token = \"{}\"\n\
         chat_ids = [{}]\n\
         recipients = [{}]\n\
         alert_connect = {}\n\
         alert_disconnect = {}\n\
         alert_t351 = {}\n\
         alert_lip = {}\n\
         alert_backhaul = {}\n\
         alert_critical_logs = {}",
        ov.enabled,
        token_escaped,
        chat_ids,
        recipients,
        ov.alert_connect,
        ov.alert_disconnect,
        ov.alert_t351,
        ov.alert_lip,
        ov.alert_backhaul,
        ov.alert_critical_logs,
    );

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 12);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("[telegram_alerts]") {
            // Replace this whole section: emit the regenerated block, then skip the old section's
            // body up to (but not including) the next section header / EOF.
            out.push(section.clone());
            replaced = true;
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim_start();
                if t.starts_with('[') && t.contains(']') && !t.starts_with("[telegram_alerts.") && !t.starts_with("[[telegram_alerts.") {
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }

    if !replaced {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push(section);
    }

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }

    let backup = format!("{config_path}.telegram.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov() -> TelegramRuntimeOverride {
        TelegramRuntimeOverride {
            enabled: true,
            bot_token: "123456:ABC-DEF".to_string(),
            chat_ids: vec![987654321, -1001234567890],
            recipients: vec![TelegramRecipient {
                chat_id: -1001234567890,
                message_thread_id: Some(42),
            }],
            alert_connect: true,
            alert_disconnect: false,
            alert_t351: true,
            alert_lip: true,
            alert_backhaul: false,
            alert_critical_logs: true,
        }
    }

    #[test]
    fn mask_short_and_long() {
        assert_eq!(mask_token(""), "");
        assert_eq!(mask_token("short"), "•••••");
        assert_eq!(mask_token("123456:ABCdefGHIjklMNOpqrsWXYZ"), "123456:A…WXYZ");
    }

    #[test]
    fn json_escape_handles_control_chars() {
        let out = json_escape("line1\nline2\t\"q\"\\x");
        assert_eq!(out, "line1\\nline2\\t\\\"q\\\"\\\\x");
        // A raw control byte becomes a \u escape, never a literal that breaks JSON.
        assert_eq!(json_escape("\u{1}"), "\\u0001");
        // The escaped output parses back as the original string.
        let wrapped = format!("\"{}\"", json_escape("a\nb\"c"));
        let v: serde_json::Value = serde_json::from_str(&wrapped).unwrap();
        assert_eq!(v.as_str().unwrap(), "a\nb\"c");
    }

    #[test]
    fn write_toml_replace_section() {
        let cfg = "[cell]\nfoo = 1\n\n[telegram_alerts]\nenabled = false\nbot_token = \"old\"\n\n[security]\nbar = 2\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_tg_test_replace.toml");
        std::fs::write(&path, cfg).unwrap();
        write_telegram_to_toml(path.to_str().unwrap(), &ov()).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("enabled = true"));
        assert!(out.contains("bot_token = \"123456:ABC-DEF\""));
        assert!(out.contains("chat_ids = [987654321, -1001234567890]"));
        assert!(out.contains("alert_disconnect = false"));
        // Other sections preserved, old value gone.
        assert!(out.contains("[cell]"));
        assert!(out.contains("[security]"));
        assert!(out.contains("bar = 2"));
        assert!(!out.contains("bot_token = \"old\""));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_toml_append_when_missing() {
        let cfg = "[cell]\nfoo = 1\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_tg_test_append.toml");
        std::fs::write(&path, cfg).unwrap();
        write_telegram_to_toml(path.to_str().unwrap(), &ov()).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[telegram_alerts]"));
        assert!(out.contains("[cell]"));
        let _ = std::fs::remove_file(&path);
    }

    /// The written section must round-trip through the real config parser.
    #[test]
    fn written_section_parses_back() {
        let cfg = "config_version = \"0.6\"\nstack_mode = \"Bs\"\n\n[phy_io]\nbackend = \"None\"\n\n[net_info]\nmcc = 901\nmnc = 9999\n\n[cell_info]\nmain_carrier = 1584\nfreq_band = 4\nfreq_offset = 0\nduplex_spacing = 4\nreverse_operation = false\nlocation_area = 1\n";
        let dir = std::env::temp_dir();
        let path = dir.join("fs_tg_test_roundtrip.toml");
        std::fs::write(&path, cfg).unwrap();
        write_telegram_to_toml(path.to_str().unwrap(), &ov()).unwrap();
        let parsed = tetra_config::bluestation::parsing::from_file(path.to_str().unwrap()).expect("written telegram section must parse");
        let tg = parsed.telegram.expect("telegram present");
        assert!(tg.enabled);
        assert_eq!(tg.chat_ids, vec![987654321, -1001234567890]);
        assert_eq!(tg.recipients, ov().recipients);
        assert!(!tg.alert_disconnect);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn destination_edits_preserve_omitted_fields_and_reject_invalid_values() {
        let current = CfgTelegram {
            chat_ids: ov().chat_ids,
            recipients: ov().recipients,
            ..Default::default()
        };
        let (chats, recipients) = parse_destinations(&serde_json::json!({}), &current).unwrap();
        assert_eq!(chats, current.chat_ids);
        assert_eq!(recipients, current.recipients);
        let cleared = parse_destinations(&serde_json::json!({"chat_ids": [], "recipients": []}), &current).unwrap();
        assert!(cleared.0.is_empty() && cleared.1.is_empty());
        for invalid in [
            serde_json::json!({"chat_ids": ["oops"]}),
            serde_json::json!({"chat_ids": [0]}),
            serde_json::json!({"recipients": [{"chat_id": -100123, "message_thread_id": 0}]}),
            serde_json::json!({"recipients": [{"chat_id": -100123, "message_thread_id": 1.5}]}),
            serde_json::json!({"recipients": [{"chat_id": -100123, "topic_id": 42}]}),
        ] {
            assert!(parse_destinations(&invalid, &current).is_err());
        }
    }

    #[test]
    fn saving_replaces_nested_recipient_tables_and_can_clear_topics() {
        let path = std::env::temp_dir().join("fs_tg_test_nested_topics.toml");
        let cfg = "[telegram_alerts]\nenabled = false\n[[telegram_alerts.recipients]]\nchat_id = -100123\nmessage_thread_id = 99\n[security]\nbar = 2\n";
        std::fs::write(&path, cfg).unwrap();
        write_telegram_to_toml(path.to_str().unwrap(), &ov()).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("message_thread_id = 42"));
        assert!(!out.contains("message_thread_id = 99"));
        assert!(out.contains("[security]\nbar = 2"));
        let mut cleared = ov();
        cleared.recipients.clear();
        write_telegram_to_toml(path.to_str().unwrap(), &cleared).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("recipients = []"));
        assert!(!out.contains("message_thread_id"));
        let _ = std::fs::remove_file(&path);
    }
}
