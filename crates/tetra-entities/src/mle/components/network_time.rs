use chrono::{Datelike, Offset, TimeZone, Utc};
use std::str::FromStr;

/// Encode current system time as a 48-bit TETRA network time value
/// per ETSI EN 300 392-2 clause 18.5.24.
///
/// Field layout (MSB first, 48 bits total):
///   - UTC time (24 bits): seconds since Jan 1 00:00 UTC of the current year, divided by 2
///   - Local time offset sign (1 bit): 0 = positive (east of UTC), 1 = negative (west of UTC)
///   - Local time offset (6 bits): magnitude in 15-minute increments
///   - Year (6 bits): current year minus 2000
///   - Reserved (11 bits): set to all 1s (0x7FF)
///
/// Returns `None` if the timezone name is invalid.
pub fn encode_tetra_network_time(tz_name: &str) -> Option<u64> {
    let tz: chrono_tz::Tz = chrono_tz::Tz::from_str(tz_name).ok()?;
    let now_utc = Utc::now();

    encode_tetra_network_time_inner(now_utc, tz)
}

fn encode_tetra_network_time_inner(now_utc: chrono::DateTime<Utc>, tz: chrono_tz::Tz) -> Option<u64> {
    // Seconds since Jan 1 00:00:00 UTC of the current year, divided by 2.
    //
    // FIX: folosim .earliest() in loc de .single() pentru a evita un None la
    // tranzitiile DST ambigue. .single() returneaza None daca chrono detecteaza
    // ambiguitate interna, ceea ce bloca silentios tot broadcast-ul.
    // .earliest() returneaza intotdeauna Some(...) pentru date UTC valide.
    let year = now_utc.year();
    let year_start = Utc
        .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
        .earliest()
        .unwrap_or_else(|| {
            tracing::error!(
                "encode_tetra_network_time: failed to compute year_start for year {}, using epoch fallback",
                year
            );
            chrono::DateTime::UNIX_EPOCH.with_timezone(&Utc)
        });

    let secs_since_year_start = (now_utc - year_start).num_seconds().max(0);
    let utc_time: u64 = (secs_since_year_start / 2) as u64 & 0xFF_FFFF; // 24 bits

    // Compute local time offset from UTC.
    // NOTA: necesita chrono cu feature "std" activ (vezi workspace Cargo.toml).
    // Fara "std", offset().fix().local_minus_utc() returneaza intotdeauna 0,
    // producand un PDU cu offset=0 (UTC) indiferent de timezone.
    let now_local = now_utc.with_timezone(&tz);
    let offset_secs = now_local.offset().fix().local_minus_utc(); // seconds east of UTC
    let offset_sign: u64 = if offset_secs < 0 { 1 } else { 0 };
    let offset_magnitude: u64 = (offset_secs.unsigned_abs() / 900) as u64 & 0x3F; // 6 bits, 15-min steps

    // Year relative to 2000
    let year_field: u64 = (year - 2000) as u64 & 0x3F; // 6 bits

    // Reserved bits set to all 1s
    let reserved: u64 = 0x7FF; // 11 bits

    // Pack into 48-bit value (MSB first):
    //   [47..24] utc_time | [23] sign | [22..17] offset | [16..11] year | [10..0] reserved
    let value = (utc_time << 24)
        | (offset_sign << 23)
        | (offset_magnitude << 17)
        | (year_field << 11)
        | reserved;

    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_encode_known_time() {
        // 2026-02-15 12:00:00 UTC
        let dt = Utc.with_ymd_and_hms(2026, 2, 15, 12, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "Europe/Amsterdam".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        let year_start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let expected_secs = (dt - year_start).num_seconds();
        let expected_utc_time = (expected_secs / 2) as u64;

        // Europe/Amsterdam in February = CET = UTC+1 -> offset_sign=0, offset=4 (4*15min=60min=1h)
        let expected_sign: u64 = 0;
        let expected_offset: u64 = 4;
        let expected_year: u64 = 26;
        let expected_reserved: u64 = 0x7FF;

        let expected =
            (expected_utc_time << 24) | (expected_sign << 23) | (expected_offset << 17) | (expected_year << 11) | expected_reserved;

        assert_eq!(value, expected);
        assert_eq!((value >> 24) & 0xFF_FFFF, expected_utc_time);
        assert_eq!((value >> 23) & 1, 0);
        assert_eq!((value >> 17) & 0x3F, 4);
        assert_eq!((value >> 11) & 0x3F, 26);
        assert_eq!(value & 0x7FF, 0x7FF);
    }

    #[test]
    fn test_encode_bucharest_summer() {
        // 2026-05-15 10:00:00 UTC — vara, EEST = UTC+3
        let dt = Utc.with_ymd_and_hms(2026, 5, 15, 10, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "Europe/Bucharest".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        assert_eq!((value >> 23) & 1, 0, "offset ar trebui sa fie pozitiv (est de UTC)");
        assert_eq!((value >> 17) & 0x3F, 12, "UTC+3 = 180min / 15 = 12 incremente");
        assert_eq!((value >> 11) & 0x3F, 26, "year 2026");
        assert_eq!(value & 0x7FF, 0x7FF, "reserved bits");
    }

    #[test]
    fn test_encode_bucharest_winter() {
        // 2026-01-15 10:00:00 UTC — iarna, EET = UTC+2
        let dt = Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "Europe/Bucharest".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        assert_eq!((value >> 23) & 1, 0, "offset ar trebui sa fie pozitiv");
        assert_eq!((value >> 17) & 0x3F, 8, "UTC+2 = 120min / 15 = 8 incremente");
    }

    #[test]
    fn test_encode_negative_offset() {
        // 2026-01-15 12:00:00 UTC, New York (EST = UTC-5)
        let dt = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "America/New_York".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        assert_eq!((value >> 23) & 1, 1);
        assert_eq!((value >> 17) & 0x3F, 20);
        assert_eq!((value >> 11) & 0x3F, 26);
        assert_eq!(value & 0x7FF, 0x7FF);
    }

    #[test]
    fn test_encode_utc_timezone() {
        let dt = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "UTC".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        assert_eq!((value >> 23) & 1, 0);
        assert_eq!((value >> 17) & 0x3F, 0);
    }

    #[test]
    fn test_invalid_timezone() {
        assert!(encode_tetra_network_time("Invalid/Timezone").is_none());
    }

    #[test]
    fn test_year_start_never_returns_none() {
        // Verifica ca earliest() nu produce None pentru Jan 1 UTC al oricarui an rezonabil
        for year in [2024, 2025, 2026, 2030, 2050] {
            let result = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).earliest();
            assert!(result.is_some(), "year_start should never be None for year {}", year);
        }
    }
}
