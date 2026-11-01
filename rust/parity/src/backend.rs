use async_trait::async_trait;
use reqwest::header::HeaderMap;
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

    /// Values in a download response's headers that point at the stored file
    /// itself (a sendfile path, a storage-service URL) rather than carrying
    /// its bytes. Each is described as `"header: value"`. The purge tests
    /// assert that nothing of the kind outlives the file.
    fn storage_locators(&self, headers: &HeaderMap) -> Vec<String>;

    /// URL paths, if any, through which one of the locators above could be
    /// requested over HTTP (a bare filesystem path is not requestable).
    fn locator_http_paths(&self, locator: &str) -> Vec<String>;
}

/// `X-Sendfile` / `X-Accel-Redirect` headers: the reverse proxy serves the
/// file named there, whatever the backend.
pub fn sendfile_locators(headers: &HeaderMap) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            name.eq_ignore_ascii_case("x-sendfile") || name.eq_ignore_ascii_case("x-accel-redirect")
        })
        .filter_map(|(name, value)| Some(format!("{}: {}", name.as_str(), value.to_str().ok()?)))
        .collect()
}

/// Delete every row of the business tables except `admins`.
pub async fn reset_business_tables(pool: &PgPool) -> Result<(), String> {
    // Children first; `upload_requests.document_id` is ON DELETE SET NULL.
    for sql in [
        "DELETE FROM upload_requests",
        "DELETE FROM documents",
        "DELETE FROM client_department_accesses",
        "DELETE FROM app_clients",
    ] {
        sqlx::query(sql)
            .execute(pool)
            .await
            .map_err(|e| format!("{sql}: {e}"))?;
    }
    // Departments reference themselves: delete leaves until none remain.
    loop {
        let n = sqlx::query(
            "DELETE FROM departments d WHERE NOT EXISTS \
             (SELECT 1 FROM departments c WHERE c.parent_id = d.id)",
        )
        .execute(pool)
        .await
        .map_err(|e| format!("delete departments: {e}"))?
        .rows_affected();
        if n == 0 {
            return Ok(());
        }
    }
}

/// Backend selected by `GED_BACKEND` (`rails` by default, or `rust`).
pub async fn backend() -> Box<dyn Backend> {
    match std::env::var("GED_BACKEND").as_deref().unwrap_or("rails") {
        "rails" => Box::new(crate::rails::RailsBackend::connect().await),
        "rust" => Box::new(crate::rust::RustBackend::connect().await),
        other => panic!("GED_BACKEND={other:?}: unknown backend (available: rails, rust)"),
    }
}
