//! `/api/v1/departments`: port of `Api::V1::DepartmentsController`.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::Utc;
use serde_json::json;
use sqlx::Postgres;

use super::documents::find_department;
use super::support::{blank, id_param, internal, pagination_window, require_scalar_param};
use crate::auth::AuthenticatedClient;
use crate::domain::app_client::AccessMode;
use crate::http::json::RailsJson;
use crate::http::params::RequestParams;
use crate::http::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/departments", get(index).post(create))
}

/// A bare array (no envelope), paginated by `limit`/`offset`.
async fn index(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    RequestParams(params): RequestParams,
) -> Result<Response, ApiError> {
    let ids = client
        .client()
        .accessible_department_ids(&state.db, AccessMode::Read)
        .await
        .map_err(internal)?;
    let (limit, offset) = pagination_window(&params);
    let rows: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
        // `order(:name)` only. A secondary `id` sorts ties Rails leaves alone,
        // and the two lists then disagree (observed 2026-11-11, two departments
        // both named "Fuzz Payroll").
        "SELECT id, name, parent_id FROM departments \
         WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY name LIMIT $2 OFFSET $3",
    )
    .bind(&ids)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    let body: Vec<_> = rows
        .into_iter()
        .map(|(id, name, parent_id)| json!({ "id": id, "name": name, "parent_id": parent_id }))
        .collect();
    Ok(RailsJson(StatusCode::OK, body).into_response())
}

/// Creates a sub-department. Top-level ones are an admin's job.
async fn create(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    RequestParams(params): RequestParams,
) -> Result<Response, ApiError> {
    let parent_id = id_param(&params, "parent_id")?.filter(|id| !blank(id));
    let Some(parent_id) = parent_id else {
        return Err(ApiError::BadRequest(
            "parent_id is required (top-level departments are created by an admin)".into(),
        ));
    };
    let parent = find_department(&state, &parent_id).await?;
    client
        .authorize_department(&state, parent.id, AccessMode::Write)
        .await?;
    let name = require_scalar_param(&params, "name")?;

    let taken = || ApiError::Unprocessable("Name has already been taken".into());
    // The uniqueness check also sees trashed siblings (a restore must not
    // collide), and the unique indexes catch a concurrent create that slips
    // past it: either way the answer is the 422, never a 500.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM departments WHERE parent_id = $1 AND name = $2)",
    )
    .bind(parent.id)
    .bind(name)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;
    if exists {
        return Err(taken());
    }
    let now = Utc::now().naive_utc();
    let inserted = sqlx::query_scalar::<Postgres, i64>(
        "INSERT INTO departments (name, parent_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $3) RETURNING id",
    )
    .bind(name)
    .bind(parent.id)
    .bind(now)
    .fetch_one(&state.db)
    .await;
    match inserted {
        Ok(id) => Ok(RailsJson(
            StatusCode::CREATED,
            json!({ "id": id, "name": name, "parent_id": parent.id }),
        )
        .into_response()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(taken()),
        Err(e) => Err(internal(e)),
    }
}
