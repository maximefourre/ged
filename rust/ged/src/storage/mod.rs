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
