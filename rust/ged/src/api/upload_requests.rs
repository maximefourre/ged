//! `GET /api/v1/upload_requests/:id`: polling for an async upload, the same
//! payload the callback carries.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::support::{internal, uuid_id};
use crate::auth::AuthenticatedClient;
use crate::domain::app_client::AccessMode;
use crate::domain::upload_request::UploadRequest;
use crate::http::json::RailsJson;
use crate::http::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/upload_requests/{id}", get(show))
}

async fn show(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = uuid_id(&id).ok_or(ApiError::NotFound)?;
    let mut conn = state.db.acquire().await.map_err(internal)?;
    let request = UploadRequest::find(&mut conn, id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)?;
    // a request whose department was trashed is gone (`department or raise`)
    let department_live: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM departments WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(request.department_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(internal)?;
    if !department_live {
        return Err(ApiError::NotFound);
    }
    client
        .authorize_department(&state, request.department_id, AccessMode::Read)
        .await?;
    let document = request.live_document(&mut conn).await.map_err(internal)?;
    Ok(RailsJson(StatusCode::OK, request.payload(document.as_ref())).into_response())
}
