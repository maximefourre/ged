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

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(ping::routes())
        .merge(docs::routes())
        .merge(documents::routes())
        .merge(tags::routes())
        .merge(departments::routes())
        .merge(upload_requests::routes())
}
