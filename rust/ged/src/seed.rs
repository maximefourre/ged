//! `ged seed`: port of `db/seeds.rb`. Idempotent, and keeps the admin
//! password in step with `GED_ADMIN_PASSWORD`.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use sqlx::PgPool;

use crate::config::{Config, Environment};

#[derive(Debug)]
pub enum SeedError {
    /// `ENV.fetch("GED_ADMIN_PASSWORD")` raising `KeyError` in production.
    MissingPassword,
    Hash(String),
    Db(sqlx::Error),
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPassword => f.write_str("GED_ADMIN_PASSWORD is not set"),
            Self::Hash(e) => write!(f, "hashing the admin password: {e}"),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for SeedError {}

impl From<sqlx::Error> for SeedError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

fn hash(password: &str) -> Result<String, SeedError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| SeedError::Hash(e.to_string()))
}

fn matches(stored: &str, password: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// Creates the admin, or re-hashes its password when it changed.
pub async fn run(pool: &PgPool, config: &Config) -> Result<(), SeedError> {
    let password = match (&config.admin_password, config.environment) {
        (Some(p), _) => p.clone(),
        (None, Environment::Production) => return Err(SeedError::MissingPassword),
        (None, _) => "changeme".to_owned(),
    };
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT encrypted_password FROM admins WHERE username = $1")
            .bind(&config.admin_username)
            .fetch_optional(pool)
            .await?;
    match existing {
        Some((stored,)) if matches(&stored, &password) => {}
        Some(_) => {
            sqlx::query(
                "UPDATE admins SET encrypted_password = $2, updated_at = now() AT TIME ZONE 'utc' WHERE username = $1",
            )
            .bind(&config.admin_username)
            .bind(hash(&password)?)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO admins (username, encrypted_password, created_at, updated_at) \
                 VALUES ($1, $2, now() AT TIME ZONE 'utc', now() AT TIME ZONE 'utc') \
                 ON CONFLICT (username) DO NOTHING",
            )
            .bind(&config.admin_username)
            .bind(hash(&password)?)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}
