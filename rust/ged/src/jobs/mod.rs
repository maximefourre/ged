//! Postgres-backed background jobs (replaces Solid Queue).
//!
//! The queue lives in the same database as the business data, so
//! [`queue::enqueue`] joins the caller's transaction. Two isolated pools
//! (`uploads`, `default`) claim with `FOR UPDATE SKIP LOCKED`; there is
//! deliberately no catch-all pool, so an uploads backlog cannot starve
//! callbacks or reconciliation.

pub mod heartbeat;
pub mod pool;
pub mod queue;
pub mod scheduler;
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
}
