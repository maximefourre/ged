//! The Rust `ged` binary as the backend under test.
//!
//! By default the adapter builds `ged`, creates a scratch business database on
//! this worktree's Postgres, runs `ged migrate` and `ged seed`, then starts
//! `ged serve` on a free port, configured like the compose stack's `web`
//! service (production mode: its boot-time checks are part of the contract).
//! The server lives for the whole test process and is stopped by a watchdog
//! when that process exits.
//!
//! To attach to a server you already run (the compose stack, later), set both
//! `GED_BASE_URL` and `DATABASE_URL`: nothing is spawned.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};

use crate::backend::Backend;
use crate::stack::{admin_credentials, stack};

/// Database created for the spawned server.
const DATABASE: &str = "ged_rust_parity";

pub struct RustBackend {
    pool: PgPool,
    base_url: String,
}

struct Running {
    base_url: String,
    database_url: String,
}

fn running() -> &'static Running {
    static RUNNING: OnceLock<Running> = OnceLock::new();
    RUNNING.get_or_init(|| {
        let explicit_url = std::env::var("GED_BASE_URL").ok().filter(|v| !v.is_empty());
        let explicit_db = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty());
        match (explicit_url, explicit_db) {
            (Some(base), Some(db)) => Running {
                base_url: base.trim_end_matches('/').to_owned(),
                database_url: db,
            },
            _ => spawn(),
        }
    })
}

/// `…/ged_production` → `…/<name>`.
fn with_database(url: &str, name: &str) -> String {
    match url.rsplit_once('/') {
        Some((base, _)) => format!("{base}/{name}"),
        None => url.to_owned(),
    }
}

fn workspace_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The `ged` executable, built once per process.
pub fn ged_binary() -> &'static PathBuf {
    static GED: OnceLock<PathBuf> = OnceLock::new();
    GED.get_or_init(build_ged)
}

/// Builds `ged` and returns the executable cargo reports.
fn build_ged() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["build", "-p", "ged", "--message-format=json"])
        .current_dir(workspace_dir())
        .stderr(Stdio::inherit())
        .output()
        .expect("run cargo build -p ged");
    assert!(out.status.success(), "cargo build -p ged failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == "ged")
        .filter_map(|m| m["executable"].as_str().map(PathBuf::from))
        .next_back()
        .expect("cargo did not report the ged executable")
}

fn block_on<T: Send>(f: impl std::future::Future<Output = T> + Send) -> T {
    // The suite is async, but this runs once inside a sync initializer: use a
    // separate thread with its own runtime so we never nest runtimes.
    std::thread::scope(|s| {
        s.spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(f)
        })
        .join()
        .expect("setup thread")
    })
}

fn spawn() -> Running {
    let ged = ged_binary().clone();
    let stack = stack();
    let server_url = with_database(&stack.database_url, "postgres");
    let database_url = with_database(&stack.database_url, DATABASE);

    block_on(async {
        let mut conn = PgConnection::connect(&server_url)
            .await
            .unwrap_or_else(|e| {
                panic!("connect {server_url}: {e} (is `bin/worktree-stack db` running?)")
            });
        for sql in [
            format!("DROP DATABASE IF EXISTS {DATABASE} WITH (FORCE)"),
            format!("CREATE DATABASE {DATABASE}"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut conn)
                .await
                .expect("prepare the scratch database");
        }
    });

    let (admin_user, admin_password) = admin_credentials();
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port")
        .port();
    // Same shape as the `web` service of docker-compose.yml.
    let envs: Vec<(&str, String)> = vec![
        ("RAILS_ENV", "production".into()),
        ("DATABASE_URL", database_url.clone()),
        ("SECRET_KEY_BASE", "0123456789abcdef".repeat(8)),
        ("GED_OIDC_ISSUER", "http://oauth-provider:9292".into()),
        (
            "GED_OIDC_JWKS_URL",
            format!("{}/.well-known/jwks.json", stack.oauth_url),
        ),
        (
            "GED_OIDC_AUDIENCE",
            std::env::var("GED_OIDC_AUDIENCE").unwrap_or_else(|_| "http://web:80".into()),
        ),
        ("GED_ADMIN_USERNAME", admin_user),
        ("GED_ADMIN_PASSWORD", admin_password),
        ("PORT", port.to_string()),
    ];
    let run = |arg: &str| {
        let out = Command::new(&ged)
            .arg(arg)
            .env_clear()
            .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
            .output()
            .unwrap_or_else(|e| panic!("run ged {arg}: {e}"));
        assert!(
            out.status.success(),
            "ged {arg} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run("migrate");
    run("seed");

    // The watchdog ends the server when this test process goes away (statics
    // are never dropped), without leaving a daemon behind.
    let script = r#"
        "$GED" serve & child=$!
        while kill -0 "$PARENT" 2>/dev/null && kill -0 "$child" 2>/dev/null; do sleep 1; done
        kill "$child" 2>/dev/null; wait "$child" 2>/dev/null
    "#;
    let mut watchdog = Command::new("sh")
        .args(["-c", script])
        .env_clear()
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
        .env("GED", &ged)
        .env("PARENT", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start ged serve");
    std::thread::spawn(move || watchdog.wait());

    let base_url = format!("http://127.0.0.1:{port}");
    wait_until_up(&base_url);
    Running {
        base_url,
        database_url,
    }
}

fn wait_until_up(base_url: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let addr = base_url.trim_start_matches("http://");
    loop {
        if std::net::TcpStream::connect(addr).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "ged serve never listened on {addr}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl RustBackend {
    pub async fn connect() -> Self {
        // Blocks once per process (build, migrate, spawn); every later call
        // returns immediately.
        let r = running();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&r.database_url)
            .await
            .unwrap_or_else(|e| panic!("connect {}: {e}", r.database_url));
        Self {
            pool,
            base_url: r.base_url.clone(),
        }
    }
}

#[async_trait]
impl Backend for RustBackend {
    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn db(&self) -> &PgPool {
        &self.pool
    }

    async fn run_job(&self, name: &str) -> Result<(), String> {
        Err(format!(
            "{name}: the Rust job runner does not exist yet (queue: T8, jobs: T9)"
        ))
    }

    async fn reset(&self) -> Result<(), String> {
        crate::backend::reset_business_tables(&self.pool).await
    }

    /// Files are served by the app itself; only a sendfile header could
    /// point at the stored file.
    fn storage_locators(&self, headers: &HeaderMap) -> Vec<String> {
        crate::backend::sendfile_locators(headers)
    }

    fn locator_http_paths(&self, _locator: &str) -> Vec<String> {
        Vec::new()
    }
}

/// A throw-away database on this worktree's Postgres, migrated by the real
/// `ged migrate`. Drop it with [`MigratedDatabase::drop_db`].
pub struct MigratedDatabase {
    pub name: String,
    pub url: String,
    server_url: String,
}

impl MigratedDatabase {
    pub async fn create() -> Self {
        let stack = stack();
        let server_url = with_database(&stack.database_url, "postgres");
        let name = format!("ged_schema_{}", uuid::Uuid::new_v4().simple());
        let url = with_database(&stack.database_url, &name);
        let mut conn = PgConnection::connect(&server_url)
            .await
            .unwrap_or_else(|e| {
                panic!("connect {server_url}: {e} (is `bin/worktree-stack db` running?)")
            });
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut conn)
            .await
            .expect("create the scratch database");
        let ged = ged_binary().clone();
        let out = tokio::task::spawn_blocking({
            let url = url.clone();
            move || {
                Command::new(ged)
                    .arg("migrate")
                    .env_clear()
                    .env("RAILS_ENV", "test")
                    .env("DATABASE_URL", url)
                    .output()
            }
        })
        .await
        .expect("join")
        .expect("run ged migrate");
        assert!(
            out.status.success(),
            "ged migrate failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self {
            name,
            url,
            server_url,
        }
    }

    pub async fn drop_db(self) {
        let mut conn = PgConnection::connect(&self.server_url)
            .await
            .expect("connect");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut conn)
        .await
        .expect("drop the scratch database");
    }
}
