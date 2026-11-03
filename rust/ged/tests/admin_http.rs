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
}

impl Browser {
    fn new(app: &Router) -> Self {
        Self {
            app: app.clone(),
            cookie: None,
        }
    }

    async fn send(&mut self, method: Method, uri: &str, form: Option<&str>) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "ged.test")
            .header(header::ACCEPT, "text/html");
        if let Some(c) = &self.cookie {
            req = req.header(header::COOKIE, c);
        }
        let body = match form {
            Some(f) => {
                req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(f.to_owned())
            }
            None => Body::empty(),
        };
        let resp = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
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
    let rails = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/assets/stylesheets/application.css"
    );
    if let Ok(original) = std::fs::read_to_string(rails) {
        assert_eq!(css.body, original, "CSS must be Rails' file, unmodified");
    }
    let js_href = page
        .split('"')
        .find(|u| u.starts_with("/assets/application-") && u.ends_with(".js"))
        .expect("script url")
        .to_owned();
    let js = b.get(&js_href).await;
    assert_eq!(js.status, 200);
    assert_eq!(js.header("content-type"), "text/javascript");
    let rails_js = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/javascript/application.js"
    );
    if let Ok(original) = std::fs::read_to_string(rails_js) {
        assert_eq!(js.body, original, "JS must be Rails' file, unmodified");
    }
    assert_eq!(b.get("/assets/application-00000000.css").await.status, 404);
    site.finish().await;
}
