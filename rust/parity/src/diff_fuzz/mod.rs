//! Differential fuzzer for `/api/v1`.
//!
//! [`generate`] is pure and seeded. The `diff_fuzz` binary sends each request
//! to a Rails stack and a Rust stack that share SQL fixtures and signing keys,
//! normalizes UUIDs, Rails timestamps and ETags, and compares status, body and
//! the contract headers.

mod allow;
mod generate;
mod minimize;
mod model;
mod normalize;
mod rng;

pub use allow::{documented_gap, has_malformed_percent};
pub use generate::{CATEGORIES, Generator, seed_upload};
pub use minimize::{minimize, shrink_sequence, smaller_candidates};
pub use model::{
    Auth, Body, CLIENT_ID, DocAction, FilePart, IdRef, Ids, Request, Route, Target, encode,
};
pub use normalize::{Diff, HeaderDiff, Normalized, compare, normalize};
pub use rng::Rng;
