//! The admin UI through the real router and a scratch database: login,
//! session, CSRF, headers, assets. Complements the black-box parity suite
//! (`rust/parity/tests/admin_*`) with checks that need no second backend.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, header};
use ged::config::Config;
use ged::{AppState, db, seed};
use http_body_util::BodyExt;
use support::ScratchDb;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

struct Site {
    db: ScratchDb,
    pool: sqlx::PgPool,
    app: Router,
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
    let app = ged::router(AppState::new(config, pool.clone()));
    Site { db, pool, app }
}

impl Site {
    async fn finish(self) {
        self.pool.close().await;
        self.db.drop_db().await;
    }
}

struct Reply {
    status: u16,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

/// A cookie-keeping browser stand-in.
struct Browser {
    app: Router,
    cookie: Option<String>,
    /// The TCP peer the server sees (public by default, like a client that
    /// reaches the service directly).
    peer: std::net::SocketAddr,
    /// Extra request headers (e.g. a forged `X-Forwarded-For`).
    extra: Vec<(String, String)>,
}

impl Browser {
    fn new(app: &Router) -> Self {
        Self {
            app: app.clone(),
            cookie: None,
            peer: "203.0.113.50:40000".parse().unwrap(),
            extra: Vec::new(),
        }
    }

    fn at(app: &Router, peer: &str) -> Self {
        let mut b = Self::new(app);
        b.peer = format!("{peer}:40000").parse().unwrap();
        b
    }

    async fn send(&mut self, method: Method, uri: &str, form: Option<&str>) -> Reply {
        let content_type = form.map(|_| "application/x-www-form-urlencoded");
        self.send_raw(
            method,
            uri,
            content_type,
            form.unwrap_or("").to_owned(),
            &[],
        )
        .await
    }

    /// A multipart POST/PATCH/DELETE carrying `parts` as `(name, value)` text fields.
    async fn send_multipart(
        &mut self,
        method: Method,
        uri: &str,
        parts: &[(&str, &str)],
        extra: &[(&str, &str)],
    ) -> Reply {
        let mut body = String::new();
        for (name, value) in parts {
            body.push_str(&format!(
                "--XB\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            ));
        }
        body.push_str("--XB--\r\n");
        self.send_raw(
            method,
            uri,
            Some("multipart/form-data; boundary=XB"),
            body,
            extra,
        )
        .await
    }

    async fn send_raw(
        &mut self,
        method: Method,
        uri: &str,
        content_type: Option<&str>,
        body: String,
        extra: &[(&str, &str)],
    ) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "ged.test")
            .header(header::ACCEPT, "text/html");
        if let Some(c) = &self.cookie {
            req = req.header(header::COOKIE, c);
        }
        if let Some(ct) = content_type {
            req = req.header(header::CONTENT_TYPE, ct);
        }
        for (k, v) in self
            .extra
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(extra.iter().copied())
        {
            req = req.header(k, v);
        }
        let mut req = req.body(Body::from(body)).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(self.peer));
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        for value in headers.get_all(header::SET_COOKIE) {
            if let Some(pair) = value.to_str().unwrap().split(';').next()
                && pair.starts_with("_ged_session=")
            {
                self.cookie = Some(pair.to_owned());
            }
        }
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        Reply {
            status,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    async fn get(&mut self, uri: &str) -> Reply {
        self.send(Method::GET, uri, None).await
    }

    async fn token(&mut self, page: &str) -> String {
        let html = self.get(page).await.body;
        let marker = "name=\"csrf-token\" content=\"";
        let start = html.find(marker).expect("csrf meta") + marker.len();
        html[start..start + html[start..].find('"').unwrap()].to_owned()
    }

    async fn login(&mut self, password: &str) -> Reply {
        let token = self.token("/admins/sign_in").await;
        let form = format!(
            "authenticity_token={token}&admin%5Busername%5D=admin&admin%5Bpassword%5D={}&commit=Log+in",
            password.replace(' ', "+")
        );
        self.send(Method::POST, "/admins/sign_in", Some(&form))
            .await
    }
}

#[tokio::test]
async fn anonymous_admin_requests_go_to_the_login_form() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    let r = b.get("/admin/departments").await;
    assert_eq!(r.status, 302);
    assert_eq!(r.header("location"), "http://ged.test/admins/sign_in");
    let form = b.get("/admins/sign_in").await;
    assert!(
        form.body
            .contains("You need to sign in or sign up before continuing.")
    );
    site.finish().await;
}

#[tokio::test]
async fn the_login_form_carries_rails_headers_cookie_and_no_csp() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    let r = b.get("/admins/sign_in").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-frame-options"), "SAMEORIGIN");
    assert_eq!(r.header("x-content-type-options"), "nosniff");
    assert_eq!(r.header("x-xss-protection"), "0");
    assert_eq!(r.header("x-permitted-cross-domain-policies"), "none");
    assert_eq!(
        r.header("referrer-policy"),
        "strict-origin-when-cross-origin"
    );
    // Rails ships its CSP initializer commented out: no policy, no meta tag.
    assert_eq!(r.header("content-security-policy"), "");
    assert!(!r.body.contains("csp-nonce"));
    let cookie = r.header("set-cookie");
    assert!(cookie.starts_with("_ged_session="), "{cookie}");
    // Exactly what Rails sends.
    assert!(
        cookie.ends_with("; path=/; httponly; samesite=lax"),
        "{cookie}"
    );
    for field in [
        "admin[username]",
        "admin[password]",
        "authenticity_token",
        "Log in",
    ] {
        assert!(r.body.contains(field), "{field}");
    }
    site.finish().await;
}

#[tokio::test]
async fn a_wrong_password_rerenders_the_form_with_422() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    let r = b.login("nope").await;
    assert_eq!(r.status, 422);
    assert!(
        r.body.contains("Invalid username or password."),
        "{}",
        r.body
    );
    site.finish().await;
}

#[tokio::test]
async fn a_right_password_signs_in_and_flashes_once() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    let r = b.login(PASSWORD).await;
    assert_eq!(r.status, 303);
    assert_eq!(r.header("location"), "http://ged.test/admin");
    let r = b.get("/admin").await;
    assert_eq!(
        (r.status, r.header("location")),
        (302, "http://ged.test/admin/documents")
    );
    let page = b.get("/admin/documents").await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Signed in successfully."));
    assert!(page.body.contains("<b>GED</b>"));
    let again = b.get("/admin/documents").await;
    assert!(
        !again.body.contains("Signed in successfully."),
        "flash must show once"
    );
    site.finish().await;
}

#[tokio::test]
async fn signing_in_returns_to_the_page_that_was_refused() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.get("/admin/departments").await;
    let r = b.login(PASSWORD).await;
    assert_eq!(r.header("location"), "http://ged.test/admin/departments");
    site.finish().await;
}

#[tokio::test]
async fn a_tampered_session_cookie_is_an_anonymous_session() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let cookie = b.cookie.clone().unwrap();
    b.cookie = Some(format!("{cookie}x"));
    let r = b.get("/admin/departments").await;
    assert_eq!(r.status, 302);
    assert!(r.header("location").contains("sign_in"));
    site.finish().await;
}

#[tokio::test]
async fn a_changed_password_ends_existing_sessions() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    sqlx::query("UPDATE admins SET encrypted_password = 'rotated'")
        .execute(&site.pool)
        .await
        .unwrap();
    let r = b.get("/admin/departments").await;
    assert_eq!(r.status, 302);
    assert!(r.header("location").contains("sign_in"));
    site.finish().await;
}

#[tokio::test]
async fn a_post_without_a_matching_token_is_rejected_with_422() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let real = b.token("/admin/departments").await;
    let cases: [(&str, String); 3] = [
        ("missing", "department%5Bname%5D=x".into()),
        (
            "forged",
            "authenticity_token=forged&department%5Bname%5D=x".into(),
        ),
        (
            "padded",
            format!("authenticity_token={}A&department%5Bname%5D=x", real),
        ),
    ];
    for (label, form) in cases {
        let r = b
            .send(Method::POST, "/admin/departments", Some(&form))
            .await;
        assert_eq!(r.status, 422, "{label}");
        assert!(
            r.body.contains("The change you wanted was rejected"),
            "{label}"
        );
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM departments")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    site.finish().await;
}

#[tokio::test]
async fn an_origin_header_from_another_site_is_rejected() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let token = b.token("/admin/departments").await;
    let req = Request::builder()
        .method("POST")
        .uri("/admin/departments")
        .header(header::HOST, "ged.test")
        .header(header::ORIGIN, "http://evil.test")
        .header(header::COOKIE, b.cookie.clone().unwrap())
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!(
            "authenticity_token={token}&department%5Bname%5D=x"
        )))
        .unwrap();
    let r = site.app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status().as_u16(), 422);
    site.finish().await;
}

#[tokio::test]
async fn the_eleventh_login_attempt_is_throttled() {
    let site = site().await;
    for i in 0..10 {
        let mut b = Browser::new(&site.app);
        assert_eq!(b.login("wrong").await.status, 422, "attempt {i}");
    }
    let mut b = Browser::new(&site.app);
    let r = b.login(PASSWORD).await;
    assert_eq!(r.status, 302, "even the right password is throttled");
    assert!(r.header("location").contains("sign_in"));
    let page = b.get("/admins/sign_in").await;
    assert!(
        page.body
            .contains("Too many login attempts. Try again in a few minutes.")
    );
    site.finish().await;
}

#[tokio::test]
async fn logout_ends_the_session() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let token = b.token("/admin/departments").await;
    let r = b
        .send(
            Method::POST,
            "/admins/sign_out",
            Some(&format!("authenticity_token={token}&_method=delete")),
        )
        .await;
    assert_eq!(r.status, 303);
    assert_eq!(r.header("location"), "http://ged.test/");
    let r = b.get("/admin/departments").await;
    assert_eq!(r.status, 302);
    assert!(r.header("location").contains("sign_in"));
    site.finish().await;
}

#[tokio::test]
async fn assets_are_served_unchanged_with_a_long_cache() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    let page = b.get("/admins/sign_in").await.body;
    let href = page
        .split("href=\"")
        .find_map(|s| {
            s.split('"')
                .next()
                .filter(|u| u.starts_with("/assets/application-") && u.ends_with(".css"))
        })
        .expect("stylesheet link")
        .to_owned();
    let css = b.get(&href).await;
    assert_eq!(css.status, 200);
    assert_eq!(css.header("content-type"), "text/css");
    assert_eq!(css.header("cache-control"), "public, max-age=31556952");
    for name in [
        "x-frame-options",
        "x-content-type-options",
        "x-xss-protection",
        "x-permitted-cross-domain-policies",
        "referrer-policy",
        "etag",
    ] {
        assert_eq!(css.header(name), "", "{name} on a static file");
    }
    assert_static_unchanged(
        "CSS",
        &css.body,
        "app/assets/stylesheets/application.css",
        "d583eaa9ac8d9ee60f17137d5eef505b47c54810ecbcf4961ced092cb4772422",
    );
    let js_href = page
        .split('"')
        .find(|u| u.starts_with("/assets/application-") && u.ends_with(".js"))
        .expect("script url")
        .to_owned();
    let js = b.get(&js_href).await;
    assert_eq!(js.status, 200);
    assert_eq!(js.header("content-type"), "text/javascript");
    assert_static_unchanged(
        "JS",
        &js.body,
        "app/javascript/application.js",
        "829d9407e9a2932d88d38975e9fa484203ae5df3ab51109b585e88f99b7eb5fc",
    );
    let missing = b.get("/assets/application-00000000.css").await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.header("x-frame-options"), "");
    let icon = b.get("/icon.png").await;
    assert_eq!(icon.status, 200);
    assert_eq!(icon.header("cache-control"), "public, max-age=31556952");
    assert_eq!(icon.header("x-frame-options"), "");
    let robots = b.get("/robots.txt").await;
    assert_eq!(robots.header("cache-control"), "public, max-age=31556952");
    assert_eq!(robots.header("x-frame-options"), "");
    site.finish().await;
}

/// SHA-256 of the bytes the server actually returned. The Rails tree is gone
/// from this checkout, so a missing `app/` file must not skip the check.
/// `rails-final` is a local tag and is absent on a clone that never fetched
/// it; the frozen digest still fails when the served asset changes.
fn assert_static_unchanged(label: &str, served: &str, rails_path: &str, sha256_hex: &str) {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(served.as_bytes()));
    assert_eq!(
        digest, sha256_hex,
        "{label} bytes changed; the frozen fingerprint must be revised on purpose"
    );
    if rails_final_commit_available() {
        let original = rails_final_blob(rails_path);
        assert_eq!(
            served.as_bytes(),
            original.as_slice(),
            "{label} must match rails-final:{rails_path}"
        );
    }
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rails_final_commit_available() -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "rails-final^{commit}"])
        .current_dir(repo_root())
        .output()
        .is_ok_and(|output| output.status.success())
}

fn rails_final_blob(path: &str) -> Vec<u8> {
    let spec = format!("rails-final:{path}");
    let output = std::process::Command::new("git")
        .args(["show", &spec])
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|err| panic!("git show {spec}: {err}"));
    assert!(
        output.status.success(),
        "rails-final is present but {spec} could not be read: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

// --- review T10: B1 (multipart CSRF/Origin) ------------------------------

#[tokio::test]
async fn multipart_without_a_token_is_rejected_on_every_mutating_admin_route() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let cases = [
        (Method::POST, "/admin/departments/1/trash"),
        (Method::PATCH, "/admin/departments/1/restore"),
        (Method::DELETE, "/admin/departments/1"),
        (Method::DELETE, "/admin/app_clients/1"),
        (Method::POST, "/admin/upload_queue_pause"),
        (Method::DELETE, "/admin/upload_queue_pause"),
        (Method::POST, "/admin/departments"),
        (Method::POST, "/admin/documents"),
        (Method::DELETE, "/admins/sign_out"),
    ];
    for (method, uri) in cases {
        let r = b
            .send_multipart(method.clone(), uri, &[("x", "1")], &[])
            .await;
        assert_eq!(r.status, 422, "{method} {uri}");
    }
    // The sign-out above must not have ended the session.
    assert_eq!(b.get("/admin/departments").await.status, 200);
    site.finish().await;
}

#[tokio::test]
async fn multipart_with_a_foreign_origin_is_rejected_even_with_a_valid_token() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let token = b.token("/admin/documents").await;
    for (method, uri) in [
        (Method::POST, "/admin/documents"),
        (Method::POST, "/admin/upload_queue_pause"),
    ] {
        let r = b
            .send_multipart(
                method.clone(),
                uri,
                &[("authenticity_token", &token)],
                &[("origin", "http://evil.test")],
            )
            .await;
        assert_eq!(r.status, 422, "{method} {uri}");
    }
    site.finish().await;
}

#[tokio::test]
async fn multipart_with_a_valid_token_field_passes_the_guard() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let token = b.token("/admin/documents").await;
    let r = b
        .send_multipart(
            Method::POST,
            "/admin/upload_queue_pause",
            &[("authenticity_token", &token)],
            &[("origin", "http://ged.test")],
        )
        .await;
    assert_eq!(r.status, 302, "{}", r.body);
    site.finish().await;
}

// --- review T10: B2 (login throttle key, blocking hash) ------------------

#[tokio::test]
async fn forged_forwarded_for_from_a_public_peer_does_not_dodge_the_throttle() {
    let site = site().await;
    for i in 0..10 {
        let mut b = Browser::new(&site.app);
        b.extra
            .push(("x-forwarded-for".into(), format!("198.51.100.{}", i + 1)));
        assert_eq!(b.login("wrong").await.status, 422, "attempt {i}");
    }
    let mut b = Browser::new(&site.app);
    b.extra
        .push(("x-forwarded-for".into(), "198.51.100.200".into()));
    let r = b.login(PASSWORD).await;
    assert_eq!(
        r.status, 302,
        "the eleventh attempt from one peer is throttled"
    );
    site.finish().await;
}

#[tokio::test]
async fn one_peer_exhausting_its_attempts_does_not_lock_out_another() {
    let site = site().await;
    for _ in 0..11 {
        let mut b = Browser::at(&site.app, "198.51.100.7");
        // Forging a private address must not reach a shared bucket either.
        b.extra.push(("x-forwarded-for".into(), "10.0.0.1".into()));
        b.login("wrong").await;
    }
    let mut other = Browser::at(&site.app, "198.51.100.8");
    assert_eq!(other.login(PASSWORD).await.status, 303);
    site.finish().await;
}

#[tokio::test]
async fn behind_a_trusted_proxy_clients_are_counted_by_forwarded_address() {
    let site = site().await;
    for _ in 0..11 {
        let mut b = Browser::at(&site.app, "10.0.0.2");
        b.extra
            .push(("x-forwarded-for".into(), "192.0.2.10".into()));
        b.login("wrong").await;
    }
    let mut other = Browser::at(&site.app, "10.0.0.2");
    other
        .extra
        .push(("x-forwarded-for".into(), "192.0.2.11".into()));
    assert_eq!(other.login(PASSWORD).await.status, 303);
    let mut same = Browser::at(&site.app, "10.0.0.2");
    same.extra
        .push(("x-forwarded-for".into(), "192.0.2.10".into()));
    assert_eq!(same.login(PASSWORD).await.status, 302);
    site.finish().await;
}

#[tokio::test]
async fn password_hashing_does_not_block_the_async_runtime() {
    let site = site().await;
    let mut warm = Browser::new(&site.app);
    let t = std::time::Instant::now();
    warm.login("wrong").await;
    let baseline = t.elapsed();

    let mut b = Browser::at(&site.app, "198.51.100.9");
    let max_gap = std::sync::Arc::new(std::sync::Mutex::new(std::time::Duration::ZERO));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ticker = {
        let (gap, stop) = (max_gap.clone(), stop.clone());
        tokio::spawn(async move {
            let mut last = std::time::Instant::now();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                let now = std::time::Instant::now();
                let mut g = gap.lock().unwrap();
                *g = (*g).max(now - last);
                last = now;
            }
        })
    };
    b.login("wrong").await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    ticker.await.unwrap();
    let gap = *max_gap.lock().unwrap();
    assert!(
        gap < baseline / 2,
        "runtime stalled {gap:?} during a login of {baseline:?}"
    );
    site.finish().await;
}

// --- review T10: M1 (Devise bcrypt hashes are not carried over) ----------

#[tokio::test]
async fn a_devise_bcrypt_hash_is_refused_until_the_seed_rewrites_it() {
    let site = site().await;
    sqlx::query("UPDATE admins SET encrypted_password = $1")
        .bind("$2a$12$abcdefghijklmnopqrstuuABCDEFGHIJKLMNOPQRSTUVWXYZ01234")
        .execute(&site.pool)
        .await
        .unwrap();
    let mut b = Browser::new(&site.app);
    assert_eq!(b.login(PASSWORD).await.status, 422);

    let env = |k: &str| match k {
        "RAILS_ENV" => Some("test".to_owned()),
        "SECRET_KEY_BASE" => Some("k".repeat(64)),
        "GED_ADMIN_USERNAME" => Some("admin".to_owned()),
        "GED_ADMIN_PASSWORD" => Some(PASSWORD.to_owned()),
        _ => None,
    };
    seed::run(&site.pool, &Config::from_lookup(&env).unwrap())
        .await
        .unwrap();
    let mut b = Browser::new(&site.app);
    assert_eq!(b.login(PASSWORD).await.status, 303);
    site.finish().await;
}

// --- review T10: M3 (provisional upload stub) ----------------------------

#[tokio::test]
async fn the_stub_upload_takes_files_above_two_mebibytes_and_refuses_oversize_with_413() {
    let site = site().await;
    let mut b = Browser::new(&site.app);
    b.login(PASSWORD).await;
    let token = b.token("/admin/documents").await;
    let dept: i64 = sqlx::query_scalar(
        "INSERT INTO departments (name, created_at, updated_at) VALUES ('d', now(), now()) RETURNING id",
    )
    .fetch_one(&site.pool)
    .await
    .unwrap();
    let upload = |size: usize| {
        let file = "a".repeat(size);
        format!(
            "--XB\r\nContent-Disposition: form-data; name=\"authenticity_token\"\r\n\r\n{token}\r\n\
             --XB\r\nContent-Disposition: form-data; name=\"document[department_id]\"\r\n\r\n{dept}\r\n\
             --XB\r\nContent-Disposition: form-data; name=\"document[file]\"; filename=\"big.txt\"\r\nContent-Type: text/plain\r\n\r\n{file}\r\n--XB--\r\n"
        )
    };
    let r = b
        .send_raw(
            Method::POST,
            "/admin/documents",
            Some("multipart/form-data; boundary=XB"),
            upload(3 * 1024 * 1024),
            &[],
        )
        .await;
    assert_eq!(r.status, 302, "{}", r.body);
    let r = b
        .send_raw(
            Method::POST,
            "/admin/documents",
            Some("multipart/form-data; boundary=XB"),
            upload(52_428_800 + 1),
            &[],
        )
        .await;
    assert_eq!(r.status, 413);
    site.finish().await;
}
