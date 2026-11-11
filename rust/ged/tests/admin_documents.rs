//! Admin document upload must not leave a file behind when the client hangs
//! up mid-body, or when the row is rejected before commit.

mod support;

use std::path::Path;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, header};
use ged::config::Config;
use ged::storage::{StagingStore, Storage};
use ged::{AppState, db, seed};
use http_body_util::BodyExt;
use support::ScratchDb;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

struct Site {
    db: ScratchDb,
    pool: sqlx::PgPool,
    app: Router,
    root: tempfile::TempDir,
}

async fn site() -> Site {
    let db = ScratchDb::create().await;
    let pool = db.pool().await;
    db::migrate(&pool).await.unwrap();
    let env = |k: &str| match k {
        "RAILS_ENV" => Some("test".to_owned()),
        "SECRET_KEY_BASE" => Some("k".repeat(64)),
        "GED_ADMIN_USERNAME" => Some("admin".to_owned()),
        "GED_ADMIN_PASSWORD" => Some(PASSWORD.to_owned()),
        _ => None,
    };
    let config = Config::from_lookup(&env).unwrap();
    seed::run(&pool, &config).await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(config, pool.clone()).with_storage(
        Storage::new(root.path().join("files")),
        StagingStore::new(root.path().join("staging")),
    );
    let app = ged::router(state);
    Site {
        db,
        pool,
        app,
        root,
    }
}

impl Site {
    async fn finish(self) {
        self.pool.close().await;
        self.db.drop_db().await;
    }

    fn files(&self) -> Vec<std::path::PathBuf> {
        files_under(&self.root.path().join("files"))
    }
}

fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

struct Browser {
    app: Router,
    cookie: Option<String>,
}

impl Browser {
    fn new(app: &Router) -> Self {
        Self {
            app: app.clone(),
            cookie: None,
        }
    }

    async fn send(&mut self, method: Method, uri: &str, form: Option<&str>) -> (u16, String) {
        let mut req = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(header::HOST, "ged.test")
            .header(header::ACCEPT, "text/html");
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, cookie);
        }
        if form.is_some() {
            req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        }
        let mut req = req.body(Body::from(form.unwrap_or("").to_owned())).unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(
            "203.0.113.50:40000"
                .parse::<std::net::SocketAddr>()
                .unwrap(),
        ));
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        for value in resp.headers().get_all(header::SET_COOKIE) {
            if let Some(pair) = value.to_str().unwrap().split(';').next()
                && pair.starts_with("_ged_session=")
            {
                self.cookie = Some(pair.to_owned());
            }
        }
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn login(&mut self) {
        let (_, html) = self.send(Method::GET, "/admins/sign_in", None).await;
        let token = csrf(&html);
        let form = format!(
            "authenticity_token={token}&admin%5Busername%5D=admin&admin%5Bpassword%5D={}&commit=Log+in",
            PASSWORD.replace(' ', "+")
        );
        let (status, _) = self
            .send(Method::POST, "/admins/sign_in", Some(&form))
            .await;
        assert!(matches!(status, 302 | 303), "login status {status}");
    }

    async fn token(&mut self) -> String {
        let (_, html) = self.send(Method::GET, "/admin/documents/new", None).await;
        csrf(&html)
    }
}

fn csrf(html: &str) -> String {
    let marker = "name=\"csrf-token\" content=\"";
    let start = html.find(marker).expect("csrf meta") + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_owned()
}

fn open_upload(token: &str) -> bytes::Bytes {
    let mut body = format!(
        "--XB\r\nContent-Disposition: form-data; name=\"authenticity_token\"\r\n\r\n{token}\r\n\
         --XB\r\nContent-Disposition: form-data; name=\"document[file]\"; filename=\"cut.pdf\"\r\n\
         Content-Type: application/pdf\r\n\r\n"
    );
    body.push_str(&"a".repeat(64 * 1024));
    bytes::Bytes::from(body)
}

#[tokio::test]
async fn a_truncated_admin_upload_leaves_no_file() {
    let site = site().await;
    let mut browser = Browser::new(&site.app);
    browser.login().await;
    let token = browser.token().await;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<bytes::Bytes>();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<_, std::io::Error>(chunk), rx))
    });
    tx.send(open_upload(&token)).unwrap();

    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/admin/documents")
        .header(header::HOST, "ged.test")
        .header(header::ACCEPT, "text/html")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=XB")
        .header(header::COOKIE, browser.cookie.expect("session"))
        .body(Body::from_stream(stream))
        .unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(
        "203.0.113.50:40000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let app = site.app.clone();
    let handle = tokio::spawn(async move { app.oneshot(req).await });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if !site.files().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "truncated upload never opened a file"
        );
        tokio::task::yield_now().await;
    }
    assert!(
        !handle.is_finished(),
        "the upload finished before the body was cut"
    );
    handle.abort();
    let _ = handle.await;
    drop(tx);
    assert!(
        site.files().is_empty(),
        "truncated upload left {:?}",
        site.files()
    );
    site.finish().await;
}

#[tokio::test]
async fn a_rejected_admin_upload_leaves_no_file() {
    let site = site().await;
    let mut browser = Browser::new(&site.app);
    browser.login().await;
    let token = browser.token().await;
    let mut body = open_upload(&token).to_vec();
    body.extend_from_slice(b"\r\n--XB--\r\n");

    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/admin/documents")
        .header(header::HOST, "ged.test")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=XB")
        .header(header::COOKIE, browser.cookie.expect("session"))
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(
        "203.0.113.50:40000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let resp = site.app.clone().oneshot(req).await.unwrap();
    assert!(
        matches!(resp.status().as_u16(), 302 | 303),
        "rejected upload status {}",
        resp.status()
    );
    assert!(
        site.files().is_empty(),
        "rejected upload left {:?}",
        site.files()
    );
    site.finish().await;
}
