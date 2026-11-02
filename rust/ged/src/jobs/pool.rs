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
use super::{JobError, Registry};

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
        let beat = tokio::spawn(heartbeat_loop(
            self.db.clone(),
            worker,
            cfg.heartbeat.clone(),
            beat_rx,
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
            queue::release_claims(&self.db, worker).await?;
        }

        let _ = beat_stop.send(true);
        let _ = beat.await;
        heartbeat::deregister(&self.db, worker).await
    }
}

async fn heartbeat_loop(
    db: PgPool,
    worker: Uuid,
    cfg: HeartbeatConfig,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if let Err(e) = heartbeat::beat(&db, worker).await {
            eprintln!("ged worker: heartbeat failed: {e}");
        }
        match queue::recover_abandoned(&db, cfg.expiry).await {
            Ok(0) => {}
            Ok(n) => eprintln!("ged worker: recovered {n} abandoned job(s)"),
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
) {
    while !*shutdown.borrow() {
        match queue::claim(&db, queue, worker).await {
            Ok(Some(job)) => run_one(&db, &registry, worker, job).await,
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

async fn run_one(db: &PgPool, registry: &Registry, worker: Uuid, job: ClaimedJob) {
    let outcome = match registry.get(&job.kind) {
        None => Err(JobError::Discard(format!("no handler for {:?}", job.kind))),
        Some(handler) => {
            // Own task: a panic becomes a failed attempt, not a dead loop.
            let task = tokio::spawn(handler.perform(job.args.clone()));
            let _guard = AbortOnDrop(task.abort_handle());
            match task.await {
                Ok(result) => result,
                Err(e) if e.is_panic() => Err(JobError::Retry("job panicked".into())),
                Err(e) => Err(JobError::Retry(format!("job aborted: {e}"))),
            }
        }
    };
    let result = match outcome {
        Ok(()) => queue::complete(db, job.id, worker).await,
        Err(JobError::Discard(msg)) => queue::fail(db, job.id, worker, &msg, None).await,
        Err(JobError::Retry(msg)) => {
            let handler = registry.get(&job.kind);
            let max = handler.map_or(1, |h| h.max_attempts());
            let retry_in = (job.attempts < max)
                .then(|| handler.map_or(Duration::ZERO, |h| h.backoff(job.attempts)));
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
