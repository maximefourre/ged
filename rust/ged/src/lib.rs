//! GED: document management system.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod domain;
pub mod http;
pub mod jobs;
pub mod seed;
pub mod telemetry;

pub mod storage;

pub use http::{AppState, router, router_with};
