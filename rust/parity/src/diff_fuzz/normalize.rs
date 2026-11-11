//! Turn a response into the part both stacks must share.
//!
//! UUIDs and Rails timestamps (`YYYY-MM-DDTHH:MM:SS.mmmZ`) are replaced in
//! order of appearance, so two bodies with the same shape compare equal even
//! when the servers minted different ids and clocks. ETags are reduced to
//! their weak/strong shape: they are a hash of the un-normalized body, so the
//! hex itself cannot match. Headers outside the contract (proxy `date`,
//! `server`, `x-runtime`, `x-request-id`, …) are dropped.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Normalized {
    pub status: u16,
    pub body: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderDiff {
    pub name: String,
    pub rails: Option<String>,
    pub rust: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diff {
    pub status_rails: u16,
    pub status_rust: u16,
    pub body_rails: String,
    pub body_rust: String,
    pub headers: Vec<HeaderDiff>,
}

pub fn is_contract_header(name: &str) -> bool {
    matches!(
        name,
        "content-type"
            | "content-length"
            | "content-disposition"
            | "content-transfer-encoding"
            | "cache-control"
            | "etag"
            | "location"
            | "www-authenticate"
            | "set-cookie"
            | "x-frame-options"
            | "x-content-type-options"
            | "x-xss-protection"
            | "x-permitted-cross-domain-policies"
            | "referrer-policy"
    )
}

/// `headers` are raw name/value pairs, already decoded as text.
pub fn normalize(status: u16, headers: &[(String, String)], body: &[u8]) -> Normalized {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        if !is_contract_header(&name) {
            continue;
        }
        grouped.entry(name).or_default().push(value.clone());
    }
    let headers = grouped
        .into_iter()
        .map(|(name, values)| {
            let joined = values.join(", ");
            let value = if name == "etag" {
                normalize_etag(&joined)
            } else {
                joined
            };
            (name, value)
        })
        .collect();
    Normalized {
        status,
        body: normalize_body(body),
        headers,
    }
}

pub fn compare(rails: &Normalized, rust: &Normalized) -> Option<Diff> {
    let mut names = std::collections::BTreeSet::new();
    names.extend(rails.headers.keys().map(String::as_str));
    names.extend(rust.headers.keys().map(String::as_str));
    let mut headers = Vec::new();
    for name in names {
        let left = rails.headers.get(name);
        let right = rust.headers.get(name);
        if left != right {
            headers.push(HeaderDiff {
                name: name.to_owned(),
                rails: left.cloned(),
                rust: right.cloned(),
            });
        }
    }
    if rails.status == rust.status && rails.body == rust.body && headers.is_empty() {
        None
    } else {
        Some(Diff {
            status_rails: rails.status,
            status_rust: rust.status,
            body_rails: rails.body.clone(),
            body_rust: rust.body.clone(),
            headers,
        })
    }
}

fn normalize_body(body: &[u8]) -> String {
    match std::str::from_utf8(body) {
        Ok(text) => normalize_text(text),
        Err(_) => format!("<binary len={} fnv={:016x}>", body.len(), fnv64(body)),
    }
}

fn normalize_text(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut uuid_n = 0u32;
    let mut date_n = 0u32;
    while i < bytes.len() {
        if let Some(len) = uuid_at(bytes, i) {
            uuid_n += 1;
            out.push_str(&format!("<uuid-{uuid_n}>"));
            i += len;
            continue;
        }
        if let Some(len) = date_at(bytes, i) {
            date_n += 1;
            out.push_str(&format!("<date-{date_n}>"));
            i += len;
            continue;
        }
        let rest = &input[i..];
        let Some(ch) = rest.chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn uuid_at(bytes: &[u8], i: usize) -> Option<usize> {
    const LEN: usize = 36;
    if i + LEN > bytes.len() {
        return None;
    }
    if i > 0 && bytes[i - 1].is_ascii_hexdigit() {
        return None;
    }
    if i + LEN < bytes.len() && bytes[i + LEN].is_ascii_hexdigit() {
        return None;
    }
    let s = &bytes[i..i + LEN];
    for dash in [8, 13, 18, 23] {
        if s[dash] != b'-' {
            return None;
        }
    }
    for (start, end) in [(0, 8), (9, 13), (14, 18), (19, 23), (24, 36)] {
        if !s[start..end].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
    }
    Some(LEN)
}

/// Rails `as_json` timestamp: three truncated digits and a trailing `Z`.
fn date_at(bytes: &[u8], i: usize) -> Option<usize> {
    const LEN: usize = 24;
    if i + LEN > bytes.len() {
        return None;
    }
    let s = &bytes[i..i + LEN];
    let ok = digits(&s[0..4])
        && s[4] == b'-'
        && digits(&s[5..7])
        && s[7] == b'-'
        && digits(&s[8..10])
        && s[10] == b'T'
        && digits(&s[11..13])
        && s[13] == b':'
        && digits(&s[14..16])
        && s[16] == b':'
        && digits(&s[17..19])
        && s[19] == b'.'
        && digits(&s[20..23])
        && s[23] == b'Z';
    if ok { Some(LEN) } else { None }
}

fn digits(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_digit)
}

fn normalize_etag(value: &str) -> String {
    let trimmed = value.trim();
    let (weak, rest) = if let Some(rest) = trimmed.strip_prefix("W/") {
        (true, rest.trim())
    } else {
        (false, trimmed)
    };
    let Some(inner) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return value.to_owned();
    };
    if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_hexdigit()) {
        return value.to_owned();
    }
    if weak {
        "W/\"<etag>\"".to_owned()
    } else {
        "\"<etag>\"".to_owned()
    }
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_dates_and_etags_are_normalized_in_appearance_order() {
        let body = br#"{"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1","created_at":"2026-01-02T03:04:05.006Z","other":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1","updated_at":"2026-01-02T03:04:05.009Z"}"#;
        let headers = [
            (
                "etag".into(),
                "W/\"0123456789abcdef0123456789abcdef\"".into(),
            ),
            (
                "content-type".into(),
                "application/json; charset=utf-8".into(),
            ),
            ("date".into(), "Thu, 01 Jan 2026 00:00:00 GMT".into()),
            ("x-runtime".into(), "0.012".into()),
            ("X-Frame-Options".into(), "SAMEORIGIN".into()),
        ];
        let norm = normalize(200, &headers, body);
        assert_eq!(norm.status, 200);
        assert!(norm.body.contains("<uuid-1>"), "{}", norm.body);
        assert!(norm.body.contains("<uuid-2>"), "{}", norm.body);
        assert!(norm.body.contains("<date-1>"), "{}", norm.body);
        assert!(norm.body.contains("<date-2>"), "{}", norm.body);
        assert!(!norm.body.contains("aaaaaaaa"), "{}", norm.body);
        assert_eq!(
            norm.headers.get("etag").map(String::as_str),
            Some("W/\"<etag>\"")
        );
        assert_eq!(
            norm.headers.get("x-frame-options").map(String::as_str),
            Some("SAMEORIGIN")
        );
        assert!(!norm.headers.contains_key("date"));
        assert!(!norm.headers.contains_key("x-runtime"));
    }

    #[test]
    fn the_same_shape_with_different_ids_compares_equal() {
        let rails_body = br#"{"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1","created_at":"2026-01-02T00:00:00.000Z"}"#;
        let rust_body = br#"{"id":"11111111-1111-4111-8111-111111111111","created_at":"2026-11-11T08:09:10.011Z"}"#;
        let rails_h = [(
            "etag".into(),
            "W/\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"".into(),
        )];
        let rust_h = [(
            "etag".into(),
            "W/\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"".into(),
        )];
        let rails = normalize(201, &rails_h, rails_body);
        let rust = normalize(201, &rust_h, rust_body);
        assert_eq!(
            compare(&rails, &rust),
            None,
            "rails={} rust={}",
            rails.body,
            rust.body
        );
    }

    #[test]
    fn a_status_or_contract_header_mismatch_is_a_diff() {
        let rails = normalize(200, &[("cache-control".into(), "no-cache".into())], b"{}");
        let rust = normalize(
            500,
            &[(
                "cache-control".into(),
                "max-age=0, private, must-revalidate".into(),
            )],
            b"{}",
        );
        let diff = compare(&rails, &rust).expect("diff");
        assert_eq!(diff.status_rails, 200);
        assert_eq!(diff.status_rust, 500);
        assert_eq!(diff.headers.len(), 1);
        assert_eq!(diff.headers[0].name, "cache-control");
    }

    #[test]
    fn a_non_rails_timestamp_is_left_visible() {
        let norm = normalize(200, &[], b"{\"t\":\"2026-01-02T00:00:00+00:00\"}");
        assert!(norm.body.contains("+00:00"), "{}", norm.body);
        assert!(!norm.body.contains("<date-"), "{}", norm.body);
    }

    #[test]
    fn binary_bodies_compare_by_length_and_hash() {
        let rails = normalize(200, &[], &[0, 159, 255]);
        let rust = normalize(200, &[], &[0, 159, 255]);
        assert_eq!(compare(&rails, &rust), None);
        let other = normalize(200, &[], &[0, 159, 254]);
        assert!(compare(&rails, &other).is_some());
    }
}
