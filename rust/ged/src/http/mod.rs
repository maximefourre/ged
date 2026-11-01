//! The HTTP edge: router, error contract, parameter parsing.

pub mod error;
pub mod json;
pub mod params;
pub mod state;

use std::panic::AssertUnwindSafe;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::http::{Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::FutureExt;

pub use error::ApiError;
pub use params::{RackParams, RequestParams};
pub use state::AppState;

/// Everything under this prefix answers errors as `{ "error": … }`.
pub const API_PREFIX: &str = "/api/v1";

fn is_api(uri: &Uri) -> bool {
    uri.path().starts_with(API_PREFIX)
}

/// Builds the HTTP router.
pub fn router(state: AppState) -> Router {
    router_with(state, Router::new())
}

/// [`router`] plus extra routes that get the same guards and error shaping
/// (how tests exercise the edge with probe handlers).
pub fn router_with(state: AppState, routes: Router<AppState>) -> Router {
    routes
        .route("/up", get(up))
        // Rails parses the query (and an urlencoded body) before dispatching
        // to any matched route, so a bad one is a 400 even on `/up`. An
        // unmatched path is a plain 404 and never parses anything.
        .route_layer(middleware::from_fn(guard_params))
        .fallback(not_found)
        .layer(middleware::from_fn(shield))
        .with_state(state)
}

/// `Rails::HealthController#show`: the green page.
async fn up() -> Response {
    let mut response = (
        StatusCode::OK,
        "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>",
    )
        .into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
}

/// Unknown route. Rails raises `RoutingError`, which its exceptions app
/// renders as JSON under `/api/v1` (any method) and as the static page
/// elsewhere.
async fn not_found(uri: Uri) -> Response {
    if is_api(&uri) {
        ApiError::NotFound.into_response()
    } else {
        plain_error(StatusCode::NOT_FOUND)
    }
}

/// Outside `/api/v1` the Rails error pages are static HTML; the admin keeps
/// to status and visible wording, so a minimal page is enough.
fn plain_error(status: StatusCode) -> Response {
    let reason = status.canonical_reason().unwrap_or("Error");
    let mut response = (
        status,
        format!("<!DOCTYPE html><html><head><title>{reason}</title></head><body><h1>{reason}</h1></body></html>"),
    )
        .into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=UTF-8"),
    );
    response
}

/// Rejects a request whose query string — or urlencoded form body — Rails
/// cannot parse, before the handler runs.
async fn guard_params(req: Request, next: Next) -> Response {
    let api = is_api(req.uri());
    if !parses(req.uri().query().unwrap_or("").to_owned()).await {
        return reject_or_unrouted(req, next, api).await;
    }

    let kind = params::BodyKind::of(req.method(), req.headers().get(CONTENT_TYPE));
    if kind != params::BodyKind::Form {
        return next.run(req).await;
    }
    // Buffer the form to validate it, then hand the same bytes on.
    let (parts, body) = req.into_parts();
    let limited = http_body_util::Limited::new(body, params::BODY_LIMIT);
    let bytes = match http_body_util::BodyExt::collect(limited).await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return reject_or_unrouted(Request::from_parts(parts, Body::empty()), next, api).await;
        }
    };
    let valid = match String::from_utf8(bytes.to_vec()) {
        Ok(text) => parses(text).await,
        Err(_) => false,
    };
    if !valid {
        return reject_or_unrouted(Request::from_parts(parts, Body::empty()), next, api).await;
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// Above this size a parse (linear, ~30 ms per MiB in release) moves to the
/// blocking pool so that an 8 MiB pre-auth body never holds an async worker
/// thread for hundreds of milliseconds.
const BLOCKING_PARSE_THRESHOLD: usize = 64 * 1024;

/// Whether Rails could parse these urlencoded parameters.
async fn parses(text: String) -> bool {
    if text.len() <= BLOCKING_PARSE_THRESHOLD {
        return RackParams::parse(&text).is_ok();
    }
    // A failed join (the parse panicked) is not "valid": fail closed.
    tokio::task::spawn_blocking(move || RackParams::parse(&text).is_ok())
        .await
        .unwrap_or(false)
}

/// The request's parameters are unparsable. Rails only parses them once the
/// route *and verb* matched, so on a verb the route does not serve the answer
/// is the routing miss (405 here, which [`shield`] turns into the 404), not a
/// 400. Dispatching a method nothing serves makes the router answer 405
/// without ever running a handler, which tells the two cases apart.
async fn reject_or_unrouted(req: Request, next: Next, api: bool) -> Response {
    let (parts, _) = req.into_parts();
    let unserved = Method::from_bytes(b"GED-PROBE").expect("static token is a valid method");
    let mut probe = Request::new(Body::empty());
    *probe.method_mut() = unserved;
    *probe.uri_mut() = parts.uri;
    let response = next.run(probe).await;
    if response.status() == StatusCode::METHOD_NOT_ALLOWED {
        // The verb in use is served only if it is in `Allow`.
        let served = response
            .headers()
            .get(axum::http::header::ALLOW)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|allow| allow.split(',').any(|m| m.trim() == parts.method.as_str()));
        if !served {
            return response;
        }
    }
    if api {
        ApiError::bad_request().into_response()
    } else {
        plain_error(StatusCode::BAD_REQUEST)
    }
}

/// Outermost layer: no panic or framework rejection under `/api/v1` ever
/// reaches the client as anything but the JSON error contract, and no
/// internal detail is ever put in the body.
async fn shield(req: Request, next: Next) -> Response {
    let api = is_api(req.uri());
    let response = match AssertUnwindSafe(next.run(req)).catch_unwind().await {
        Ok(response) => response,
        Err(_) => {
            return if api {
                ApiError::Internal.into_response()
            } else {
                plain_error(StatusCode::INTERNAL_SERVER_ERROR)
            };
        }
    };
    let status = response.status();
    // A known path with the wrong verb is a routing miss to Rails: 404,
    // never 405, on every path.
    if status == StatusCode::METHOD_NOT_ALLOWED {
        return if api {
            ApiError::NotFound.into_response()
        } else {
            plain_error(StatusCode::NOT_FOUND)
        };
    }
    if !api {
        return response;
    }
    // 413 keeps its empty body: that is the contract observed on the stack
    // (docs/migration/parity-gaps.md).
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if (status.is_client_error() || status.is_server_error())
        && !is_json
        && status != StatusCode::PAYLOAD_TOO_LARGE
    {
        return error::json_error_for_status(status);
    }
    response
}
