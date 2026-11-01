//! The API error contract: every error under `/api/v1` is
//! `{ "error": "<message>" }` with `application/json; charset=utf-8`.
//!
//! Messages are either chosen by the handler (the exact Rails strings such as
//! `"missing bearer token"`) or derived from the status like Rails'
//! `JsonApiExceptions` does: `Rack::Utils::HTTP_STATUS_CODES[status].downcase`.
//! The raw text of an internal error is never put in a response.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::json::RailsJson;

/// Content type of every JSON response, as Rails sends it.
pub const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Status 400: handler-chosen text, or `"bad request"` for framework-level
    /// rejections (malformed JSON, invalid query encoding, …).
    BadRequest(String),
    /// 401.
    Unauthorized(String),
    /// 403.
    Forbidden(String),
    /// 404 `"not found"`.
    NotFound,
    /// 422.
    Unprocessable(String),
    /// 500 `"internal server error"`. Carries no detail on purpose.
    Internal,
}

impl ApiError {
    /// The generic 400 Rails renders for requests it cannot parse.
    pub fn bad_request() -> Self {
        Self::BadRequest(status_message(StatusCode::BAD_REQUEST).into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest(m)
            | Self::Unauthorized(m)
            | Self::Forbidden(m)
            | Self::Unprocessable(m) => m,
            Self::NotFound => status_message(StatusCode::NOT_FOUND),
            Self::Internal => status_message(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status().as_u16(), self.message())
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        json_error(self.status(), self.message())
    }
}

/// `{ "error": message }` with the given status.
pub fn json_error(status: StatusCode, message: &str) -> Response {
    RailsJson(status, json!({ "error": message })).into_response()
}

/// The JSON error Rails renders for a bare status (`JsonApiExceptions`).
pub fn json_error_for_status(status: StatusCode) -> Response {
    json_error(status, status_message(status))
}

/// `Rack::Utils::HTTP_STATUS_CODES[status].downcase` (Rack 3.2), `"error"`
/// for codes Rack does not name.
pub fn status_message(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad request",
        401 => "unauthorized",
        402 => "payment required",
        403 => "forbidden",
        404 => "not found",
        405 => "method not allowed",
        406 => "not acceptable",
        407 => "proxy authentication required",
        408 => "request timeout",
        409 => "conflict",
        410 => "gone",
        411 => "length required",
        412 => "precondition failed",
        413 => "content too large",
        414 => "uri too long",
        415 => "unsupported media type",
        416 => "range not satisfiable",
        417 => "expectation failed",
        421 => "misdirected request",
        422 => "unprocessable content",
        423 => "locked",
        424 => "failed dependency",
        425 => "too early",
        426 => "upgrade required",
        428 => "precondition required",
        429 => "too many requests",
        431 => "request header fields too large",
        451 => "unavailable for legal reasons",
        500 => "internal server error",
        501 => "not implemented",
        502 => "bad gateway",
        503 => "service unavailable",
        504 => "gateway timeout",
        505 => "http version not supported",
        506 => "variant also negotiates",
        507 => "insufficient storage",
        508 => "loop detected",
        511 => "network authentication required",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use http_body_util::BodyExt;

    async fn body_of(response: Response) -> (StatusCode, String, String) {
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            content_type,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    #[tokio::test]
    async fn renders_the_error_contract() {
        let cases = [
            (ApiError::bad_request(), 400, "bad request"),
            (
                ApiError::BadRequest("limit must be a string".into()),
                400,
                "limit must be a string",
            ),
            (
                ApiError::Unauthorized("missing bearer token".into()),
                401,
                "missing bearer token",
            ),
            (
                ApiError::Forbidden("client not registered".into()),
                403,
                "client not registered",
            ),
            (ApiError::NotFound, 404, "not found"),
            (
                ApiError::Unprocessable("Name can't be blank".into()),
                422,
                "Name can't be blank",
            ),
            (ApiError::Internal, 500, "internal server error"),
        ];
        for (error, status, message) in cases {
            let (got_status, content_type, body) = body_of(error.into_response()).await;
            assert_eq!(got_status.as_u16(), status);
            assert_eq!(content_type, "application/json; charset=utf-8");
            assert_eq!(
                body,
                crate::http::json::to_rails_json(&json!({ "error": message })).unwrap()
            );
        }
    }

    #[test]
    fn bare_statuses_use_rack_names() {
        assert_eq!(
            status_message(StatusCode::PAYLOAD_TOO_LARGE),
            "content too large"
        );
        assert_eq!(
            status_message(StatusCode::UNPROCESSABLE_ENTITY),
            "unprocessable content"
        );
        assert_eq!(status_message(StatusCode::from_u16(599).unwrap()), "error");
    }
}
