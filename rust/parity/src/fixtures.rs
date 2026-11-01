//! SQL fixtures on the business tables only. Names carry a random suffix so
//! tests can run concurrently against one stack without colliding.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// `prefix-1a2b3c4d`: unique, short, safe in a client id / URL.
pub fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

pub async fn create_app_client(db: &PgPool, identifier: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO app_clients (client_identifier, created_at, updated_at) \
         VALUES ($1, now(), now()) RETURNING id",
    )
    .bind(identifier)
    .fetch_one(db)
    .await
    .expect("insert app_client")
}

pub async fn create_department(db: &PgPool, name: &str, parent_id: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO departments (name, parent_id, created_at, updated_at) \
         VALUES ($1, $2, now(), now()) RETURNING id",
    )
    .bind(name)
    .bind(parent_id)
    .fetch_one(db)
    .await
    .expect("insert department")
}

pub async fn grant(db: &PgPool, app_client_id: i64, department_id: i64, read: bool, write: bool) {
    sqlx::query(
        "INSERT INTO client_department_accesses \
         (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, now(), now())",
    )
    .bind(app_client_id)
    .bind(department_id)
    .bind(read)
    .bind(write)
    .execute(db)
    .await
    .expect("insert client_department_access");
}

pub async fn app_client_id(db: &PgPool, identifier: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM app_clients WHERE client_identifier = $1")
        .bind(identifier)
        .fetch_one(db)
        .await
        .expect("select app_client id")
}

pub fn random_uuid() -> String {
    Uuid::new_v4().to_string()
}

fn parse_uuid(id: &str) -> Uuid {
    Uuid::parse_str(id).unwrap_or_else(|e| panic!("uuid {id}: {e}"))
}

pub async fn set_document_tags(db: &PgPool, id: &str, tags: &Value) {
    sqlx::query("UPDATE documents SET tags = $2 WHERE id = $1")
        .bind(parse_uuid(id))
        .bind(tags)
        .execute(db)
        .await
        .expect("set document tags");
}

pub async fn document_tags(db: &PgPool, id: &str) -> Value {
    sqlx::query_scalar("SELECT tags FROM documents WHERE id = $1")
        .bind(parse_uuid(id))
        .fetch_one(db)
        .await
        .expect("select document tags")
}

pub async fn document_is_live(db: &PgPool, id: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(parse_uuid(id))
    .fetch_one(db)
    .await
    .expect("document liveness")
}

/// `deleted_at` is set and `purge_at` is exactly 30 days later (Rails `30.days`).
pub async fn document_retained_for_30_days(db: &PgPool, id: &str) -> bool {
    sqlx::query_scalar(
        "SELECT deleted_at IS NOT NULL \
         AND purge_at = deleted_at + interval '30 days' \
         FROM documents WHERE id = $1",
    )
    .bind(parse_uuid(id))
    .fetch_one(db)
    .await
    .expect("document retention")
}

pub async fn count_documents(db: &PgPool, department_id: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*)::bigint FROM documents WHERE department_id = $1")
        .bind(department_id)
        .fetch_one(db)
        .await
        .expect("count documents")
}

pub async fn count_upload_requests(db: &PgPool, department_id: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*)::bigint FROM upload_requests WHERE department_id = $1")
        .bind(department_id)
        .fetch_one(db)
        .await
        .expect("count upload_requests")
}

pub async fn child_department_count(db: &PgPool, parent_id: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*)::bigint FROM departments WHERE parent_id = $1")
        .bind(parent_id)
        .fetch_one(db)
        .await
        .expect("count child departments")
}

pub async fn trash_department(db: &PgPool, id: i64) {
    sqlx::query("UPDATE departments SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .expect("trash department");
}

pub async fn backdate_document(db: &PgPool, id: &str, rfc3339: &str) {
    let at = DateTime::parse_from_rfc3339(rfc3339)
        .unwrap_or_else(|e| panic!("time {rfc3339}: {e}"))
        .with_timezone(&Utc);
    sqlx::query(
        "UPDATE documents SET uploaded_at = $2, created_at = $2, updated_at = $2 WHERE id = $1",
    )
    .bind(parse_uuid(id))
    .bind(at)
    .execute(db)
    .await
    .expect("backdate document");
}

pub async fn set_upload_origin(db: &PgPool, id: &str, stored_async: bool, uploaded_by: &str) {
    sqlx::query("UPDATE documents SET stored_async = $2, uploaded_by = $3 WHERE id = $1")
        .bind(parse_uuid(id))
        .bind(stored_async)
        .bind(uploaded_by)
        .execute(db)
        .await
        .expect("set upload origin");
}

/// Soft-delete state of a document, to age it past its retention window
/// before running the purge job.
pub async fn make_purgeable(db: &PgPool, document_id: Uuid) {
    sqlx::query(
        "UPDATE documents SET deleted_at = now() - interval '31 days', \
         purge_at = now() - interval '1 day' WHERE id = $1",
    )
    .bind(document_id)
    .execute(db)
    .await
    .expect("age document");
}

/// `make_purgeable` addressed by the document id string returned by the API.
pub async fn make_purgeable_id(db: &PgPool, document_id: &str) {
    make_purgeable(db, parse_id(document_id)).await;
}

/// A business-table document with no file attached. Purge observes the row,
/// never storage tables; file-bearing documents are created through HTTP.
pub async fn insert_document(
    db: &PgPool,
    department_id: i64,
    filename: &str,
    uploaded_by: &str,
) -> String {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO documents \
         (department_id, filename, content_type, byte_size, uploaded_by, uploaded_at, \
          tags, stored_async, created_at, updated_at) \
         VALUES ($1, $2, 'application/pdf', 4, $3, now(), '{}'::jsonb, false, now(), now()) \
         RETURNING id",
    )
    .bind(department_id)
    .bind(filename)
    .bind(uploaded_by)
    .fetch_one(db)
    .await
    .expect("insert document");
    id.to_string()
}

pub async fn insert_upload_request(
    db: &PgPool,
    department_id: i64,
    callback_url: &str,
    filename: &str,
    uploaded_by: &str,
) -> String {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO upload_requests \
         (department_id, callback_url, filename, content_type, uploaded_by, status, tags, \
          created_at, updated_at) \
         VALUES ($1, $2, $3, 'application/pdf', $4, 'pending', '{}'::jsonb, now(), now()) \
         RETURNING id",
    )
    .bind(department_id)
    .bind(callback_url)
    .bind(filename)
    .bind(uploaded_by)
    .fetch_one(db)
    .await
    .expect("insert upload request");
    id.to_string()
}

/// Same columns as `Document#soft_delete!`: hidden, purge_at 30 days out.
pub async fn soft_delete_document(db: &PgPool, document_id: &str) {
    sqlx::query(
        "UPDATE documents SET deleted_at = now(), \
         purge_at = now() + interval '30 days', updated_at = now() WHERE id = $1",
    )
    .bind(parse_id(document_id))
    .execute(db)
    .await
    .expect("soft-delete document");
}

/// `deleted_at` only. Does not trash documents (that is `Department#soft_delete!`).
pub async fn mark_department_deleted(db: &PgPool, department_id: i64) {
    sqlx::query("UPDATE departments SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(department_id)
        .execute(db)
        .await
        .expect("soft-delete department");
}

pub async fn document_count(db: &PgPool, department_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE department_id = $1")
        .bind(department_id)
        .fetch_one(db)
        .await
        .expect("document count")
}

pub async fn document_exists(db: &PgPool, document_id: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE id = $1)")
        .bind(parse_id(document_id))
        .fetch_one(db)
        .await
        .expect("document exists")
}

pub async fn department_exists(db: &PgPool, department_id: i64) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM departments WHERE id = $1)")
        .bind(department_id)
        .fetch_one(db)
        .await
        .expect("department exists")
}

pub async fn grant_count(db: &PgPool, department_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM client_department_accesses WHERE department_id = $1")
        .bind(department_id)
        .fetch_one(db)
        .await
        .expect("grant count")
}

pub async fn upload_request_count(db: &PgPool, department_id: i64, callback_url: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM upload_requests WHERE department_id = $1 AND callback_url = $2",
    )
    .bind(department_id)
    .bind(callback_url)
    .fetch_one(db)
    .await
    .expect("upload request count")
}

/// Seconds between `deleted_at` and `purge_at`, for the retention window.
pub async fn retention_seconds(db: &PgPool, document_id: &str) -> Option<f64> {
    sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (purge_at - deleted_at))::float8 FROM documents WHERE id = $1",
    )
    .bind(parse_id(document_id))
    .fetch_one(db)
    .await
    .expect("retention")
}

/// `tags` as stored, or `None` when the row is gone.
pub async fn document_tags_opt(db: &PgPool, document_id: &str) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT tags FROM documents WHERE id = $1")
        .bind(parse_id(document_id))
        .fetch_optional(db)
        .await
        .expect("document tags")
}

fn parse_id(id: &str) -> Uuid {
    Uuid::parse_str(id).unwrap_or_else(|e| panic!("document id {id:?}: {e}"))
}
