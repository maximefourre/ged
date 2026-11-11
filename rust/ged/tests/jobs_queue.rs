//! T8: the Postgres-backed job queue, against scratch databases on the
//! worktree's Dockerised Postgres (`bin/worktree-stack db`).

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use futures_util::future::BoxFuture;
use ged::jobs::heartbeat::{self, HeartbeatConfig};
use ged::jobs::pool::{Runner, RunnerConfig};
use ged::jobs::queue::{self, JobPayload, QueueName};
use ged::jobs::scheduler::{self, Cadence, ScheduledTask};
use ged::jobs::{Job, JobError, Registry};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::ScratchDb;
use tokio::sync::{Notify, watch};

async fn migrated() -> (ScratchDb, PgPool) {
    let db = ScratchDb::create().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(30)
        .connect(&db.url)
        .await
        .unwrap();
    ged::db::migrate(&pool).await.unwrap();
    (db, pool)
}

async fn finish(db: ScratchDb, pool: PgPool) {
    pool.close().await;
    db.drop_db().await;
}

type Handler = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<(), JobError>> + Send + Sync>;

struct FnJob {
    kind: &'static str,
    max_attempts: i32,
    handler: Handler,
}

impl Job for FnJob {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn max_attempts(&self) -> i32 {
        self.max_attempts
    }
    fn backoff(&self, _attempt: i32) -> Duration {
        Duration::ZERO
    }
    fn perform(&self, args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        (self.handler)(args)
    }
}

fn job<F, Fut>(kind: &'static str, max_attempts: i32, f: F) -> Arc<dyn Job>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), JobError>> + Send + 'static,
{
    Arc::new(FnJob {
        kind,
        max_attempts,
        handler: Arc::new(move |a| Box::pin(f(a))),
    })
}

fn registry(jobs: Vec<Arc<dyn Job>>) -> Arc<Registry> {
    let mut r = Registry::new();
    for j in jobs {
        r.register(j);
    }
    Arc::new(r)
}

fn fast(queue: QueueName, threads: usize) -> RunnerConfig {
    RunnerConfig {
        queue,
        threads,
        polling_interval: Duration::from_millis(20),
        heartbeat: HeartbeatConfig {
            interval: Duration::from_millis(100),
            expiry: Duration::from_secs(30),
        },
        shutdown_timeout: Duration::from_secs(5),
    }
}

struct Running {
    stop: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        self.stop.send(true).unwrap();
        self.handle.await.unwrap();
    }
}

fn start(pool: &PgPool, reg: &Arc<Registry>, cfg: RunnerConfig) -> Running {
    let (stop, rx) = watch::channel(false);
    let runner = Runner::new(pool.clone(), reg.clone(), cfg);
    let handle = tokio::spawn(async move { runner.run(rx).await.unwrap() });
    Running { stop, handle }
}

async fn enqueue(pool: &PgPool, kind: &str, args: Value, q: QueueName) -> queue::JobId {
    let mut tx = pool.begin().await.unwrap();
    let id = queue::enqueue(&mut tx, JobPayload::new(kind, args), q)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap()
}

// --- enqueue ---------------------------------------------------------------

#[tokio::test]
async fn enqueue_is_rolled_back_with_its_transaction() {
    let (db, pool) = migrated().await;
    let mut tx = pool.begin().await.unwrap();
    queue::enqueue(&mut tx, JobPayload::new("x", json!({})), QueueName::Uploads)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM jobs").await, 0);

    let id = enqueue(&pool, "x", json!({"a": 1}), QueueName::Default).await;
    assert_eq!(count(&pool, "SELECT count(*) FROM jobs").await, 1);
    let (queue, kind, args): (String, String, Value) =
        sqlx::query_as("SELECT queue, kind, args FROM jobs WHERE id = $1")
            .bind(id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((queue.as_str(), kind.as_str()), ("default", "x"));
    assert_eq!(args, json!({"a": 1}));
    finish(db, pool).await;
}

#[tokio::test]
async fn queue_names_round_trip_and_unknown_names_are_refused() {
    assert_eq!(QueueName::Uploads.as_str(), "uploads");
    assert_eq!(QueueName::Default.as_str(), "default");
    assert_eq!("uploads".parse::<QueueName>(), Ok(QueueName::Uploads));
    assert!("*".parse::<QueueName>().is_err());
}

// --- pools -------------------------------------------------------------------

#[tokio::test]
async fn an_uploads_backlog_never_starves_the_default_pool() {
    let (db, pool) = migrated().await;
    let gate = Arc::new(Notify::new());
    let started = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let reg = {
        let (gate, started) = (gate.clone(), started.clone());
        let done = done.clone();
        registry(vec![
            job("slow", 1, move |_| {
                let (gate, started) = (gate.clone(), started.clone());
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    gate.notified().await;
                    Ok(())
                }
            }),
            job("quick", 1, move |_| {
                let done = done.clone();
                async move {
                    done.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }),
        ])
    };
    for _ in 0..20 {
        enqueue(&pool, "slow", json!({}), QueueName::Uploads).await;
    }
    let uploads = start(&pool, &reg, fast(QueueName::Uploads, 2));
    let default = start(&pool, &reg, fast(QueueName::Default, 1));
    eventually("uploads pool saturated", || async {
        started.load(Ordering::SeqCst) == 2
    })
    .await;

    enqueue(&pool, "quick", json!({}), QueueName::Default).await;
    eventually("default job runs under uploads backlog", || async {
        done.load(Ordering::SeqCst) == 1
    })
    .await;
    // Uploads never run on the default pool, nor default jobs on uploads.
    assert_eq!(started.load(Ordering::SeqCst), 2);

    uploads.stop.send(true).unwrap();
    for _ in 0..40 {
        gate.notify_one();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    uploads.handle.await.unwrap();
    default.stop().await;
    finish(db, pool).await;
}

// --- pause -------------------------------------------------------------------

#[tokio::test]
async fn pause_holds_jobs_for_every_replica_until_resume() {
    let (db, pool) = migrated().await;
    let ran = Arc::new(AtomicUsize::new(0));
    let reg = {
        let ran = ran.clone();
        registry(vec![job("work", 1, move |_| {
            let ran = ran.clone();
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })])
    };
    assert!(!queue::is_paused(&pool, QueueName::Uploads).await.unwrap());
    queue::pause(&pool, QueueName::Uploads).await.unwrap();
    queue::pause(&pool, QueueName::Uploads).await.unwrap(); // idempotent
    assert!(queue::is_paused(&pool, QueueName::Uploads).await.unwrap());
    assert!(!queue::is_paused(&pool, QueueName::Default).await.unwrap());

    for _ in 0..5 {
        enqueue(&pool, "work", json!({}), QueueName::Uploads).await;
    }
    let a = start(&pool, &reg, fast(QueueName::Uploads, 2));
    let b = start(&pool, &reg, fast(QueueName::Uploads, 2));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(ran.load(Ordering::SeqCst), 0, "paused: nothing is claimed");

    queue::resume(&pool, QueueName::Uploads).await.unwrap();
    assert!(!queue::is_paused(&pool, QueueName::Uploads).await.unwrap());
    eventually("backlog drains after resume", || async {
        ran.load(Ordering::SeqCst) == 5
    })
    .await;
    a.stop().await;
    b.stop().await;
    assert_eq!(ran.load(Ordering::SeqCst), 5, "each job ran exactly once");
    finish(db, pool).await;
}

#[tokio::test]
async fn a_running_job_finishes_when_its_queue_is_paused() {
    let (db, pool) = migrated().await;
    let gate = Arc::new(Notify::new());
    let ok = Arc::new(AtomicUsize::new(0));
    let reg = {
        let (gate, ok) = (gate.clone(), ok.clone());
        registry(vec![job("held", 1, move |_| {
            let (gate, ok) = (gate.clone(), ok.clone());
            async move {
                gate.notified().await;
                ok.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })])
    };
    enqueue(&pool, "held", json!({}), QueueName::Uploads).await;
    let r = start(&pool, &reg, fast(QueueName::Uploads, 1));
    eventually("claimed", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE claimed_by IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    queue::pause(&pool, QueueName::Uploads).await.unwrap();
    gate.notify_one();
    eventually("running job completes", || async {
        ok.load(Ordering::SeqCst) == 1
    })
    .await;
    r.stop().await;
    finish(db, pool).await;
}

// --- scheduler -----------------------------------------------------------------

#[tokio::test]
async fn a_scheduled_task_runs_once_per_slot_with_three_schedulers() {
    let (db, pool) = migrated().await;
    let tasks = vec![ScheduledTask {
        key: "reconcile_upload_requests".into(),
        kind: "reconcile".into(),
        queue: QueueName::Default,
        cadence: Cadence::EveryMinutes(10),
    }];
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 10, 14, 30).unwrap();
    let mut handles = Vec::new();
    for _ in 0..3 {
        let (pool, tasks) = (pool.clone(), tasks.clone());
        handles.push(tokio::spawn(async move {
            scheduler::tick(&pool, &tasks, now).await.unwrap()
        }));
    }
    let mut enqueued = 0;
    for h in handles {
        enqueued += h.await.unwrap();
    }
    assert_eq!(enqueued, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM jobs").await, 1);
    // Same slot again: nothing. Next slot: one more.
    assert_eq!(scheduler::tick(&pool, &tasks, now).await.unwrap(), 0);
    let later = Utc.with_ymd_and_hms(2026, 11, 2, 10, 20, 1).unwrap();
    assert_eq!(scheduler::tick(&pool, &tasks, later).await.unwrap(), 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM jobs").await, 2);
    finish(db, pool).await;
}

#[test]
fn cadences_resolve_to_the_latest_slot_not_after_now() {
    let at = |h, m, s| Utc.with_ymd_and_hms(2026, 11, 2, h, m, s).unwrap();
    assert_eq!(
        Cadence::EveryMinutes(10).slot(at(10, 14, 30)),
        at(10, 10, 0)
    );
    assert_eq!(Cadence::EveryMinutes(10).slot(at(10, 20, 0)), at(10, 20, 0));
    assert_eq!(Cadence::HourlyAt(12).slot(at(10, 11, 59)), at(9, 12, 0));
    assert_eq!(Cadence::HourlyAt(12).slot(at(10, 12, 0)), at(10, 12, 0));
    assert_eq!(
        Cadence::DailyAt(3).slot(at(2, 59, 59)),
        at(3, 0, 0) - chrono::Days::new(1)
    );
    assert_eq!(Cadence::DailyAt(3).slot(at(15, 0, 0)), at(3, 0, 0));
}

#[test]
fn the_default_schedule_is_reconcile_housekeeping_and_purge() {
    let s: HashMap<_, _> = scheduler::default_schedule()
        .into_iter()
        .map(|t| (t.key.clone(), t))
        .collect();
    assert_eq!(
        s["reconcile_upload_requests"].cadence,
        Cadence::EveryMinutes(10)
    );
    assert_eq!(s["clear_finished_jobs"].cadence, Cadence::HourlyAt(12));
    assert_eq!(s["purge_deleted_documents"].cadence, Cadence::DailyAt(3));
    assert!(s.values().all(|t| t.queue == QueueName::Default));
}

// --- retries ---------------------------------------------------------------------

#[tokio::test]
async fn a_failing_job_is_retried_up_to_its_attempts_then_marked_failed() {
    let (db, pool) = migrated().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let reg = {
        let calls = calls.clone();
        registry(vec![job("flaky", 3, move |_| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(JobError::Retry("boom".into()))
            }
        })])
    };
    let id = enqueue(&pool, "flaky", json!({}), QueueName::Default).await;
    let r = start(&pool, &reg, fast(QueueName::Default, 1));
    eventually("job is failed", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    r.stop().await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let (attempts, err, finished): (i32, Option<String>, bool) = sqlx::query_as(
        "SELECT attempts, last_error, finished_at IS NOT NULL FROM jobs WHERE id = $1",
    )
    .bind(id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (attempts, err.as_deref(), finished),
        (3, Some("boom"), false)
    );
    finish(db, pool).await;
}

#[tokio::test]
async fn a_job_that_recovers_is_finished_and_a_discard_is_not_retried() {
    let (db, pool) = migrated().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let discards = Arc::new(AtomicUsize::new(0));
    let reg = {
        let (calls, discards) = (calls.clone(), discards.clone());
        registry(vec![
            job("recovers", 5, move |_| {
                let calls = calls.clone();
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                        Err(JobError::Retry("not yet".into()))
                    } else {
                        Ok(())
                    }
                }
            }),
            job("refused", 5, move |_| {
                let discards = discards.clone();
                async move {
                    discards.fetch_add(1, Ordering::SeqCst);
                    Err(JobError::Discard("forbidden".into()))
                }
            }),
            job("panics", 2, |_| async { panic!("kaboom") }),
        ])
    };
    enqueue(&pool, "recovers", json!({}), QueueName::Default).await;
    enqueue(&pool, "refused", json!({}), QueueName::Default).await;
    enqueue(&pool, "panics", json!({}), QueueName::Default).await;
    let r = start(&pool, &reg, fast(QueueName::Default, 2));
    eventually("all settled", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE finished_at IS NULL AND failed_at IS NULL",
        )
        .await
            == 0
    })
    .await;
    r.stop().await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(discards.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE finished_at IS NOT NULL"
        )
        .await,
        1
    );
    let errs: Vec<(String, i32, String)> = sqlx::query_as(
        "SELECT kind, attempts, last_error FROM jobs WHERE failed_at IS NOT NULL ORDER BY kind",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(errs.len(), 2);
    assert_eq!((errs[0].0.as_str(), errs[0].1), ("panics", 2));
    assert!(errs[0].2.contains("panic"), "{errs:?}");
    assert_eq!(errs[1].1, 1);
    finish(db, pool).await;
}

#[test]
fn default_backoff_is_polynomial_like_active_job() {
    use ged::jobs::polynomial_backoff;
    assert_eq!(polynomial_backoff(1), Duration::from_secs(3));
    assert_eq!(polynomial_backoff(2), Duration::from_secs(18));
    assert_eq!(polynomial_backoff(4), Duration::from_secs(258));
}

// --- heartbeat & recovery (Review Focus 3) ------------------------------------------

#[tokio::test]
async fn heartbeats_are_recorded_and_expire() {
    let (db, pool) = migrated().await;
    let id = heartbeat::register(&pool, "uploads").await.unwrap();
    let live = heartbeat::live_workers(&pool, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!((live[0].id, live[0].pool.as_str()), (id, "uploads"));

    sqlx::query("UPDATE job_workers SET last_heartbeat_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        heartbeat::live_workers(&pool, Duration::from_secs(30))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !heartbeat::beat(&pool, id, "uploads").await.unwrap(),
        "a stale but present row is not a re-registration"
    );
    assert_eq!(
        heartbeat::live_workers(&pool, Duration::from_secs(30))
            .await
            .unwrap()
            .len(),
        1
    );
    heartbeat::deregister(&pool, id).await.unwrap();
    assert!(
        heartbeat::live_workers(&pool, Duration::from_secs(30))
            .await
            .unwrap()
            .is_empty()
    );
    finish(db, pool).await;
}

#[tokio::test]
async fn a_claim_held_by_a_dead_worker_is_failed_like_solid_queue_and_never_re_run() {
    let (db, pool) = migrated().await;
    let effects = Arc::new(AtomicUsize::new(0));
    let reg = {
        let effects = effects.clone();
        registry(vec![job("effect", 5, move |_| {
            let effects = effects.clone();
            async move {
                effects.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })])
    };
    let id = enqueue(&pool, "effect", json!({}), QueueName::Uploads).await;

    // A worker claims the job and is killed: no completion, no more heartbeats.
    let dead = heartbeat::register(&pool, "uploads").await.unwrap();
    let claimed = queue::claim(&pool, QueueName::Uploads, dead).await.unwrap();
    assert_eq!(claimed.map(|c| c.id), Some(id));
    sqlx::query("UPDATE job_workers SET last_heartbeat_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();

    // Live replicas start: their heartbeat loops prune the dead worker.
    let mut cfg = fast(QueueName::Uploads, 2);
    cfg.heartbeat.expiry = Duration::from_secs(5);
    let a = start(&pool, &reg, cfg.clone());
    let b = start(&pool, &reg, cfg);
    eventually("job failed by the prune", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    // A fresh job proves the replicas are alive and polling.
    enqueue(&pool, "effect", json!({}), QueueName::Uploads).await;
    eventually("fresh job ran", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE finished_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    a.stop().await;
    b.stop().await;
    assert_eq!(effects.load(Ordering::SeqCst), 1, "only the fresh job ran");
    // Solid Queue: a crashed claim is a failure (ProcessPrunedError), the
    // attempt is kept, and the job is never retried automatically.
    let (attempts, err, claimed_by): (i32, Option<String>, Option<uuid::Uuid>) =
        sqlx::query_as("SELECT attempts, last_error, claimed_by FROM jobs WHERE id = $1")
            .bind(id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, 1, "the crashed attempt counts");
    assert!(err.unwrap().contains("heartbeat"), "names the cause");
    assert_eq!(claimed_by, None);
    let dead_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM job_workers WHERE id = $1")
        .bind(dead)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dead_rows, 0, "the dead worker's registration is purged");
    finish(db, pool).await;
}

// A shared gate: handlers wait until it opens.
#[derive(Clone)]
struct Gate(watch::Sender<bool>);

impl Gate {
    fn new() -> Self {
        Self(watch::channel(false).0)
    }
    fn open(&self) {
        self.0.send_replace(true);
    }
    async fn wait(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|open| *open).await;
    }
}

struct CountOnDrop(Arc<AtomicUsize>);

impl Drop for CountOnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_live_worker_purged_by_a_peer_re_registers_and_never_double_runs_a_job() {
    let (db, pool) = migrated().await;
    let gate = Gate::new();
    let started = Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let completed = Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let dropped = Arc::new(AtomicUsize::new(0));
    let reg = {
        let (gate, started, completed, dropped) = (
            gate.clone(),
            started.clone(),
            completed.clone(),
            dropped.clone(),
        );
        registry(vec![job("gated", 3, move |args| {
            let (gate, started, completed, dropped) = (
                gate.clone(),
                started.clone(),
                completed.clone(),
                dropped.clone(),
            );
            async move {
                let n = args["n"].as_i64().unwrap();
                let _guard = CountOnDrop(dropped);
                started.lock().unwrap().push(n);
                gate.wait().await;
                completed.lock().unwrap().push(n);
                Ok(())
            }
        })])
    };
    let r = start(&pool, &reg, fast(QueueName::Default, 1));
    let worker: uuid::Uuid = {
        eventually("registered", || async {
            count(&pool, "SELECT count(*) FROM job_workers").await == 1
        })
        .await;
        sqlx::query_scalar("SELECT id FROM job_workers")
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    // Job 1 is running when a peer prunes this (alive, merely slow) worker.
    let first = enqueue(&pool, "gated", json!({"n": 1}), QueueName::Default).await;
    eventually("job 1 running", || async {
        started.lock().unwrap().len() == 1
    })
    .await;
    assert_eq!(
        queue::recover_abandoned(&pool, Duration::ZERO)
            .await
            .unwrap(),
        1,
        "the peer's prune frees job 1"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM job_workers").await, 0);

    // The next heartbeat notices, re-registers under the same id and cancels
    // the in-flight run whose claim is gone.
    eventually("re-registered", || async {
        count(
            &pool,
            &format!("SELECT count(*) FROM job_workers WHERE id = '{worker}'"),
        )
        .await
            == 1
    })
    .await;
    eventually("in-flight run cancelled", || async {
        dropped.load(Ordering::SeqCst) == 1
    })
    .await;

    // Job 2 is claimed by the re-registered worker: a peer's recovery pass
    // leaves it alone, so it runs exactly once.
    let second = enqueue(&pool, "gated", json!({"n": 2}), QueueName::Default).await;
    eventually("job 2 running", || async {
        started.lock().unwrap().contains(&2)
    })
    .await;
    assert_eq!(
        queue::recover_abandoned(&pool, Duration::from_secs(30))
            .await
            .unwrap(),
        0,
        "a live, registered worker keeps its claims"
    );
    gate.open();
    eventually("job 2 finished", || async {
        count(
            &pool,
            &format!(
                "SELECT count(*) FROM jobs WHERE id = {} AND finished_at IS NOT NULL",
                second.0
            ),
        )
        .await
            == 1
    })
    .await;
    r.stop().await;
    assert_eq!(*completed.lock().unwrap(), vec![2], "job 1 never completes");
    let mut started = started.lock().unwrap().clone();
    started.sort_unstable();
    assert_eq!(started, vec![1, 2], "each job started once");
    let first_failed: bool =
        sqlx::query_scalar("SELECT failed_at IS NOT NULL FROM jobs WHERE id = $1")
            .bind(first.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(first_failed, "the pruned claim was failed, not re-run");
    finish(db, pool).await;
}

#[tokio::test]
async fn a_pruned_job_cannot_be_completed_by_its_former_owner() {
    let (db, pool) = migrated().await;
    let id = enqueue(&pool, "x", json!({}), QueueName::Default).await;
    let slow = heartbeat::register(&pool, "default").await.unwrap();
    queue::claim(&pool, QueueName::Default, slow)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE job_workers SET last_heartbeat_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        queue::recover_abandoned(&pool, Duration::from_secs(30))
            .await
            .unwrap(),
        1
    );
    // Failed, not re-queued: a fresh worker has nothing to claim.
    let fresh = heartbeat::register(&pool, "default").await.unwrap();
    assert!(
        queue::claim(&pool, QueueName::Default, fresh)
            .await
            .unwrap()
            .is_none()
    );
    // The zombie finally reports in: ignored.
    assert!(!queue::complete(&pool, id, slow).await.unwrap());
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE finished_at IS NOT NULL"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL"
        )
        .await,
        1
    );
    finish(db, pool).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_replicas_polling_one_backlog_never_run_a_job_twice() {
    let (db, pool) = migrated().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let reg = {
        let seen = seen.clone();
        registry(vec![job("once", 1, move |args| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(args["n"].as_i64().unwrap());
                Ok(())
            }
        })])
    };
    for n in 0..200 {
        enqueue(&pool, "once", json!({"n": n}), QueueName::Uploads).await;
    }
    let a = start(&pool, &reg, fast(QueueName::Uploads, 4));
    let b = start(&pool, &reg, fast(QueueName::Uploads, 4));
    eventually("drained", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE finished_at IS NOT NULL",
        )
        .await
            == 200
    })
    .await;
    a.stop().await;
    b.stop().await;
    let mut seen = seen.lock().unwrap().clone();
    seen.sort_unstable();
    assert_eq!(seen, (0..200).collect::<Vec<_>>());
    finish(db, pool).await;
}

// --- graceful shutdown -----------------------------------------------------------------

#[tokio::test]
async fn shutdown_waits_for_running_jobs_then_releases_stragglers_after_the_timeout() {
    let (db, pool) = migrated().await;
    let quick_done = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let reg = {
        let (quick_done, started) = (quick_done.clone(), started.clone());
        let started2 = started.clone();
        registry(vec![
            job("quick", 1, move |_| {
                let (q, s) = (quick_done.clone(), started.clone());
                async move {
                    s.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    q.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }),
            job("endless", 3, move |_| {
                let s = started2.clone();
                async move {
                    s.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(())
                }
            }),
        ])
    };
    enqueue(&pool, "quick", json!({}), QueueName::Uploads).await;
    enqueue(&pool, "endless", json!({}), QueueName::Uploads).await;
    let mut cfg = fast(QueueName::Uploads, 2);
    cfg.shutdown_timeout = Duration::from_secs(1);
    let r = start(&pool, &reg, cfg);
    eventually("both started", || async {
        started.load(Ordering::SeqCst) == 2
    })
    .await;

    let t = Instant::now();
    r.stop().await;
    let took = t.elapsed();
    assert!(
        took >= Duration::from_millis(900),
        "waited for the timeout: {took:?}"
    );
    assert!(
        took < Duration::from_secs(4),
        "bounded by the timeout: {took:?}"
    );
    assert_eq!(quick_done.load(Ordering::SeqCst), 1);
    let rows: Vec<(String, bool, Option<uuid::Uuid>, i32)> = sqlx::query_as(
        "SELECT kind, finished_at IS NOT NULL, claimed_by, attempts FROM jobs ORDER BY kind",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("endless".to_owned(), false, None, 0),
            ("quick".to_owned(), true, None, 1),
        ]
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM job_workers").await, 0);
    finish(db, pool).await;
}

#[tokio::test]
async fn shutdown_does_not_claim_new_jobs() {
    let (db, pool) = migrated().await;
    let reg = registry(vec![job("x", 1, |_| async { Ok(()) })]);
    let r = start(&pool, &reg, fast(QueueName::Default, 1));
    tokio::time::sleep(Duration::from_millis(100)).await;
    r.stop().await;
    enqueue(&pool, "x", json!({}), QueueName::Default).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE claimed_by IS NOT NULL"
        )
        .await,
        0
    );
    finish(db, pool).await;
}

// --- housekeeping ------------------------------------------------------------------------

#[tokio::test]
async fn clear_finished_removes_only_old_terminal_jobs() {
    let (db, pool) = migrated().await;
    for (state, age) in [
        ("finished_at", "2 days"),
        ("finished_at", "1 hour"),
        ("failed_at", "2 days"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO jobs (queue, kind, {state}) VALUES ('default', 'x', now() - interval '{age}')"
        )))
        .execute(&pool)
        .await
        .unwrap();
    }
    enqueue(&pool, "x", json!({}), QueueName::Default).await;
    let removed = queue::clear_finished(&pool, Duration::from_secs(86_400), 500)
        .await
        .unwrap();
    assert_eq!(
        removed, 1,
        "only the old finished job; failed ones are kept for diagnosis"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM jobs").await, 3);
    finish(db, pool).await;
}

// --- review T8 follow-ups -----------------------------------------------------------

struct RealBackoff;

impl Job for RealBackoff {
    fn kind(&self) -> &'static str {
        "real_backoff"
    }
    fn max_attempts(&self) -> i32 {
        3
    }
    fn perform(&self, _args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        Box::pin(async { Err(JobError::Retry("later".into())) })
    }
}

#[tokio::test]
async fn a_retry_waits_for_the_real_polynomial_backoff() {
    let (db, pool) = migrated().await;
    let reg = registry(vec![Arc::new(RealBackoff)]);
    let id = enqueue(&pool, "real_backoff", json!({}), QueueName::Default).await;
    let r = start(&pool, &reg, fast(QueueName::Default, 1));
    eventually("first attempt recorded", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE last_error = 'later' AND claimed_by IS NULL",
        )
        .await
            == 1
    })
    .await;
    // Attempt 1 fails => 1^4 + 2 = 3 s from now.
    let (attempts, delay): (i32, f64) = sqlx::query_as(
        "SELECT attempts, EXTRACT(EPOCH FROM (run_at - now()))::float8 FROM jobs WHERE id = $1",
    )
    .bind(id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1);
    assert!((1.5..=3.1).contains(&delay), "delay was {delay}");
    // Still waiting: it is not claimable now.
    assert!(
        queue::claim(&pool, QueueName::Default, uuid::Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
    r.stop().await;
    finish(db, pool).await;
}

#[tokio::test]
async fn an_unknown_kind_is_retried_with_backoff_not_discarded() {
    let (db, pool) = migrated().await;
    let id = enqueue(
        &pool,
        "from_the_future",
        json!({"n": 1}),
        QueueName::Default,
    )
    .await;
    let old = registry(vec![]);
    let r = start(&pool, &old, fast(QueueName::Default, 1));
    eventually("attempt recorded", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE last_error LIKE '%no handler%' AND claimed_by IS NULL",
        )
        .await
            == 1
    })
    .await;
    r.stop().await;
    let (attempts, failed, delayed): (i32, bool, bool) = sqlx::query_as(
        "SELECT attempts, failed_at IS NOT NULL, run_at > now() + interval '1 second' FROM jobs WHERE id = $1",
    )
    .bind(id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((attempts, failed, delayed), (1, false, true));

    // A rolled-out worker that knows the kind picks it up.
    sqlx::query("UPDATE jobs SET run_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let new = {
        let seen = seen.clone();
        registry(vec![job("from_the_future", 1, move |_| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })])
    };
    let r = start(&pool, &new, fast(QueueName::Default, 1));
    eventually("finished by the new worker", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE finished_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    r.stop().await;
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    finish(db, pool).await;
}

#[tokio::test]
async fn an_unknown_kind_gives_up_after_the_attempt_cap() {
    let (db, pool) = migrated().await;
    let id = enqueue(&pool, "never_deployed", json!({}), QueueName::Default).await;
    let cap = i32::try_from(ged::jobs::pool::UNKNOWN_KIND_MAX_ATTEMPTS).unwrap();
    sqlx::query("UPDATE jobs SET attempts = $2 WHERE id = $1")
        .bind(id.0)
        .bind(cap - 1)
        .execute(&pool)
        .await
        .unwrap();
    let r = start(&pool, &registry(vec![]), fast(QueueName::Default, 1));
    eventually("failed at the cap", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    r.stop().await;
    finish(db, pool).await;
}

#[tokio::test]
async fn a_synchronous_panic_in_perform_does_not_kill_the_claim_loop() {
    let (db, pool) = migrated().await;
    let ran = Arc::new(AtomicUsize::new(0));
    let reg = {
        let ran = ran.clone();
        registry(vec![
            job(
                "explodes",
                1,
                |_| -> std::future::Ready<Result<(), JobError>> {
                    panic!("before the future exists")
                },
            ),
            job("fine", 1, move |_| {
                let ran = ran.clone();
                async move {
                    ran.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }),
        ])
    };
    enqueue(&pool, "explodes", json!({}), QueueName::Default).await;
    // One thread only: if its loop died, nothing would ever claim "fine".
    let r = start(&pool, &reg, fast(QueueName::Default, 1));
    eventually("panicking job failed", || async {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    enqueue(&pool, "fine", json!({}), QueueName::Default).await;
    eventually("loop still alive", || async {
        ran.load(Ordering::SeqCst) == 1
    })
    .await;
    r.stop().await;
    let claimed: i64 = count(
        &pool,
        "SELECT count(*) FROM jobs WHERE claimed_by IS NOT NULL",
    )
    .await;
    assert_eq!(claimed, 0, "no job left claimed by a live worker");
    finish(db, pool).await;
}

#[tokio::test]
async fn shutdown_still_deregisters_when_releasing_claims_fails() {
    let (db, pool) = migrated().await;
    sqlx::query(
        "CREATE FUNCTION refuse_release() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF OLD.claimed_by IS NOT NULL AND NEW.claimed_by IS NULL THEN \
             RAISE EXCEPTION 'release refused'; \
           END IF; \
           RETURN NEW; \
         END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER refuse_release BEFORE UPDATE ON jobs FOR EACH ROW EXECUTE FUNCTION refuse_release()")
        .execute(&pool)
        .await
        .unwrap();
    let started = Arc::new(AtomicUsize::new(0));
    let reg = {
        let started = started.clone();
        registry(vec![job("endless", 1, move |_| {
            let started = started.clone();
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Ok(())
            }
        })])
    };
    enqueue(&pool, "endless", json!({}), QueueName::Default).await;
    let mut cfg = fast(QueueName::Default, 1);
    cfg.shutdown_timeout = Duration::from_millis(300);
    let (stop, rx) = watch::channel(false);
    let runner = Runner::new(pool.clone(), reg, cfg);
    let handle = tokio::spawn(async move { runner.run(rx).await });
    eventually("running", || async { started.load(Ordering::SeqCst) == 1 }).await;
    stop.send(true).unwrap();
    let result = handle.await.unwrap();
    assert!(result.is_err(), "the release failure is still reported");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM job_workers").await,
        0,
        "yet the worker deregistered"
    );
    finish(db, pool).await;
}

#[tokio::test]
async fn the_ready_index_carries_run_at_so_future_retries_are_skipped_in_the_index() {
    let (db, pool) = migrated().await;
    let def: String =
        sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE indexname = 'index_jobs_ready'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(def.contains("run_at"), "{def}");
    finish(db, pool).await;
}

/// Final review m5: a callback whose worker died mid-delivery is re-queued
/// (at least once) while it has attempts left; other kinds keep the Solid
/// Queue behaviour (failed, never replayed).
#[tokio::test]
async fn a_dead_workers_requeueable_job_is_redelivered_while_attempts_remain() {
    let (db, pool) = migrated().await;
    let deliver = enqueue(&pool, "deliver", json!({}), QueueName::Default).await;
    let other = enqueue(&pool, "other", json!({}), QueueName::Default).await;
    let spent = enqueue(&pool, "deliver", json!({}), QueueName::Default).await;
    let dead = heartbeat::register(&pool, "default").await.unwrap();
    for _ in 0..3 {
        queue::claim(&pool, QueueName::Default, dead)
            .await
            .unwrap()
            .unwrap();
    }
    sqlx::query("UPDATE jobs SET attempts = 5 WHERE id = $1")
        .bind(spent.0)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE job_workers SET last_heartbeat_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();

    let recovered =
        queue::recover_abandoned_requeuing(&pool, Duration::from_secs(30), &[("deliver", 5)])
            .await
            .unwrap();
    assert_eq!((recovered.requeued, recovered.failed), (1, 2));

    let state = |id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (bool, bool, bool)>(
                "SELECT claimed_by IS NULL, failed_at IS NOT NULL, finished_at IS NOT NULL \
                 FROM jobs WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(state(deliver.0).await, (true, false, false), "re-queued");
    assert_eq!(state(other.0).await, (true, true, false), "failed");
    assert_eq!(
        state(spent.0).await,
        (true, true, false),
        "no attempts left"
    );
    // And a live worker can claim the re-queued delivery again.
    let fresh = heartbeat::register(&pool, "default").await.unwrap();
    let again = queue::claim(&pool, QueueName::Default, fresh)
        .await
        .unwrap()
        .expect("the re-queued callback is claimable");
    assert_eq!((again.id.0, again.attempts), (deliver.0, 2));
    finish(db, pool).await;
}

/// Final review m5: a clean stop hands back a claim whose result could not be
/// recorded (the job is still `claimed` although the run is over).
#[tokio::test]
async fn a_clean_stop_releases_a_claim_left_without_a_recorded_result() {
    let (db, pool) = migrated().await;
    let reg = registry(vec![]);
    let r = start(&pool, &reg, fast(QueueName::Default, 1));
    eventually("registered", || async {
        count(&pool, "SELECT count(*) FROM job_workers").await == 1
    })
    .await;
    let worker: uuid::Uuid = sqlx::query_scalar("SELECT id FROM job_workers")
        .fetch_one(&pool)
        .await
        .unwrap();
    // A claim of this worker whose outcome never reached the table.
    let orphan = enqueue(&pool, "orphan", json!({}), QueueName::Default).await;
    sqlx::query("UPDATE jobs SET claimed_by = $2, claimed_at = now(), attempts = 1 WHERE id = $1")
        .bind(orphan.0)
        .bind(worker)
        .execute(&pool)
        .await
        .unwrap();
    r.stop().await;
    let (claimed, attempts): (bool, i32) =
        sqlx::query_as("SELECT claimed_by IS NOT NULL, attempts FROM jobs WHERE id = $1")
            .bind(orphan.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!claimed, "the claim must be released on a clean stop");
    assert_eq!(attempts, 0, "the interrupted attempt is not counted");
    finish(db, pool).await;
}
