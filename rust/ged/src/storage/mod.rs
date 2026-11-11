//! File storage: the disk store, the async-upload staging area, content type
//! identification (a port of Active Storage's Marcel call) and download names.

pub mod content_type;
pub mod disk;
pub mod disposition;
pub mod files;
mod marcel_tables;
pub mod staging;
mod transliteration_table;

pub use content_type::{SNIFF_BYTES, detect_content_type};
pub use disk::{Storage, Stored};
pub use files::StorageError;
pub use staging::StagingStore;

/// Blobs and staging, relative to the process working directory.
///
/// The image working directory is `/rails` and compose mounts the `storage`
/// volume at `/rails/storage`, so this is the directory the HTTP process, the
/// worker and `/admin/status` share. It is not read from the environment.
pub const STORAGE_DIR: &str = "storage";

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::STORAGE_DIR;

    #[tokio::test]
    async fn http_worker_and_status_share_one_directory() {
        assert!(
            !include_str!("../admin/mod.rs").contains("GED_STORAGE_ROOT"),
            "the status disk check must not grow a second storage root"
        );
        assert!(
            !include_str!("../jobs/env.rs").contains("GED_STORAGE_ROOT"),
            "the worker must not grow a second storage root"
        );
        let config = crate::config::Config::from_lookup(&|key| {
            (key == "RAILS_ENV").then(|| "test".to_owned())
        })
        .expect("test config");
        let state = crate::http::state::AppState::without_database(config);
        assert_eq!(state.storage.root(), Path::new(STORAGE_DIR));
        assert_eq!(state.staging.root(), Path::new("storage/staging"));
        assert_eq!(crate::admin::storage_root(), Path::new(STORAGE_DIR));

        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(
            sqlx::postgres::PgConnectOptions::new()
                .host("localhost")
                .database("ged_unused"),
        );
        let env = crate::jobs::env::PipelineEnv::from_config(pool, None);
        assert_eq!(env.storage.root(), Path::new(STORAGE_DIR));
        assert_eq!(env.staging.root(), Path::new("storage/staging"));
    }
}
