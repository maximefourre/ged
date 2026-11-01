//! JSON responses serialized like Rails' `render json:`.
//!
//! ActiveSupport escapes `<`, `>`, `&`, U+2028 and U+2029 inside strings as
//! `<`, `>`, `&`, ` `, ` ` (HTML-safe JSON). They can
//! only occur inside string literals, so a replace on the serialized text is
//! exact.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::error::JSON_CONTENT_TYPE;

/// Serializes `value` the way Rails does.
pub fn to_rails_json<T: Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    let raw = serde_json::to_string(value)?;
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// A JSON response with the given status.
pub struct RailsJson<T>(pub StatusCode, pub T);

impl<T: Serialize> IntoResponse for RailsJson<T> {
    fn into_response(self) -> Response {
        match to_rails_json(&self.1) {
            Ok(body) => {
                let mut response = (self.0, body).into_response();
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(JSON_CONTENT_TYPE),
                );
                response
            }
            // Serializing our own types cannot fail; never leak why if it does.
            Err(_) => super::error::ApiError::Internal.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn escapes_like_active_support() {
        let input = format!("a<b>&c{}d{}", '\u{2028}', '\u{2029}');
        let body = to_rails_json(&json!({"error": input})).unwrap();
        let want = format!(
            "{{\"error\":\"a{bs}u003cb{bs}u003e{bs}u0026c{bs}u2028d{bs}u2029\"}}",
            bs = '\\'
        );
        assert_eq!(body, want);
    }

    #[test]
    fn leaves_everything_else_alone() {
        let body = to_rails_json(&json!({"n": 1, "s": "é\"\\", "a": [null, true]})).unwrap();
        assert_eq!(body, r#"{"n":1,"s":"é\"\\","a":[null,true]}"#);
    }
}
