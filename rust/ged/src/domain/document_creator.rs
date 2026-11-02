//! `DocumentCreator`: the single home for "build a Document, attach the
//! stored file, save". The sync API upload, the admin UI upload and the async
//! store job all create documents through here (port of
//! `app/services/document_creator.rb`).
//!
//! The bytes are already on disk (`Storage::put`) when this runs; the caller
//! owns that file until `create` returns `Ok` and must `Storage::delete` it if
//! the call fails or the transaction rolls back.

use chrono::{NaiveDateTime, Utc};
use serde_json::Value;

use super::document::{COLUMNS, Document, sql};
use super::tags;
use crate::db::PgTx;
use crate::storage::{Stored, detect_content_type};

pub struct NewDocument {
    pub department_id: Option<i64>,
    pub filename: String,
    /// The type the client declared (multipart part header), stored as is.
    pub content_type: Option<String>,
    pub file: Stored,
    pub uploaded_by: String,
    pub tags: Value,
    pub uploaded_at: Option<NaiveDateTime>,
    pub stored_async: bool,
    /// `Document::MAX_BYTE_SIZE` (`GED_MAX_UPLOAD_BYTES`).
    pub max_byte_size: u64,
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

pub struct DocumentCreator;

impl DocumentCreator {
    pub async fn create(tx: &mut PgTx, input: NewDocument) -> Result<Document, CreateError> {
        let filename = Document::normalize_filename(&input.filename);
        let tags = tags::normalize(input.tags);
        let byte_size = i64::try_from(input.file.written.byte_size).unwrap_or(i64::MAX);

        // Validation order = declaration order in the Rails model: tags
        // (KvTaggable), department (belongs_to), filename, uploaded_by, byte_size.
        let mut errors = tags::full_messages(&tags::validate(&tags));
        let department_exists = match input.department_id {
            Some(id) => sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM departments WHERE id = $1 AND deleted_at IS NULL)",
            )
            .bind(id)
            .fetch_one(&mut **tx)
            .await?,
            None => false,
        };
        if !department_exists {
            errors.push("Department must exist".to_owned());
        }
        if input.uploaded_by.chars().all(char::is_whitespace) {
            errors.push("Uploaded by can't be blank".to_owned());
        }
        if input.file.written.byte_size > input.max_byte_size {
            errors.push(format!(
                "Byte size must be less than or equal to {}",
                input.max_byte_size
            ));
        }
        if !errors.is_empty() {
            return Err(CreateError::Invalid(errors));
        }

        let now = Utc::now().naive_utc();
        let document: Document = sqlx::query_as(sql(&format!(
            "INSERT INTO documents (department_id, filename, content_type, byte_size, uploaded_by, \
             tags, uploaded_at, stored_async, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9) RETURNING {COLUMNS}"
        )))
        .bind(input.department_id)
        .bind(&filename)
        .bind(&input.content_type)
        .bind(byte_size)
        .bind(&input.uploaded_by)
        .bind(&tags)
        .bind(input.uploaded_at.unwrap_or(now))
        .bind(input.stored_async)
        .bind(now)
        .fetch_one(&mut **tx)
        .await?;

        // What Active Storage records on the blob: identified from the content,
        // the sanitised file name and the declared type.
        let detected = detect_content_type(
            &input.file.written.head,
            Some(&filename),
            input.content_type.as_deref(),
        );
        sqlx::query(
            "INSERT INTO stored_files (key, document_id, filename, content_type, byte_size, checksum, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&input.file.key)
        .bind(document.id)
        .bind(&filename)
        .bind(detected)
        .bind(byte_size)
        .bind(&input.file.written.checksum)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        Ok(document)
    }
}
