use async_trait::async_trait;
use reqwest::header::HeaderMap;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::backend::Backend;
use crate::stack::{self, stack};

/// The Rails app running in this worktree's compose stack.
pub struct RailsBackend {
    pool: PgPool,
}

impl RailsBackend {
    pub async fn connect() -> Self {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&stack().database_url)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "connect {}: {e} (is `bin/worktree-stack up` running?)",
                    stack().database_url
                )
            });
        Self { pool }
    }
}

#[async_trait]
impl Backend for RailsBackend {
    fn base_url(&self) -> &str {
        &stack().base_url
    }

    fn db(&self) -> &PgPool {
        &self.pool
    }

    async fn run_job(&self, name: &str) -> Result<(), String> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("invalid job name {name:?}"));
        }
        let code = format!("{name}.perform_now");
        stack::exec("web", &["bin/rails", "runner", &code]).map(|_| ())
    }

    async fn reset(&self) -> Result<(), String> {
        crate::backend::reset_business_tables(&self.pool).await
    }

    /// Active Storage: sendfile headers, plus `/rails/storage/…` and
    /// `/rails/active_storage/…` locations.
    fn storage_locators(&self, headers: &HeaderMap) -> Vec<String> {
        let mut found = crate::backend::sendfile_locators(headers);
        for (name, value) in headers.iter() {
            let Ok(raw) = value.to_str() else {
                continue;
            };
            let mentions_storage = raw.contains("/rails/storage")
                || raw.contains("/rails/active_storage/")
                || raw.contains("active_storage");
            let line = format!("{}: {raw}", name.as_str());
            if mentions_storage && !found.contains(&line) {
                found.push(line);
            }
        }
        found
    }

    /// Active Storage HTTP routes mentioned by a locator. A filesystem path
    /// such as `/rails/storage/<key>` is not a route, so it is not requested.
    fn locator_http_paths(&self, locator: &str) -> Vec<String> {
        locator
            .split_whitespace()
            .filter_map(|token| {
                let idx = token.find("/rails/active_storage/")?;
                Some(
                    token[idx..]
                        .trim_matches(|c: char| c == '"' || c == '\'')
                        .to_owned(),
                )
            })
            .collect()
    }
}
