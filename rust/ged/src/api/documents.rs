//! `/api/v1/documents`: port of `Api::V1::DocumentsController`.

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequest, FromRequestParts, Path, Request, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use chrono::{NaiveDateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::multipart::{self, Intake, IntakeError};
use super::support::{
    blank, document_json, id_param_text, integer_id, internal, pagination_window, param_blank,
    parameter_missing, parse_iso8601, present_param, require_scalar_param, tags_param, uuid_id,
};
use crate::auth::AuthenticatedClient;
use crate::domain::app_client::AccessMode;
use crate::domain::department::Department;
use crate::domain::document::{COLUMNS, Document};
use crate::domain::document_creator::{CreateError, DocumentCreator, NewDocument};
use crate::domain::tags::{ruby_downcase, ruby_strip};
use crate::domain::upload_request::{self, NewUploadRequest, UploadRequest};
use crate::http::json::RailsJson;
use crate::http::params::{ParamValue, RackParams, RequestParams};
use crate::http::{ApiError, AppState};
use crate::jobs::queue::{self, JobPayload, QueueName};
use crate::storage::disposition::content_disposition;
use crate::telemetry::{self, UploadMode};

/// Most ids one `bulk_destroy` call may carry.
pub const BULK_MAX: usize = 500;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/documents", get(list).post(create))
        // `GET …/bulk_destroy` is a `show` of the id "bulk_destroy" to Rails:
        // a 404, but only after authentication.
        .route(
            "/api/v1/documents/bulk_destroy",
            delete(bulk_destroy).get(show_bulk_destroy_name),
        )
        .route("/api/v1/documents/{id}", get(show).delete(destroy))
        .route("/api/v1/documents/{id}/download", get(download))
}

// --- finders ---------------------------------------------------------------

/// `Document.find(id)`: live documents only, ids cast like the PG adapter.
async fn find_document(state: &AppState, id: &str) -> Result<Document, ApiError> {
    let uuid = uuid_id(id).ok_or(ApiError::NotFound)?;
    let mut conn = state.db.acquire().await.map_err(internal)?;
    Document::find(&mut conn, uuid)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)
}

/// `find_authorized_document(mode:)`.
pub(super) async fn find_authorized_document(
    state: &AppState,
    client: &AuthenticatedClient,
    id: &str,
    mode: AccessMode,
) -> Result<Document, ApiError> {
    let document = find_document(state, id).await?;
    client
        .authorize_department(state, document.department_id, mode)
        .await?;
    Ok(document)
}

/// `Department.find(id)`: a live department, id cast with `to_i`.
pub(super) async fn find_department(state: &AppState, id: &str) -> Result<Department, ApiError> {
    Department::find(&state.db, integer_id(id))
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)
}

// --- index -------------------------------------------------------------------

struct Filters {
    departments: Vec<i64>,
    filename: Option<String>,
    tag: Option<(String, Option<String>)>,
    uploaded_by: Option<String>,
    stored_async: Option<bool>,
    uploaded: (Option<NaiveDateTime>, Option<NaiveDateTime>),
    indexed: (Option<NaiveDateTime>, Option<NaiveDateTime>),
    updated: (Option<NaiveDateTime>, Option<NaiveDateTime>),
}

impl Filters {
    fn push_where(&self, b: &mut QueryBuilder<Postgres>) {
        b.push(" WHERE deleted_at IS NULL AND department_id = ANY(");
        b.push_bind(self.departments.clone());
        b.push(")");
        if let Some(name) = &self.filename {
            // sanitize_sql_like: escape the LIKE metacharacters with `\`
            let escaped = name
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            b.push(" AND filename ILIKE ");
            b.push_bind(format!("%{escaped}%"));
        }
        match &self.tag {
            Some((key, Some(value))) => {
                b.push(" AND tags @> ");
                b.push_bind(json!({ key: value }));
            }
            Some((key, None)) => {
                // `?` is the jsonb key-exists operator (GIN-served)
                b.push(" AND tags ? ");
                b.push_bind(key.clone());
            }
            None => {}
        }
        if let Some(owner) = &self.uploaded_by {
            b.push(" AND uploaded_by = ");
            b.push_bind(owner.clone());
        }
        if let Some(flag) = self.stored_async {
            b.push(" AND stored_async = ");
            b.push_bind(flag);
        }
        for (column, (after, before)) in [
            ("uploaded_at", &self.uploaded),
            ("created_at", &self.indexed),
            ("updated_at", &self.updated),
        ] {
            if let Some(t) = after {
                b.push(format!(" AND {column} >= "));
                b.push_bind(*t);
            }
            if let Some(t) = before {
                b.push(format!(" AND {column} <= "));
                b.push_bind(*t);
            }
        }
    }
}

fn time_param(params: &RackParams, key: &str) -> Result<Option<NaiveDateTime>, ApiError> {
    match present_param(params, key)? {
        None => Ok(None),
        Some(value) => parse_iso8601(value)
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest(format!("{key} must be an ISO8601 datetime"))),
    }
}

async fn list(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    RequestParams(params): RequestParams,
) -> Result<Response, ApiError> {
    let mut departments = client
        .client()
        .accessible_department_ids(&state.db, AccessMode::Read)
        .await
        .map_err(internal)?;
    if let Some(id) = present_param(&params, "department_id")? {
        let department = find_department(&state, id).await?;
        client
            .authorize_department(&state, department.id, AccessMode::Read)
            .await?;
        departments = department.subtree_ids(&state.db).await.map_err(internal)?;
    }
    let filename = present_param(&params, "filename")?.map(str::to_owned);
    let tag = if let Some(tag) = present_param(&params, "tag")? {
        let value = present_param(&params, "tag_value")?.map(|v| ruby_strip(v).to_owned());
        Some((ruby_downcase(ruby_strip(tag)), value))
    } else if present_param(&params, "tag_value")?.is_some() {
        return Err(ApiError::BadRequest("tag_value requires tag".into()));
    } else {
        None
    };
    let uploaded_by = present_param(&params, "uploaded_by")?.map(str::to_owned);
    let stored_async = match present_param(&params, "upload_mode")? {
        None => None,
        Some("sync") => Some(false),
        Some("async") => Some(true),
        Some(_) => {
            return Err(ApiError::BadRequest(
                "upload_mode must be sync or async".into(),
            ));
        }
    };
    let filters = Filters {
        departments,
        filename,
        tag,
        uploaded_by,
        stored_async,
        uploaded: (
            time_param(&params, "uploaded_after")?,
            time_param(&params, "uploaded_before")?,
        ),
        indexed: (
            time_param(&params, "indexed_after")?,
            time_param(&params, "indexed_before")?,
        ),
        updated: (
            time_param(&params, "updated_after")?,
            time_param(&params, "updated_before")?,
        ),
    };

    let (limit, offset) = pagination_window(&params);
    let mut page = QueryBuilder::<Postgres>::new(format!("SELECT {COLUMNS} FROM documents"));
    filters.push_where(&mut page);
    page.push(" ORDER BY created_at DESC, id DESC LIMIT ");
    page.push_bind(limit);
    page.push(" OFFSET ");
    page.push_bind(offset);
    let documents: Vec<Document> = page
        .build_query_as()
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;

    // `total` counts the filtered scope *before* limit/offset: counting the
    // limited relation would just return the page size.
    let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM documents");
    filters.push_where(&mut count);
    let total: i64 = count
        .build_query_scalar()
        .fetch_one(&state.db)
        .await
        .map_err(internal)?;

    Ok(RailsJson(
        StatusCode::OK,
        json!({
            "documents": documents.iter().map(document_json).collect::<Vec<_>>(),
            "pagination": { "limit": limit, "offset": offset, "total": total },
        }),
    )
    .into_response())
}

// --- show / download / destroy ------------------------------------------------

async fn show(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let document = find_authorized_document(&state, &client, &id, AccessMode::Read).await?;
    Ok(RailsJson(StatusCode::OK, document_json(&document)).into_response())
}

async fn show_bulk_destroy_name(
    State(state): State<AppState>,
    client: AuthenticatedClient,
) -> Result<Response, ApiError> {
    find_authorized_document(&state, &client, "bulk_destroy", AccessMode::Read)
        .await
        .map(|d| RailsJson(StatusCode::OK, document_json(&d)).into_response())
}

/// `send_file` of the stored file, streamed from disk: never loaded whole.
async fn download(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let document = find_authorized_document(&state, &client, &id, AccessMode::Read).await?;
    let key: String = sqlx::query_scalar("SELECT key FROM stored_files WHERE document_id = $1")
        .bind(document.id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("document has no stored file"))?;
    let file = state.storage.open(&key).await.map_err(internal)?;
    let length = file.metadata().await.map_err(internal)?.len();

    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let headers = response.headers_mut();
    let content_type = document
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    // A declared type that cannot be a header value cannot be sent as is.
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(false, &document.filename)).map_err(internal)?,
    );
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(
        "content-transfer-encoding",
        HeaderValue::from_static("binary"),
    );
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    Ok(response)
}

async fn destroy(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let document = find_authorized_document(&state, &client, &id, AccessMode::Write).await?;
    let mut conn = state.db.acquire().await.map_err(internal)?;
    // gone in the meantime (a concurrent delete): the document is not found
    Document::soft_delete(&mut conn, document.id, Utc::now().naive_utc())
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /documents/bulk_destroy`: all or nothing. Any id the caller cannot
/// write — or that is unknown or already trashed — refuses the whole request.
async fn bulk_destroy(
    State(state): State<AppState>,
    client: AuthenticatedClient,
    RequestParams(params): RequestParams,
) -> Result<Response, ApiError> {
    // `Array(params[:document_ids]).map(&:to_s).reject(&:blank?)`
    let ids: Vec<String> = match params.get("document_ids") {
        None => Vec::new(),
        Some(ParamValue::Array(items)) => items.iter().map(id_param_text).collect(),
        Some(single) => vec![id_param_text(single)],
    }
    .into_iter()
    .filter(|id| !blank(id))
    .collect();
    if ids.is_empty() {
        return Err(ApiError::BadRequest("document_ids is required".into()));
    }
    if ids.len() > BULK_MAX {
        return Err(ApiError::BadRequest(format!(
            "too many document_ids (max {BULK_MAX})"
        )));
    }

    // ids that are not uuids match nothing: the count below then differs
    let uuids: Vec<Uuid> = ids.iter().filter_map(|id| uuid_id(id)).collect();
    let mut distinct: Vec<&String> = ids.iter().collect();
    distinct.sort();
    distinct.dedup();

    let mut tx = state.db.begin().await.map_err(internal)?;
    let found: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, department_id FROM documents WHERE id = ANY($1) AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(&uuids)
    .fetch_all(&mut *tx)
    .await
    .map_err(internal)?;
    if found.len() != distinct.len() {
        return Err(ApiError::NotFound);
    }
    let writable = client
        .client()
        .accessible_department_ids(&state.db, AccessMode::Write)
        .await
        .map_err(internal)?;
    if !found.iter().all(|(_, dept)| writable.contains(dept)) {
        return Err(ApiError::Forbidden(
            "no write access to one or more documents".into(),
        ));
    }
    let deleted = Document::bulk_soft_delete(&mut tx, &uuids, Utc::now().naive_utc())
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    Ok(RailsJson(StatusCode::OK, json!({ "deleted": deleted })).into_response())
}

// --- create ------------------------------------------------------------------

fn too_large() -> Response {
    // The edge refuses an oversized body with an empty 413 (see
    // docs/migration/parity-gaps.md): no JSON, no content type.
    crate::http::payload_too_large()
}

fn part_limit() -> Response {
    // Rack's `MultipartPartLimitError` / `MultipartTotalPartLimitError`
    // escape the app. Thruster answers 500 with an empty body and no
    // content type. Observed 2026-11-11: the 128th file part, and the
    // 4096th part of any kind.
    let mut response = crate::http::bare(StatusCode::INTERNAL_SERVER_ERROR.into_response());
    response
        .extensions_mut()
        .insert(crate::http::rails_headers::PreserveBody);
    response
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// `POST /api/v1/documents`: synchronous (201 + document) or, with a
/// `callback_url`, asynchronous (202 + request id).
async fn create(State(state): State<AppState>, req: Request) -> Response {
    let max = state.config.max_upload_bytes;
    let (mut parts, body) = req.into_parts();

    // The edge in front of Rails refuses a body over the limit before anything
    // else — before authentication too.
    if content_length(&parts.headers).is_some_and(|n| n > max) {
        return too_large();
    }

    let (client, intake) = match multipart::boundary(&parts.headers) {
        Some(boundary) => {
            // Rack parses (and spools) a multipart body before the controller
            // runs, so a broken one is a 400 even without a token.
            let intake = match multipart::read(boundary, body, &state.storage, max).await {
                Ok(intake) => intake,
                Err(IntakeError::TooLarge) => return too_large(),
                Err(IntakeError::Malformed) => return ApiError::bad_request().into_response(),
                // Raised by Rack before the controller, so a missing token
                // stays a 500 rather than a 401. Observed 2026-11-11.
                Err(IntakeError::PartLimit) => return part_limit(),
            };
            let client = match AuthenticatedClient::from_request_parts(&mut parts, &state).await {
                Ok(client) => client,
                Err(e) => {
                    discard(&state, &intake).await;
                    return e.into_response();
                }
            };
            let query = match RackParams::parse(parts.uri.query().unwrap_or("")) {
                Ok(query) => query,
                Err(e) => {
                    discard(&state, &intake).await;
                    return ApiError::from(e).into_response();
                }
            };
            let Intake { params, file } = intake;
            (
                client,
                Intake {
                    params: RackParams::merged(params, query),
                    file,
                },
            )
        }
        None => {
            let client = match AuthenticatedClient::from_request_parts(&mut parts, &state).await {
                Ok(client) => client,
                Err(e) => return e.into_response(),
            };
            let request = Request::from_parts(parts, body);
            match RequestParams::from_request(request, &state).await {
                Ok(RequestParams(params)) => (client, Intake { params, file: None }),
                Err(e) => return e.into_response(),
            }
        }
    };

    match create_inner(&state, &client, &intake).await {
        Ok(response) => response,
        Err(e) => {
            discard(&state, &intake).await;
            e.into_response()
        }
    }
}

/// Removes the spooled upload, if any.
async fn discard(state: &AppState, intake: &Intake) {
    if let Some(file) = &intake.file {
        let _ = state.storage.delete(&file.stored.key).await;
    }
}

/// Everything after the body is read. On an `Err` the caller discards the
/// spooled file; on `Ok` the file is kept (sync) or has moved to staging.
async fn create_inner(
    state: &AppState,
    client: &AuthenticatedClient,
    intake: &Intake,
) -> Result<Response, ApiError> {
    let params = &intake.params;
    let department = find_department(state, require_scalar_param(params, "department_id")?).await?;
    client
        .authorize_department(state, department.id, AccessMode::Write)
        .await?;

    // `params.require(:file)`, then the "is it really an upload" check
    let file = match (&intake.file, params.get("file")) {
        (Some(file), _) => file,
        (None, Some(value)) if !param_blank(value) => {
            return Err(ApiError::BadRequest(
                "file must be a multipart file upload".into(),
            ));
        }
        (None, _) => return Err(parameter_missing("file")),
    };

    let tags = tags_param(params).unwrap_or_else(|| json!({}));
    let uploaded_by = client.client().client_identifier.clone();
    let callback = present_param(params, "callback_url")?;

    match callback {
        Some(callback_url) => {
            create_async(state, &department, file, callback_url, uploaded_by, tags).await
        }
        None => create_sync(state, &department, file, uploaded_by, tags).await,
    }
}

async fn create_sync(
    state: &AppState,
    department: &Department,
    file: &multipart::UploadedFile,
    uploaded_by: String,
    tags: Value,
) -> Result<Response, ApiError> {
    let mut tx = state.db.begin().await.map_err(internal)?;
    let created = DocumentCreator::create(
        &mut tx,
        NewDocument {
            department_id: Some(department.id),
            filename: file.original_filename.clone(),
            content_type: file.content_type.clone(),
            file: file.stored.clone(),
            uploaded_by,
            tags,
            uploaded_at: None,
            stored_async: false,
            max_byte_size: state.config.max_upload_bytes,
        },
    )
    .await;
    let document = match created {
        Ok(document) => document,
        Err(CreateError::Invalid(messages)) => {
            return Err(ApiError::Unprocessable(messages.join(", ")));
        }
        Err(CreateError::Db(e)) => return Err(internal(e)),
    };
    tx.commit().await.map_err(internal)?;
    telemetry::documents_uploaded(UploadMode::Sync);
    Ok(RailsJson(StatusCode::CREATED, document_json(&document)).into_response())
}

async fn create_async(
    state: &AppState,
    department: &Department,
    file: &multipart::UploadedFile,
    callback_url: &str,
    uploaded_by: String,
    tags: Value,
) -> Result<Response, ApiError> {
    let max = state.config.max_upload_bytes;
    if file.stored.written.byte_size > max {
        return Err(ApiError::Unprocessable(format!(
            "file is too large (maximum {max} bytes)"
        )));
    }
    let id = Uuid::new_v4();
    let mut tx = state.db.begin().await.map_err(internal)?;
    let request = UploadRequest::create(
        &mut tx,
        id,
        NewUploadRequest {
            department_id: department.id,
            callback_url: callback_url.to_owned(),
            filename: file.original_filename.clone(),
            content_type: file.content_type.clone(),
            uploaded_by,
            tags,
        },
    )
    .await;
    let request = match request {
        Ok(request) => request,
        Err(upload_request::CreateError::Invalid(messages)) => {
            return Err(ApiError::Unprocessable(messages.join(", ")));
        }
        Err(upload_request::CreateError::Db(e)) => return Err(internal(e)),
    };

    // The raw bytes move to the staging area under the request id (a rename on
    // the same volume: nothing is copied), then the job is enqueued in the
    // same transaction as the row.
    let staged = state.staging.path(request.id);
    let from = state.storage.path_for(&file.stored.key).map_err(internal)?;
    tokio::fs::create_dir_all(state.staging.root())
        .await
        .map_err(internal)?;
    tokio::fs::rename(&from, &staged).await.map_err(internal)?;
    let enqueued = queue::enqueue(
        &mut tx,
        JobPayload::new(
            "StoreDocumentJob",
            json!({ "upload_request_id": request.id.to_string() }),
        ),
        QueueName::Uploads,
    )
    .await;
    let committed = match enqueued {
        Ok(_) => tx.commit().await,
        Err(e) => Err(e),
    };
    if let Err(e) = committed {
        // no row, no job: the staging file must not outlive the request
        let _ = state.staging.delete(request.id).await;
        return Err(internal(e));
    }
    Ok(RailsJson(
        StatusCode::ACCEPTED,
        json!({ "request_id": request.id.to_string(), "status": "pending" }),
    )
    .into_response())
}
