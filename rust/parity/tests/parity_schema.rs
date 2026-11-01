//! The business tables are the same on both sides: `pg_dump --schema-only`
//! of `admins`, `app_clients`, `client_department_accesses`, `departments`,
//! `documents` and `upload_requests` — columns and their order, types,
//! defaults, constraints, indexes, foreign keys, sequences — compared after
//! dropping what pg_dump varies run to run (comments, session settings, the
//! `\restrict` token).
//!
//! The Rails side is `fixtures/rails_business_schema.sql`, taken from the
//! Rails stack. The first test proves the Rust migrations reproduce it; the
//! second proves, while Rails exists, that the committed file is still what
//! Rails produces — so the golden file can never drift unnoticed.
//!
//! Regenerate the golden file after a Rails schema change:
//!
//! ```text
//! cd rust && GED_BACKEND=rails UPDATE_RAILS_SCHEMA=1 cargo test -p parity --test parity_schema
//! ```

use parity::rust::MigratedDatabase;
use parity::stack::{exec, stack};

const TABLES: [&str; 6] = [
    "admins",
    "app_clients",
    "client_department_accesses",
    "departments",
    "documents",
    "upload_requests",
];

const GOLDEN: &str = include_str!("../fixtures/rails_business_schema.sql");

/// Name of the database in a `postgres://…/name` URL.
fn database_of(url: &str) -> &str {
    url.rsplit_once('/').map_or(url, |(_, name)| name)
}

/// `pg_dump` of the business tables, run inside the stack's Postgres.
fn dump(database: &str) -> String {
    let mut args = vec![
        "pg_dump",
        "-U",
        "postgres",
        "--schema-only",
        "--no-owner",
        "--no-privileges",
    ];
    let tables: Vec<String> = TABLES.iter().map(|t| format!("public.{t}")).collect();
    for table in &tables {
        args.push("-t");
        args.push(table);
    }
    args.push(database);
    normalize(&exec("postgres", &args).unwrap_or_else(|e| panic!("pg_dump {database}: {e}")))
}

fn normalize(dump: &str) -> String {
    let mut out = String::new();
    for line in dump.lines() {
        let line = line.trim_end();
        let noise = line.is_empty()
            || line.starts_with("--")
            || line.starts_with("\\restrict")
            || line.starts_with("\\unrestrict")
            || line.starts_with("SET ")
            || line.starts_with("SELECT pg_catalog.set_config");
        if !noise {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[track_caller]
fn assert_same(what: &str, left_name: &str, left: &str, right_name: &str, right: &str) {
    if left == right {
        return;
    }
    let (l, r): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
    let first = l
        .iter()
        .zip(&r)
        .position(|(a, b)| a != b)
        .unwrap_or(l.len().min(r.len()));
    panic!(
        "{what}: {left_name} and {right_name} differ at line {}:\n  {left_name}: {:?}\n  {right_name}: {:?}\n({} vs {} lines)",
        first + 1,
        l.get(first),
        r.get(first),
        l.len(),
        r.len()
    );
}

#[tokio::test]
async fn parity_schema_the_rust_migrations_reproduce_the_rails_business_tables() {
    let db = MigratedDatabase::create().await;
    let rust = dump(&db.name);
    db.drop_db().await;
    assert!(
        rust.contains("CREATE TABLE public.documents"),
        "empty dump:\n{rust}"
    );
    assert_same(
        "business schema",
        "rust",
        &rust,
        "rails golden",
        &normalize(GOLDEN),
    );
}

#[tokio::test]
async fn parity_schema_the_golden_file_is_what_the_rails_stack_produces() {
    if std::env::var("GED_BACKEND").as_deref().unwrap_or("rails") != "rails" {
        eprintln!("skipped: GED_BACKEND is not rails, there is no Rails database to read");
        return;
    }
    let rails = dump(database_of(&stack().database_url));
    if std::env::var("UPDATE_RAILS_SCHEMA").is_ok() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/rails_business_schema.sql"
        );
        std::fs::write(path, &rails).expect("write the golden file");
    }
    assert_same(
        "rails schema",
        "rails stack",
        &rails,
        "golden file",
        &normalize(GOLDEN),
    );
}
