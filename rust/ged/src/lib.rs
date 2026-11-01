use axum::{Router, http::StatusCode, routing::get};

/// Builds the HTTP router.
pub fn router() -> Router {
    Router::new().route("/up", get(up))
}

async fn up() -> StatusCode {
    StatusCode::OK
}
