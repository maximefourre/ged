use std::sync::Arc;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use crate::config::Config;

/// Shared by every handler. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        Self {
            config: Arc::new(config),
            db,
        }
    }
}

impl AppState {
    /// A state whose pool connects on first use, for code (and tests) that
    /// never touches the database.
    pub fn without_database(config: Config) -> Self {
        let options = PgConnectOptions::new()
            .host("localhost")
            .database("ged_unused");
        Self::new(config, PgPoolOptions::new().connect_lazy_with(options))
    }
}
