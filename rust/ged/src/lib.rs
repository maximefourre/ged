//! GED: document management system.

pub mod config;
pub mod db;
pub mod domain;
pub mod http;
pub mod seed;

pub use http::{AppState, router, router_with};
