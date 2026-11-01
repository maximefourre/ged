//! SQL fixtures on the business tables only. Names carry a random suffix so
//! tests can run concurrently against one stack without colliding.

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
