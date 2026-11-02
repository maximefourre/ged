//! `GET /api/v1/ping`: who the token says you are.

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::json;

use crate::auth::AuthenticatedClient;
use crate::http::AppState;
use crate::http::json::RailsJson;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/ping", get(show))
}

async fn show(client: AuthenticatedClient) -> impl IntoResponse {
    RailsJson(
        StatusCode::OK,
        json!({ "client": client.client().client_identifier }),
    )
}
