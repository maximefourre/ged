//! Worker registration and heartbeats; read by `/admin/status`.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

/// Solid Queue's defaults: beat every minute, dead after five.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatConfig {
    pub interval: Duration,
    pub expiry: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            expiry: Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkerInfo {
    pub id: Uuid,
    pub pool: String,
    pub hostname: String,
    pub pid: i32,
}

fn current_hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())
}

fn current_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(i32::MAX)
}

pub async fn register(db: &PgPool, pool: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO job_workers (id, pool, hostname, pid) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(pool)
        .bind(current_hostname())
        .bind(current_pid())
        .execute(db)
        .await?;
    Ok(id)
}

/// Records a heartbeat. A worker that is alive but was pruned by a peer
/// (stalled process, long network partition) has lost its row: the beat then
/// re-registers it under the same id and returns `true` so the caller knows
/// the claims it held were given up. `false` = business as usual.
pub async fn beat(db: &PgPool, id: Uuid, pool: &str) -> Result<bool, sqlx::Error> {
    // `xmax = 0` only on a freshly inserted row (not on the update path).
    sqlx::query_scalar(
        "INSERT INTO job_workers (id, pool, hostname, pid) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (id) DO UPDATE SET last_heartbeat_at = now() \
         RETURNING (xmax = 0)",
    )
    .bind(id)
    .bind(pool)
    .bind(current_hostname())
    .bind(current_pid())
    .fetch_one(db)
    .await
}

pub async fn deregister(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM job_workers WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Workers whose last heartbeat is within `expiry`.
pub async fn live_workers(db: &PgPool, expiry: Duration) -> Result<Vec<WorkerInfo>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, pool, hostname, pid FROM job_workers \
         WHERE last_heartbeat_at > now() - make_interval(secs => $1) ORDER BY started_at, id",
    )
    .bind(expiry.as_secs_f64())
    .fetch_all(db)
    .await
}
