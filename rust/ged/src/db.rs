//! Database access shared by `serve`, `worker`, `migrate` and `seed`.

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

/// Schema migrations, embedded at compile time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(10).connect(url).await
}

/// A pool that connects on first use: lets routes that never touch the
/// database (and tests) run without one.
pub fn connect_lazy(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(10).connect_lazy(url)
}

/// `ged migrate`: applies pending migrations; idempotent.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
pub type PgTx = sqlx::Transaction<'static, sqlx::Postgres>;
