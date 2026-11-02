//! One worker process: `threads` claim loops on a single queue, a heartbeat
//! task, and a bounded graceful shutdown.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinSet};
use uuid::Uuid;

use super::heartbeat::{self, HeartbeatConfig};
use super::queue::{self, ClaimedJob, QueueName};
use super::{JobError, Registry, polynomial_backoff};

/// A job whose `kind` this binary does not know (rolling deploy) is retried
/// this many times before it is given up on.
pub const UNKNOWN_KIND_MAX_ATTEMPTS: u32 = 8;

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub queue: QueueName,
    pub threads: usize,
    pub polling_interval: Duration,
    pub heartbeat: HeartbeatConfig,
    /// `SOLID_QUEUE_SHUTDOWN_TIMEOUT`.
    pub shutdown_timeout: Duration,
}

pub struct Runner {
    db: PgPool,
    registry: Arc<Registry>,
    config: RunnerConfig,
}

/// Aborts the wrapped task when dropped, so cancelling a claim loop also
/// cancels the job it was running.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Runner {
    pub fn new(db: PgPool, registry: Arc<Registry>, config: RunnerConfig) -> Self {
        Self {
            db,
            registry,
            config,
        }
    }

    /// Runs until `shutdown` turns `true`, then stops claiming, lets running
    /// jobs finish for at most `shutdown_timeout`, releases what is left and
    /// deregisters.
    pub async fn run(self, shutdown: watch::Receiver<bool>) -> Result<(), sqlx::Error> {
        let cfg = &self.config;
        let worker = heartbeat::register(&self.db, cfg.queue.as_str()).await?;

        let (beat_stop, beat_rx) = watch::channel(false);
        // Bumped each time the heartbeat finds the registration gone.
        let (lost_tx, lost_rx) = watch::channel(0_u64);
        let beat = tokio::spawn(heartbeat_loop(
            self.db.clone(),
            worker,
            cfg.queue,
            cfg.heartbeat.clone(),
            beat_rx,
            lost_tx,
        ));

        let mut loops = JoinSet::new();
        for _ in 0..cfg.threads.max(1) {
            loops.spawn(claim_loop(
                self.db.clone(),
                self.registry.clone(),
                cfg.queue,
                worker,
                cfg.polling_interval,
                shutdown.clone(),
                lost_rx.clone(),
            ));
        }

        let mut stop = shutdown;
        let _ = stop.wait_for(|down| *down).await;

        let drained = tokio::time::timeout(cfg.shutdown_timeout, async {
            while loops.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        if !drained {
            eprintln!(
                "ged worker: shutdown timeout ({:?}) reached, releasing running jobs",
                cfg.shutdown_timeout
            );
            loops.abort_all();
            while loops.join_next().await.is_some() {}
        }
        // Best effort: whatever fails below, still stop the heartbeat and
        // deregister (a stale row would otherwise linger until pruned), then
        // report the first error.
        let released = if drained {
            Ok(0)
        } else {
            queue::release_claims(&self.db, worker).await
        };
        if let Err(e) = &released {
            eprintln!("ged worker: could not release running jobs: {e}");
        }

        let _ = beat_stop.send(true);
        let _ = beat.await;
        let deregistered = heartbeat::deregister(&self.db, worker).await;
        released.map(|_| ()).and(deregistered)
    }
}

async fn heartbeat_loop(
    db: PgPool,
    worker: Uuid,
    queue: QueueName,
    cfg: HeartbeatConfig,
    mut stop: watch::Receiver<bool>,
    lost: watch::Sender<u64>,
) {
    loop {
        match heartbeat::beat(&db, worker, queue.as_str()).await {
            Ok(false) => {}
            Ok(true) => {
                // A peer pruned us while we were stalled: our claims were
                // failed. Re-registered; running jobs must stop.
                eprintln!(
                    "ged worker: registration was pruned, re-registered; cancelling running jobs"
                );
                lost.send_modify(|epoch| *epoch += 1);
            }
            Err(e) => eprintln!("ged worker: heartbeat failed: {e}"),
        }
        match queue::recover_abandoned(&db, cfg.expiry).await {
            Ok(0) => {}
            Ok(n) => eprintln!("ged worker: failed {n} job(s) abandoned by dead workers"),
            Err(e) => eprintln!("ged worker: recovery failed: {e}"),
        }
        tokio::select! {
            () = tokio::time::sleep(cfg.interval) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

async fn claim_loop(
    db: PgPool,
    registry: Arc<Registry>,
    queue: QueueName,
    worker: Uuid,
    polling: Duration,
    mut shutdown: watch::Receiver<bool>,
    mut lost: watch::Receiver<u64>,
) {
    while !*shutdown.borrow() {
        let epoch = *lost.borrow_and_update();
        match queue::claim(&db, queue, worker).await {
            Ok(Some(job)) => run_one(&db, &registry, worker, job, &mut lost, epoch).await,
            Ok(None) => idle(polling, &mut shutdown).await,
            Err(e) => {
                eprintln!("ged worker: claim failed: {e}");
                idle(polling.max(Duration::from_secs(1)), &mut shutdown).await;
            }
        }
    }
}

async fn idle(polling: Duration, shutdown: &mut watch::Receiver<bool>) {
    tokio::select! {
        () = tokio::time::sleep(polling) => {}
        _ = shutdown.wait_for(|s| *s) => {}
    }
}

async fn run_one(
    db: &PgPool,
    registry: &Registry,
    worker: Uuid,
    job: ClaimedJob,
    lost: &mut watch::Receiver<u64>,
    epoch: u64,
) {
    let handler = registry.get(&job.kind).cloned();
    let outcome = match &handler {
        // Not deployed here (yet): a rolling deploy, not a poison job.
        None => Err(JobError::Retry(format!("no handler for {:?}", job.kind))),
        Some(handler) => {
            // Own task, `perform` included: a panic -- even before the future
            // exists -- becomes a failed attempt, not a dead loop.
            let (handler, args) = (handler.clone(), job.args.clone());
            let task = tokio::spawn(async move { handler.perform(args).await });
            let _guard = AbortOnDrop(task.abort_handle());
            tokio::select! {
                joined = task => match joined {
                    Ok(result) => result,
                    Err(e) if e.is_panic() => Err(JobError::Retry("job panicked".into())),
                    Err(e) => Err(JobError::Retry(format!("job aborted: {e}"))),
                },
                // Our registration was pruned meanwhile: the claim is gone
                // and a peer may re-run the job, so stop this run now.
                _ = lost.wait_for(|e| *e != epoch) => {
                    Err(JobError::Retry("worker registration lost".into()))
                }
            }
        }
    };
    let result = match outcome {
        Ok(()) => queue::complete(db, job.id, worker).await,
        Err(JobError::Discard(msg)) => queue::fail(db, job.id, worker, &msg, None).await,
        Err(JobError::Retry(msg)) => {
            let (max, backoff) = match &handler {
                Some(h) => (h.max_attempts(), h.backoff(job.attempts)),
                None => (
                    i32::try_from(UNKNOWN_KIND_MAX_ATTEMPTS).unwrap_or(i32::MAX),
                    polynomial_backoff(job.attempts),
                ),
            };
            let retry_in = (job.attempts < max).then_some(backoff);
            queue::fail(db, job.id, worker, &msg, retry_in).await
        }
    };
    match result {
        Ok(true) => {}
        Ok(false) => eprintln!(
            "ged worker: job {} was recovered by another worker",
            job.id.0
        ),
        Err(e) => eprintln!("ged worker: could not record job {}: {e}", job.id.0),
    }
}
