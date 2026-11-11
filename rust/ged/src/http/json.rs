//! JSON responses serialized like Rails 8.1 `render json:`.
//!
//! `config.load_defaults 8.1` sets `action_controller.escape_json_responses`
//! to false, so a controller calls `to_json(escape: false)`. Observed
//! 2026-11-11 in the Rails process: `<`, `>`, `&`, U+2028 and U+2029 stay as
//! themselves (`{"name":"a<b>&c"}.to_json(escape: false)`). `{}.to_json`
//! without that option still rewrites `<`, `>`, `&`; the exceptions app uses
//! it, and its messages are fixed status phrases, so the bytes match.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::error::JSON_CONTENT_TYPE;

/// Serializes `value` the way Rails does.
pub fn to_rails_json<T: Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
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
    fn render_json_leaves_html_and_line_separators() {
        // Rails 8.1 `render json:` is `to_json(escape: false)`. Observed
        // 2026-11-11: `<`, `>`, `&`, U+2028 and U+2029 are not rewritten.
        let input = format!("a<b>&c{}d{}", '\u{2028}', '\u{2029}');
        let body = to_rails_json(&json!({"error": input})).unwrap();
        let want = format!("{{\"error\":\"{input}\"}}");
        assert_eq!(body, want);
    }

    #[test]
    fn leaves_everything_else_alone() {
        let body = to_rails_json(&json!({"n": 1, "s": "é\"\\", "a": [null, true]})).unwrap();
        assert_eq!(body, r#"{"n":1,"s":"é\"\\","a":[null,true]}"#);
    }
}
