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

    /// Blobs at [`crate::storage::STORAGE_DIR`], staging beside them. The HTTP
    /// process and the status disk check use that same constant.
    pub fn from_config(db: PgPool, max_byte_size: Option<u64>) -> Self {
        let root = crate::storage::STORAGE_DIR;
        Self::new(
            db,
            Storage::new(root),
            StagingStore::new(format!("{root}/staging")),
            max_byte_size.unwrap_or(DEFAULT_MAX_UPLOAD_BYTES),
        )
    }
}
