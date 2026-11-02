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
fn the_default_schedule_matches_config_recurring_yml() {
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
    enqueue(&pool, "nobody-handles-this", json!({}), QueueName::Default).await;
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
    assert_eq!(errs.len(), 3);
    assert_eq!(errs[0].0, "nobody-handles-this");
    assert!(errs[0].2.contains("no handler"), "{errs:?}");
    assert_eq!((errs[1].0.as_str(), errs[1].1), ("panics", 2));
    assert!(errs[1].2.contains("panic"), "{errs:?}");
    assert_eq!(errs[2].1, 1);
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
    heartbeat::beat(&pool, id).await.unwrap();
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
async fn a_claim_held_by_a_dead_worker_is_recovered_and_runs_exactly_once() {
    let (db, pool) = migrated().await;
    let effects = Arc::new(AtomicUsize::new(0));
    let reg = {
        let effects = effects.clone();
        registry(vec![job("effect", 1, move |_| {
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
    assert!(
        queue::claim(&pool, QueueName::Uploads, dead)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE job_workers SET last_heartbeat_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();

    // A live replica starts: its heartbeat loop recovers the abandoned claim.
    let mut cfg = fast(QueueName::Uploads, 2);
    cfg.heartbeat.expiry = Duration::from_secs(5);
    let a = start(&pool, &reg, cfg.clone());
    let b = start(&pool, &reg, cfg);
    eventually("recovered and run", || async {
        effects.load(Ordering::SeqCst) >= 1
            && count(
                &pool,
                "SELECT count(*) FROM jobs WHERE finished_at IS NOT NULL",
            )
            .await
                == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.stop().await;
    b.stop().await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "one effect, two replicas"
    );
    let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM jobs WHERE id = $1")
        .bind(id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1, "a crashed claim does not burn a retry");
    let dead_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM job_workers WHERE id = $1")
        .bind(dead)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dead_rows, 0, "the dead worker's registration is purged");
    finish(db, pool).await;
}

#[tokio::test]
async fn a_recovered_job_cannot_be_completed_by_its_former_owner() {
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
    let fresh = heartbeat::register(&pool, "default").await.unwrap();
    queue::claim(&pool, QueueName::Default, fresh)
        .await
        .unwrap()
        .unwrap();
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
    assert!(queue::complete(&pool, id, fresh).await.unwrap());
    finish(db, pool).await;
}

#[tokio::test]
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
            ("quick".to_owned(), true, rows[1].2, 1),
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
