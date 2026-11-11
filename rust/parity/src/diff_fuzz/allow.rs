//! Narrow matchers for divergences already written in `docs/migration/parity-gaps.md`.
//!
//! A matcher fires only for the observed Rails behaviour. Anything else stays a
//! failure so a new bug cannot hide behind a documented one.

use super::model::Target;
use super::normalize::{Normalized, compare};

/// Body of the generic parse failure, after normalization (no ids or dates).
const BAD_REQUEST_BODY: &str = r#"{"error":"bad request"}"#;

/// `Some(reason)` when this difference is an entry in `parity-gaps.md`.
///
/// `sanitized` is the Rust response to the same request after Thruster's
/// rewrite (the bad query or urlencoded parameter removed). It must equal
/// the Rails response: status, normalized body, and contract headers. A
/// malformed `%` in the path is not this gap — Thruster's documented rewrite
/// drops parameters, not path segments.
pub fn documented_gap(
    target: &Target,
    rails: &Normalized,
    rust: &Normalized,
    sanitized: Option<&Normalized>,
) -> Option<&'static str> {
    malformed_percent(target, rails, rust, sanitized)
        .or_else(|| malformed_percent_in_path(target, rails, rust))
}

/// Thruster rewrites a `%` that is not two hex digits by dropping that
/// parameter, and Rails then sees a normal request. The Rust edge applies
/// Rails' own rule and answers `400 {"error":"bad request"}`. See
/// parity-gaps, "Requête non analysable".
fn malformed_percent(
    target: &Target,
    rails: &Normalized,
    rust: &Normalized,
    sanitized: Option<&Normalized>,
) -> Option<&'static str> {
    percent_form(target)?;
    if rust.status != 400 || rust.body != BAD_REQUEST_BODY {
        return None;
    }
    let sanitized = sanitized?;
    if compare(rails, sanitized).is_some() {
        return None;
    }
    Some(
        "Thruster supprime un % mal formé ; Rust répond 400 {\"error\":\"bad request\"} (parity-gaps.md, requête non analysable)",
    )
}

/// Thruster rejects a `%` in the path that is not two hex digits: `400`,
/// body `400 Bad Request`, `text/plain; charset=utf-8`, no `Content-Length`.
/// Observed 2026-11-11 on the published Rails port, for `GET` and `POST`,
/// with or without a query. Puma on port 3000 does not: it accepts the path,
/// and so does Rust. This matcher allows only the pair where Rust's answer
/// is the unparsable-parameter `400` (no security headers) and the query or
/// the urlencoded body is itself unparsable. A path-only `%`, a `401`, or a
/// `404` stays a failure.
fn malformed_percent_in_path(
    target: &Target,
    rails: &Normalized,
    rust: &Normalized,
) -> Option<&'static str> {
    let (path, _) = split_path_query(&target.path_and_query);
    if !has_malformed_percent(path) || !explains_unparsable(target) {
        return None;
    }
    if !is_thruster_path_rejection(rails) || !is_rust_unparsable(rust) {
        return None;
    }
    Some(
        "Thruster rejette un % mal formé dans le chemin (400 text/plain) ; Rust suit Puma et répond 400 {\"error\":\"bad request\"} (parity-gaps.md, % dans le chemin)",
    )
}

fn is_thruster_path_rejection(rails: &Normalized) -> bool {
    rails.status == 400
        && rails.body == "400 Bad Request"
        && rails.headers.len() == 1
        && rails.headers.get("content-type").map(String::as_str)
            == Some("text/plain; charset=utf-8")
}

fn is_rust_unparsable(rust: &Normalized) -> bool {
    rust.status == 400
        && rust.body == BAD_REQUEST_BODY
        && rust.headers.len() == 2
        && rust.headers.get("content-type").map(String::as_str)
            == Some("application/json; charset=utf-8")
        && rust.headers.get("content-length").map(String::as_str) == Some("23")
}

/// Query or urlencoded body that Puma rejects before the controller: a `%`
/// that is not two hex digits, or a well-formed `%HH` that is not UTF-8.
fn explains_unparsable(target: &Target) -> bool {
    let (_, query) = split_path_query(&target.path_and_query);
    if parameter_is_unparsable(query) {
        return true;
    }
    urlencoded_body(target).is_some_and(parameter_is_unparsable)
}

fn parameter_is_unparsable(raw: &str) -> bool {
    has_malformed_percent(raw) || decodes_to_invalid_utf8(raw)
}

/// Well-formed `%HH` sequences only. A malformed `%` is left to
/// [`has_malformed_percent`].
fn decodes_to_invalid_utf8(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let Some(hi) = bytes.get(i + 1).copied().and_then(hex_digit) else {
            return false;
        };
        let Some(lo) = bytes.get(i + 2).copied().and_then(hex_digit) else {
            return false;
        };
        out.push(hi << 4 | lo);
        i += 3;
    }
    std::str::from_utf8(&out).is_err()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Label printed in the campaign distribution: path, query, and body, each
/// when that part is unparsable (a malformed `%`, or a well-formed `%HH`
/// that is not UTF-8).
pub fn documented_form(target: &Target) -> String {
    let (path, query) = split_path_query(&target.path_and_query);
    let mut parts = Vec::new();
    if parameter_is_unparsable(path) {
        parts.push(format!("path:{path}"));
    }
    if !query.is_empty() && parameter_is_unparsable(query) {
        parts.push(format!("query:{query}"));
    }
    if let Some(text) = urlencoded_body(target)
        && parameter_is_unparsable(text)
    {
        parts.push(format!("body:{text}"));
    }
    if parts.is_empty() {
        "unspecified".to_owned()
    } else {
        parts.join(" ")
    }
}

/// The raw query and/or urlencoded body that carries the malformed `%`.
/// `None` when the only break is in the path, or when nothing is malformed.
pub fn percent_form(target: &Target) -> Option<String> {
    let (path, query) = split_path_query(&target.path_and_query);
    if has_malformed_percent(path) {
        return None;
    }
    let mut parts = Vec::new();
    if has_malformed_percent(query) {
        parts.push(format!("query:{query}"));
    }
    if let Some(text) = urlencoded_body(target)
        && has_malformed_percent(text)
    {
        parts.push(format!("body:{text}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// The request Rails receives once Thruster has dropped every query or
/// urlencoded parameter whose raw text contains a malformed `%`. `None` when
/// this is not that rewrite (clean request, or a `%` in the path).
pub fn thruster_sanitize(target: &Target) -> Option<Target> {
    percent_form(target)?;
    let (path, query) = split_path_query(&target.path_and_query);
    let mut out = target.clone();
    if has_malformed_percent(query) {
        let cleaned = drop_bad_params(query);
        out.path_and_query = if cleaned.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{cleaned}")
        };
    }
    if let Some(text) = urlencoded_body(target)
        && has_malformed_percent(text)
    {
        out.body = drop_bad_params(text).into_bytes();
    }
    Some(out)
}

fn split_path_query(path_and_query: &str) -> (&str, &str) {
    match path_and_query.split_once('?') {
        Some((path, query)) => (path, query),
        None => (path_and_query, ""),
    }
}

fn urlencoded_body(target: &Target) -> Option<&str> {
    let form = target.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
    });
    if !form {
        return None;
    }
    std::str::from_utf8(&target.body).ok()
}

/// Drop `&`-separated parameters whose own text contains a malformed `%`.
fn drop_bad_params(raw: &str) -> String {
    raw.split('&')
        .filter(|part| !part.is_empty() && !has_malformed_percent(part))
        .collect::<Vec<_>>()
        .join("&")
}

/// True when some `%` is not followed by two hexadecimal digits.
pub fn has_malformed_percent(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return true;
            }
            if !bytes[i + 1].is_ascii_hexdigit() || !bytes[i + 2].is_ascii_hexdigit() {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_fuzz::model::{Auth, Body, Ids, Request, Route, encode};
    use crate::diff_fuzz::normalize::normalize;

    fn ids() -> Ids<'static> {
        Ids {
            seed_doc: "",
            created_docs: &[],
            created_uploads: &[],
            token_valid: "t",
            token_expired: "e",
            token_bad_aud: "b",
            token_unknown: "u",
            token_nogrant: "n",
            token_other: "o",
        }
    }

    fn target_with_query(query: &str) -> crate::diff_fuzz::model::Target {
        let mut req = Request::get("malformed-percent", Route::Ping);
        req.raw_query = Some(query.into());
        req.auth = Auth::None;
        encode(&req, &ids())
    }

    fn gap<'a>(
        target: &crate::diff_fuzz::model::Target,
        rails: &'a Normalized,
        rust: &Normalized,
        sanitized: Option<&'a Normalized>,
    ) -> Option<&'static str> {
        documented_gap(target, rails, rust, sanitized)
    }

    #[test]
    fn percent_predicates() {
        assert!(has_malformed_percent("a=%zz"));
        assert!(has_malformed_percent("%"));
        assert!(has_malformed_percent("a=%2"));
        assert!(has_malformed_percent("%2G"));
        assert!(!has_malformed_percent("a=%25"));
        assert!(!has_malformed_percent("a=%ff"));
        assert!(!has_malformed_percent("a=%2F"));
        assert!(!has_malformed_percent("plain"));
    }

    #[test]
    fn thruster_drops_only_the_bad_parameter() {
        let target = target_with_query("a=1&b=%zz&c=3");
        let cleaned = thruster_sanitize(&target).expect("query gap");
        assert_eq!(cleaned.path_and_query, "/api/v1/ping?a=1&c=3");
        assert!(thruster_sanitize(&target_with_query("a=%ff")).is_none());
        assert!(thruster_sanitize(&target_with_query("limit=1")).is_none());
    }

    #[test]
    fn a_thruster_malformed_percent_is_documented_only_when_the_replay_matches() {
        let target = target_with_query("a=%zz");
        let rails = normalize(
            200,
            &[("content-type".into(), "text/html".into())],
            b"<!DOCTYPE html>",
        );
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert!(compare(&rails, &rust).is_some());
        assert!(gap(&target, &rails, &rust, None).is_none());
        assert!(gap(&target, &rails, &rust, Some(&rails)).is_some());
        let other = normalize(500, &[], b"nope");
        assert!(gap(&target, &rails, &rust, Some(&other)).is_none());
    }

    #[test]
    fn a_body_that_only_contains_the_words_is_not_the_gap() {
        let target = target_with_query("a=%zz");
        let rails = normalize(200, &[], b"ok");
        let rust = normalize(400, &[], br#"{"error":"bad request","extra":true}"#);
        assert!(gap(&target, &rails, &rust, Some(&rails)).is_none());
    }

    #[test]
    fn a_real_status_mismatch_on_a_clean_request_is_not_documented() {
        let target = target_with_query("limit=1");
        let rails = normalize(200, &[], b"[]");
        let rust = normalize(500, &[], br#"{"error":"bad request"}"#);
        assert!(gap(&target, &rails, &rust, Some(&rails)).is_none());
    }

    #[test]
    fn a_valid_hex_percent_is_not_the_thruster_gap() {
        let target = target_with_query("a=%ff");
        let rails = normalize(401, &[], br#"{"error":"missing bearer token"}"#);
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert!(gap(&target, &rails, &rust, Some(&rails)).is_none());
    }

    #[test]
    fn a_malformed_percent_in_the_path_is_not_this_gap() {
        let mut req = Request::get("malformed-percent", Route::Documents);
        req.raw_path = Some("/api/v1/documents/%zz".into());
        req.raw_query = Some("a=%zz".into());
        req.auth = Auth::None;
        let target = encode(&req, &ids());
        let rails = normalize(200, &[], b"[]");
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert!(percent_form(&target).is_none());
        assert!(thruster_sanitize(&target).is_none());
        assert!(gap(&target, &rails, &rust, Some(&rails)).is_none());
    }

    fn path_percent(query: &str) -> crate::diff_fuzz::model::Target {
        let mut req = Request::get("malformed-percent", Route::Documents);
        req.raw_path = Some("/api/v1/documents/%zz".into());
        req.raw_query = Some(query.into());
        req.auth = Auth::Valid;
        encode(&req, &ids())
    }

    fn thruster_path_rejection() -> Normalized {
        normalize(
            400,
            &[
                ("Content-Type".into(), "text/plain; charset=utf-8".into()),
                ("Connection".into(), "close".into()),
            ],
            b"400 Bad Request",
        )
    }

    fn rust_unparsable() -> Normalized {
        normalize(
            400,
            &[
                (
                    "content-type".into(),
                    "application/json; charset=utf-8".into(),
                ),
                ("content-length".into(), "23".into()),
            ],
            br#"{"error":"bad request"}"#,
        )
    }

    #[test]
    fn a_malformed_percent_in_the_path_is_its_own_gap_when_both_sides_match() {
        let target = path_percent("a=%2");
        let rails = thruster_path_rejection();
        let rust = rust_unparsable();
        assert!(compare(&rails, &rust).is_some());
        let reason = gap(&target, &rails, &rust, None).expect("path gap");
        assert!(reason.contains("chemin"));
        assert_eq!(
            documented_form(&target),
            "path:/api/v1/documents/%zz query:a=%2"
        );
        // `%ff` is well-formed and not UTF-8. Puma and Rust both answer the
        // unparsable 400; Thruster still rejects the path first. The
        // distribution label still names that query.
        let ff = path_percent("a=%ff");
        assert!(percent_form(&ff).is_none());
        assert_eq!(
            documented_form(&ff),
            "path:/api/v1/documents/%zz query:a=%ff"
        );
        assert_eq!(
            documented_form(&path_percent("limit=1")),
            "path:/api/v1/documents/%zz"
        );
        assert!(gap(&ff, &rails, &rust, None).is_some());
    }

    #[test]
    fn a_path_percent_does_not_hide_another_rust_answer() {
        let target = path_percent("a=%2");
        let rails = thruster_path_rejection();
        let missing = normalize(
            401,
            &[
                (
                    "content-type".into(),
                    "application/json; charset=utf-8".into(),
                ),
                ("content-length".into(), "32".into()),
                ("x-frame-options".into(), "SAMEORIGIN".into()),
                ("cache-control".into(), "no-cache".into()),
            ],
            br#"{"error":"missing bearer token"}"#,
        );
        assert!(gap(&target, &rails, &missing, None).is_none());
        let with_shield = normalize(
            400,
            &[
                (
                    "content-type".into(),
                    "application/json; charset=utf-8".into(),
                ),
                ("content-length".into(), "23".into()),
                ("x-frame-options".into(), "SAMEORIGIN".into()),
            ],
            br#"{"error":"bad request"}"#,
        );
        assert!(gap(&target, &rails, &with_shield, None).is_none());
        let rails_with_length = normalize(
            400,
            &[
                ("content-type".into(), "text/plain; charset=utf-8".into()),
                ("content-length".into(), "15".into()),
            ],
            b"400 Bad Request",
        );
        assert!(gap(&target, &rails_with_length, &rust_unparsable(), None).is_none());
        // A clean query: Rust follows Puma (401), which is not the unparsable 400.
        let clean = path_percent("limit=1");
        assert!(gap(&clean, &rails, &missing, None).is_none());
        assert!(gap(&clean, &rails, &rust_unparsable(), None).is_none());
    }

    #[test]
    fn an_urlencoded_body_with_a_raw_malformed_percent_is_documented() {
        let req = Request {
            category: "malformed-percent",
            method: "POST",
            route: Route::Documents,
            raw_path: None,
            query: Vec::new(),
            raw_query: None,
            extra_headers: Vec::new(),
            auth: Auth::Valid,
            body: Body::Raw {
                content_type: "application/x-www-form-urlencoded".into(),
                bytes: b"a=%zz&b=1".to_vec(),
            },
        };
        let target = encode(&req, &ids());
        let cleaned = thruster_sanitize(&target).expect("body gap");
        assert_eq!(cleaned.body, b"b=1");
        let rails = normalize(401, &[], br#"{"error":"missing bearer token"}"#);
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert_eq!(percent_form(&target).as_deref(), Some("body:a=%zz&b=1"));
        assert!(gap(&target, &rails, &rust, Some(&rails)).is_some());
        let mismatched = normalize(
            401,
            &[("content-type".into(), "text/plain".into())],
            br#"{"error":"missing bearer token"}"#,
        );
        assert!(gap(&target, &rails, &rust, Some(&mismatched)).is_none());
    }
}
