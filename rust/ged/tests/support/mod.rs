#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Request, StatusCode, header};
use axum::routing::{get, post};
use ged::config::Config;
use ged::http::{RackParams, RequestParams};
use ged::{AppState, router_with};
use http_body_util::BodyExt;
use tower::ServiceExt;

pub fn state() -> AppState {
    let config = Config::from_lookup(&|k| (k == "RAILS_ENV").then(|| "test".to_owned())).unwrap();
    AppState::without_database(config)
}

pub fn app() -> Router {
    router_with(state(), Router::new())
}

/// The router plus probe handlers under `/api/v1` and `/admin`.
pub fn app_with_probes() -> Router {
    let routes = Router::new()
        .route("/api/v1/probe", get(echo_query).post(echo_body))
        .route("/api/v1/boom", get(boom))
        .route("/api/v1/items/{id}", get(item))
        .route("/admin/probe", post(echo_body).get(boom));
    router_with(state(), routes)
}

async fn echo_query(params: RackParams) -> String {
    format!(
        "limit={:?} present={}",
        params.str("limit"),
        params.get("limit").is_some()
    )
}

async fn echo_body(RequestParams(params): RequestParams) -> String {
    format!("name={:?}", params.str("name"))
}

async fn boom() -> &'static str {
    panic!("secret internal detail: SELECT * FROM documents")
}

async fn item(Path(id): Path<i64>) -> String {
    id.to_string()
}

pub struct Reply {
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
}

pub async fn send(app: Router, req: Request<Body>) -> Reply {
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        content_type,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

pub async fn get_(uri: &str) -> Reply {
    send(
        app_with_probes(),
        Request::get(uri).body(Body::empty()).unwrap(),
    )
    .await
}

pub async fn request(method: &str, uri: &str, content_type: Option<&str>, body: &str) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    send(
        app_with_probes(),
        builder.body(Body::from(body.to_owned())).unwrap(),
    )
    .await
}

pub const JSON_CT: &str = "application/json; charset=utf-8";

// --- scratch databases ----------------------------------------------------

use sqlx::{Connection, PgConnection};

/// Server-level URL (database `postgres`): `GED_TEST_DATABASE_URL`, or this
/// worktree's Dockerised Postgres (`bin/worktree-stack db`).
pub fn server_url() -> String {
    if let Ok(url) = std::env::var("GED_TEST_DATABASE_URL") {
        return url;
    }
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bin/worktree-stack");
    let out = std::process::Command::new(script)
        .arg("db-port")
        .output()
        .expect("run bin/worktree-stack db-port");
    let port = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    assert!(
        !port.is_empty(),
        "bin/worktree-stack db-port printed nothing"
    );
    format!("postgres://postgres:postgres@localhost:{port}/postgres")
}

/// A throw-away database on the dev/test server, dropped by [`ScratchDb::drop_db`].
pub struct ScratchDb {
    pub name: String,
    pub url: String,
    server: String,
}

impl ScratchDb {
    pub async fn create() -> Self {
        let server = server_url();
        let name = format!("ged_test_{}", uuid::Uuid::now_v7().simple());
        let mut conn = PgConnection::connect(&server).await.unwrap_or_else(|e| {
            panic!("connect {server}: {e} (is `bin/worktree-stack db` running?)")
        });
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut conn)
            .await
            .unwrap();
        let (base, _) = server.rsplit_once('/').unwrap();
        Self {
            url: format!("{base}/{name}"),
            name,
            server,
        }
    }

    pub async fn pool(&self) -> sqlx::PgPool {
        sqlx::PgPool::connect(&self.url).await.unwrap()
    }

    pub async fn drop_db(self) {
        let mut conn = PgConnection::connect(&self.server).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut conn)
        .await
        .unwrap();
    }
}
