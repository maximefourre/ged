//! `/api/v1`.

pub mod departments;
pub mod docs;
pub mod documents;
pub mod multipart;
pub mod ping;
pub mod support;
pub mod tags;
pub mod upload_requests;

use axum::Router;

use crate::http::AppState;

/// Every `/api/v1` path [`routes`] registers, including the public OpenAPI
/// document. `guards_api_docs` requires this table to match the `.route("…")`
/// literals under `src/` and the OpenAPI document. A path built with
/// `concat!`, `format!`, or `.nest` cannot be added here: the guard rejects
/// those forms.
pub const V1_PATHS: &[&str] = &[
    "/api/v1/departments",
    "/api/v1/documents",
    "/api/v1/documents/bulk_destroy",
    "/api/v1/documents/{document_id}/tags",
    "/api/v1/documents/{id}",
    "/api/v1/documents/{id}/download",
    "/api/v1/openapi.yaml",
    "/api/v1/ping",
    "/api/v1/upload_requests/{id}",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(ping::routes())
        .merge(docs::routes())
        .merge(documents::routes())
        .merge(tags::routes())
        .merge(departments::routes())
        .merge(upload_requests::routes())
}
