//! `ged worker`: the pools of `config/queue.yml`, the scheduler and the
//! heartbeat, stopped cleanly by SIGTERM/SIGINT.

use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use tokio::sync::watch;

use super::Registry;
use super::deliver_callback::{CallbackPolicy, DeliverCallbackJob, production_policy};
use super::env::PipelineEnv;
use super::heartbeat::HeartbeatConfig;
use super::pool::{Runner, RunnerConfig};
use super::purge::PurgeDeletedDocumentsJob;
use super::queue::QueueName;
use super::reconcile::ReconcileUploadRequestsJob;
use super::scheduler::{self, ClearFinishedJobs};
use super::store_document::StoreDocumentJob;

/// The `default` pool is fixed: 2 threads, 1 process.
const DEFAULT_POOL_THREADS: usize = 2;
const SCHEDULER_TICK: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSettings {
    /// `JOB_THREADS` (default 3).
    pub threads: usize,
    /// `JOB_CONCURRENCY` (default 1): uploads processes per container.
    pub concurrency: usize,
    /// `SOLID_QUEUE_SHUTDOWN_TIMEOUT` seconds (default 30).
    pub shutdown_timeout: Duration,
    /// `SOLID_QUEUE_PROCESS_HEARTBEAT_INTERVAL` / `..._ALIVE_THRESHOLD`
    /// seconds (defaults 60 / 300).
    pub heartbeat: HeartbeatConfig,
}

fn seconds(raw: Option<String>) -> Option<Duration> {
    raw.and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(Duration::from_secs_f64)
}

/// An interval that is not shorter than the threshold would get every healthy
/// worker pruned: fall back to the defaults.
fn heartbeat_from(lookup: &dyn Fn(&str) -> Option<String>) -> HeartbeatConfig {
    let default = HeartbeatConfig::default();
    let cfg = HeartbeatConfig {
        interval: seconds(lookup("SOLID_QUEUE_PROCESS_HEARTBEAT_INTERVAL"))
            .unwrap_or(default.interval),
        expiry: seconds(lookup("SOLID_QUEUE_PROCESS_ALIVE_THRESHOLD")).unwrap_or(default.expiry),
    };
    if cfg.interval < cfg.expiry {
        cfg
    } else {
        default
    }
}

/// Ruby `Integer(x)`-ish: a positive integer, else the default.
fn positive(raw: Option<String>, default: usize) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

impl WorkerSettings {
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let secs = lookup("SOLID_QUEUE_SHUTDOWN_TIMEOUT")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
            .unwrap_or(30.0);
        Self {
            threads: positive(lookup("JOB_THREADS"), 3),
            concurrency: positive(lookup("JOB_CONCURRENCY"), 1),
            shutdown_timeout: Duration::from_secs_f64(secs),
            heartbeat: heartbeat_from(lookup),
        }
    }

    pub fn from_process_env() -> Self {
        Self::from_lookup(&|k| std::env::var(k).ok())
    }

    /// One entry per worker process, as in `config/queue.yml`.
    pub fn runners(&self) -> Vec<RunnerConfig> {
        let base = |queue, threads, polling_ms| RunnerConfig {
            queue,
            threads,
            polling_interval: Duration::from_millis(polling_ms),
            heartbeat: self.heartbeat.clone(),
            shutdown_timeout: self.shutdown_timeout,
        };
        let mut all: Vec<_> = (0..self.concurrency)
            .map(|_| base(QueueName::Uploads, self.threads, 100))
            .collect();
        all.push(base(QueueName::Default, DEFAULT_POOL_THREADS, 500));
        all
    }

    /// Connections: every claim loop, plus heartbeat/scheduler headroom.
    pub fn db_connections(&self) -> u32 {
        let loops = self.threads * self.concurrency + DEFAULT_POOL_THREADS;
        u32::try_from(loops + 2 * (self.concurrency + 1) + 2).unwrap_or(u32::MAX)
    }
}

/// Handlers known to this binary: housekeeping plus the four pipeline jobs.
pub fn registry(env: &PipelineEnv, policy: Arc<dyn CallbackPolicy>) -> Registry {
    let mut r = Registry::new();
    r.register(Arc::new(ClearFinishedJobs { db: env.db.clone() }));
    r.register(Arc::new(StoreDocumentJob::new(env.clone())));
    r.register(Arc::new(DeliverCallbackJob::new(env.clone(), policy)));
    r.register(Arc::new(ReconcileUploadRequestsJob::new(env.clone())));
    r.register(Arc::new(PurgeDeletedDocumentsJob::new(env.clone())));
    r
}

/// `ged run-job`: performs one job in the foreground (`Job.perform_now`).
/// `args` is the job's JSON argument object (`{}` when absent).
pub async fn perform_now(
    database_url: &str,
    kind: &str,
    args: Option<&str>,
    max_upload_bytes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await?;
    let env = PipelineEnv::from_config(db.clone(), Some(max_upload_bytes));
    let registry = registry(&env, production_policy());
    let job = registry
        .get(kind)
        .ok_or_else(|| format!("unknown job {kind}"))?;
    let args: serde_json::Value = match args {
        Some(raw) => serde_json::from_str(raw)?,
        None => serde_json::json!({}),
    };
    let result = job.perform(args).await;
    db.close().await;
    result.map_err(|e| {
        match e {
            super::JobError::Retry(m) | super::JobError::Discard(m) => m,
        }
        .into()
    })
}

/// Runs until SIGTERM or Ctrl-C, then shuts down within the timeout.
pub async fn run(
    database_url: &str,
    settings: WorkerSettings,
    max_upload_bytes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(settings.db_connections())
        .connect(database_url)
        .await?;
    let env = PipelineEnv::from_config(db.clone(), Some(max_upload_bytes));
    // Integration (T7 lands `policy::callback_url`): pass its `decide` adapter here.
    let registry = Arc::new(registry(&env, production_policy()));
    let (stop, rx) = watch::channel(false);

    let mut runners = tokio::task::JoinSet::new();
    for cfg in settings.runners() {
        let runner = Runner::new(db.clone(), registry.clone(), cfg);
        let rx = rx.clone();
        runners.spawn(async move { runner.run(rx).await });
    }
    let tasks: Vec<_> = scheduler::default_schedule()
        .into_iter()
        .filter(|t| registry.has(&t.kind))
        .collect();
    let scheduler = tokio::spawn(scheduler::run(db.clone(), tasks, SCHEDULER_TICK, rx));

    wait_for_signal().await?;
    eprintln!("ged worker: shutting down");
    let _ = stop.send(true);

    let mut failure = None;
    while let Some(joined) = runners.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => failure = Some(e.to_string()),
            Err(e) => failure = Some(e.to_string()),
        }
    }
    let _ = scheduler.await;
    db.close().await;
    failure.map_or(Ok(()), |e| Err(e.into()))
}

async fn wait_for_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = term.recv() => {}
        r = tokio::signal::ctrl_c() => r?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(pairs: &[(&str, &str)]) -> WorkerSettings {
        let map: HashMap<_, _> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        WorkerSettings::from_lookup(&|k| map.get(k).cloned())
    }

    #[tokio::test]
    async fn the_registry_knows_every_pipeline_kind() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@localhost/unused")
            .unwrap();
        let env = PipelineEnv::from_config(db, None);
        let r = registry(&env, production_policy());
        for kind in [
            "StoreDocumentJob",
            "DeliverCallbackJob",
            "ReconcileUploadRequestsJob",
            "PurgeDeletedDocumentsJob",
        ] {
            assert!(r.has(kind), "{kind}");
        }
        // every scheduled task now has a handler, so none is filtered out
        assert!(
            scheduler::default_schedule().iter().all(|t| r.has(&t.kind)),
            "a scheduled kind has no handler"
        );
    }

    #[test]
    fn defaults_follow_queue_yml() {
        let s = settings(&[]);
        assert_eq!((s.threads, s.concurrency), (3, 1));
        assert_eq!(s.shutdown_timeout, Duration::from_secs(30));
        let runners = s.runners();
        assert_eq!(runners.len(), 2);
        assert_eq!(
            (runners[0].queue, runners[0].threads),
            (QueueName::Uploads, 3)
        );
        assert_eq!(
            (runners[1].queue, runners[1].threads),
            (QueueName::Default, 2)
        );
    }

    #[test]
    fn env_knobs_scale_the_uploads_pool_only() {
        let s = settings(&[
            ("JOB_THREADS", "5"),
            ("JOB_CONCURRENCY", "2"),
            ("SOLID_QUEUE_SHUTDOWN_TIMEOUT", "7.5"),
        ]);
        let runners = s.runners();
        assert_eq!(runners.len(), 3);
        assert!(
            runners[..2]
                .iter()
                .all(|r| r.queue == QueueName::Uploads && r.threads == 5)
        );
        assert_eq!(runners[2].queue, QueueName::Default);
        assert_eq!(s.shutdown_timeout, Duration::from_millis(7500));
        assert!(s.db_connections() >= 12);
    }

    #[test]
    fn nonsense_falls_back_to_defaults() {
        let s = settings(&[
            ("JOB_THREADS", "0"),
            ("JOB_CONCURRENCY", "x"),
            ("SOLID_QUEUE_SHUTDOWN_TIMEOUT", "-3"),
        ]);
        assert_eq!(s, settings(&[]));
    }

    #[test]
    fn heartbeat_follows_solid_queue_defaults_and_its_env_knobs() {
        let d = settings(&[]).runners();
        assert_eq!(d[0].heartbeat.interval, Duration::from_secs(60));
        assert_eq!(d[0].heartbeat.expiry, Duration::from_secs(300));
        let s = settings(&[
            ("SOLID_QUEUE_PROCESS_HEARTBEAT_INTERVAL", "0.5"),
            ("SOLID_QUEUE_PROCESS_ALIVE_THRESHOLD", "2"),
        ]);
        for r in s.runners() {
            assert_eq!(r.heartbeat.interval, Duration::from_millis(500));
            assert_eq!(r.heartbeat.expiry, Duration::from_secs(2));
        }
        // Nonsense (or an interval that would outlive the threshold) falls back.
        let bad = settings(&[
            ("SOLID_QUEUE_PROCESS_HEARTBEAT_INTERVAL", "0"),
            ("SOLID_QUEUE_PROCESS_ALIVE_THRESHOLD", "nope"),
        ]);
        assert_eq!(bad, settings(&[]));
    }
}
