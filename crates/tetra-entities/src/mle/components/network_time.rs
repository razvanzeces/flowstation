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
    // Seconds since Jan 1 00:00:00 UTC of the current year, divided by 2
    let year = now_utc.year();
    let year_start = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single()?;
    let secs_since_year_start = (now_utc - year_start).num_seconds();
    let utc_time: u64 = (secs_since_year_start / 2) as u64 & 0xFF_FFFF; // 24 bits

    // Compute local time offset from UTC
    let now_local = now_utc.with_timezone(&tz);
    let offset_secs = now_local.offset().fix().local_minus_utc(); // seconds east of UTC
    let offset_sign: u64 = if offset_secs < 0 { 1 } else { 0 };
    let offset_magnitude: u64 = (offset_secs.unsigned_abs() / 900) as u64 & 0x3F; // 6 bits, 15-min steps

    // Year relative to 2000
    let year_field: u64 = (year - 2000) as u64 & 0x3F; // 6 bits

    // Reserved bits set to all 1s
    let reserved: u64 = 0x7FF; // 11 bits

    // Pack into 48-bit value (MSB first):
    //   [23..0] utc_time | [0] sign | [5..0] offset | [5..0] year | [10..0] reserved
    let value = (utc_time << 24) | (offset_sign << 23) | (offset_magnitude << 17) | (year_field << 11) | reserved;

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

        // Seconds since 2026-01-01 00:00 UTC:
        // Jan=31 days + 14 days (Feb 1-14) + 12 hours = 45 days + 12h
        let year_start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let expected_secs = (dt - year_start).num_seconds();
        let expected_utc_time = (expected_secs / 2) as u64;

        // Europe/Amsterdam in February = CET = UTC+1 -> offset_sign=0, offset=4 (4*15min=60min=1h)
        let expected_sign: u64 = 0;
        let expected_offset: u64 = 4; // 60 minutes / 15 = 4
        let expected_year: u64 = 26; // 2026 - 2000
        let expected_reserved: u64 = 0x7FF;

        let expected =
            (expected_utc_time << 24) | (expected_sign << 23) | (expected_offset << 17) | (expected_year << 11) | expected_reserved;

        assert_eq!(value, expected);

        // Verify individual fields by extraction
        assert_eq!((value >> 24) & 0xFF_FFFF, expected_utc_time);
        assert_eq!((value >> 23) & 1, 0); // positive offset
        assert_eq!((value >> 17) & 0x3F, 4); // +1h = 4 * 15min
        assert_eq!((value >> 11) & 0x3F, 26); // year 2026
        assert_eq!(value & 0x7FF, 0x7FF); // reserved
    }

    #[test]
    fn test_encode_negative_offset() {
        // 2026-01-15 12:00:00 UTC, New York (EST = UTC-5)
        let dt = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "America/New_York".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        assert_eq!((value >> 23) & 1, 1); // negative offset
        assert_eq!((value >> 17) & 0x3F, 20); // 5h = 300min / 15 = 20
        assert_eq!((value >> 11) & 0x3F, 26); // year 2026
        assert_eq!(value & 0x7FF, 0x7FF);
    }

    #[test]
    fn test_encode_utc_timezone() {
        let dt = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let tz: chrono_tz::Tz = "UTC".parse().unwrap();

        let value = encode_tetra_network_time_inner(dt, tz).unwrap();

        assert_eq!((value >> 23) & 1, 0); // positive
        assert_eq!((value >> 17) & 0x3F, 0); // zero offset
    }

    #[test]
    fn test_invalid_timezone() {
        assert!(encode_tetra_network_time("Invalid/Timezone").is_none());
    }
}
