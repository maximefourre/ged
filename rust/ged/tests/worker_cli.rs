//! `ged worker` as a real process: runs jobs, registers heartbeats, and stops
//! cleanly on SIGTERM within `SOLID_QUEUE_SHUTDOWN_TIMEOUT`.

mod support;

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use support::ScratchDb;

fn spawn(url: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ged"))
        .arg("worker")
        .env_clear()
        .env("RAILS_ENV", "test")
        .env("DATABASE_URL", url)
        .env("JOB_THREADS", "2")
        .env("SOLID_QUEUE_SHUTDOWN_TIMEOUT", "5")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn signal(child: &Child, sig: &str) {
    let status = Command::new("kill")
        .args([sig, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

async fn count(pool: &sqlx::PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn eventually(what: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn worker_runs_scheduled_jobs_and_stops_cleanly_on_sigterm() {
    let db = ScratchDb::create().await;
    let pool = db.pool().await;
    ged::db::migrate(&pool).await.unwrap();
    // An old finished job the scheduled clear_finished_jobs must sweep.
    sqlx::query(
        "INSERT INTO jobs (queue, kind, finished_at) VALUES ('default', 'x', now() - interval '3 days')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut child = spawn(&db.url);
    // 2 uploads-pool... one uploads process + the default pool.
    eventually("two worker registrations", async || {
        count(&pool, "SELECT count(*) FROM job_workers").await == 2
    })
    .await;
    eventually("scheduled sweep ran", async || {
        count(&pool, "SELECT count(*) FROM jobs WHERE kind = 'x'").await == 0
    })
    .await;
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM scheduled_runs WHERE task_key = 'clear_finished_jobs'"
        )
        .await,
        1
    );

    let t = Instant::now();
    signal(&child, "-TERM");
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
    assert_eq!(count(&pool, "SELECT count(*) FROM job_workers").await, 0);
    pool.close().await;
    db.drop_db().await;
}

#[tokio::test]
async fn a_killed_worker_leaves_a_stale_registration_for_recovery() {
    let db = ScratchDb::create().await;
    let pool = db.pool().await;
    ged::db::migrate(&pool).await.unwrap();
    let mut child = spawn(&db.url);
    eventually("registered", async || {
        count(&pool, "SELECT count(*) FROM job_workers").await == 2
    })
    .await;
    signal(&child, "-KILL");
    child.wait().unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM job_workers").await, 2);
    pool.close().await;
    db.drop_db().await;
}
