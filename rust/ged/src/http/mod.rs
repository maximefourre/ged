//! The HTTP edge: router, error contract, parameter parsing.

pub mod error;
pub mod json;
pub mod params;
pub mod rails_headers;
pub mod state;

use std::panic::AssertUnwindSafe;

use axum::Router;
use axum::body::{Body, HttpBody as _};
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::http::{Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::FutureExt;

pub use error::ApiError;
pub use params::{RackParams, RequestParams};
pub use rails_headers::{bare, payload_too_large};
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
        .merge(crate::api::routes())
        .route("/up", get(up))
        // Rails parses the query (and an urlencoded body) before dispatching
        // to any matched route, so a bad one is a 400 even on `/up`. An
        // unmatched path is a plain 404 and never parses anything.
        .route_layer(middleware::from_fn(guard_params))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), shield))
        // Outermost: what Rails' controller layer adds to every answer it
        // produced (and only those: the exceptions app and the edge are `bare`).
        .layer(middleware::from_fn(rails_headers::layer))
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
        bare(ApiError::NotFound.into_response())
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
    bare(response)
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
    let limited = http_body_util::Limited::new(body, params::FORM_BYTES_LIMIT);
    let bytes = match http_body_util::BodyExt::collect(limited).await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return reject_or_unrouted(Request::from_parts(parts, Body::empty()), next, api).await;
        }
    };
    let valid = parses_form_body(bytes.clone()).await;
    if !valid {
        return reject_or_unrouted(Request::from_parts(parts, Body::empty()), next, api).await;
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// Above this size a parse (linear, ~30 ms per MiB in release) moves to the
/// blocking pool so that a 4 MiB pre-auth body never holds an async worker
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

/// Whether Rails could parse this urlencoded body (size limits included).
async fn parses_form_body(body: bytes::Bytes) -> bool {
    if body.len() <= BLOCKING_PARSE_THRESHOLD {
        return RackParams::parse_form_body(&body).is_ok();
    }
    tokio::task::spawn_blocking(move || RackParams::parse_form_body(&body).is_ok())
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
        bare(ApiError::bad_request().into_response())
    } else {
        plain_error(StatusCode::BAD_REQUEST)
    }
}

/// Outermost layer: no panic or framework rejection under `/api/v1` ever
/// reaches the client as anything but the JSON error contract, and no
/// internal detail is ever put in the body.
async fn shield(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let max = state.config.max_upload_bytes;
    // The proxy in front of Rails refuses, before anything else (routing and
    // authentication included), a body announced larger than its limit — on
    // every endpoint, not just the upload.
    let announced_too_large = content_length(req.headers()).is_some_and(|n| n > max);
    let (req, unread, overrun) = share_body(req, max);
    let response = if announced_too_large {
        payload_too_large()
    } else {
        shield_inner(req, next).await
    };
    // A body streamed (no Content-Length) past the limit is cut off at the
    // limit and refused the same way, whatever the handler made of it.
    if overrun.load(std::sync::atomic::Ordering::SeqCst) {
        return payload_too_large();
    }
    drain(unread, drain_limit(max)).await;
    response
}

fn content_length(headers: &axum::http::HeaderMap) -> Option<u64> {
    headers
        .get(axum::http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// How much unread body is read out before giving up: `GED_MAX_UPLOAD_BYTES`,
/// the size the proxy in front of Rails (`MAX_REQUEST_BODY`) accepts at all —
/// beyond it the request is refused (413), so reading further is pointless.
pub fn drain_limit(max_upload_bytes: u64) -> usize {
    usize::try_from(max_upload_bytes).unwrap_or(usize::MAX)
}

/// The request body, shared so that [`drain`] can finish reading what a
/// handler left unread. Answering a request early (a 404 on a path that reads
/// nothing, say) while the client is still sending a large body would reset
/// the connection under it; the proxy in front of Rails reads it out, and so
/// does this.
type SharedBody = std::sync::Arc<std::sync::Mutex<Body>>;

/// Reads the shared body for the handler, counting what it takes: past `max`
/// bytes the read fails and `overrun` is raised (see [`shield`]).
struct Shared {
    body: SharedBody,
    read: u64,
    max: u64,
    overrun: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl axum::body::HttpBody for Shared {
    type Data = bytes::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let polled = {
            let mut body = this.body.lock().unwrap_or_else(|e| e.into_inner());
            std::pin::Pin::new(&mut *body).poll_frame(cx)
        };
        if let std::task::Poll::Ready(Some(Ok(frame))) = &polled {
            this.read += frame.data_ref().map_or(0, |d| d.len() as u64);
            if this.read > this.max {
                this.overrun
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return std::task::Poll::Ready(Some(Err(axum::Error::new("body too large"))));
            }
        }
        polled
    }
}

fn share_body(
    req: Request,
    max: u64,
) -> (
    Request,
    SharedBody,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (parts, body) = req.into_parts();
    let shared: SharedBody = std::sync::Arc::new(std::sync::Mutex::new(body));
    let overrun = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let counted = Shared {
        body: shared.clone(),
        read: 0,
        max,
        overrun: overrun.clone(),
    };
    (
        Request::from_parts(parts, Body::new(counted)),
        shared,
        overrun,
    )
}

/// Longest wait spent reading out an unread body (the byte limit is
/// [`drain_limit`]).
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn drain(unread: SharedBody, max_bytes: usize) {
    let read_out = async {
        let mut total = 0usize;
        loop {
            let frame = std::future::poll_fn(|cx| {
                let mut body = unread.lock().unwrap_or_else(|e| e.into_inner());
                std::pin::Pin::new(&mut *body).poll_frame(cx)
            })
            .await;
            match frame {
                Some(Ok(frame)) => {
                    total += frame.data_ref().map_or(0, |d| d.len());
                    if total > max_bytes {
                        break;
                    }
                }
                _ => break,
            }
        }
    };
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, read_out).await;
}

async fn shield_inner(req: Request, next: Next) -> Response {
    // The web server in front of Rails refuses an over-long query before any
    // routing: an empty 400 with no content type, whatever path or verb.
    if req
        .uri()
        .query()
        .is_some_and(|q| q.len() > params::QUERY_BYTES_LIMIT)
    {
        return bare(StatusCode::BAD_REQUEST.into_response());
    }
    let api = is_api(req.uri());
    let response = match AssertUnwindSafe(next.run(req)).catch_unwind().await {
        Ok(response) => response,
        Err(_) => {
            return if api {
                bare(ApiError::Internal.into_response())
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
            bare(ApiError::NotFound.into_response())
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
        return bare(error::json_error_for_status(status));
    }
    response
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn counted_body(chunks: usize, read: Arc<AtomicUsize>) -> SharedBody {
        let stream = futures_util::stream::iter((0..chunks).map(move |_| {
            read.fetch_add(1024, Ordering::SeqCst);
            Ok::<_, std::io::Error>(bytes::Bytes::from(vec![0u8; 1024]))
        }));
        Arc::new(Mutex::new(Body::from_stream(stream)))
    }

    #[tokio::test]
    async fn drain_stops_at_the_configured_limit() {
        let read = Arc::new(AtomicUsize::new(0));
        drain(counted_body(1000, read.clone()), 10 * 1024).await;
        let got = read.load(Ordering::SeqCst);
        assert!((10 * 1024..=12 * 1024).contains(&got), "read {got} bytes");
    }

    #[tokio::test]
    async fn drain_reads_a_small_body_to_the_end() {
        let read = Arc::new(AtomicUsize::new(0));
        drain(counted_body(5, read.clone()), 1 << 20).await;
        assert_eq!(read.load(Ordering::SeqCst), 5 * 1024);
    }

    #[test]
    fn the_limit_is_the_upload_limit() {
        assert_eq!(drain_limit(52_428_800), 52_428_800);
        assert_eq!(drain_limit(1024), 1024);
        assert_eq!(drain_limit(u64::MAX), usize::MAX);
    }
}
