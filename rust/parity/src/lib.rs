//! Black-box parity suite for GED.
//!
//! The suite speaks HTTP to `GED_BASE_URL` and reads/writes the **business
//! tables only** (`departments`, `app_clients`, `client_department_accesses`,
//! `documents`, `upload_requests`, `admins`) through `DATABASE_URL`. It never
//! touches storage or queue tables: files always go through HTTP, and jobs
//! are triggered through [`Backend::run_job`].
//!
//! The only backend is Rust (`ged` on this worktree's compose stack).
//! `GED_BACKEND=rails` is rejected: the Rails tree is the tag `rails-final`.
//! Stack coordinates come from `GED_BASE_URL` / `GED_OAUTH_URL` /
//! `DATABASE_URL`, or are derived from this worktree's `bin/worktree-stack`.

pub mod admin;
pub mod backend;
pub mod diff_fuzz;
pub mod fixtures;
pub mod forge;
pub mod http;
pub mod json;
pub mod oauth;
pub mod rust;
pub mod stack;

pub use backend::{Backend, backend};
pub use http::{Api, Resp};
pub use sqlx::PgPool;
