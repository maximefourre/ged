//! Admin web session: form login, CSRF token handling, form submissions.
//!
//! Login attempts are rate limited per Rails process (10 per 3 minutes), so
//! use [`AdminSession::login`] sparingly and prefer [`AdminSession::cached`].

use std::sync::{Arc, Mutex};

use regex::Regex;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::{Client, Method, Url, redirect};

use crate::http::{Resp, read};
use crate::stack::admin_credentials;

pub struct AdminSession {
    base: String,
    http: Client,
    jar: Arc<Jar>,
    /// Copy session rotations into the process-wide cookie string. Each
    /// `#[tokio::test]` has its own runtime, so a `Client` cached in a static
    /// would keep a dead connection pool. Only the cookie string is shared;
    /// every call builds a new client.
    tracks_cookie: bool,
}

/// Extract `<meta name="csrf-token" content="…">`.
pub fn csrf_token(html: &str) -> Option<String> {
    let re = Regex::new(r#"<meta name="csrf-token" content="([^"]+)""#).expect("regex");
    re.captures(html).map(|c| html_unescape(&c[1]))
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn cookie_slot() -> &'static Mutex<Option<String>> {
    static COOKIE: Mutex<Option<String>> = Mutex::new(None);
    &COOKIE
}

fn stored_cookie() -> Option<String> {
    cookie_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn store_cookie(cookie: String) {
    *cookie_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(cookie);
}

/// `_ged_session=<value>` from a `Set-Cookie` header, attributes stripped.
fn session_from_set_cookie(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("_ged_session="))
        .map(|v| v.split(';').next().unwrap_or_default().to_owned())
        .filter(|v| !v.is_empty())
}

fn session_from_jar(jar: &Jar, base: &str) -> Option<String> {
    let url = Url::parse(&format!("{base}/admin")).ok()?;
    let header = jar.cookies(&url)?;
    let text = header.to_str().ok()?;
    text.split(';')
        .map(str::trim)
        .find(|part| part.starts_with("_ged_session="))
        .map(str::to_owned)
}

/// Browser-like client: cookie jar, no redirects followed, and an `Accept`
/// header like a browser's (Devise answers 204 instead of redirecting when the
/// client does not ask for HTML).
///
/// The session cookie is seeded into the jar, not sent as a default `Cookie`
/// header. reqwest skips the jar whenever that header is already set, which
/// would freeze the pre-rotation session and 422 the next CSRF check.
fn new_client(base: &str, cookie: Option<&str>) -> (Client, Arc<Jar>) {
    let jar = Arc::new(Jar::default());
    if let Some(cookie) = cookie {
        let url = Url::parse(&format!("{base}/")).expect("base url");
        let line = if cookie.to_ascii_lowercase().contains("path=") {
            cookie.to_owned()
        } else {
            format!("{cookie}; Path=/")
        };
        jar.add_cookie_str(&line, &url);
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
            .parse()
            .expect("accept"),
    );
    let http = Client::builder()
        .redirect(redirect::Policy::none())
        .cookie_provider(Arc::clone(&jar))
        .default_headers(headers)
        .build()
        .expect("build http client");
    (http, jar)
}

impl AdminSession {
    /// Log in through the HTML form with `GED_ADMIN_*` credentials.
    pub async fn login(base: &str) -> Result<Self, String> {
        let (user, pass) = admin_credentials();
        Self::login_as(base, &user, &pass).await.map(|(s, _)| s)
    }

    /// Log in with explicit credentials; also returns the login response
    /// (a redirect on success, a re-rendered form on failure).
    pub async fn login_as(base: &str, user: &str, pass: &str) -> Result<(Self, Resp), String> {
        let base = base.trim_end_matches('/').to_owned();
        let (http, jar) = new_client(&base, None);
        let this = Self {
            base,
            http,
            jar,
            tracks_cookie: false,
        };
        let form = this.get("/admins/sign_in").await;
        if form.status != 200 {
            return Err(format!("GET /admins/sign_in answered {}", form.status));
        }
        let token = csrf_token(&form.text()).ok_or("no csrf-token meta on the login page")?;
        let resp = this
            .post_form_with_token(
                "/admins/sign_in",
                &token,
                &[
                    ("admin[username]", user),
                    ("admin[password]", pass),
                    ("commit", "Log in"),
                ],
            )
            .await;
        if !matches!(resp.status, 302 | 303)
            || resp
                .header("location")
                .is_some_and(|l| l.contains("sign_in"))
        {
            return Err(format!(
                "login did not succeed: HTTP {} {:?}",
                resp.status,
                resp.header("location")
            ));
        }
        Ok((this, resp))
    }

    /// One login per process. Later callers rebuild a client from the session
    /// cookie, so each tokio runtime owns the pool it uses.
    pub async fn cached(base: &str) -> Result<Self, String> {
        let base = base.trim_end_matches('/').to_owned();
        if let Some(cookie) = stored_cookie() {
            let (http, jar) = new_client(&base, Some(&cookie));
            return Ok(Self {
                base,
                http,
                jar,
                tracks_cookie: true,
            });
        }
        let mut this = Self::login(&base).await?;
        let cookie = this.session_cookie().await?;
        store_cookie(cookie);
        this.tracks_cookie = true;
        Ok(this)
    }

    /// `name=value` of the session cookie, read from the jar after an
    /// authenticated page load (Rails rewrites `Set-Cookie` when it saves
    /// the session). Public so a browser suite can reuse this login (Rails
    /// allows 10 `POST /admins/sign_in` attempts per 3 minutes).
    pub async fn session_cookie(&self) -> Result<String, String> {
        let resp = self
            .http
            .get(format!("{}/admin", self.base))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let from_header = session_from_set_cookie(&resp);
        let _ = resp.bytes().await;
        from_header
            .or_else(|| session_from_jar(&self.jar, &self.base))
            .ok_or_else(|| "no session cookie after GET /admin".into())
    }

    fn remember_rotation(&self, resp: &reqwest::Response) {
        if !self.tracks_cookie {
            return;
        }
        if let Some(cookie) =
            session_from_set_cookie(resp).or_else(|| session_from_jar(&self.jar, &self.base))
        {
            store_cookie(cookie);
        }
    }

    async fn finish(&self, resp: reqwest::Response) -> Resp {
        self.remember_rotation(&resp);
        read(resp).await
    }

    pub async fn get(&self, path: &str) -> Resp {
        let resp = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("GET");
        self.finish(resp).await
    }

    /// Fresh CSRF token, read from an authenticated (or login) page.
    pub async fn csrf(&self) -> String {
        let page = self.get("/admin/documents").await;
        csrf_token(&page.text())
            .unwrap_or_else(|| panic!("no csrf-token on /admin/documents (HTTP {})", page.status))
    }

    /// Pause the uploads queue (admin button). Idempotent at the HTTP layer:
    /// callers should resume afterwards, including after a failed assertion.
    pub async fn pause_upload_queue(&self) -> Resp {
        self.submit(Method::POST, "/admin/upload_queue_pause", &[])
            .await
    }

    /// Resume the uploads queue.
    pub async fn resume_upload_queue(&self) -> Resp {
        self.submit(Method::DELETE, "/admin/upload_queue_pause", &[])
            .await
    }

    /// Submit a form with a valid CSRF token. `method` other than POST is sent
    /// as POST with `_method`, exactly as browsers do for Rails forms.
    pub async fn submit(&self, method: Method, path: &str, fields: &[(&str, &str)]) -> Resp {
        let token = self.csrf().await;
        let mut all: Vec<(&str, &str)> = fields.to_vec();
        if method != Method::POST {
            all.push(("_method", method.as_str()));
        }
        self.post_form_with_token(path, &token, &all).await
    }

    /// POST a form with an explicit token (use a wrong one to test CSRF).
    pub async fn post_form_with_token(
        &self,
        path: &str,
        token: &str,
        fields: &[(&str, &str)],
    ) -> Resp {
        let mut all = vec![("authenticity_token", token)];
        all.extend_from_slice(fields);
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .form(&all)
            .send()
            .await
            .expect("POST form");
        self.finish(resp).await
    }

    /// Multipart POST (document upload) with a valid CSRF token.
    pub async fn submit_multipart(&self, path: &str, form: reqwest::multipart::Form) -> Resp {
        let token = self.csrf().await;
        let form = form.text("authenticity_token", token);
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .multipart(form)
            .send()
            .await
            .expect("POST multipart");
        self.finish(resp).await
    }

    /// Sign out (`DELETE /admins/sign_out`).
    pub async fn logout(&self) -> Resp {
        self.submit(Method::DELETE, "/admins/sign_out", &[]).await
    }
}
