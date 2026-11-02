//! `ged migrate` and `ged seed` against a scratch database on the
//! worktree's Dockerised Postgres (`bin/worktree-stack db`).

mod support;

use std::process::Command;

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordVerifier};
use support::ScratchDb;

/// Tables of the Rails schema (compared byte for byte by `parity_schema`) plus
/// the tables the Rust side adds for the storage layer (T6).
const BUSINESS_TABLES: [&str; 7] = [
    "admins",
    "app_clients",
    "client_department_accesses",
    "departments",
    "documents",
    "stored_files",
    "upload_requests",
];

fn ged(args: &[&str], url: &str, env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ged"));
    cmd.args(args)
        .env_clear()
        .env("RAILS_ENV", "test")
        .env("DATABASE_URL", url);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

async fn tables(pool: &sqlx::PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations' ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn migrate_creates_the_business_tables_and_is_idempotent() {
    let db = ScratchDb::create().await;
    for run in 1..=2 {
        let out = ged(&["migrate"], &db.url, &[]);
        assert!(
            out.status.success(),
            "run {run}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(tables(&db.pool().await).await, BUSINESS_TABLES);
    db.drop_db().await;
}

#[tokio::test]
async fn migrate_without_a_database_url_fails_loudly() {
    let out = Command::new(env!("CARGO_BIN_EXE_ged"))
        .arg("migrate")
        .env_clear()
        .env("RAILS_ENV", "test")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("DATABASE_URL is not set"));
}

async fn admin(pool: &sqlx::PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT username, encrypted_password FROM admins ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

fn verifies(hash: &str, password: &str) -> bool {
    let parsed = PasswordHash::new(hash).unwrap();
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[tokio::test]
async fn seed_creates_the_admin_once_and_follows_a_changed_password() {
    let db = ScratchDb::create().await;
    assert!(ged(&["migrate"], &db.url, &[]).status.success());
    let pool = db.pool().await;
    let env = |pw: &'static str| [("GED_ADMIN_USERNAME", "root"), ("GED_ADMIN_PASSWORD", pw)];

    assert!(ged(&["seed"], &db.url, &env("first-pass")).status.success());
    let rows = admin(&pool).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "root");
    assert!(rows[0].1.starts_with("$argon2"), "{}", rows[0].1);
    assert!(verifies(&rows[0].1, "first-pass"));

    // Same password: untouched (same hash), still one row.
    assert!(ged(&["seed"], &db.url, &env("first-pass")).status.success());
    assert_eq!(admin(&pool).await, rows);

    // Changed password: the hash follows.
    assert!(
        ged(&["seed"], &db.url, &env("second-pass"))
            .status
            .success()
    );
    let after = admin(&pool).await;
    assert_eq!(after.len(), 1);
    assert!(verifies(&after[0].1, "second-pass"));
    assert!(!verifies(&after[0].1, "first-pass"));
    db.drop_db().await;
}

#[tokio::test]
async fn seed_in_production_requires_the_admin_password() {
    let db = ScratchDb::create().await;
    assert!(ged(&["migrate"], &db.url, &[]).status.success());
    let out = ged(
        &["seed"],
        &db.url,
        &[
            ("RAILS_ENV", "production"),
            ("SECRET_KEY_BASE", "x"),
            ("GED_OIDC_ISSUER", "i"),
            ("GED_OIDC_JWKS_URL", "j"),
            ("GED_OIDC_AUDIENCE", "a"),
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("GED_ADMIN_PASSWORD is not set"));
    assert!(admin(&db.pool().await).await.is_empty());
    db.drop_db().await;
}

#[tokio::test]
async fn seed_in_development_defaults_the_password_like_rails() {
    let db = ScratchDb::create().await;
    assert!(ged(&["migrate"], &db.url, &[]).status.success());
    assert!(
        ged(&["seed"], &db.url, &[("GED_ADMIN_USERNAME", "dev")])
            .status
            .success()
    );
    let rows = admin(&db.pool().await).await;
    assert!(verifies(&rows[0].1, "changeme"));
    db.drop_db().await;
}
