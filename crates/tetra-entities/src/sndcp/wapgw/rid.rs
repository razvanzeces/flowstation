//! "Consulta en radioid.net" (`/rid`): a callsign or a DMR ID looked up in the radioid.net
//! database, users first and repeaters when no user has it.
//!
//! Only the check of what was typed, the API URLs and the reading and rendering of the replies
//! live here; the download is the pool's ([`super::fetch`]), with its network policy, its time
//! limit and the per-radio request limit. Nothing is cached.

use serde_json::Value;

use super::home::percent_encode;
use crate::sndcp::wap_status::escape_xhtml_text;

const API: &str = "https://radioid.net/api/dmr";
/// Longest input: a callsign or a DMR ID.
pub const MAX_INPUT: usize = 10;
/// Rows of a list of IDs shown at most.
pub const MAX_ROWS: usize = 8;

/// What was asked: a DMR ID (digits only) or a callsign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RidQuery {
    Id(u32),
    Callsign(String),
}

impl RidQuery {
    /// As typed in a link back to `/rid`.
    pub fn as_text(&self) -> String {
        match self {
            RidQuery::Id(id) => id.to_string(),
            RidQuery::Callsign(callsign) => callsign.clone(),
        }
    }

    /// "Not in the database" for this query, in Spanish.
    pub fn not_found(&self) -> String {
        match self {
            RidQuery::Id(id) => format!("radioid.net no tiene el ID {id}."),
            RidQuery::Callsign(callsign) => format!("radioid.net no tiene el indicativo {callsign}."),
        }
    }
}

/// The value of the form: digits only are a DMR ID, anything else a callsign. Of "EA5GVK/P" or
/// "EA8/EA5GVK" only the longest part counts. Empty input and anything but A-Z, a-z, 0-9 and `/`
/// (`%` and `*` included) are refused: the API takes `%` as a wildcard and an empty callsign as
/// "everything", with replies of 70 KB.
pub fn parse_input(input: &str) -> Result<RidQuery, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Escribe un indicativo (EA5GVK) o un ID DMR (2145007).".to_string());
    }
    let invalid = || {
        let shown: String = input.chars().take(MAX_INPUT + 2).collect();
        format!("«{shown}» no es un indicativo ni un ID válido.")
    };
    if !input.chars().all(|c| c.is_ascii_alphanumeric() || c == '/') {
        return Err(invalid());
    }
    // "EA5GVK/P", "EA8/EA5GVK": the callsign is the longest part.
    let asked = input.split('/').max_by_key(|part| part.len()).unwrap_or(input);
    if asked.is_empty() || asked.len() > MAX_INPUT {
        return Err(invalid());
    }
    if asked.chars().all(|c| c.is_ascii_digit()) {
        return asked.parse().map(RidQuery::Id).map_err(|_| invalid());
    }
    Ok(RidQuery::Callsign(asked.to_ascii_uppercase()))
}

/// API URL of the user database for a query.
pub fn user_url(query: &RidQuery) -> String {
    api_url("user", query)
}

/// API URL of the repeater database for a query.
pub fn repeater_url(query: &RidQuery) -> String {
    api_url("repeater", query)
}

fn api_url(database: &str, query: &RidQuery) -> String {
    match query {
        RidQuery::Id(id) => format!("{API}/{database}/?id={id}"),
        RidQuery::Callsign(callsign) => format!("{API}/{database}/?callsign={}", percent_encode(callsign)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RidUser {
    pub id: u32,
    pub callsign: String,
    pub name: String,
    /// City, region (country).
    pub place: String,
    /// "Mon, 05 Oct 2026 15:26:19 GMT" as radioid.net gives it.
    pub last_heard: String,
    pub last_tg: String,
    pub last_master: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RidRepeater {
    pub id: Option<u32>,
    pub callsign: String,
    pub frequency: String,
    pub offset: String,
    pub color_code: String,
    pub network: String,
    pub place: String,
    pub trustee: String,
}

/// What radioid.net knows of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RidFound {
    Users(Vec<RidUser>),
    Repeaters(Vec<RidRepeater>),
}

/// The `results` of a reply; an error when it is not radioid.net's JSON.
fn results(body: &str) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    value
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "no results".to_string())
}

/// A field as text (radioid.net sends some numbers as strings and some strings as numbers).
fn text(entry: &Value, key: &str) -> String {
    match entry.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn number(entry: &Value, key: &str) -> Option<u32> {
    match entry.get(key) {
        Some(Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// "City, region (country)", leaving out what is empty.
fn place(entry: &Value) -> String {
    let local: Vec<String> = ["city", "state"]
        .iter()
        .map(|key| text(entry, key))
        .filter(|s| !s.is_empty())
        .collect();
    let country = text(entry, "country");
    match (local.is_empty(), country.is_empty()) {
        (true, _) => country,
        (false, true) => local.join(", "),
        (false, false) => format!("{} ({country})", local.join(", ")),
    }
}

/// The users of a reply of `user/`.
pub fn parse_users(body: &str) -> Result<Vec<RidUser>, String> {
    Ok(results(body)?
        .iter()
        .filter_map(|entry| {
            let id = number(entry, "id").or_else(|| number(entry, "radio_id"))?;
            let name = format!("{} {}", text(entry, "fname"), text(entry, "surname")).trim().to_string();
            Some(RidUser {
                id,
                callsign: text(entry, "callsign"),
                name: if name.is_empty() { text(entry, "name") } else { name },
                place: place(entry),
                last_heard: text(entry, "lastheard"),
                last_tg: text(entry, "lasttg"),
                last_master: text(entry, "lastmaster"),
            })
        })
        .collect())
}

/// The repeaters of a reply of `repeater/` (their DMR ID is the `locator` field).
pub fn parse_repeaters(body: &str) -> Result<Vec<RidRepeater>, String> {
    Ok(results(body)?
        .iter()
        .map(|entry| RidRepeater {
            id: number(entry, "locator").or_else(|| number(entry, "id")),
            callsign: text(entry, "callsign"),
            frequency: text(entry, "frequency"),
            offset: text(entry, "offset"),
            color_code: text(entry, "color_code"),
            network: text(entry, "ipsc_network"),
            place: place(entry),
            trustee: entry
                .get("trustee")
                .and_then(Value::as_array)
                .map(|t| t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
        })
        .collect())
}

/// "Mon, 05 Oct 2026 15:26:19 GMT" as (day, month, year, "hh:mm").
fn heard_parts(heard: &str) -> Option<(String, u32, String, String)> {
    let parts: Vec<&str> = heard.split_whitespace().collect();
    let [_, day, month, year, time, ..] = parts.as_slice() else {
        return None;
    };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(month))? as u32 + 1;
    let hhmm = time.get(..5).filter(|t| t.as_bytes().get(2) == Some(&b':'))?;
    Some((format!("{:0>2}", day), month, year.to_string(), hhmm.to_string()))
}

/// "05/10/2026 15:26 UTC".
fn heard_long(heard: &str) -> Option<String> {
    heard_parts(heard).map(|(d, m, y, t)| format!("{d}/{m:02}/{y} {t} UTC"))
}

/// "05/10 15:26".
fn heard_short(heard: &str) -> Option<String> {
    heard_parts(heard).map(|(d, m, _, t)| format!("{d}/{m:02} {t}"))
}

/// "438.52500" -> "438.525".
fn trim_decimals(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

fn link(id: u32) -> String {
    format!("<a href=\"/rid?q={id}\">{id}</a>")
}

fn paragraph(lines: &[String]) -> String {
    let lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        String::new()
    } else {
        format!("<p>{}</p>", lines.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("<br/>"))
    }
}

/// A text line, escaped.
fn esc(s: &str) -> String {
    escape_xhtml_text(s)
}

fn user_detail(u: &RidUser) -> String {
    let heard = heard_long(&u.last_heard).map(|h| format!("Oído {h}")).unwrap_or_default();
    let tg = match (u.last_tg.is_empty(), u.last_master.is_empty()) {
        (false, false) => format!("TG {} en {}", u.last_tg, u.last_master),
        (false, true) => format!("TG {}", u.last_tg),
        _ => String::new(),
    };
    paragraph(&[
        format!("<b>{}</b> {}", esc(&u.callsign), u.id),
        esc(&u.name),
        esc(&u.place),
        esc(&heard),
        esc(&tg),
    ])
}

fn more(total: usize, shown: usize) -> String {
    if total > shown {
        format!("… y {} más", total - shown)
    } else {
        String::new()
    }
}

fn user_list(users: &[RidUser], asked: &RidQuery, rows: usize) -> String {
    let first = &users[0];
    let same = users
        .iter()
        .all(|u| u.callsign == first.callsign && u.name == first.name && u.place == first.place);
    let head = if same {
        paragraph(&[
            format!("<b>{}</b>: {} ID", esc(&first.callsign), users.len()),
            esc(&first.name),
            esc(&first.place),
        ])
    } else {
        paragraph(&[format!("<b>{}</b>: {} ID", esc(&asked.as_text()), users.len())])
    };
    let mut lines: Vec<String> = users
        .iter()
        .take(rows)
        .map(|u| {
            let heard = heard_short(&u.last_heard).unwrap_or_default();
            let who = if same {
                String::new()
            } else {
                format!("{} {}", u.callsign, u.name)
            };
            [link(u.id), esc(who.trim()), esc(&heard)]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    lines.push(more(users.len(), rows.min(users.len())));
    format!("{head}{}", paragraph(&lines))
}

fn repeater_detail(r: &RidRepeater) -> String {
    let id = r.id.map(|id| format!(" {id}")).unwrap_or_default();
    let mut radio = Vec::new();
    if !r.frequency.is_empty() {
        radio.push(format!("{} MHz", trim_decimals(&r.frequency)));
    }
    if !r.offset.is_empty() {
        radio.push(trim_decimals(&r.offset).to_string());
    }
    if !r.color_code.is_empty() {
        radio.push(format!("CC{}", r.color_code));
    }
    if !r.network.is_empty() {
        radio.push(r.network.clone());
    }
    let trustee = if r.trustee.is_empty() {
        String::new()
    } else {
        format!("Responsable {}", r.trustee)
    };
    paragraph(&[
        format!("<b>{}</b>{id} (repetidor)", esc(&r.callsign)),
        esc(&radio.join(" ")),
        esc(&r.place),
        esc(&trustee),
    ])
}

fn repeater_list(repeaters: &[RidRepeater], asked: &RidQuery, rows: usize) -> String {
    let head = paragraph(&[format!("<b>{}</b>: {} repetidores", esc(&asked.as_text()), repeaters.len())]);
    let mut lines: Vec<String> = repeaters
        .iter()
        .take(rows)
        .map(|r| {
            let id = r.id.map(link).unwrap_or_default();
            let what = format!("{} {} MHz {}", r.callsign, trim_decimals(&r.frequency), r.place);
            format!("{id} {}", esc(what.trim())).trim().to_string()
        })
        .collect();
    lines.push(more(repeaters.len(), rows.min(repeaters.len())));
    format!("{head}{}", paragraph(&lines))
}

/// The answer as XHTML-MP markup: the detail of a single entry, or a list with a link to each
/// ID (at most `rows` of them).
pub fn render(found: &RidFound, asked: &RidQuery, rows: usize) -> String {
    let rows = rows.max(1);
    match found {
        RidFound::Users(users) if users.len() == 1 => user_detail(&users[0]),
        RidFound::Users(users) if !users.is_empty() => user_list(users, asked, rows),
        RidFound::Repeaters(repeaters) if repeaters.len() == 1 => repeater_detail(&repeaters[0]),
        RidFound::Repeaters(repeaters) if !repeaters.is_empty() => repeater_list(repeaters, asked, rows),
        _ => paragraph(&[esc(&asked.not_found())]),
    }
}

#[cfg(test)]
pub(crate) mod samples {
    //! Replies of radioid.net captured on 2026-10-05.

    pub const USERS_EA5GVK: &str = r#"{"count":4,"page":1,"pages":1,"per_page":200,"results":[{"account_unvalidated":0,"account_validation_status":"validated","callsign":"EA5GVK","city":"Alguazas","country":"Spain","fname":"Joaquin","has_valid_callsign":"1","has_valid_email":1,"id":2145007,"lastheard":"Mon, 05 Oct 2026 15:26:19 GMT","lastmaster":"OBP-PORTUG","lastsource":"OBP-PORTUG","lasttg":"214","name":"Joaquin","radio_id":2145007,"state":"Murcia","surname":""},{"account_unvalidated":0,"account_validation_status":"validated","callsign":"EA5GVK","city":"Alguazas","country":"Spain","fname":"Joaquin","has_valid_callsign":"1","has_valid_email":1,"id":2145016,"lastheard":"Mon, 05 Oct 2026 14:35:32 GMT","lastmaster":"OBP-PORTUG","lastsource":"OBP-PORTUG","lasttg":"214","name":"Joaquin","radio_id":2145016,"state":"Murcia","surname":""},{"account_unvalidated":0,"account_validation_status":"validated","callsign":"EA5GVK","city":"Alguazas","country":"Spain","fname":"Joaquin","has_valid_callsign":"1","has_valid_email":1,"id":2150171,"lastheard":"Sat, 03 Oct 2026 15:28:31 GMT","lastmaster":"OBP-PORTUG","lastsource":"OBP-PORTUG","lasttg":"2143","name":"Joaquin","radio_id":2150171,"state":"Murcia","surname":""},{"account_unvalidated":0,"account_validation_status":"validated","callsign":"EA5GVK","city":"Alguazas","country":"Spain","fname":"Joaquin","has_valid_callsign":"1","has_valid_email":1,"id":2150295,"lastheard":"Mon, 28 Sep 2026 14:19:37 GMT","lastmaster":"OBP-PORTUG","lastsource":"OBP-PORTUG","lasttg":"2143","name":"Joaquin","radio_id":2150295,"state":"Murcia","surname":""}]}"#;

    pub const USER_2145007: &str = r#"{"count":1,"page":1,"pages":1,"per_page":200,"results":[{"account_unvalidated":0,"account_validation_status":"validated","callsign":"EA5GVK","city":"Alguazas","country":"Spain","fname":"Joaquin","has_valid_callsign":"1","has_valid_email":1,"id":2145007,"lastheard":"Mon, 05 Oct 2026 15:26:19 GMT","lastmaster":"OBP-PORTUG","lastsource":"OBP-PORTUG","lasttg":"214","name":"Joaquin","radio_id":2145007,"state":"Murcia","surname":""}]}"#;

    pub const NONE: &str = r#"{"count":0,"page":1,"pages":0,"per_page":200,"results":[]}"#;

    pub const REPEATER_ED3ZAQ: &str = r#"{"count":1,"page":1,"pages":1,"per_page":200,"results":[{"callsign":"ED3ZAQ","city":"Castellar del Valles","color_code":1,"country":"Spain","coverage":"Master","details":"21460","frequency":"438.52500","identity_id":727,"ipsc_network":"BM","last_master":null,"locator":214301,"manufacturer":null,"offset":"-7.600","state":"Barcelona","status":"on-air","talkgroups":[{"description":"Los jóvenes","discovery":0,"talkgroup":21469,"timeslot":1},{"description":"Regional EA3","discovery":0,"talkgroup":2143,"timeslot":2},{"description":"BM-CAT","discovery":0,"talkgroup":21460,"timeslot":2}],"trustee":["EA3ABN"]}]}"#;
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;

    #[test]
    fn input_digits_are_an_id_anything_else_a_callsign() {
        assert_eq!(parse_input("2145007"), Ok(RidQuery::Id(2_145_007)));
        assert_eq!(parse_input(" ea5gvk "), Ok(RidQuery::Callsign("EA5GVK".to_string())));
        // Callsigns starting with a digit always carry a letter.
        assert_eq!(parse_input("2E0ABC"), Ok(RidQuery::Callsign("2E0ABC".to_string())));
        // A portable or mobile suffix, or a prefix of another country, is left out.
        for with_slash in ["EA5GVK/P", "ea5gvk/m ", "EA8/EA5GVK", "EA5GVK/"] {
            assert_eq!(
                parse_input(with_slash),
                Ok(RidQuery::Callsign("EA5GVK".to_string())),
                "{with_slash}"
            );
        }
    }

    #[test]
    fn input_wildcards_empty_and_symbols_refused() {
        assert!(parse_input("").unwrap_err().starts_with("Escribe un indicativo"));
        assert!(parse_input("   ").is_err());
        for bad in [
            "EA5%",
            "%",
            "*",
            "EA5*",
            "EA5GVK/P%",
            "/",
            "EA 5GVK",
            "ñ",
            "a_b",
            "EA5GVK1234X",
            "99999999999",
        ] {
            let err = parse_input(bad).unwrap_err();
            assert!(err.contains("no es un indicativo ni un ID válido"), "{bad}: {err}");
        }
    }

    #[test]
    fn urls_are_encoded() {
        assert_eq!(
            user_url(&RidQuery::Callsign("EA5GVK".to_string())),
            "https://radioid.net/api/dmr/user/?callsign=EA5GVK"
        );
        assert_eq!(
            repeater_url(&RidQuery::Id(214_301)),
            "https://radioid.net/api/dmr/repeater/?id=214301"
        );
        // Never reached through parse_input, still encoded.
        assert_eq!(
            user_url(&RidQuery::Callsign("A%B C".to_string())),
            "https://radioid.net/api/dmr/user/?callsign=A%25B%20C"
        );
    }

    #[test]
    fn one_user_is_a_detail() {
        let users = parse_users(USER_2145007).unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].place, "Alguazas, Murcia (Spain)");
        let html = render(&RidFound::Users(users), &RidQuery::Id(2_145_007), MAX_ROWS);
        assert_eq!(
            html,
            "<p><b>EA5GVK</b> 2145007<br/>Joaquin<br/>Alguazas, Murcia (Spain)<br/>Oído 05/10/2026 15:26 UTC<br/>TG 214 en OBP-PORTUG</p>"
        );
    }

    #[test]
    fn several_users_are_a_list_with_a_link_per_id() {
        let users = parse_users(USERS_EA5GVK).unwrap();
        assert_eq!(
            users.iter().map(|u| u.id).collect::<Vec<_>>(),
            [2_145_007, 2_145_016, 2_150_171, 2_150_295]
        );
        let asked = RidQuery::Callsign("EA5GVK".to_string());
        let html = render(&RidFound::Users(users.clone()), &asked, MAX_ROWS);
        assert!(
            html.starts_with("<p><b>EA5GVK</b>: 4 ID<br/>Joaquin<br/>Alguazas, Murcia (Spain)</p>"),
            "{html}"
        );
        assert!(html.contains("<a href=\"/rid?q=2145007\">2145007</a> 05/10 15:26<br/>"), "{html}");
        assert!(html.contains("<a href=\"/rid?q=2150295\">2150295</a> 28/09 14:19"), "{html}");
        assert!(!html.contains("más"));
        let two = render(&RidFound::Users(users), &asked, 2);
        assert!(
            two.contains("2145016") && !two.contains("2150171") && two.contains("… y 2 más"),
            "{two}"
        );
    }

    #[test]
    fn repeater_detail() {
        let repeaters = parse_repeaters(REPEATER_ED3ZAQ).unwrap();
        assert_eq!(repeaters[0].id, Some(214_301));
        let html = render(&RidFound::Repeaters(repeaters), &RidQuery::Id(214_301), MAX_ROWS);
        assert_eq!(
            html,
            "<p><b>ED3ZAQ</b> 214301 (repetidor)<br/>438.525 MHz -7.6 CC1 BM<br/>Castellar del Valles, Barcelona (Spain)<br/>Responsable EA3ABN</p>"
        );
    }

    #[test]
    fn nothing_found_and_bad_replies() {
        assert!(parse_users(NONE).unwrap().is_empty());
        assert!(parse_repeaters(NONE).unwrap().is_empty());
        assert!(parse_users("<html>busy</html>").is_err());
        assert!(parse_users("{\"error\":1}").is_err());
        let html = render(&RidFound::Users(Vec::new()), &RidQuery::Callsign("XX0XXX".to_string()), MAX_ROWS);
        assert_eq!(html, "<p>radioid.net no tiene el indicativo XX0XXX.</p>");
        assert_eq!(RidQuery::Id(5).not_found(), "radioid.net no tiene el ID 5.");
    }

    #[test]
    fn last_heard_dates() {
        assert_eq!(heard_long("Mon, 05 Oct 2026 15:26:19 GMT").as_deref(), Some("05/10/2026 15:26 UTC"));
        assert_eq!(heard_short("Sat, 3 Oct 2026 09:05:00 GMT").as_deref(), Some("03/10 09:05"));
        assert_eq!(heard_long(""), None);
        assert_eq!(heard_long("yesterday"), None);
    }
}
