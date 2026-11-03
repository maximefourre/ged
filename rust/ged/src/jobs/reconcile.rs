//! `ReconcileUploadRequestsJob` (queue `default`, every 10 minutes): recovers
//! async uploads orphaned by a crash or deploy between the 202 and the end of
//! `StoreDocumentJob`, and purges old terminal requests and staging files.
//! Port of `app/jobs/reconcile_upload_requests_job.rb`.
//!
//! The orphaned-blob sweep of the Rails job lives in the daily purge job here
//! (`purge.rs`): there are no blob rows to query, only files to look for.

use std::time::{Duration, SystemTime};

use chrono::{DateTime, NaiveDateTime, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use uuid::Uuid;

use super::env::PipelineEnv;
use super::queue::{self, QueueName};
use super::{Job, JobError, store_document, timed};
use crate::telemetry::{JobClass, JobQueue};

pub const KIND: &str = "ReconcileUploadRequestsJob";
/// `STUCK_AFTER`: 10 minutes.
pub const STUCK_AFTER: Duration = Duration::from_secs(600);
/// `RETENTION`: 30 days.
pub const RETENTION: Duration = Duration::from_secs(30 * 86_400);

pub struct ReconcileUploadRequestsJob {
    env: PipelineEnv,
}

impl ReconcileUploadRequestsJob {
    pub fn new(env: PipelineEnv) -> Self {
        Self { env }
    }

    /// One pass as of `now`.
    pub async fn run(&self, now: DateTime<Utc>) -> Result<(), JobError> {
        reconcile(&self.env, now)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))
    }
}

impl Job for ReconcileUploadRequestsJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn perform(&self, _args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        let env = self.env.clone();
        Box::pin(timed(
            JobClass::ReconcileUploadRequestsJob,
            JobQueue::Default,
            async move {
                reconcile(&env, Utc::now())
                    .await
                    .map_err(|e| JobError::Retry(e.to_string()))
            },
        ))
    }
}

fn before(now: DateTime<Utc>, age: Duration) -> NaiveDateTime {
    (now - chrono::Duration::from_std(age).unwrap_or(chrono::Duration::MAX)).naive_utc()
}

async fn reconcile(env: &PipelineEnv, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    // Each step runs even if an earlier one fails; the first error is reported.
    let steps = [
        recover_stuck_pending(env, now).await,
        recover_stuck_processing(env, now).await,
        purge_old_terminal_requests(env, now).await,
        purge_old_staging_files(env, now).await,
    ];
    steps.into_iter().collect::<Result<Vec<_>, _>>().map(drop)
}

async fn ids(
    env: &PipelineEnv,
    sql: &'static str,
    cutoff: NaiveDateTime,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(sql)
        .bind(cutoff)
        .fetch_all(&env.db)
        .await
}

async fn recover_stuck_pending(env: &PipelineEnv, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    // A paused uploads queue means pending requests are intentionally
    // waiting; re-enqueueing every run would pile duplicates into it.
    if queue::is_paused(&env.db, QueueName::Uploads).await? {
        return Ok(());
    }
    let stuck = ids(
        env,
        "SELECT id FROM upload_requests WHERE status = 'pending' AND created_at <= $1 ORDER BY created_at",
        before(now, STUCK_AFTER),
    )
    .await?;
    for id in stuck {
        let result = if env.staging.exists(id).await {
            enqueue_store(env, id).await
        } else {
            fail_and_notify(env, id, "pending").await
        };
        keep_going(id, result)?;
    }
    Ok(())
}

/// A worker died mid-job: the claim happened but neither the terminal update
/// nor the cleanup ran. With the staging file still on disk the upload is
/// retried from scratch; without it there is nothing left to store.
async fn recover_stuck_processing(
    env: &PipelineEnv,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let cutoff = before(now, STUCK_AFTER);
    let stuck = ids(
        env,
        "SELECT id FROM upload_requests WHERE status = 'processing' AND updated_at <= $1 ORDER BY updated_at",
        cutoff,
    )
    .await?;
    for id in stuck {
        let result = if env.staging.exists(id).await {
            requeue_processing(env, id, cutoff).await
        } else {
            fail_and_notify(env, id, "processing").await
        };
        keep_going(id, result)?;
    }
    Ok(())
}

/// One bad row must not stop the others: log it and carry on.
fn keep_going(id: Uuid, result: Result<(), sqlx::Error>) -> Result<(), sqlx::Error> {
    if let Err(e) = result {
        eprintln!("ReconcileUploadRequestsJob: request {id}: {e}");
    }
    Ok(())
}

async fn enqueue_store(env: &PipelineEnv, id: Uuid) -> Result<(), sqlx::Error> {
    let mut tx = env.db.begin().await?;
    queue::enqueue(&mut tx, store_document::job_payload(id), QueueName::Uploads).await?;
    tx.commit().await
}

/// processing -> pending and re-enqueue, only if still stuck.
async fn requeue_processing(
    env: &PipelineEnv,
    id: Uuid,
    cutoff: NaiveDateTime,
) -> Result<(), sqlx::Error> {
    let mut tx = env.db.begin().await?;
    let moved = sqlx::query(
        "UPDATE upload_requests SET status = 'pending', updated_at = (now() AT TIME ZONE 'UTC') \
         WHERE id = $1 AND status = 'processing' AND updated_at <= $2",
    )
    .bind(id)
    .bind(cutoff)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if moved == 1 {
        queue::enqueue(&mut tx, store_document::job_payload(id), QueueName::Uploads).await?;
    }
    tx.commit().await
}

/// failed + callback, atomically, only if the request is still in `from`.
async fn fail_and_notify(env: &PipelineEnv, id: Uuid, from: &str) -> Result<(), sqlx::Error> {
    let mut tx = env.db.begin().await?;
    let failed = sqlx::query(
        "UPDATE upload_requests SET status = 'failed', \
         error_message = 'staged file lost before processing', updated_at = (now() AT TIME ZONE 'UTC') \
         WHERE id = $1 AND status = $2",
    )
    .bind(id)
    .bind(from)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if failed == 1 {
        let payload = queue::JobPayload::new(
            super::deliver_callback::KIND,
            json!({ "upload_request_id": id.to_string() }),
        );
        queue::enqueue(&mut tx, payload, QueueName::Default).await?;
    }
    tx.commit().await
}

async fn purge_old_terminal_requests(
    env: &PipelineEnv,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let old = ids(
        env,
        "SELECT id FROM upload_requests WHERE status IN ('completed', 'failed') AND created_at < $1",
        before(now, RETENTION),
    )
    .await?;
    for id in old {
        let result = async {
            if let Err(e) = env.staging.delete(id).await {
                eprintln!("ReconcileUploadRequestsJob: staging file of {id}: {e}");
            }
            sqlx::query(
                "DELETE FROM upload_requests WHERE id = $1 AND status IN ('completed', 'failed')",
            )
            .bind(id)
            .execute(&env.db)
            .await
            .map(drop)
        }
        .await;
        keep_going(id, result)?;
    }
    Ok(())
}

/// Staging files with no live pending request (rows deleted by hand, ...);
/// anything this old is garbage — stuck requests are handled long before.
async fn purge_old_staging_files(env: &PipelineEnv, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    let cutoff: SystemTime = now.into();
    let cutoff = cutoff - RETENTION;
    let Ok(mut dir) = tokio::fs::read_dir(env.staging.root()).await else {
        return Ok(()); // no staging directory yet
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        // Deleted concurrently (StoreDocumentJob finishing) between listing and
        // stat/delete: already gone, nothing to purge.
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if meta.is_file() && meta.modified().is_ok_and(|m| m < cutoff) {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
    Ok(())
}
