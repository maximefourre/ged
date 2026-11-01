use async_trait::async_trait;
use sqlx::PgPool;

/// What the suite needs from an implementation under test. Everything else
/// (HTTP, OAuth, fixtures) is backend-independent.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Root URL of the app, without trailing slash.
    fn base_url(&self) -> &str;

    /// Connection pool on the business database.
    fn db(&self) -> &PgPool;

    /// Run a background job synchronously by its canonical (Rails class) name,
    /// e.g. `PurgeDeletedDocumentsJob`. Time-dependent behaviour is tested by
    /// moving `purge_at` etc. in SQL, then calling this.
    async fn run_job(&self, name: &str) -> Result<(), String>;

    /// Delete every row of the business tables except `admins`. Only safe when
    /// no other test runs against the stack (`--test-threads=1`): tests isolate
    /// themselves with unique names and do not call this.
    async fn reset(&self) -> Result<(), String>;
}

/// Backend selected by `GED_BACKEND` (default `rails`).
pub async fn backend() -> Box<dyn Backend> {
    match std::env::var("GED_BACKEND").as_deref().unwrap_or("rails") {
        "rails" => Box::new(crate::rails::RailsBackend::connect().await),
        other => panic!("GED_BACKEND={other:?}: unknown backend (available: rails)"),
    }
}
