//! `/api/v1`.

pub mod ping;

use axum::Router;

use crate::http::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().merge(ping::routes())
}
