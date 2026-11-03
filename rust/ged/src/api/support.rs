//! What `Api::V1::BaseController` gives every endpoint: scalar/id parameter
//! checks with Rails' messages, `limit`/`offset` windows, and the casts
//! ActiveRecord applies to ids and ISO8601 filters.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::token_validator::string_to_i;
use crate::domain::document::Document;
use crate::domain::time::rails_json_naive;
use crate::http::ApiError;
use crate::http::params::{ParamValue, RackParams};

pub const DEFAULT_PAGE_SIZE: i64 = 100;
pub const MAX_PAGE_SIZE: i64 = 500;
/// Cap offset well below the Postgres bigint range: an unbounded offset
/// overflows int64 (an uncontracted 500) long before that.
pub const MAX_OFFSET: i64 = 1_000_000;

/// The text Ruby's `blank?` tests: empty or only Unicode whitespace.
pub fn blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// A database failure: logged, never put in a response (the 500 contract).
pub fn internal(error: impl std::fmt::Display) -> ApiError {
    eprintln!("ged: request failed: {error}");
    ApiError::Internal
}

/// Ruby's `blank?` on a request parameter.
pub fn param_blank(value: &ParamValue) -> bool {
    match value {
        ParamValue::Nil | ParamValue::Bool(false) => true,
        ParamValue::Str(s) => blank(s),
        ParamValue::Array(items) => items.is_empty(),
        ParamValue::Map(map) => map.is_empty(),
        ParamValue::Num(_) | ParamValue::Bool(true) => false,
    }
}

/// `scalar_param`: absent/`nil` or a String; anything else (array, hash,
/// number, boolean) is a 400 instead of a 500 further down.
pub fn scalar_param<'a>(params: &'a RackParams, key: &str) -> Result<Option<&'a str>, ApiError> {
    match params.get(key) {
        None | Some(ParamValue::Nil) => Ok(None),
        Some(ParamValue::Str(s)) => Ok(Some(s)),
        Some(_) => Err(ApiError::BadRequest(format!("{key} must be a string"))),
    }
}

/// `scalar_param(key).present?` — the value only when it is not blank.
pub fn present_param<'a>(params: &'a RackParams, key: &str) -> Result<Option<&'a str>, ApiError> {
    Ok(scalar_param(params, key)?.filter(|v| !blank(v)))
}

/// `ActionController::ParameterMissing` message.
pub fn parameter_missing(key: &str) -> ApiError {
    ApiError::BadRequest(format!(
        "param is missing or the value is empty or invalid: {key}"
    ))
}

pub fn require_scalar_param<'a>(params: &'a RackParams, key: &str) -> Result<&'a str, ApiError> {
    present_param(params, key)?.ok_or_else(|| parameter_missing(key))
}

/// `id_param`: ids legitimately arrive as JSON integers.
pub fn id_param(params: &RackParams, key: &str) -> Result<Option<String>, ApiError> {
    match params.get(key) {
        None | Some(ParamValue::Nil) => Ok(None),
        Some(ParamValue::Str(s)) => Ok(Some(s.clone())),
        Some(ParamValue::Num(n)) if n.is_i64() || n.is_u64() => Ok(Some(n.to_string())),
        Some(_) => Err(ApiError::BadRequest(format!(
            "{key} must be a string or an integer"
        ))),
    }
}

/// `to_s` of a parameter (an element of `document_ids`): only strings can be
/// ids; anything else is text that matches no document.
pub fn id_param_text(value: &ParamValue) -> String {
    match value {
        ParamValue::Nil => String::new(),
        ParamValue::Str(s) => s.clone(),
        ParamValue::Num(n) => n.to_string(),
        ParamValue::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// `tags_param`: the nested map as sent, anything else passed through so the
/// model's validation answers 422 instead of failing here. `None` when absent.
pub fn tags_param(params: &RackParams) -> Option<Value> {
    // `tags_param || {}`: nil and false fall back to the empty map
    match params.get("tags") {
        None | Some(ParamValue::Nil | ParamValue::Bool(false)) => None,
        Some(value) => Some(param_to_json(value)),
    }
}

pub fn param_to_json(value: &ParamValue) -> Value {
    match value {
        ParamValue::Nil => Value::Null,
        ParamValue::Str(s) => Value::String(s.clone()),
        ParamValue::Num(n) => Value::Number(n.clone()),
        ParamValue::Bool(b) => Value::Bool(*b),
        ParamValue::Array(items) => Value::Array(items.iter().map(param_to_json).collect()),
        ParamValue::Map(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.to_owned(), param_to_json(v)))
                .collect(),
        ),
    }
}

/// `int_param`: a String's `to_i`, 0 for anything else.
fn int_param(params: &RackParams, key: &str) -> i64 {
    params.str(key).map_or(0, string_to_i)
}

/// `pagination_window`: non-positive, non-numeric or non-scalar values fall
/// back to the default, oversized limits and offsets are clamped.
pub fn pagination_window(params: &RackParams) -> (i64, i64) {
    let requested = int_param(params, "limit");
    let limit = if requested > 0 {
        requested.min(MAX_PAGE_SIZE)
    } else {
        DEFAULT_PAGE_SIZE
    };
    (limit, int_param(params, "offset").clamp(0, MAX_OFFSET))
}

/// `Model.find("12abc")`: ActiveRecord casts a String id with `to_i`
/// (`"abc"` is 0). Nothing has id 0 or a saturated value, so those 404.
pub fn integer_id(text: &str) -> i64 {
    string_to_i(text)
}

/// `Document.find(text)` / `where(id: text)` on a uuid column: the PG adapter
/// accepts `{…}`, optional hyphens after any group of four hex digits, either
/// case, and casts anything else to `nil` (not found).
pub fn uuid_id(text: &str) -> Option<Uuid> {
    let inner = match text.strip_prefix('{') {
        Some(rest) => rest.strip_suffix('}')?,
        None => text,
    };
    let bytes = inner.as_bytes();
    let mut digits = String::with_capacity(32);
    let mut i = 0;
    for _ in 0..8 {
        let group = bytes.get(i..i + 4)?;
        if !group.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        digits.extend(group.iter().map(|b| *b as char));
        i += 4;
        if bytes.get(i) == Some(&b'-') {
            i += 1;
        }
    }
    if i != bytes.len() {
        return None;
    }
    Uuid::parse_str(&digits).ok()
}

/// A document as the API shows it (`DocumentJson#document_json`).
pub fn document_json(document: &Document) -> Value {
    serde_json::json!({
        "id": document.id.to_string(),
        "filename": document.filename,
        "content_type": document.content_type,
        "byte_size": document.byte_size,
        "department_id": document.department_id,
        "uploaded_by": document.uploaded_by,
        "tags": document.tags,
        "created_at": rails_json_naive(document.created_at),
        "metadata": document.metadata_hash(),
    })
}

/// `Time.iso8601(value)` as a UTC timestamp truncated to microseconds, the
/// precision Rails binds. `None` where Ruby raises `ArgumentError`.
/// A zone-less time is local to the server, which runs in UTC.
pub fn parse_iso8601(value: &str) -> Option<NaiveDateTime> {
    let text = value.trim_matches(|c: char| c.is_ascii_whitespace());
    let (date, rest) = split_date(text)?;
    let (year, month, day) = date;
    let (mut hour, mut minute, mut second, mut micros) = (0u32, 0u32, 0u32, 0u32);
    let mut zone = rest;
    if let Some(time) = rest.strip_prefix(['T', 't']) {
        let b = time.as_bytes();
        if b.len() < 8 || b[2] != b':' || b[5] != b':' {
            return None;
        }
        let two = |s: &[u8]| -> Option<u32> {
            (s.len() == 2 && s.iter().all(u8::is_ascii_digit))
                .then(|| u32::from(s[0] - b'0') * 10 + u32::from(s[1] - b'0'))
        };
        hour = two(&b[0..2])?;
        minute = two(&b[3..5])?;
        second = two(&b[6..8])?;
        zone = &time[8..];
        if let Some(fraction) = zone.strip_prefix('.') {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let mut padded = fraction[..digits.min(6)].to_owned();
            while padded.len() < 6 {
                padded.push('0');
            }
            micros = padded.parse().ok()?;
            zone = &fraction[digits..];
        }
    }
    let offset_seconds = parse_zone(zone)?;
    // Time.utc range checks: 24 only as 24:00:00, seconds up to 60.
    if hour > 24 || minute > 59 || second > 60 || (hour == 24 && (minute != 0 || second != 0)) {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Time.utc rolls an impossible day (Feb 30) into the next month.
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let date = first.checked_add_signed(Duration::days(i64::from(day) - 1))?;
    let seconds = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    let utc = date.and_hms_opt(0, 0, 0)?
        + Duration::seconds(seconds - offset_seconds)
        + Duration::microseconds(i64::from(micros));
    // keep years inside what Postgres timestamps and Ruby both represent
    (utc.year() > 0 && utc.year() < 10_000).then_some(utc)
}

/// `YYYY-MM-DD` (year may be negative in Ruby; only positive years are kept)
/// and what follows it.
fn split_date(text: &str) -> Option<((i32, u32, u32), &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let year: i32 = text[..digits].parse().ok()?;
    let rest = text[digits..].strip_prefix('-')?;
    let b = rest.as_bytes();
    if b.len() < 5
        || b[2] != b'-'
        || !b[..2].iter().all(u8::is_ascii_digit)
        || !b[3..5].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let month: u32 = rest[..2].parse().ok()?;
    let day: u32 = rest[3..5].parse().ok()?;
    Some(((year, month, day), &rest[5..]))
}

/// `Z`, `±HH`, `±HH:MM`, `±HHMM`, or nothing (local = UTC): the offset in seconds.
fn parse_zone(zone: &str) -> Option<i64> {
    if zone.is_empty() || zone.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let sign = match zone.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest = &zone[1..];
    let digits: String = rest.chars().filter(|c| *c != ':').collect();
    let well_formed = matches!(rest.len(), 2 | 4 | 5)
        && rest.bytes().enumerate().all(|(i, b)| {
            if rest.len() == 5 && i == 2 {
                b == b':'
            } else {
                b.is_ascii_digit()
            }
        });
    if !well_formed {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = if digits.len() == 4 {
        digits[2..4].parse().ok()?
    } else {
        0
    };
    Some(sign * (hours * 3600 + minutes * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(query: &str) -> RackParams {
        RackParams::parse(query).unwrap()
    }

    #[test]
    fn scalar_param_accepts_strings_and_rejects_the_rest() {
        let p = params("a=1&b[]=x&c[k]=v&d");
        assert_eq!(scalar_param(&p, "a").unwrap(), Some("1"));
        assert_eq!(scalar_param(&p, "missing").unwrap(), None);
        assert_eq!(scalar_param(&p, "d").unwrap(), None);
        for key in ["b", "c"] {
            assert_eq!(
                scalar_param(&p, key),
                Err(ApiError::BadRequest(format!("{key} must be a string")))
            );
        }
    }

    #[test]
    fn a_blank_required_param_is_missing() {
        let p = params("name=%20%20&other=x");
        assert_eq!(
            require_scalar_param(&p, "name"),
            Err(ApiError::BadRequest(
                "param is missing or the value is empty or invalid: name".into()
            ))
        );
        assert_eq!(require_scalar_param(&p, "other").unwrap(), "x");
    }

    #[test]
    fn pagination_defaults_clamps_and_ignores_what_is_not_a_string() {
        assert_eq!(pagination_window(&params("")), (100, 0));
        assert_eq!(pagination_window(&params("limit=7&offset=3")), (7, 3));
        assert_eq!(pagination_window(&params("limit=9999")), (500, 0));
        assert_eq!(pagination_window(&params("limit=0&offset=-5")), (100, 0));
        assert_eq!(pagination_window(&params("limit=-3")), (100, 0));
        assert_eq!(pagination_window(&params("limit=abc&offset=x")), (100, 0));
        assert_eq!(pagination_window(&params("limit[]=1")), (100, 0));
        assert_eq!(
            pagination_window(&params("limit[a]=1&offset[]=4")),
            (100, 0)
        );
        assert_eq!(
            pagination_window(&params("offset=99999999999999999999")),
            (100, MAX_OFFSET)
        );
        assert_eq!(pagination_window(&params("limit=1&limit=7")), (7, 0));
        assert_eq!(pagination_window(&params("limit=%2012")), (12, 0));
    }

    #[test]
    fn integer_ids_cast_like_to_i() {
        assert_eq!(integer_id("12"), 12);
        assert_eq!(integer_id("12abc"), 12);
        assert_eq!(integer_id("abc"), 0);
        assert_eq!(integer_id(""), 0);
    }

    #[test]
    fn uuid_ids_cast_like_the_postgres_adapter() {
        let canonical = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let want = Uuid::parse_str(canonical).unwrap();
        assert_eq!(uuid_id(canonical), Some(want));
        assert_eq!(uuid_id(&canonical.to_uppercase()), Some(want));
        assert_eq!(uuid_id("0f8fad5bd9cb469fa16570867728950e"), Some(want));
        assert_eq!(uuid_id(&format!("{{{canonical}}}")), Some(want));
        assert_eq!(uuid_id("0f8fad5bd9cb-469fa165-70867728950e"), Some(want));
        for bad in [
            "",
            "abc",
            "1",
            "{0f8fad5b-d9cb-469f-a165-70867728950e",
            "0f8fad5b-d9cb-469f-a165-70867728950e}",
            "0f8fad5b-d9cb-469f-a165-70867728950",
            "0f8fad5b-d9cb-469f-a165-70867728950ee",
            "0f8fad5b-d9cb-469f-a165-7086772895zz",
            "urn:uuid:0f8fad5b-d9cb-469f-a165-70867728950e",
            "0f8fad5b--d9cb-469f-a165-70867728950e",
            "bulk_destroy",
        ] {
            assert_eq!(uuid_id(bad), None, "{bad:?}");
        }
    }

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.6f").unwrap()
    }

    #[test]
    fn iso8601_matches_time_iso8601() {
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:00Z"),
            Some(at("2026-11-01 10:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:00"),
            Some(at("2026-11-01 10:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:00.123456789Z"),
            Some(at("2026-11-01 10:00:00.123456"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:00+02:00"),
            Some(at("2026-11-01 08:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:00-0130"),
            Some(at("2026-11-01 11:30:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:00+02"),
            Some(at("2026-11-01 08:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601(" 2026-11-01t10:00:00z\n"),
            Some(at("2026-11-01 10:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01"),
            Some(at("2026-11-01 00:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01Z"),
            Some(at("2026-11-01 00:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T24:00:00Z"),
            Some(at("2026-11-02 00:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-02-30T00:00:00Z"),
            Some(at("2026-03-02 00:00:00.000000"))
        );
        assert_eq!(
            parse_iso8601("2026-11-01T10:00:60Z"),
            Some(at("2026-11-01 10:01:00.000000"))
        );
        for bad in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-00-10T00:00:00Z",
            "2026-11-32T00:00:00Z",
            "2026-11-01T25:00:00Z",
            "2026-11-01T24:00:01Z",
            "2026-11-01T10:61:00Z",
            "2026-11-01T10:00Z",
            "2026-11-01 10:00:00Z",
            "2026-11-01T10:00:00.Z",
            "2026-11-01T10:00:00+2",
            "2026-11-01T10:00:00+02:0",
            "2026-1-01T10:00:00Z",
            "20261101T100000Z",
            "2026-11-01T10:00:00Zjunk",
        ] {
            assert_eq!(parse_iso8601(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn document_json_has_rails_key_order() {
        let at = at("2026-11-01 21:12:39.123999");
        let doc = Document {
            id: Uuid::nil(),
            byte_size: Some(3),
            content_type: None,
            created_at: at,
            deleted_at: None,
            department_id: 1,
            filename: "a<b>.pdf".into(),
            purge_at: None,
            stored_async: true,
            tags: serde_json::json!({"type": "x"}),
            updated_at: at,
            uploaded_at: at,
            uploaded_by: "c".into(),
        };
        assert_eq!(
            crate::http::json::to_rails_json(&document_json(&doc)).unwrap(),
            r#"{"id":"00000000-0000-0000-0000-000000000000","filename":"a\u003cb\u003e.pdf","content_type":null,"byte_size":3,"department_id":1,"uploaded_by":"c","tags":{"type":"x"},"created_at":"2026-11-01T21:12:39.123Z","metadata":{"uploaded_at":"2026-11-01T21:12:39.123Z","updated_at":"2026-11-01T21:12:39.123Z","upload_mode":"async"}}"#
        );
    }
}
