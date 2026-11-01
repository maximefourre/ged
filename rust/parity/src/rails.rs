use async_trait::async_trait;
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
        // Children first; `upload_requests.document_id` is ON DELETE NULLIFY.
        for sql in [
            "DELETE FROM upload_requests",
            "DELETE FROM documents",
            "DELETE FROM client_department_accesses",
            "DELETE FROM app_clients",
        ] {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .map_err(|e| format!("{sql}: {e}"))?;
        }
        // Departments reference themselves: delete leaves until none remain.
        loop {
            let n = sqlx::query(
                "DELETE FROM departments d WHERE NOT EXISTS \
                 (SELECT 1 FROM departments c WHERE c.parent_id = d.id)",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| format!("delete departments: {e}"))?
            .rows_affected();
            if n == 0 {
                return Ok(());
            }
        }
    }
}
