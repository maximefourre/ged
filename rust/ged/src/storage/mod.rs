//! File storage: the disk store, the async-upload staging area, and content
//! type identification (a port of Active Storage's Marcel call).

pub mod content_type;
mod marcel_tables;

pub use content_type::detect_content_type;
