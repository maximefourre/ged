//! The API contract for humans and tooling: `ApiDocsController`. Both
//! endpoints are deliberately public (no bearer token): the contract is not
//! secret, and the real `/api/v1` endpoints stay protected.

use axum::Router;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::http::AppState;

/// `docs/api/openapi.yaml`, byte for byte: embedded, so the binary does not
/// need the repository next to it.
pub const SPEC: &str = include_str!("../../../../docs/api/openapi.yaml");

/// Redoc renders the spec above. The bundle loads from jsdelivr
/// (major-pinned), so the page needs internet reachable from the viewer's
/// browser; the raw spec endpoint does not.
pub const REDOC_PAGE: &str = r#"<!DOCTYPE html>
<html>
  <head>
    <title>GED API — Reference</title>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1"/>
    <style>body { margin: 0; padding: 0; }</style>
  </head>
  <body>
    <redoc spec-url="/api/v1/openapi.yaml"></redoc>
    <script src="https://cdn.jsdelivr.net/npm/redoc@2/bundles/redoc.standalone.js"></script>
  </body>
</html>
"#;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/openapi.yaml", get(spec))
        .route("/api/docs", get(page))
}

async fn spec() -> Response {
    typed("application/yaml; charset=utf-8", SPEC)
}

async fn page() -> Response {
    typed("text/html; charset=utf-8", REDOC_PAGE)
}

fn typed(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
