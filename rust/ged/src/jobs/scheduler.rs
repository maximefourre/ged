//! Recurring tasks (port of `config/recurring.yml`), de-duplicated across
//! replicas by `(task_key, run_at)`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::watch;

use super::queue::{self, JobPayload, QueueName};
use super::{Job, JobError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// `every N minutes`, aligned on the epoch.
    EveryMinutes(u32),
    /// `every hour at minute M`.
    HourlyAt(u32),
    /// `every day at H` (UTC).
    DailyAt(u32),
}

impl Cadence {
    /// Latest slot not after `now`.
    pub fn slot(self, now: DateTime<Utc>) -> DateTime<Utc> {
        let (period, offset) = match self {
            Self::EveryMinutes(n) => (i64::from(n.max(1)) * 60, 0),
            Self::HourlyAt(m) => (3_600, i64::from(m.min(59)) * 60),
            Self::DailyAt(h) => (86_400, i64::from(h.min(23)) * 3_600),
        };
        let t = now.timestamp();
        let slot = (t - offset).div_euclid(period) * period + offset;
        Utc.timestamp_opt(slot, 0).single().unwrap_or(now)
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub key: String,
    pub kind: String,
    pub queue: QueueName,
    pub cadence: Cadence,
}

/// `reconcile_upload_requests` / `clear_finished_jobs` /
/// `purge_deleted_documents` of `config/recurring.yml`.
pub fn default_schedule() -> Vec<ScheduledTask> {
    let task = |key: &str, kind: &str, cadence| ScheduledTask {
        key: key.into(),
        kind: kind.into(),
        queue: QueueName::Default,
        cadence,
    };
    vec![
        task(
            "reconcile_upload_requests",
            super::reconcile::KIND,
            Cadence::EveryMinutes(10),
        ),
        task("clear_finished_jobs", CLEAR_FINISHED, Cadence::HourlyAt(12)),
        task(
            "purge_deleted_documents",
            super::purge::KIND,
            Cadence::DailyAt(3),
        ),
    ]
}

/// Enqueues each task's current slot unless another replica already did.
/// Claim and enqueue share one transaction. Returns how many this call enqueued.
pub async fn tick(
    db: &PgPool,
    tasks: &[ScheduledTask],
    now: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let mut enqueued = 0;
    for task in tasks {
        let slot = task.cadence.slot(now);
        let mut tx = db.begin().await?;
        let claimed = sqlx::query(
            "INSERT INTO scheduled_runs (task_key, run_at) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(&task.key)
        .bind(slot)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if claimed {
            let payload =
                JobPayload::new(&task.kind, json!({ "scheduled_for": slot.to_rfc3339() }));
            queue::enqueue(&mut tx, payload, task.queue).await?;
            enqueued += 1;
        }
        tx.commit().await?;
    }
    Ok(enqueued)
}

/// Ticks every `every` until `shutdown`. Also prunes old dedup rows.
pub async fn run(
    db: PgPool,
    tasks: Vec<ScheduledTask>,
    every: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if let Err(e) = tick(&db, &tasks, Utc::now()).await {
            eprintln!("ged worker: scheduler tick failed: {e}");
        }
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            _ = shutdown.wait_for(|s| *s) => return,
        }
    }
}

pub const CLEAR_FINISHED: &str = "clear_finished_jobs";

/// `SolidQueue::Job.clear_finished_in_batches`: finished jobs older than a
/// day, in batches, plus the matching scheduler dedup rows.
pub struct ClearFinishedJobs {
    pub db: PgPool,
}

impl Job for ClearFinishedJobs {
    fn kind(&self) -> &'static str {
        CLEAR_FINISHED
    }

    fn perform(&self, _args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        let db = Arc::new(self.db.clone());
        Box::pin(async move {
            let day = Duration::from_secs(86_400);
            let err = |e: sqlx::Error| JobError::Retry(e.to_string());
            while queue::clear_finished(&db, day, 500).await.map_err(err)? == 500 {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            sqlx::query("DELETE FROM scheduled_runs WHERE run_at < now() - interval '1 day'")
                .execute(&*db)
                .await
                .map_err(err)?;
            Ok(())
        })
    }
}
