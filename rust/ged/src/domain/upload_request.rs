//! `UploadRequest`: the record of an asynchronous upload (port of
//! `app/models/upload_request.rb`). The request writes the row and the staging
//! file, `StoreDocumentJob` (T9) turns it into a document, `DeliverCallbackJob`
//! reports the outcome to `callback_url`.

use chrono::{NaiveDateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::document::{Document, sql};
use super::tags;
use crate::policy::callback_url;

pub const STATUSES: [&str; 4] = ["pending", "processing", "completed", "failed"];

const COLUMNS: &str = "id, callback_url, content_type, created_at, department_id, document_id, \
     error_message, filename, status, tags, updated_at, uploaded_by";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UploadRequest {
    pub id: Uuid,
    pub callback_url: String,
    pub content_type: Option<String>,
    pub created_at: NaiveDateTime,
    pub department_id: i64,
    pub document_id: Option<Uuid>,
    pub error_message: Option<String>,
    pub filename: String,
    pub status: String,
    pub tags: Value,
    pub updated_at: NaiveDateTime,
    pub uploaded_by: String,
}

pub struct NewUploadRequest {
    pub department_id: i64,
    pub callback_url: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub uploaded_by: String,
    pub tags: Value,
}

#[derive(Debug)]
pub enum CreateError {
    /// `ActiveRecord::RecordInvalid`: `errors.full_messages`, in Rails' order.
    Invalid(Vec<String>),
    Db(sqlx::Error),
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(messages) => f.write_str(&messages.join(", ")),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for CreateError {}

impl From<sqlx::Error> for CreateError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// Ruby's `blank?` on a String: empty or only Unicode whitespace.
fn blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// Validation messages in declaration order (the `KvTaggable` concern first,
/// then `belongs_to :department`, status, presence, the callback URL).
pub fn validate(input: &NewUploadRequest, department_exists: bool) -> Vec<String> {
    let mut errors = tags::full_messages(&tags::validate(&input.tags));
    if !department_exists {
        errors.push("Department must exist".to_owned());
    }
    // status is always "pending" on create
    if blank(&input.callback_url) {
        errors.push("Callback url can't be blank".to_owned());
    }
    if blank(&input.filename) {
        errors.push("Filename can't be blank".to_owned());
    }
    if blank(&input.uploaded_by) {
        errors.push("Uploaded by can't be blank".to_owned());
    }
    // Syntactic SSRF check only (no DNS — a fast 422 for the client); the
    // resolution-based barrier runs at delivery time.
    if !blank(&input.callback_url)
        && let Some(reason) = callback_url::reject_reason(&input.callback_url)
    {
        errors.push(format!("Callback url {reason}"));
    }
    errors
}

impl UploadRequest {
    /// Creates a `pending` request; `id` is chosen by the caller so the staging
    /// file can be named before the row exists.
    pub async fn create(
        conn: &mut PgConnection,
        id: Uuid,
        input: NewUploadRequest,
    ) -> Result<Self, CreateError> {
        let mut input = input;
        input.tags = tags::normalize(input.tags);
        let department_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM departments WHERE id = $1 AND deleted_at IS NULL)",
        )
        .bind(input.department_id)
        .fetch_one(&mut *conn)
        .await?;
        let errors = validate(&input, department_exists);
        if !errors.is_empty() {
            return Err(CreateError::Invalid(errors));
        }
        let now = Utc::now().naive_utc();
        let row = sqlx::query_as(sql(&format!(
            "INSERT INTO upload_requests (id, department_id, callback_url, filename, content_type, \
             uploaded_by, tags, status, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8, $8) RETURNING {COLUMNS}"
        )))
        .bind(id)
        .bind(input.department_id)
        .bind(&input.callback_url)
        .bind(&input.filename)
        .bind(&input.content_type)
        .bind(&input.uploaded_by)
        .bind(&input.tags)
        .bind(now)
        .fetch_one(conn)
        .await?;
        Ok(row)
    }

    pub async fn find(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<Self>> {
        sqlx::query_as(sql(&format!(
            "SELECT {COLUMNS} FROM upload_requests WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await
    }

    /// Atomic `pending → processing` claim: a reconcile re-enqueue racing the
    /// original job must not store the document twice.
    pub async fn claim(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<bool> {
        let done = sqlx::query(
            "UPDATE upload_requests SET status = 'processing', updated_at = $2 \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(Utc::now().naive_utc())
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// `update!(status: "completed", document_id:)`.
    pub async fn complete(
        conn: &mut PgConnection,
        id: Uuid,
        document_id: Uuid,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE upload_requests SET status = 'completed', document_id = $2, updated_at = $3 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(document_id)
        .bind(Utc::now().naive_utc())
        .execute(conn)
        .await?;
        Ok(())
    }

    /// `update!(status: "failed", error_message:)`.
    pub async fn fail(conn: &mut PgConnection, id: Uuid, message: &str) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE upload_requests SET status = 'failed', error_message = $2, updated_at = $3 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(message)
        .bind(Utc::now().naive_utc())
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The request's live document: a trashed one is invisible
    /// (`belongs_to :document` goes through the default scope).
    pub async fn live_document(&self, conn: &mut PgConnection) -> sqlx::Result<Option<Document>> {
        match self.document_id {
            Some(id) => Document::find(conn, id).await,
            None => Ok(None),
        }
    }

    /// The payload shared by the polling endpoint and the callback:
    /// `{ request_id, status, document_id, error_message, tags, metadata }`
    /// (`tags` and `metadata` are `null` until the document exists).
    pub fn payload(&self, document: Option<&Document>) -> Value {
        json!({
            "request_id": self.id.to_string(),
            "status": self.status,
            "document_id": self.document_id.map(|d| d.to_string()),
            "error_message": self.error_message,
            "tags": document.map(|d| d.tags.clone()),
            "metadata": document.map(Document::metadata_hash),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> NewUploadRequest {
        NewUploadRequest {
            department_id: 1,
            callback_url: "https://example.com/cb".into(),
            filename: "report.pdf".into(),
            content_type: Some("application/pdf".into()),
            uploaded_by: "client".into(),
            tags: json!({}),
        }
    }

    #[test]
    fn a_complete_request_is_valid() {
        assert!(validate(&input(), true).is_empty());
    }

    #[test]
    fn messages_follow_the_model_declaration_order() {
        let mut bad = input();
        bad.tags = json!("nope");
        bad.callback_url = "http://127.0.0.1/cb".into();
        bad.filename = " ".into();
        assert_eq!(
            validate(&bad, false),
            [
                "Tags must be a map of key/value pairs",
                "Department must exist",
                "Filename can't be blank",
                "Callback url must not target a private or internal address",
            ]
        );
    }

    #[test]
    fn a_blank_callback_url_is_only_reported_as_blank() {
        let mut bad = input();
        bad.callback_url = "".into();
        assert_eq!(validate(&bad, true), ["Callback url can't be blank"]);
    }

    #[test]
    fn the_payload_has_null_tags_and_metadata_without_a_document() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let request = UploadRequest {
            id: Uuid::nil(),
            callback_url: "https://example.com".into(),
            content_type: None,
            created_at: at,
            department_id: 1,
            document_id: None,
            error_message: None,
            filename: "a".into(),
            status: "pending".into(),
            tags: json!({}),
            updated_at: at,
            uploaded_by: "c".into(),
        };
        assert_eq!(
            request.payload(None).to_string(),
            r#"{"request_id":"00000000-0000-0000-0000-000000000000","status":"pending","document_id":null,"error_message":null,"tags":null,"metadata":null}"#
        );
    }
}
