/// Format a byte slice as uppercase comma-separated hex for diagnostics.
pub fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(", ")
}

/// Parse a dashboard/operator-entered hex string. Accepts whitespace and common separators,
/// with or without `0x` prefixes.
pub fn parse_hex_payload(raw: &str) -> Result<Vec<u8>, String> {
    let normalized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_whitespace() || matches!(c, ',' | ';' | ':' | '-') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut bytes = Vec::new();
    for token in normalized.split_whitespace() {
        let hex = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")).unwrap_or(token);
        if hex.is_empty() {
            return Err(format!("hex token '{}' has no digits", token));
        }
        if hex.len() % 2 != 0 {
            return Err(format!("hex token '{}' has an odd number of digits", token));
        }
        for pos in (0..hex.len()).step_by(2) {
            let pair = &hex[pos..pos + 2];
            let byte = u8::from_str_radix(pair, 16).map_err(|_| format!("invalid hex byte '{}'", pair))?;
            bytes.push(byte);
        }
    }
    Ok(bytes)
}

/// TPG2200 text payload bytes. Characters outside ISO-8859-1 are represented as '?' because
/// the tested Motorola payload is byte-oriented.
pub fn iso_8859_1_or_ascii_bytes(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if code <= 0xFF { code as u8 } else { b'?' }
        })
        .collect()
}

pub fn tpg2200_callout_id_byte(callout_id: u16) -> u8 {
    callout_id.min(255) as u8
}

pub fn tpg2200_priority_byte(priority: u8) -> u8 {
    priority.min(15)
}

pub fn tpg2200_incident_byte(incident: u16) -> u8 {
    let incident = incident.clamp(1, 256);
    let zero_based = incident - 1;
    let major = ((zero_based + 1) & 0x0F) as u8;
    let minor = (((zero_based / 16) + 1) & 0x0F) as u8;
    (major << 4) | minor
}

pub fn tpg2200_incident_from_byte(selector: u8) -> u16 {
    let major = (selector >> 4) as u16;
    let minor = (selector & 0x0F) as u16;
    let slot = if major == 0 { 16 } else { major };
    let block = if minor == 0 { 16 } else { minor };
    ((block - 1) * 16) + slot
}

pub fn default_tpg2200_ric() -> u32 {
    0x0009_0D10
}

pub fn tpg2200_ric_bytes(ric: u32) -> [u8; 4] {
    ric.to_be_bytes()
}

pub fn build_tpg2200_callout_payload(tpg_ric: u32, callout_id: u16, priority: u8, message: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(11 + message.len());
    payload.push(0xC3);
    payload.extend_from_slice(&tpg2200_ric_bytes(tpg_ric));
    payload.extend_from_slice(&[
        tpg2200_callout_id_byte(callout_id),
        0x27,
        tpg2200_priority_byte(priority),
        0x02,
        0x30,
        0x8D,
    ]);
    payload.extend_from_slice(&iso_8859_1_or_ascii_bytes(message));
    payload
}

/// Build the bare text payload expected by `ControlCommand::SendSds`. CMCE wraps this in the
/// SDS-TL header and message reference before sending it over RF.
pub fn build_sds_text_payload(text: &str) -> (u16, Vec<u8>) {
    let all_latin = text.chars().all(|c| c as u32 <= 0xFF);
    let (coding_scheme, text_bytes): (u8, Vec<u8>) = if all_latin {
        let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
        (0x01, bytes)
    } else {
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        (0x02, bytes)
    };
    let mut payload = vec![coding_scheme];
    payload.extend_from_slice(&text_bytes);
    ((payload.len() * 8) as u16, payload)
}

#[cfg(test)]
mod tests {
    use super::{
        build_sds_text_payload, build_tpg2200_callout_payload, default_tpg2200_ric, parse_hex_payload, tpg2200_callout_id_byte,
        tpg2200_incident_byte, tpg2200_incident_from_byte, tpg2200_priority_byte, tpg2200_ric_bytes,
    };

    #[test]
    fn tpg2200_callout_id_and_priority_bytes_are_direct_fields() {
        assert_eq!(tpg2200_callout_id_byte(0), 0x00);
        assert_eq!(tpg2200_callout_id_byte(1), 0x01);
        assert_eq!(tpg2200_callout_id_byte(15), 0x0F);
        assert_eq!(tpg2200_callout_id_byte(33), 0x21);
        assert_eq!(tpg2200_callout_id_byte(49), 0x31);
        assert_eq!(tpg2200_callout_id_byte(65), 0x41);
        assert_eq!(tpg2200_callout_id_byte(255), 0xFF);
        assert_eq!(tpg2200_callout_id_byte(256), 0xFF);

        let selectors = (0..=255).map(tpg2200_callout_id_byte).collect::<std::collections::HashSet<_>>();
        assert_eq!(selectors.len(), 256);

        assert_eq!(tpg2200_priority_byte(0), 0x00);
        assert_eq!(tpg2200_priority_byte(10), 0x0A);
        assert_eq!(tpg2200_priority_byte(15), 0x0F);
        assert_eq!(tpg2200_priority_byte(16), 0x0F);

        assert_eq!(default_tpg2200_ric(), 0x0009_0D10);
        assert_eq!(tpg2200_ric_bytes(0x0009_0D10), [0x00, 0x09, 0x0D, 0x10]);
    }

    #[test]
    fn tpg2200_incident_selector_preserves_known_values() {
        assert_eq!(tpg2200_incident_byte(1), 0x11);
        assert_eq!(tpg2200_incident_byte(2), 0x21);
        assert_eq!(tpg2200_incident_byte(3), 0x31);
        assert_eq!(tpg2200_incident_byte(4), 0x41);
        assert_eq!(tpg2200_incident_byte(15), 0xF1);
        assert_eq!(tpg2200_incident_byte(16), 0x01);
        assert_eq!(tpg2200_incident_byte(256), 0x00);

        let selectors = (1..=256).map(tpg2200_incident_byte).collect::<std::collections::HashSet<_>>();
        assert_eq!(selectors.len(), 256);

        assert_eq!(tpg2200_incident_from_byte(0x11), 1);
        assert_eq!(tpg2200_incident_from_byte(0x21), 2);
        assert_eq!(tpg2200_incident_from_byte(0x31), 3);
        assert_eq!(tpg2200_incident_from_byte(0x00), 256);
    }

    #[test]
    fn parse_hex_payload_accepts_common_separators_and_prefixes() {
        assert_eq!(
            parse_hex_payload("C3 00,0x09;0D:10-21").unwrap(),
            vec![0xC3, 0x00, 0x09, 0x0D, 0x10, 0x21]
        );
        assert_eq!(parse_hex_payload("C300090D").unwrap(), vec![0xC3, 0x00, 0x09, 0x0D]);
        assert!(parse_hex_payload("C3 0X").is_err());
        assert!(parse_hex_payload("C3 0").is_err());
    }

    #[test]
    fn build_tpg2200_callout_payload_matches_known_alarm_shape() {
        assert_eq!(
            build_tpg2200_callout_payload(default_tpg2200_ric(), 0x11, 0x0F, "ALARM"),
            vec![
                0xC3, 0x00, 0x09, 0x0D, 0x10, 0x11, 0x27, 0x0F, 0x02, 0x30, 0x8D, 0x41, 0x4C, 0x41, 0x52, 0x4D
            ]
        );
        assert_eq!(
            build_tpg2200_callout_payload(default_tpg2200_ric(), 0x21, 0x0F, "ALARM"),
            vec![
                0xC3, 0x00, 0x09, 0x0D, 0x10, 0x21, 0x27, 0x0F, 0x02, 0x30, 0x8D, 0x41, 0x4C, 0x41, 0x52, 0x4D
            ]
        );
        assert_eq!(
            &build_tpg2200_callout_payload(0x000A_BCDE, 0x21, 0x03, "ALARM")[1..8],
            &[0x00, 0x0A, 0xBC, 0xDE, 0x21, 0x27, 0x03]
        );
    }

    #[test]
    fn sds_text_payload_selects_latin_or_utf16() {
        assert_eq!(build_sds_text_payload("abc"), (32, vec![0x01, b'a', b'b', b'c']));
        assert_eq!(build_sds_text_payload("日"), (24, vec![0x02, 0x65, 0xE5]));
    }
}
