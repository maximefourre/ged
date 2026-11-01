//! JSON time format. Fixed once for the whole API (plan T4): Rails'
//! `as_json` for a UTC time, `YYYY-MM-DDTHH:MM:SS.mmmZ` — always three
//! fractional digits, **truncated** (not rounded), always `Z`.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};

/// Formats a timestamp like Rails does.
pub fn rails_json(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// `timestamp(6) without time zone` columns hold UTC.
pub fn rails_json_naive(time: NaiveDateTime) -> String {
    rails_json(time.and_utc())
}

/// `#[serde(with = "ged::domain::time::serde_rails")]` for `DateTime<Utc>`.
pub mod serde_rails {
    use chrono::{DateTime, Utc};
    use serde::Serializer;

    pub fn serialize<S: Serializer>(time: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::rails_json(*time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(micros: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 21, 12, 39).unwrap()
            + chrono::Duration::microseconds(micros as i64)
    }

    #[test]
    fn three_fractional_digits_and_z() {
        assert_eq!(rails_json(at(492_000)), "2026-11-01T21:12:39.492Z");
        assert_eq!(rails_json(at(0)), "2026-11-01T21:12:39.000Z");
        assert_eq!(rails_json(at(7_000)), "2026-11-01T21:12:39.007Z");
    }

    #[test]
    fn truncates_instead_of_rounding() {
        assert_eq!(rails_json(at(123_999)), "2026-11-01T21:12:39.123Z");
        assert_eq!(rails_json(at(999_999)), "2026-11-01T21:12:39.999Z");
    }

    #[test]
    fn naive_columns_are_utc() {
        let naive =
            NaiveDateTime::parse_from_str("2026-01-02 03:04:05.5", "%Y-%m-%d %H:%M:%S%.f").unwrap();
        assert_eq!(rails_json_naive(naive), "2026-01-02T03:04:05.500Z");
    }

    #[test]
    fn serde_helper() {
        #[derive(serde::Serialize)]
        struct T(#[serde(with = "serde_rails")] DateTime<Utc>);
        assert_eq!(
            serde_json::to_string(&T(at(1_000))).unwrap(),
            "\"2026-11-01T21:12:39.001Z\""
        );
    }
}
