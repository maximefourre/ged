//! What the pipeline jobs need from the process: the database, the document
//! store, the staging area and the size cap.

use sqlx::PgPool;

use crate::config::DEFAULT_MAX_UPLOAD_BYTES;
use crate::storage::{StagingStore, Storage};

#[derive(Clone)]
pub struct PipelineEnv {
    pub db: PgPool,
    pub storage: Storage,
    pub staging: StagingStore,
    /// `Document::MAX_BYTE_SIZE` (`GED_MAX_UPLOAD_BYTES`).
    pub max_byte_size: u64,
}

impl PipelineEnv {
    pub fn new(db: PgPool, storage: Storage, staging: StagingStore, max_byte_size: u64) -> Self {
        Self {
            db,
            storage,
            staging,
            max_byte_size,
        }
    }

    /// Rails' layout: Active Storage's disk service at `storage/`, the
    /// staging area at `storage/staging` (relative to the working directory,
    /// like `Rails.root`).
    pub fn from_config(db: PgPool, max_byte_size: Option<u64>) -> Self {
        Self::new(
            db,
            Storage::new("storage"),
            StagingStore::new("storage/staging"),
            max_byte_size.unwrap_or(DEFAULT_MAX_UPLOAD_BYTES),
        )
    }
}
