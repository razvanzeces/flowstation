//! Country flag for an amateur-radio callsign ("indicativ").
//!
//! RadioID resolves an ISSI to a callsign such as `YO6RZV`; the leading characters are an ITU
//! international call-sign-series prefix that identifies the country (`YO` → Romania, `HA` →
//! Hungary, `DL` → Germany …). We map that prefix to an ISO 3166-1 alpha-2 code and turn it into a
//! flag emoji built from Unicode *regional indicator* symbols — `🇷🇴` is just `R`+`O`. That renders
//! natively wherever the dashboard text is shown (and in Telegram/WhatsApp, if reused there) with
//! no image, no re-rendering, no pixelation.
//!
//! The mapping uses the ITU "Table of International Call Sign Series", which allocates blocks by the
//! first two characters of a call sign. We store those allocations as inclusive ranges over the
//! two-character prefix and find the one containing a given call sign — e.g. Romania is `YO`..=`YR`.
//! A few ITU blocks are split below two-character granularity (the third character decides, e.g.
//! Egypt/Sudan inside `SS`, Eswatini/Fiji inside `3D`); those rare cases resolve to the more common
//! occupant. Whole-letter allocations (`F`, `G`, `I`, `K`, `N`, `R`, `W`, `M`) use a `X0`..=`XZ`
//! range so any digit or letter in the second position matches.

/// Inclusive ranges over the two-character call-sign prefix → ISO 3166-1 alpha-2 country code.
/// Derived from the ITU Table of International Call Sign Series. Ranges are non-overlapping, so a
/// linear scan returning the first containing range is unambiguous. Comparison is plain ASCII
/// lexicographic on the two-character code (digits `0`-`9` sort before letters `A`-`Z`).
#[rustfmt::skip]
const PREFIX_RANGES: &[(&str, &str, &str)] = &[
    // ── A ──
    ("AA","AL","US"), ("AM","AO","ES"), ("AP","AS","PK"), ("AT","AW","IN"), ("AX","AX","AU"),
    ("AY","AZ","AR"), ("A2","A2","BW"), ("A3","A3","TO"), ("A4","A4","OM"), ("A5","A5","BT"),
    ("A6","A6","AE"), ("A7","A7","QA"), ("A8","A8","LR"), ("A9","A9","BH"),
    // ── B (China, with Taiwan carve-outs the ham community uses) ──
    ("B0","B9","CN"), ("BA","BL","CN"), ("BM","BO","TW"), ("BP","BT","CN"), ("BU","BX","TW"),
    ("BY","BZ","CN"),
    // ── C ──
    ("CA","CE","CL"), ("CF","CK","CA"), ("CL","CM","CU"), ("CN","CN","MA"), ("CO","CO","CU"),
    ("CP","CP","BO"), ("CQ","CU","PT"), ("CV","CX","UY"), ("CY","CZ","CA"), ("C2","C2","NR"),
    ("C3","C3","AD"), ("C4","C4","CY"), ("C5","C5","GM"), ("C6","C6","BS"), ("C8","C9","MZ"),
    // ── D ──
    ("DA","DR","DE"), ("DS","DT","KR"), ("DU","DZ","PH"), ("D2","D3","AO"), ("D4","D4","CV"),
    ("D5","D5","LR"), ("D6","D6","KM"), ("D7","D9","KR"),
    // ── E ──
    ("EA","EH","ES"), ("EI","EJ","IE"), ("EK","EK","AM"), ("EL","EL","LR"), ("EM","EO","UA"),
    ("EP","EQ","IR"), ("ER","ER","MD"), ("ES","ES","EE"), ("ET","ET","ET"), ("EU","EW","BY"),
    ("EX","EX","KG"), ("EY","EY","TJ"), ("EZ","EZ","TM"), ("E2","E2","TH"), ("E3","E3","ER"),
    ("E4","E4","PS"), ("E5","E5","CK"), ("E6","E6","NU"), ("E7","E7","BA"),
    // ── F, G (whole-letter) ──
    ("F0","FZ","FR"), ("G0","GZ","GB"),
    // ── H ──
    ("HA","HA","HU"), ("HB","HB","CH"), ("HC","HD","EC"), ("HE","HE","CH"), ("HF","HF","PL"),
    ("HG","HG","HU"), ("HH","HH","HT"), ("HI","HI","DO"), ("HJ","HK","CO"), ("HL","HL","KR"),
    ("HM","HM","KP"), ("HN","HN","IQ"), ("HO","HP","PA"), ("HQ","HR","HN"), ("HS","HS","TH"),
    ("HT","HT","NI"), ("HU","HU","SV"), ("HV","HV","VA"), ("HW","HY","FR"), ("HZ","HZ","SA"),
    ("H2","H2","CY"), ("H3","H3","PA"), ("H4","H4","SB"), ("H6","H7","NI"), ("H8","H9","PA"),
    // ── I (whole-letter) ──
    ("I0","IZ","IT"),
    // ── J ──
    ("JA","JS","JP"), ("JT","JV","MN"), ("JW","JX","NO"), ("JY","JY","JO"), ("JZ","JZ","ID"),
    ("J2","J2","DJ"), ("J3","J3","GD"), ("J4","J4","GR"), ("J5","J5","GW"), ("J6","J6","LC"),
    ("J7","J7","DM"), ("J8","J8","VC"),
    // ── K (whole-letter) ──
    ("K0","KZ","US"),
    // ── L ──
    ("LA","LN","NO"), ("LO","LW","AR"), ("LX","LX","LU"), ("LY","LY","LT"), ("LZ","LZ","BG"),
    ("L2","L9","AR"),
    // ── M, N (whole-letter) ──
    ("M0","MZ","GB"), ("N0","NZ","US"),
    // ── O ──
    ("OA","OC","PE"), ("OD","OD","LB"), ("OE","OE","AT"), ("OF","OJ","FI"), ("OK","OL","CZ"),
    ("OM","OM","SK"), ("ON","OT","BE"), ("OU","OZ","DK"),
    // ── P ──
    ("PA","PI","NL"), ("PJ","PJ","NL"), ("PK","PO","ID"), ("PP","PY","BR"), ("PZ","PZ","SR"),
    ("P2","P2","PG"), ("P3","P3","CY"), ("P4","P4","AW"), ("P5","P9","KP"),
    // ── R (whole-letter) ──
    ("R0","RZ","RU"),
    // ── S ──
    ("SA","SM","SE"), ("SN","SR","PL"), ("SS","SS","EG"), ("ST","ST","SD"), ("SU","SU","EG"),
    ("SV","SZ","GR"), ("S2","S2","BD"), ("S5","S5","SI"), ("S6","S6","SG"), ("S7","S7","SC"),
    ("S8","S8","ZA"), ("S9","S9","ST"),
    // ── T ──
    ("TA","TC","TR"), ("TD","TD","GT"), ("TE","TE","CR"), ("TF","TF","IS"), ("TG","TG","GT"),
    ("TH","TH","FR"), ("TI","TI","CR"), ("TJ","TJ","CM"), ("TK","TK","FR"), ("TL","TL","CF"),
    ("TM","TM","FR"), ("TN","TN","CG"), ("TO","TQ","FR"), ("TR","TR","GA"), ("TS","TS","TN"),
    ("TT","TT","TD"), ("TU","TU","CI"), ("TV","TX","FR"), ("TY","TY","BJ"), ("TZ","TZ","ML"),
    ("T2","T2","TV"), ("T3","T3","KI"), ("T4","T4","CU"), ("T5","T5","SO"), ("T6","T6","AF"),
    ("T7","T7","SM"), ("T8","T8","PW"),
    // ── U ──
    ("UA","UI","RU"), ("UJ","UM","UZ"), ("UN","UQ","KZ"), ("UR","UZ","UA"),
    // ── V ──
    ("VA","VG","CA"), ("VH","VN","AU"), ("VO","VO","CA"), ("VP","VQ","GB"), ("VR","VR","HK"),
    ("VS","VS","GB"), ("VT","VW","IN"), ("VX","VY","CA"), ("VZ","VZ","AU"), ("V2","V2","AG"),
    ("V3","V3","BZ"), ("V4","V4","KN"), ("V5","V5","NA"), ("V6","V6","FM"), ("V7","V7","MH"),
    ("V8","V8","BN"),
    // ── W (whole-letter) ──
    ("W0","WZ","US"),
    // ── X ──
    ("XA","XI","MX"), ("XJ","XO","CA"), ("XP","XP","GL"), ("XQ","XR","CL"), ("XS","XS","CN"),
    ("XT","XT","BF"), ("XU","XU","KH"), ("XV","XV","VN"), ("XW","XW","LA"), ("XX","XX","MO"),
    ("XY","XZ","MM"),
    // ── Y ──
    ("YA","YA","AF"), ("YB","YH","ID"), ("YI","YI","IQ"), ("YJ","YJ","VU"), ("YK","YK","SY"),
    ("YL","YL","LV"), ("YM","YM","TR"), ("YN","YN","NI"), ("YO","YR","RO"), ("YS","YS","SV"),
    ("YT","YU","RS"), ("YV","YY","VE"), ("YZ","YZ","RS"),
    // ── Z ──
    ("ZA","ZA","AL"), ("ZB","ZJ","GB"), ("ZK","ZM","NZ"), ("ZN","ZO","GB"), ("ZP","ZP","PY"),
    ("ZQ","ZQ","GB"), ("ZR","ZU","ZA"), ("ZV","ZZ","BR"), ("Z2","Z2","ZW"), ("Z3","Z3","MK"),
    ("Z6","Z6","XK"), ("Z8","Z8","SS"),
    // ── Number-leading series ──
    ("20","2Z","GB"),
    ("3A","3A","MC"), ("3B","3B","MU"), ("3C","3C","GQ"), ("3D","3D","SZ"), ("3E","3F","PA"),
    ("3G","3G","CL"), ("3H","3U","CN"), ("3V","3V","TN"), ("3W","3W","VN"), ("3X","3X","GN"),
    ("3Y","3Y","NO"), ("3Z","3Z","PL"),
    ("4A","4C","MX"), ("4D","4I","PH"), ("4J","4K","AZ"), ("4L","4L","GE"), ("4M","4M","VE"),
    ("4O","4O","ME"), ("4P","4S","LK"), ("4T","4T","PE"), ("4V","4V","HT"), ("4W","4W","TL"),
    ("4X","4X","IL"), ("4Z","4Z","IL"),
    ("5A","5A","LY"), ("5B","5B","CY"), ("5C","5G","MA"), ("5H","5I","TZ"), ("5J","5K","CO"),
    ("5L","5M","LR"), ("5N","5O","NG"), ("5P","5Q","DK"), ("5R","5S","MG"), ("5T","5T","MR"),
    ("5U","5U","NE"), ("5V","5V","TG"), ("5W","5W","WS"), ("5X","5X","UG"), ("5Y","5Z","KE"),
    ("6A","6B","EG"), ("6C","6C","SY"), ("6D","6J","MX"), ("6K","6N","KR"), ("6O","6O","SO"),
    ("6P","6S","PK"), ("6T","6U","SD"), ("6V","6W","SN"), ("6X","6X","MG"), ("6Y","6Y","JM"),
    ("6Z","6Z","LR"),
    ("7A","7I","ID"), ("7J","7N","JP"), ("7O","7O","YE"), ("7P","7P","LS"), ("7Q","7Q","MW"),
    ("7R","7R","DZ"), ("7S","7S","SE"), ("7T","7Y","DZ"), ("7Z","7Z","SA"),
    ("8A","8I","ID"), ("8J","8N","JP"), ("8O","8O","BW"), ("8P","8P","BB"), ("8Q","8Q","MV"),
    ("8R","8R","GY"), ("8S","8S","SE"), ("8T","8Y","IN"), ("8Z","8Z","SA"),
    ("9A","9A","HR"), ("9B","9D","IR"), ("9E","9F","ET"), ("9G","9G","GH"), ("9H","9H","MT"),
    ("9I","9J","ZM"), ("9K","9K","KW"), ("9L","9L","SL"), ("9M","9M","MY"), ("9N","9N","NP"),
    ("9O","9T","CD"), ("9U","9U","BI"), ("9V","9V","SG"), ("9W","9W","MY"), ("9X","9X","RW"),
    ("9Y","9Z","TT"),
];

/// Look up the ISO 3166-1 alpha-2 country code for a callsign's two-character prefix.
fn iso_country(callsign: &str) -> Option<&'static str> {
    // The country is decided by the first two characters of the call sign. RadioID returns the bare
    // call sign (no portable `/P` or `DL/` decorations), so the leading two characters are the
    // prefix.
    let up = callsign.trim().to_ascii_uppercase();
    let code: String = up.chars().take(2).collect();
    if code.len() != 2 {
        return None;
    }
    PREFIX_RANGES
        .iter()
        .find(|(start, end, _)| code.as_str() >= *start && code.as_str() <= *end)
        .map(|(_, _, iso)| *iso)
}

/// Turn an ISO 3166-1 alpha-2 code into a flag emoji built from Unicode regional indicators
/// (`"RO"` → `"🇷🇴"`). Returns `None` for anything that is not two ASCII uppercase letters.
fn iso_to_flag(iso: &str) -> Option<String> {
    if iso.len() != 2 {
        return None;
    }
    let mut out = String::new();
    for c in iso.chars() {
        if !c.is_ascii_uppercase() {
            return None;
        }
        // Regional Indicator Symbol Letter A is U+1F1E6.
        out.push(char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32))?);
    }
    Some(out)
}

/// Flag emoji for a callsign, e.g. `"YO6RZV"` → `Some("🇷🇴")`. Returns `None` when the prefix is not
/// allocated or maps to a non-country (so callers can simply show no flag).
pub fn callsign_flag(callsign: &str) -> Option<String> {
    iso_to_flag(iso_country(callsign)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_prefixes_map_to_expected_country() {
        assert_eq!(iso_country("YO6RZV"), Some("RO"));
        assert_eq!(iso_country("yo6rzv"), Some("RO")); // case-insensitive
        assert_eq!(iso_country("HA5XX"), Some("HU"));
        assert_eq!(iso_country("DL1ABC"), Some("DE"));
        assert_eq!(iso_country("G3XYZ"), Some("GB")); // whole-letter, digit second char
        assert_eq!(iso_country("9A1AA"), Some("HR")); // number-leading
        assert_eq!(iso_country("4X4ABC"), Some("IL"));
        assert_eq!(iso_country("K1ABC"), Some("US"));
        assert_eq!(iso_country("BV2AA"), Some("TW")); // Taiwan carve-out inside B
        assert_eq!(iso_country("BG2AA"), Some("CN"));
    }

    #[test]
    fn flag_is_regional_indicator_pair() {
        // 🇷🇴 = U+1F1F7 U+1F1F4
        assert_eq!(callsign_flag("YO6RZV").as_deref(), Some("\u{1F1F7}\u{1F1F4}"));
        assert_eq!(callsign_flag("DL1ABC").as_deref(), Some("\u{1F1E9}\u{1F1EA}"));
    }

    #[test]
    fn unknown_or_too_short_yields_none() {
        assert_eq!(callsign_flag(""), None);
        assert_eq!(callsign_flag("Q"), None);
        assert_eq!(iso_country("Q1AA"), None); // Q series is not an allocated country block
    }

    #[test]
    fn ranges_are_well_formed_and_non_overlapping() {
        for (start, end, iso) in PREFIX_RANGES {
            assert!(start <= end, "range {start}..{end} is inverted");
            assert_eq!(start.len(), 2, "prefix {start} must be two chars");
            assert_eq!(end.len(), 2, "prefix {end} must be two chars");
            assert_eq!(iso.len(), 2, "ISO code {iso} must be two chars");
        }
        // Quadratic but tiny; guards against accidental overlap when editing the table.
        for (i, (s1, e1, _)) in PREFIX_RANGES.iter().enumerate() {
            for (s2, e2, _) in &PREFIX_RANGES[i + 1..] {
                let overlap = s1 <= e2 && s2 <= e1;
                assert!(!overlap, "ranges {s1}..{e1} and {s2}..{e2} overlap");
            }
        }
    }
}
