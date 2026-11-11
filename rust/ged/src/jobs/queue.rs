//! Enqueue, claim, complete/fail, pause and recovery, all plain SQL.

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// A transaction as handed out by `PgPool::begin`.
pub type PgTx = Transaction<'static, Postgres>;

/// Closed set of queues; there is no catch-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueName {
    Uploads,
    Default,
}

impl QueueName {
    pub const ALL: [QueueName; 2] = [QueueName::Uploads, QueueName::Default];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uploads => "uploads",
            Self::Default => "default",
        }
    }
}

impl FromStr for QueueName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "uploads" => Ok(Self::Uploads),
            "default" => Ok(Self::Default),
            other => Err(format!("unknown queue {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobId(pub i64);

#[derive(Debug, Clone)]
pub struct JobPayload {
    pub kind: String,
    pub args: Value,
    /// Not before this instant; `None` = now.
    pub run_at: Option<DateTime<Utc>>,
}

impl JobPayload {
    pub fn new(kind: impl Into<String>, args: Value) -> Self {
        Self {
            kind: kind.into(),
            args,
            run_at: None,
        }
    }

    pub fn run_at(mut self, at: DateTime<Utc>) -> Self {
        self.run_at = Some(at);
        self
    }
}

/// Enqueues inside the caller's transaction: rolled back with it.
pub async fn enqueue(
    tx: &mut PgTx,
    job: JobPayload,
    queue: QueueName,
) -> Result<JobId, sqlx::Error> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO jobs (queue, kind, args, run_at) \
         VALUES ($1, $2, $3, COALESCE($4, now())) RETURNING id",
    )
    .bind(queue.as_str())
    .bind(&job.kind)
    .bind(&job.args)
    .bind(job.run_at)
    .fetch_one(&mut **tx)
    .await?;
    Ok(JobId(id))
}

/// Pauses claiming on `queue` for every replica. Running jobs finish.
pub async fn pause(db: &PgPool, queue: QueueName) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO queue_pauses (queue) VALUES ($1) ON CONFLICT (queue) DO NOTHING")
        .bind(queue.as_str())
        .execute(db)
        .await?;
    Ok(())
}

pub async fn resume(db: &PgPool, queue: QueueName) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM queue_pauses WHERE queue = $1")
        .bind(queue.as_str())
        .execute(db)
        .await?;
    Ok(())
}

pub async fn is_paused(db: &PgPool, queue: QueueName) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM queue_pauses WHERE queue = $1)")
        .bind(queue.as_str())
        .fetch_one(db)
        .await
}

#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub id: JobId,
    pub kind: String,
    pub args: Value,
    /// Including this one.
    pub attempts: i32,
}

/// Atomically claims the oldest ready job of `queue` for `worker`; `None`
/// when nothing is ready, the queue is paused, or `worker` is not registered
/// (a pruned worker must not take claims no one would protect).
///
/// Known, accepted: under READ COMMITTED, if the row picked by the subquery
/// is modified by a concurrent transaction before its lock is taken, the
/// `LIMIT 1` may yield no row although others are ready. The effect is one
/// `polling_interval` of delay, never a double claim or an error.
pub async fn claim(
    db: &PgPool,
    queue: QueueName,
    worker: Uuid,
) -> Result<Option<ClaimedJob>, sqlx::Error> {
    let row: Option<(i64, String, Value, i32)> = sqlx::query_as(
        "UPDATE jobs SET claimed_by = $2, claimed_at = now(), attempts = attempts + 1 \
         WHERE id = ( \
           SELECT id FROM jobs \
           WHERE queue = $1 AND finished_at IS NULL AND failed_at IS NULL \
             AND claimed_by IS NULL AND run_at <= now() \
             AND NOT EXISTS (SELECT 1 FROM queue_pauses p WHERE p.queue = $1) \
             AND EXISTS (SELECT 1 FROM job_workers w WHERE w.id = $2) \
           ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1) \
         RETURNING id, kind, args, attempts",
    )
    .bind(queue.as_str())
    .bind(worker)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id, kind, args, attempts)| ClaimedJob {
        id: JobId(id),
        kind,
        args,
        attempts,
    }))
}

/// Marks the job finished. `false` when `worker` no longer owns the claim
/// (it was recovered after a missed heartbeat): the result is ignored.
pub async fn complete(db: &PgPool, id: JobId, worker: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE jobs SET finished_at = now(), claimed_by = NULL, claimed_at = NULL \
         WHERE id = $1 AND claimed_by = $2",
    )
    .bind(id.0)
    .bind(worker)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Records a failed attempt: re-scheduled after `retry_in`, or terminally
/// failed when `retry_in` is `None`. `false` when the claim was lost.
pub async fn fail(
    db: &PgPool,
    id: JobId,
    worker: Uuid,
    error: &str,
    retry_in: Option<Duration>,
) -> Result<bool, sqlx::Error> {
    let done = match retry_in {
        Some(delay) => {
            sqlx::query(
                "UPDATE jobs SET claimed_by = NULL, claimed_at = NULL, last_error = $3, \
                 run_at = now() + make_interval(secs => $4) \
                 WHERE id = $1 AND claimed_by = $2",
            )
            .bind(id.0)
            .bind(worker)
            .bind(error)
            .bind(delay.as_secs_f64())
            .execute(db)
            .await?
        }
        None => {
            sqlx::query(
                "UPDATE jobs SET claimed_by = NULL, claimed_at = NULL, last_error = $3, \
                 failed_at = now() WHERE id = $1 AND claimed_by = $2",
            )
            .bind(id.0)
            .bind(worker)
            .bind(error)
            .execute(db)
            .await?
        }
    };
    Ok(done.rows_affected() == 1)
}

/// Gives back every unfinished claim of `worker` (graceful shutdown past the
/// timeout). The interrupted attempt is not counted.
pub async fn release_claims(db: &PgPool, worker: Uuid) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE jobs SET claimed_by = NULL, claimed_at = NULL, attempts = GREATEST(attempts - 1, 0) \
         WHERE claimed_by = $1 AND finished_at IS NULL AND failed_at IS NULL",
    )
    .bind(worker)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// What one recovery pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Recovered {
    /// Claims of dead workers given back to the queue.
    pub requeued: u64,
    /// Claims of dead workers marked failed.
    pub failed: u64,
}

/// [`recover_abandoned_requeuing`] with nothing to re-queue: every orphan
/// claim is failed. Returns the number of jobs failed.
pub async fn recover_abandoned(db: &PgPool, expiry: Duration) -> Result<u64, sqlx::Error> {
    Ok(recover_abandoned_requeuing(db, expiry, &[]).await?.failed)
}

/// Recovers the claims whose owner has no heartbeat within `expiry` (or no
/// registration at all) and forgets those dead workers.
///
/// Like Solid Queue's `Process#prune` (`ProcessPrunedError` ->
/// `fail_all_claimed_executions_with`) a crashed claim is a *failure*: the
/// attempt stays counted and the job is not retried by the queue, so a job
/// that kills its process cannot loop forever (the application's reconcile
/// jobs re-enqueue what matters). The exception is `requeue`, a list of
/// `(kind, max_attempts)`: those jobs are at-least-once (a callback delivery)
/// and go back to the queue, due now, as long as `attempts < max_attempts`.
/// The attempt that was running stays counted, so the loop is bounded.
pub async fn recover_abandoned_requeuing(
    db: &PgPool,
    expiry: Duration,
    requeue: &[(&str, i32)],
) -> Result<Recovered, sqlx::Error> {
    let kinds: Vec<String> = requeue.iter().map(|(k, _)| (*k).to_owned()).collect();
    let maxes: Vec<i32> = requeue.iter().map(|(_, m)| *m).collect();
    // One statement per outcome, both on the same predicate "owner is gone".
    let requeued = sqlx::query(
        "UPDATE jobs SET claimed_by = NULL, claimed_at = NULL, run_at = now(), \
           last_error = 'Worker heartbeat expired mid-run: re-queued (at least once)' \
         WHERE claimed_by IS NOT NULL AND finished_at IS NULL AND failed_at IS NULL \
           AND NOT EXISTS (SELECT 1 FROM job_workers w WHERE w.id = jobs.claimed_by \
                           AND w.last_heartbeat_at > now() - make_interval(secs => $1)) \
           AND EXISTS (SELECT 1 FROM unnest($2::text[], $3::int[]) AS r(kind, max_attempts) \
                       WHERE r.kind = jobs.kind AND jobs.attempts < r.max_attempts)",
    )
    .bind(expiry.as_secs_f64())
    .bind(&kinds)
    .bind(&maxes)
    .execute(db)
    .await?;
    let failed = sqlx::query(
        "UPDATE jobs SET claimed_by = NULL, claimed_at = NULL, failed_at = now(), \
           last_error = 'Process pruned: worker heartbeat expired (the worker process died or stalled)' \
         WHERE claimed_by IS NOT NULL AND finished_at IS NULL AND failed_at IS NULL \
           AND NOT EXISTS (SELECT 1 FROM job_workers w WHERE w.id = jobs.claimed_by \
                           AND w.last_heartbeat_at > now() - make_interval(secs => $1))",
    )
    .bind(expiry.as_secs_f64())
    .execute(db)
    .await?;
    sqlx::query(
        "DELETE FROM job_workers WHERE last_heartbeat_at <= now() - make_interval(secs => $1)",
    )
    .bind(expiry.as_secs_f64())
    .execute(db)
    .await?;
    Ok(Recovered {
        requeued: requeued.rows_affected(),
        failed: failed.rows_affected(),
    })
}

/// Deletes finished jobs older than `older_than`, at most `batch` per call
/// (failed jobs stay for diagnosis). Returns the number deleted.
pub async fn clear_finished(
    db: &PgPool,
    older_than: Duration,
    batch: i64,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM jobs WHERE id IN ( \
           SELECT id FROM jobs WHERE finished_at < now() - make_interval(secs => $1) \
           ORDER BY id LIMIT $2)",
    )
    .bind(older_than.as_secs_f64())
    .bind(batch)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}
