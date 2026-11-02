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
        // Short liveness so a killed worker is pruned within seconds.
        .env("SOLID_QUEUE_PROCESS_HEARTBEAT_INTERVAL", "0.2")
        .env("SOLID_QUEUE_PROCESS_ALIVE_THRESHOLD", "2")
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

/// Review focus 3, with real processes: `ged worker` is SIGKILLed while its
/// scheduled job is in flight (the job's DELETE waits on a row lock held by
/// the test), a second `ged worker` prunes it after the alive threshold. As in
/// Solid Queue the crashed claim is *failed* (never silently re-run), the
/// effect happens once, and the survivor keeps serving.
#[tokio::test]
async fn a_worker_sigkilled_mid_job_is_pruned_and_its_job_never_runs_twice() {
    let db = ScratchDb::create().await;
    let pool = db.pool().await;
    ged::db::migrate(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO jobs (queue, kind, finished_at) VALUES ('default', 'x', now() - interval '3 days')",
    )
    .execute(&pool)
    .await
    .unwrap();
    for stmt in [
        "CREATE TABLE job_events (job_id bigint, event text, worker uuid)",
        "CREATE FUNCTION log_job_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
           IF TG_OP = 'DELETE' THEN \
             INSERT INTO job_events VALUES (OLD.id, 'deleted', NULL); RETURN OLD; \
           END IF; \
           IF NEW.claimed_by IS NOT NULL AND OLD.claimed_by IS DISTINCT FROM NEW.claimed_by THEN \
             INSERT INTO job_events VALUES (NEW.id, 'claimed', NEW.claimed_by); \
           END IF; \
           IF NEW.finished_at IS NOT NULL AND OLD.finished_at IS NULL THEN \
             INSERT INTO job_events VALUES (NEW.id, 'finished', NEW.claimed_by); \
           END IF; \
           RETURN NEW; END $$",
        "CREATE TRIGGER log_job_event AFTER UPDATE OR DELETE ON jobs FOR EACH ROW EXECUTE FUNCTION log_job_event()",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(stmt.to_owned()))
            .execute(&pool)
            .await
            .unwrap();
    }

    // The test holds the old row: the sweep's DELETE blocks mid-job.
    let mut lock = pool.begin().await.unwrap();
    let old_id: i64 = sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'x' FOR UPDATE")
        .fetch_one(&mut *lock)
        .await
        .unwrap();

    let mut doomed = spawn(&db.url);
    eventually("job in flight, blocked on the lock", async || {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE kind = 'clear_finished_jobs' AND claimed_by IS NOT NULL",
        )
        .await
            == 1
            && count(
                &pool,
                "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' \
                 AND query LIKE 'DELETE FROM jobs%'",
            )
            .await
                == 1
    })
    .await;
    let job_id: i64 = sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'clear_finished_jobs'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let doomed_pid = i32::try_from(doomed.id()).unwrap();
    signal(&doomed, "-KILL");
    doomed.wait().unwrap();

    let mut survivor = spawn(&db.url);
    eventually("survivor pruned the dead worker and failed its job", async || {
        count(
            &pool,
            &format!("SELECT count(*) FROM jobs WHERE id = {job_id} AND failed_at IS NOT NULL AND claimed_by IS NULL"),
        )
        .await
            == 1
            && count(
                &pool,
                &format!("SELECT count(*) FROM job_workers WHERE pid = {doomed_pid}"),
            )
            .await
                == 0
    })
    .await;
    let err: String = sqlx::query_scalar("SELECT last_error FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(err.contains("heartbeat"), "{err}");

    // Unblock the database, then give the survivor fresh work.
    lock.rollback().await.unwrap();
    sqlx::query("INSERT INTO jobs (queue, kind) VALUES ('default', 'clear_finished_jobs')")
        .execute(&pool)
        .await
        .unwrap();
    eventually("survivor serves new work", async || {
        count(
            &pool,
            "SELECT count(*) FROM jobs WHERE kind = 'clear_finished_jobs' AND finished_at IS NOT NULL",
        )
        .await
            == 1
    })
    .await;
    eventually("old row swept", async || {
        count(
            &pool,
            &format!("SELECT count(*) FROM jobs WHERE id = {old_id}"),
        )
        .await
            == 0
    })
    .await;

    // One claim, no completion, no retry for the crashed job; its effect (the
    // sweep of the old row) happened exactly once overall.
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT count(*) FROM job_events WHERE job_id = {job_id} AND event = 'claimed'"
            )
        )
        .await,
        1,
        "the crashed job was claimed once and never re-run"
    );
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT count(*) FROM job_events WHERE job_id = {job_id} AND event = 'finished'"
            )
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT count(*) FROM job_events WHERE job_id = {old_id} AND event = 'deleted'"
            )
        )
        .await,
        1,
        "one effect"
    );

    signal(&survivor, "-TERM");
    assert!(survivor.wait().unwrap().success());
    pool.close().await;
    db.drop_db().await;
}
