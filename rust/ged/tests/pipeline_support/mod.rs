#![allow(dead_code)]
//! Fixtures for the T9 pipeline tests: a scratch database, a temp storage
//! root, departments and upload requests inserted with plain SQL.

use ged::jobs::env::PipelineEnv;
use ged::storage::{StagingStore, Storage};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::ScratchDb;

pub struct Fixture {
    scratch: ScratchDb,
    pub pool: PgPool,
    pub dir: tempfile::TempDir,
    pub env: PipelineEnv,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_max_bytes(52_428_800).await
    }

    pub async fn with_max_bytes(max: u64) -> Self {
        let scratch = ScratchDb::create().await;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(&scratch.url)
            .await
            .unwrap();
        ged::db::migrate(&pool).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path());
        let staging = StagingStore::new(dir.path().join("staging"));
        std::fs::create_dir_all(staging.root()).unwrap();
        let env = PipelineEnv::new(pool.clone(), storage, staging, max);
        Self {
            scratch,
            pool,
            dir,
            env,
        }
    }

    pub fn url(&self) -> String {
        self.scratch.url.clone()
    }

    pub async fn finish(self) {
        self.pool.close().await;
        self.scratch.drop_db().await;
    }

    pub async fn department(&self, name: &str, parent: Option<i64>) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO departments (name, parent_id, created_at, updated_at) \
             VALUES ($1, $2, now(), now()) RETURNING id",
        )
        .bind(name)
        .bind(parent)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// A request with its staging file (when `bytes` is `Some`).
    pub async fn upload_request(
        &self,
        department_id: i64,
        status: &str,
        callback_url: &str,
        bytes: Option<&[u8]>,
    ) -> Uuid {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO upload_requests (callback_url, department_id, filename, status, tags, \
             uploaded_by, created_at, updated_at) \
             VALUES ($1, $2, 'report.txt', $3, '{\"type\": \"facture\", \"urgent\": null}', \
             'client-a', '2026-11-01 10:00:00.123456', now()) RETURNING id",
        )
        .bind(callback_url)
        .bind(department_id)
        .bind(status)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        if let Some(bytes) = bytes {
            std::fs::write(self.env.staging.path(id), bytes).unwrap();
        }
        id
    }

    pub async fn set_times(&self, id: Uuid, created_ago: &str, updated_ago: &str) {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE upload_requests SET created_at = (now() AT TIME ZONE 'UTC') - interval '{created_ago}', \
             updated_at = (now() AT TIME ZONE 'UTC') - interval '{updated_ago}' WHERE id = $1"
        )))
        .bind(id)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    pub async fn status(&self, id: Uuid) -> (String, Option<String>, Option<Uuid>) {
        sqlx::query_as(
            "SELECT status, error_message, document_id FROM upload_requests WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// `(queue, kind, args)` of every enqueued job, oldest first.
    pub async fn jobs(&self) -> Vec<(String, String, Value)> {
        sqlx::query_as("SELECT queue, kind, args FROM jobs ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    /// Every regular file under the storage root, staging excluded.
    pub fn stored_files(&self) -> Vec<std::path::PathBuf> {
        fn walk(
            dir: &std::path::Path,
            staging: &std::path::Path,
            out: &mut Vec<std::path::PathBuf>,
        ) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if p == staging {
                    continue;
                }
                if p.is_dir() {
                    walk(&p, staging, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(self.dir.path(), self.env.staging.root(), &mut out);
        out
    }
}

// --- a callback receiver -----------------------------------------------------

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Accepts connections on 127.0.0.1 and records each raw HTTP request.
pub struct Hook {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Hook {
    /// Answers every request with `status` (and a `Location` for 3xx).
    pub async fn start(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&raw).into_owned();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let want = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if body.len() >= want {
                                seen.lock().unwrap().push(text);
                                break;
                            }
                        }
                    }
                    let extra = if (300..400).contains(&status) {
                        "Location: http://127.0.0.1:1/elsewhere\r\n"
                    } else {
                        ""
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                });
            }
        });
        Self {
            addr,
            requests,
            task,
        }
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    pub fn url(&self) -> String {
        format!("http://{}/hook", self.addr)
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Splits a recorded request into (request line, lowercase header map text, body).
pub fn parse_request(raw: &str) -> (String, String, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let (line, headers) = head.split_once("\r\n").unwrap_or((head, ""));
    (line.to_owned(), headers.to_owned(), body.to_owned())
}

/// Captures the first bytes a client sends (a TLS ClientHello) and hangs up.
pub async fn hello_sniffer() -> (SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap_or(0);
        buf.truncate(n);
        buf
    });
    (addr, task)
}

// --- documents with a stored file ----------------------------------------------

impl Fixture {
    /// A document and its file on disk; `purge_at` set means trashed.
    pub async fn document(
        &self,
        department_id: i64,
        purge_at: Option<chrono::NaiveDateTime>,
    ) -> (Uuid, String) {
        let key = Uuid::new_v4().simple().to_string();
        let path = self.env.storage.path_for(&key).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"stored bytes").unwrap();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO documents (department_id, filename, content_type, byte_size, uploaded_by, \
             tags, uploaded_at, stored_async, created_at, updated_at, deleted_at, purge_at) \
             VALUES ($1, 'a.txt', 'text/plain', 12, 'client-a', '{}', now(), false, now(), now(), \
             CASE WHEN $2::timestamp IS NULL THEN NULL ELSE now() END, $2) RETURNING id",
        )
        .bind(department_id)
        .bind(purge_at)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO stored_files (key, document_id, filename, content_type, byte_size, checksum, created_at) \
             VALUES ($1, $2, 'a.txt', 'text/plain', 12, 'x', now())",
        )
        .bind(&key)
        .bind(id)
        .execute(&self.pool)
        .await
        .unwrap();
        (id, key)
    }

    pub fn file_exists(&self, key: &str) -> bool {
        self.env.storage.path_for(key).unwrap().exists()
    }

    pub async fn trash_department(&self, id: i64) {
        sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

pub fn age_file(path: &std::path::Path, days: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}
