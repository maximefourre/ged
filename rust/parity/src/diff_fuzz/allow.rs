//! Narrow matchers for divergences already written in `docs/migration/parity-gaps.md`.
//!
//! A matcher fires only for the observed Rails behaviour. Anything else stays a
//! failure so a new bug cannot hide behind a documented one.

use super::model::Target;
use super::normalize::Normalized;

/// `Some(reason)` when this difference is an entry in `parity-gaps.md`.
pub fn documented_gap(
    target: &Target,
    rails: &Normalized,
    rust: &Normalized,
) -> Option<&'static str> {
    malformed_percent(target, rails, rust)
}

/// Thruster rewrites a `%` that is not two hex digits and Rails then sees a
/// normal request. The Rust edge applies Rails' own rule and answers
/// `400 {"error":"bad request"}`. See parity-gaps, "Requête non analysable".
fn malformed_percent(
    target: &Target,
    rails: &Normalized,
    rust: &Normalized,
) -> Option<&'static str> {
    if rust.status != 400 || !rust.body.contains("bad request") {
        return None;
    }
    // Both sides already agreed. `compare` would not call us, but keep the
    // matcher from swallowing a shared 400.
    if rails.status == 400 && rails.body == rust.body {
        return None;
    }
    let bad_target = has_malformed_percent(&target.path_and_query);
    let form = target.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
    });
    let bad_body = form && std::str::from_utf8(&target.body).is_ok_and(has_malformed_percent);
    if bad_target || bad_body {
        Some(
            "Thruster supprime un % mal formé ; Rust répond 400 {\"error\":\"bad request\"} (parity-gaps.md, requête non analysable)",
        )
    } else {
        None
    }
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
    use crate::diff_fuzz::model::{Auth, Body, Request, Route, encode};
    use crate::diff_fuzz::normalize::{compare, normalize};

    fn target_with_query(query: &str) -> crate::diff_fuzz::model::Target {
        let mut req = Request::get("malformed-percent", Route::Ping);
        req.raw_query = Some(query.into());
        req.auth = Auth::None;
        let empty: [String; 0] = [];
        encode(
            &req,
            &crate::diff_fuzz::model::Ids {
                seed_doc: "",
                created_docs: &empty,
                created_uploads: &empty,
                token_valid: "",
                token_expired: "",
                token_bad_aud: "",
            },
        )
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
    fn a_thruster_malformed_percent_is_documented() {
        let target = target_with_query("a=%zz");
        let rails = normalize(200, &[], b"<!DOCTYPE html>");
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert!(compare(&rails, &rust).is_some());
        assert!(documented_gap(&target, &rails, &rust).is_some());
    }

    #[test]
    fn a_real_status_mismatch_on_a_clean_request_is_not_documented() {
        let target = target_with_query("limit=1");
        let rails = normalize(200, &[], b"[]");
        let rust = normalize(500, &[], br#"{"error":"bad request"}"#);
        assert!(documented_gap(&target, &rails, &rust).is_none());
    }

    #[test]
    fn a_valid_hex_percent_is_not_the_thruster_gap() {
        let target = target_with_query("a=%ff");
        let rails = normalize(401, &[], br#"{"error":"missing bearer token"}"#);
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert!(documented_gap(&target, &rails, &rust).is_none());
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
        let empty: [String; 0] = [];
        let target = encode(
            &req,
            &crate::diff_fuzz::model::Ids {
                seed_doc: "",
                created_docs: &empty,
                created_uploads: &empty,
                token_valid: "t",
                token_expired: "e",
                token_bad_aud: "b",
            },
        );
        let rails = normalize(401, &[], br#"{"error":"missing bearer token"}"#);
        let rust = normalize(400, &[], br#"{"error":"bad request"}"#);
        assert!(documented_gap(&target, &rails, &rust).is_some());
    }
}
