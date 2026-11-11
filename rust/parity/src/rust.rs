//! The Rust `ged` binary as the backend under test.
//!
//! By default the adapter talks to this worktree's compose stack (`ged serve`
//! / `ged worker`). `run_job` execs `ged run-job` inside the `web` container
//! so the job sees the shared storage volume.
//!
//! `GED_RUST_SPAWN=1` builds `ged`, creates a scratch database on this
//! worktree's Postgres, runs `ged migrate` and `ged seed`, then starts
//! `ged serve` on a free port. The worker is a child of this process: `reset`
//! stops it, truncates the business tables, and starts it again on the same
//! database. Each process owns `ged_rust_parity_<pid>`, so a parallel test
//! binary cannot drop the database out from under the worker.
//!
//! Set both `GED_BASE_URL` and `DATABASE_URL` to attach to a server that is
//! already running somewhere else. Nothing is spawned.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};

use crate::backend::Backend;
use crate::stack::{admin_credentials, stack};

/// Prefix of the scratch database created for [`spawn`]. The pid suffix keeps
/// parallel `cargo test` binaries from dropping each other's database.
const DATABASE_PREFIX: &str = "ged_rust_parity";

pub struct RustBackend {
    pool: PgPool,
    base_url: String,
    /// `GED_RUST_SPAWN=1`: this process owns `ged serve` and `ged worker`.
    spawned: bool,
}

struct Running {
    base_url: String,
    database_url: String,
    spawned: bool,
}

struct SpawnEnv {
    ged: PathBuf,
    envs: Vec<(String, String)>,
    worker_pid_file: PathBuf,
}

static SPAWN_ENV: OnceLock<SpawnEnv> = OnceLock::new();
static WORKER: Mutex<Option<Child>> = Mutex::new(None);

fn database_name_for(pid: u32) -> String {
    format!("{DATABASE_PREFIX}_{pid}")
}

fn worker_slot() -> std::sync::MutexGuard<'static, Option<Child>> {
    WORKER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn running() -> &'static Running {
    static RUNNING: OnceLock<Running> = OnceLock::new();
    RUNNING.get_or_init(|| {
        if std::env::var("GED_RUST_SPAWN").ok().as_deref() == Some("1") {
            return spawn();
        }
        let explicit_url = std::env::var("GED_BASE_URL").ok().filter(|v| !v.is_empty());
        let explicit_db = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty());
        match (explicit_url, explicit_db) {
            (Some(base), Some(db)) => Running {
                base_url: base.trim_end_matches('/').to_owned(),
                database_url: db,
                spawned: false,
            },
            // One side missing: `stack()` fills it from the worktree compose
            // file. The server under test is that stack, not a local spawn.
            _ => {
                let stack = stack();
                Running {
                    base_url: stack.base_url.clone(),
                    database_url: stack.database_url.clone(),
                    spawned: false,
                }
            }
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

fn is_safe_db_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Drops this process's scratch database, the legacy shared name, and names
/// left by dead pids. A live sibling's database is left alone.
async fn prepare_scratch_database(server_url: &str, name: &str) {
    assert!(is_safe_db_name(name), "unsafe database name {name}");
    let mut conn = PgConnection::connect(server_url).await.unwrap_or_else(|e| {
        panic!("connect {server_url}: {e} (is `bin/worktree-stack db` running?)")
    });
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT datname FROM pg_database \
         WHERE datname = 'ged_rust_parity' \
            OR datname LIKE 'ged\\_rust\\_parity\\_%' ESCAPE '\\'",
    )
    .fetch_all(&mut conn)
    .await
    .expect("list parity databases");
    let me = std::process::id();
    for (existing,) in rows {
        if !is_safe_db_name(&existing) {
            continue;
        }
        let drop_it = if existing == "ged_rust_parity" {
            true
        } else if let Some(rest) = existing.strip_prefix("ged_rust_parity_") {
            rest.parse::<u32>()
                .is_ok_and(|pid| pid == me || !pid_alive(pid))
        } else {
            false
        };
        if !drop_it {
            continue;
        }
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {existing} WITH (FORCE)"
        )))
        .execute(&mut conn)
        .await
        .expect("drop a stale parity database");
    }
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut conn)
        .await
        .expect("create the scratch database");
}

fn stop_spawned_worker() -> Result<(), String> {
    let mut slot = worker_slot();
    let Some(mut child) = slot.take() else {
        return Ok(());
    };
    let pid = child.id();
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                if let Some(env) = SPAWN_ENV.get() {
                    let _ = std::fs::remove_file(&env.worker_pid_file);
                }
                return Ok(());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(env) = SPAWN_ENV.get() {
                    let _ = std::fs::remove_file(&env.worker_pid_file);
                }
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("wait for ged worker: {e}")),
        }
    }
}

fn start_spawned_worker() -> Result<(), String> {
    stop_spawned_worker()?;
    let env = SPAWN_ENV.get().ok_or("spawned ged env is missing")?;
    let child = Command::new(&env.ged)
        .arg("worker")
        .env_clear()
        .envs(env.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("start ged worker: {e}"))?;
    std::fs::write(&env.worker_pid_file, child.id().to_string())
        .map_err(|e| format!("record worker pid: {e}"))?;
    *worker_slot() = Some(child);
    Ok(())
}

fn spawn() -> Running {
    let ged = ged_binary().clone();
    let stack = stack();
    let server_url = with_database(&stack.database_url, "postgres");
    let name = database_name_for(std::process::id());
    let database_url = with_database(&stack.database_url, &name);

    block_on(prepare_scratch_database(&server_url, &name));

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

    let worker_pid_file =
        std::env::temp_dir().join(format!("ged-parity-worker-{}.pid", std::process::id()));
    let owned_envs: Vec<(String, String)> = envs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    let _ = SPAWN_ENV.set(SpawnEnv {
        ged: ged.clone(),
        envs: owned_envs,
        worker_pid_file: worker_pid_file.clone(),
    });

    // The watchdog ends `ged serve` and the worker pid when this test process
    // goes away (statics are never dropped). `reset` restarts the worker
    // itself, so the shell must not own that process.
    let script = r#"
        "$GED" serve & child=$!
        while kill -0 "$PARENT" 2>/dev/null && kill -0 "$child" 2>/dev/null; do sleep 1; done
        kill "$child" 2>/dev/null
        if [ -f "$WORKER_PID" ]; then kill "$(cat "$WORKER_PID")" 2>/dev/null; fi
        wait "$child" 2>/dev/null
    "#;
    let mut watchdog = Command::new("sh")
        .args(["-c", script])
        .env_clear()
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
        .env("GED", &ged)
        .env("PARENT", std::process::id().to_string())
        .env("WORKER_PID", &worker_pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start ged serve");
    std::thread::spawn(move || watchdog.wait());
    start_spawned_worker().expect("start ged worker");

    let base_url = format!("http://127.0.0.1:{port}");
    wait_until_up(&base_url);
    Running {
        base_url,
        database_url,
        spawned: true,
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
            spawned: r.spawned,
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
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("invalid job name {name:?}"));
        }
        if self.spawned {
            // Same env and working directory as the spawned server, so the
            // job sees the same `storage/` and the same database.
            let env = SPAWN_ENV.get().ok_or("spawned ged env is missing")?;
            let out = Command::new(&env.ged)
                .args(["run-job", name])
                .env_clear()
                .envs(env.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .output()
                .map_err(|e| format!("run ged run-job {name}: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "{name}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        } else {
            // Compose web and worker share the storage volume. A host-side
            // `ged run-job` would miss those files.
            crate::stack::exec("web", &["ged", "run-job", name]).map(|_| ())
        }
    }

    async fn reset(&self) -> Result<(), String> {
        // Stop the spawned worker before truncating, then start it again on
        // the database that still exists. The compose worker is the stack's:
        // it stays up and keeps its pool.
        if self.spawned {
            tokio::task::spawn_blocking(stop_spawned_worker)
                .await
                .map_err(|e| format!("stop ged worker: {e}"))??;
        }
        let truncated = crate::backend::reset_business_tables(&self.pool).await;
        if self.spawned {
            tokio::task::spawn_blocking(start_spawned_worker)
                .await
                .map_err(|e| format!("start ged worker: {e}"))??;
        }
        truncated
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

#[cfg(test)]
mod tests {
    use super::database_name_for;

    #[test]
    fn each_process_gets_its_own_parity_database() {
        let first = database_name_for(10);
        let second = database_name_for(11);
        assert_eq!(first, "ged_rust_parity_10");
        assert_ne!(first, second);
        assert_ne!(first, "ged_rust_parity");
    }
}
