//! Postgres-backed background jobs (replaces Solid Queue).
//!
//! The queue lives in the same database as the business data, so
//! [`queue::enqueue`] joins the caller's transaction. Two isolated pools
//! (`uploads`, `default`) claim with `FOR UPDATE SKIP LOCKED`; there is
//! deliberately no catch-all pool, so an uploads backlog cannot starve
//! callbacks or reconciliation.

pub mod deliver_callback;
pub mod env;
pub mod heartbeat;
pub mod pool;
pub mod purge;
pub mod queue;
pub mod reconcile;
pub mod scheduler;
pub mod store_document;
pub mod worker;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::Value;

/// How a failed `perform` is handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// Try again later, until `Job::max_attempts` is reached.
    Retry(String),
    /// Give up now (Active Job's `discard_on` / an early `return`).
    Discard(String),
}

/// A unit of background work, registered by `kind`.
pub trait Job: Send + Sync + 'static {
    fn kind(&self) -> &'static str;

    fn max_attempts(&self) -> i32 {
        1
    }

    /// Delay before attempt `attempt + 1`, `attempt` being the one that failed.
    fn backoff(&self, attempt: i32) -> Duration {
        polynomial_backoff(attempt)
    }

    fn perform(&self, args: Value) -> BoxFuture<'static, Result<(), JobError>>;

    /// Called once when a `Retry` used the last attempt (Active Job's
    /// `retry_on ... do |job, error|` block). Nothing by default.
    fn exhausted(&self, _args: &Value, _error: &str) {}
}

/// Active Job's `:polynomially_longer`, without jitter: `attempt^4 + 2` s.
pub fn polynomial_backoff(attempt: i32) -> Duration {
    let n = u64::try_from(attempt.max(1)).unwrap_or(1);
    Duration::from_secs(n.saturating_pow(4).saturating_add(2))
}

/// Job handlers by `kind`.
#[derive(Default)]
pub struct Registry {
    jobs: HashMap<&'static str, Arc<dyn Job>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, job: Arc<dyn Job>) {
        self.jobs.insert(job.kind(), job);
    }

    pub fn get(&self, kind: &str) -> Option<&Arc<dyn Job>> {
        self.jobs.get(kind)
    }

    pub fn has(&self, kind: &str) -> bool {
        self.jobs.contains_key(kind)
    }

    /// Registered kind names, sorted. The C4 guard uses this so a new job
    /// that never appears in `docs/architecture/c4.md` fails the build.
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.jobs.keys().copied().collect();
        names.sort_unstable();
        names
    }
}

/// Records `ged.job.duration` around a job body (the Rails
/// `perform.active_job` subscriber), whatever its outcome.
pub async fn timed<T>(
    class: crate::telemetry::JobClass,
    queue: crate::telemetry::JobQueue,
    body: impl std::future::Future<Output = T>,
) -> T {
    let started = std::time::Instant::now();
    let out = body.await;
    crate::telemetry::job_duration(class, queue, started.elapsed().as_secs_f64());
    out
}
