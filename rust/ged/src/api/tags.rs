//! `PUT|PATCH /api/v1/documents/:document_id/tags`: replaces a document's
//! whole tag map (the client owns the full map; omitting `tags` clears it).

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use chrono::Utc;
use serde_json::json;

use super::documents::find_authorized_document;
use super::support::{document_json, internal, tags_param};
use crate::auth::AuthenticatedClient;
use crate::domain::app_client::AccessMode;
use crate::domain::document::{COLUMNS, Document, sql};
use crate::domain::tags;
use crate::http::json::RailsJson;
use crate::http::params::RequestParams;
use crate::http::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/documents/{document_id}/tags",
        put(update).patch(update),
    )
}

async fn update(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    Path(document_id): Path<String>,
    RequestParams(params): RequestParams,
) -> Result<Response, ApiError> {
    let document =
        find_authorized_document(&state, &client, &document_id, AccessMode::Write).await?;
    let new_tags = tags::normalize(tags_param(&params).unwrap_or_else(|| json!({})));
    let errors = tags::validate(&new_tags);
    if !errors.is_empty() {
        return Err(ApiError::Unprocessable(
            tags::full_messages(&errors).join(", "),
        ));
    }
    // like `update!`: nothing is written (and `updated_at` stays) when the
    // map is unchanged
    if new_tags == document.tags {
        return Ok(RailsJson(StatusCode::OK, document_json(&document)).into_response());
    }
    let updated: Option<Document> = sqlx::query_as(sql(&format!(
        "UPDATE documents SET tags = $2, updated_at = $3 \
         WHERE id = $1 AND deleted_at IS NULL RETURNING {COLUMNS}"
    )))
    .bind(document.id)
    .bind(&new_tags)
    .bind(Utc::now().naive_utc())
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;
    let updated = updated.ok_or(ApiError::NotFound)?;
    Ok(RailsJson(StatusCode::OK, document_json(&updated)).into_response())
}
