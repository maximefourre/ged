//! The business tables are the same on both sides: `pg_dump --schema-only`
//! of `admins`, `app_clients`, `client_department_accesses`, `departments`,
//! `documents` and `upload_requests` — columns and their order, types,
//! defaults, constraints, indexes, foreign keys, sequences — compared after
//! dropping what pg_dump varies run to run (comments, session settings, the
//! `\restrict` token).
//!
//! The Rails side is the committed golden `fixtures/rails_business_schema.sql`,
//! captured from the Rails stack before it was removed. This test proves the
//! Rust migrations still reproduce that file. To regenerate it, check out the
//! tag `rails-final` and follow `docs/migration/parity-gaps.md` (replaying
//! Rails).

use parity::rust::MigratedDatabase;
use parity::stack::exec;

const TABLES: [&str; 6] = [
    "admins",
    "app_clients",
    "client_department_accesses",
    "departments",
    "documents",
    "upload_requests",
];

const GOLDEN: &str = include_str!("../fixtures/rails_business_schema.sql");

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
