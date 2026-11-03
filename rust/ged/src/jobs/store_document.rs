//! `StoreDocumentJob` (queue `uploads`): turns a staged async upload into a
//! document. Port of `app/jobs/store_document_job.rb`.
//!
//! Exactly once for documents: the pending -> processing claim is atomic, and
//! the document, its stored-file row, the `completed` update and the callback
//! enqueue commit in **one** transaction. A failure (or a crash) between the
//! writes rolls all of it back, so a re-run never finds a half-stored upload.

use chrono::NaiveDateTime;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::env::PipelineEnv;
use super::queue::{self, JobPayload, QueueName};
use super::{Job, JobError, timed};
use crate::domain::document_creator::{CreateError, DocumentCreator, NewDocument};
use crate::telemetry::{self, JobClass, JobQueue, UploadMode};

pub const KIND: &str = "StoreDocumentJob";

pub struct StoreDocumentJob {
    env: PipelineEnv,
}

impl StoreDocumentJob {
    pub fn new(env: PipelineEnv) -> Self {
        Self { env }
    }
}

impl Job for StoreDocumentJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn perform(&self, args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        let env = self.env.clone();
        Box::pin(timed(
            JobClass::StoreDocumentJob,
            JobQueue::Uploads,
            async move {
                let id = request_id(&args)?;
                store(&env, id).await
            },
        ))
    }
}

/// `{"upload_request_id": uuid}`; anything else can never succeed.
pub(crate) fn request_id(args: &Value) -> Result<Uuid, JobError> {
    args.get("upload_request_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| JobError::Discard(format!("bad arguments: {args}")))
}

#[derive(sqlx::FromRow)]
struct Request {
    department_id: i64,
    filename: String,
    content_type: Option<String>,
    tags: Value,
    uploaded_by: String,
    created_at: NaiveDateTime,
}

fn retry(e: sqlx::Error) -> JobError {
    JobError::Retry(e.to_string())
}

async fn store(env: &PipelineEnv, id: Uuid) -> Result<(), JobError> {
    let request: Option<Request> = sqlx::query_as(
        "SELECT department_id, filename, content_type, tags, uploaded_by, created_at \
         FROM upload_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&env.db)
    .await
    .map_err(retry)?;
    // `UploadRequest.find` raising RecordNotFound: nothing to ever retry.
    let request =
        request.ok_or_else(|| JobError::Discard(format!("upload request {id} not found")))?;
    let Some(claim) = claim(&env.db, id).await.map_err(retry)? else {
        return Ok(());
    };

    let outcome = store_and_complete(env, id, &request, claim).await;
    let result = match outcome {
        Ok(Outcome::Completed) => {
            telemetry::documents_uploaded(UploadMode::Async);
            Ok(())
        }
        // Reconcile reset the request while this run was slow: the new run
        // owns it now, and needs the staging file.
        Ok(Outcome::Superseded) => return Ok(()),
        Err(message) => {
            eprintln!("Async upload {id} failed: {message}");
            match fail(&env.db, id, claim, &message).await {
                Ok(true) => Ok(()),
                // Overtaken meanwhile: the new run owns the request and
                // needs the staging file, like the `Superseded` success path.
                Ok(false) => return Ok(()),
                Err(e) => Err(retry(e)),
            }
        }
    };
    // `ensure`: the staging file never outlives the attempt.
    if let Err(e) = env.staging.delete(id).await {
        eprintln!("Async upload {id}: could not delete staging file: {e}");
    }
    result
}

/// Proof of ownership of a request: the `updated_at` written by [`claim`].
/// A later run (after a reconcile reset) stamps a different value, so a stale
/// run can neither complete nor fail the request it no longer holds.
#[derive(Clone, Copy)]
struct Claim(NaiveDateTime);

/// Atomic pending -> processing: a reconcile re-enqueue racing the original
/// job must not store the document twice. `None`: someone else has it.
async fn claim(db: &PgPool, id: Uuid) -> Result<Option<Claim>, sqlx::Error> {
    let stamp: Option<NaiveDateTime> = sqlx::query_scalar(
        "UPDATE upload_requests SET status = 'processing', updated_at = (clock_timestamp() AT TIME ZONE 'UTC') \
         WHERE id = $1 AND status = 'pending' RETURNING updated_at",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(stamp.map(Claim))
}

enum Outcome {
    Completed,
    Superseded,
}

/// Err carries `e.message` for the request's `error_message`.
async fn store_and_complete(
    env: &PipelineEnv,
    id: Uuid,
    request: &Request,
    claim: Claim,
) -> Result<Outcome, String> {
    let path = env.staging.path(id);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| ruby_open_error(&e, &path))?;
    // The cap is enforced by the model validation (as in Rails), not while
    // copying: an oversize file gets the same message as any other path.
    let stored = env
        .storage
        .put(file, u64::MAX)
        .await
        .map_err(|e| e.to_string())?;
    let key = stored.key.clone();

    let committed = create_and_complete(env, id, request, claim, stored).await;
    if !matches!(committed, Ok(Outcome::Completed)) {
        // Rolled back: the bytes written for it are unreachable, drop them.
        if let Err(e) = env.storage.delete(&key).await {
            eprintln!("Async upload {id}: could not delete stored file {key}: {e}");
        }
    }
    committed
}

async fn create_and_complete(
    env: &PipelineEnv,
    id: Uuid,
    request: &Request,
    claim: Claim,
    stored: crate::storage::Stored,
) -> Result<Outcome, String> {
    let mut tx = env.db.begin().await.map_err(|e| e.to_string())?;
    let document = DocumentCreator::create(
        &mut tx,
        NewDocument {
            department_id: Some(request.department_id),
            filename: request.filename.clone(),
            content_type: request.content_type.clone(),
            file: stored,
            uploaded_by: request.uploaded_by.clone(),
            tags: request.tags.clone(),
            uploaded_at: Some(request.created_at),
            stored_async: true,
            max_byte_size: env.max_byte_size,
        },
    )
    .await
    .map_err(|e| match e {
        // ActiveRecord::RecordInvalid#message
        CreateError::Invalid(messages) => format!("Validation failed: {}", messages.join(", ")),
        CreateError::Db(e) => e.to_string(),
    })?;
    let completed = sqlx::query(
        "UPDATE upload_requests SET status = 'completed', document_id = $2, \
         updated_at = (now() AT TIME ZONE 'UTC') WHERE id = $1 AND status = 'processing' AND updated_at = $3",
    )
    .bind(id)
    .bind(document.id)
    .bind(claim.0)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .rows_affected();
    if completed == 0 {
        return Ok(Outcome::Superseded); // dropping `tx` rolls the document back
    }
    // Same transaction: a committed document always has its callback queued.
    queue::enqueue(&mut tx, callback_job(id), QueueName::Default)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(Outcome::Completed)
}

fn callback_job(id: Uuid) -> JobPayload {
    JobPayload::new(
        super::deliver_callback::KIND,
        json!({ "upload_request_id": id.to_string() }),
    )
}

/// Marks the request failed and queues its callback, atomically, only while
/// this run still holds the claim (`processing`). `false`: not the owner.
async fn fail(db: &PgPool, id: Uuid, claim: Claim, message: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let done = sqlx::query(
        "UPDATE upload_requests SET status = 'failed', error_message = $2, updated_at = (now() AT TIME ZONE 'UTC') \
         WHERE id = $1 AND status = 'processing' AND updated_at = $3",
    )
    .bind(id)
    .bind(message)
    .bind(claim.0)
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    queue::enqueue(&mut tx, callback_job(id), QueueName::Default).await?;
    tx.commit().await?;
    Ok(true)
}

/// Ruby's `File.open` failure text, which ends up in `error_message`.
fn ruby_open_error(e: &std::io::Error, path: &std::path::Path) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            format!(
                "No such file or directory @ rb_sysopen - {}",
                path.display()
            )
        }
        _ => e.to_string(),
    }
}

/// Used by reconcile: re-queue a stored-nothing upload.
pub fn job_payload(id: Uuid) -> JobPayload {
    JobPayload::new(KIND, json!({ "upload_request_id": id.to_string() }))
}
