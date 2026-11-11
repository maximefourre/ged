use std::sync::Arc;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use crate::auth::{JwksCache, TokenValidator};
use crate::config::Config;
use crate::storage::{StagingStore, Storage};

/// Shared by every handler. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    /// Process-local JWKS cache, shared by every request.
    pub jwks: Arc<JwksCache>,
    /// Document files (Active Storage's disk service: `storage/xx/yy/<key>`).
    pub storage: Storage,
    /// Raw bytes of async uploads awaiting their job (`storage/staging/<id>`).
    pub staging: StagingStore,

    /// Admin sessions and login throttle.
    pub admin: Arc<crate::admin::AdminRuntime>,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        // Without a URL the cache can never fetch: bearer auth fails closed.
        let jwks = Arc::new(JwksCache::new(
            config.oidc.jwks_url.clone().unwrap_or_default(),
        ));
        Self {
            admin: Arc::new(crate::admin::AdminRuntime::new(
                config.secret_key_base.as_deref(),
            )),
            config: Arc::new(config),
            db,
            jwks,
            storage: Storage::new(crate::storage::STORAGE_DIR),
            staging: StagingStore::new(format!("{}/staging", crate::storage::STORAGE_DIR)),
        }
    }

    /// Where files live instead of the default `storage/` (tests).
    pub fn with_storage(mut self, storage: Storage, staging: StagingStore) -> Self {
        self.storage = storage;
        self.staging = staging;
        self
    }
}

/// Same directory as [`crate::storage::STORAGE_DIR`].
pub use crate::storage::STORAGE_DIR;

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

impl AppState {
    pub fn token_validator(&self) -> TokenValidator {
        TokenValidator::new(self.jwks.clone(), self.config.oidc.clone())
    }
}
