//! What Rails adds to every response a *controller* produces, observed on the
//! stack (2026-11-03) and ported as one layer:
//!
//! * `ActionDispatch::Response` default headers (Rails 8.1,
//!   `load_defaults 7.0+`): `X-Frame-Options`, `X-XSS-Protection`,
//!   `X-Content-Type-Options`, `X-Permitted-Cross-Domain-Policies`,
//!   `Referrer-Policy`. No global CSP, no HSTS.
//! * `Rack::ETag` (`"no-cache"` flavour, as Rails stacks it): a 200/201 with an
//!   in-memory body gets `ETag: W/"<first 32 hex of SHA-256(body)>"` and
//!   `Cache-Control: max-age=0, private, must-revalidate`; any response left
//!   without `Cache-Control` gets `no-cache`.
//! * `Rack::ConditionalGet`: on GET/HEAD, a 200 whose ETag equals
//!   `If-None-Match` verbatim becomes an empty 304 (no Content-Type/Length).
//!
//! Responses that never went through a Rails controller — the exceptions app
//! (routing miss, unparsable params, crash), `public/*.html`, and the edge's
//! own answers (413, over-long query) — carry none of it: they are marked
//! with [`bare`] and pass through untouched.

use axum::body::{Body, HttpBody as _};
use axum::extract::Request;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH, LAST_MODIFIED,
};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// Response extension: not produced by a Rails controller, leave it alone.
#[derive(Clone, Copy, Debug)]
pub struct Bare;

/// The body is already the client-facing answer. The API shield must not
/// replace an empty 500 with the JSON error contract. Rack's multipart part
/// limits escape the app; Thruster answers 500 with an empty body.
#[derive(Clone, Copy, Debug)]
pub struct PreserveBody;

/// Marks `response` as one Rails' controller layer would not have touched.
pub fn bare(mut response: Response) -> Response {
    response.extensions_mut().insert(Bare);
    response
}

/// The edge's refusal of an oversized body: empty 413, nothing else.
pub fn payload_too_large() -> Response {
    bare(StatusCode::PAYLOAD_TOO_LARGE.into_response())
}

const SECURITY_HEADERS: [(&str, &str); 5] = [
    ("x-frame-options", "SAMEORIGIN"),
    ("x-xss-protection", "0"),
    ("x-content-type-options", "nosniff"),
    ("x-permitted-cross-domain-policies", "none"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
];

const REVALIDATE: &str = "max-age=0, private, must-revalidate";

/// Mime type of a Content-Type value, the part before `;`.
fn media_type(value: &str) -> Option<&str> {
    let media = value.split(';').next().unwrap_or("").trim();
    if media.is_empty() { None } else { Some(media) }
}

fn default_content_type(response: &mut Response) {
    // Rails deletes the header again for these (`handle_no_content!`).
    if matches!(
        response.status().as_u16(),
        100 | 101 | 102 | 103 | 204 | 205 | 304
    ) {
        return;
    }
    let raw = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if media_type(raw).is_some() {
        return;
    }
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
}

pub async fn layer(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let if_none_match = req.headers().get(IF_NONE_MATCH).cloned();
    let response = next.run(req).await;
    if response.extensions().get::<Bare>().is_some() {
        return response;
    }
    decorate(response, &method, if_none_match.as_ref()).await
}

async fn decorate(
    mut response: Response,
    method: &Method,
    if_none_match: Option<&HeaderValue>,
) -> Response {
    // `assign_default_content_type_and_charset!`, observed 2026-11-11: a
    // response with no media type (missing header, or `send_file` of an empty
    // type) is committed as `text/html; charset=utf-8`. 204/304 drop it later.
    default_content_type(&mut response);
    for (name, value) in SECURITY_HEADERS {
        response
            .headers_mut()
            .entry(HeaderName::from_static(name))
            .or_insert(HeaderValue::from_static(value));
    }

    let mut digested = false;
    let status = response.status();
    let settled =
        response.headers().contains_key(ETAG) || response.headers().contains_key(LAST_MODIFIED);
    // Only a body held in memory (exact size known) is digested; a stream
    // (file download) never is, as `send_file` is not either.
    if matches!(status.as_u16(), 200 | 201)
        && !settled
        && response.body().size_hint().exact().is_some()
    {
        let (parts, body) = response.into_parts();
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        response = Response::from_parts(parts, Body::from(bytes.clone()));
        if !bytes.is_empty() {
            let digest = Sha256::digest(&bytes);
            let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
            if let Ok(value) = HeaderValue::from_str(&format!("W/\"{hex}\"")) {
                response.headers_mut().insert(ETAG, value);
                digested = true;
            }
        }
    }

    if !response.headers().contains_key(CACHE_CONTROL) {
        let value = if digested { REVALIDATE } else { "no-cache" };
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(value));
    }

    let readable = matches!(*method, Method::GET | Method::HEAD);
    let fresh = status == StatusCode::OK
        && readable
        && matches!((if_none_match, response.headers().get(ETAG)), (Some(a), Some(b)) if a == b);
    if fresh {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response.headers_mut().remove(CONTENT_TYPE);
        response.headers_mut().remove(CONTENT_LENGTH);
        *response.body_mut() = Body::empty();
    }
    // Rack::Head, observed 2026-11-11 on controller responses (`HEAD /api/v1/ping`,
    // both 200 and 401): the headers of the GET stay, including the ETag, the
    // body is not sent, and Content-Length is 0. Bare responses (the exceptions
    // app) never reach here, and keep the length of the body they would have sent.
    if *method == Method::HEAD && response.status() != StatusCode::NOT_MODIFIED {
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
        *response.body_mut() = Body::empty();
    }
    response
}
