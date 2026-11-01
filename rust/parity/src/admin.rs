//! Admin web session: form login, CSRF token handling, form submissions.
//!
//! Login attempts are rate limited per Rails process (10 per 3 minutes), so
//! use [`AdminSession::login`] sparingly and prefer [`AdminSession::cached`].

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;

use regex::Regex;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::{Client, Method, Url, redirect};

use crate::http::{Resp, read};
use crate::stack::{admin_credentials, stack};

/// Rails' `Admins::SessionsController` limit: 10 creates per 3 minutes.
const LOGIN_BUDGET: usize = 10;
/// Slightly past Rails' 3-minute window so a wait is not still inside it.
const LOGIN_WINDOW_SECS: u64 = 190;

/// Session cookie name. Rails names it `_ged_session` (the application key).
/// `GED_SESSION_COOKIE` overrides it for a backend that signs a different
/// cookie; unset, the Rails name is unchanged.
pub fn session_cookie_name() -> String {
    std::env::var("GED_SESSION_COOKIE")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "_ged_session".to_owned())
}

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

/// `<cookie name>=<value>` from a `Set-Cookie` header, attributes stripped.
fn session_from_set_cookie(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{}=", session_cookie_name())))
        .map(|v| v.split(';').next().unwrap_or_default().to_owned())
        .filter(|v| !v.is_empty())
}

fn session_from_jar(jar: &Jar, base: &str) -> Option<String> {
    let url = Url::parse(&format!("{base}/admin")).ok()?;
    let header = jar.cookies(&url)?;
    let text = header.to_str().ok()?;
    text.split(';')
        .map(str::trim)
        .find(|part| part.starts_with(&format!("{}=", session_cookie_name())))
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

    /// A client with the browser `Accept` header and a cookie jar, not logged in.
    pub fn anonymous(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_owned();
        let (http, jar) = new_client(&base, None);
        Self {
            base,
            http,
            jar,
            tracks_cookie: false,
        }
    }

    /// POST the login form and return the response whether or not it succeeded.
    ///
    /// Counts against the process-wide 10/3 min budget. Does not wait for a
    /// free slot — [`AdminSession::login_as`] does, so a rate-limit test can
    /// observe a refusal.
    pub async fn attempt_login(base: &str, user: &str, pass: &str) -> Result<(Self, Resp), String> {
        let this = Self::anonymous(base);
        let form = this.get("/admins/sign_in").await;
        if form.status != 200 {
            return Err(format!("GET /admins/sign_in answered {}", form.status));
        }
        let token = csrf_token(&form.text()).ok_or("no csrf-token meta on the login page")?;
        record_login_attempt();
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
        Ok((this, resp))
    }

    /// Log in with explicit credentials; also returns the login response
    /// (a redirect on success, a re-rendered form on failure).
    pub async fn login_as(base: &str, user: &str, pass: &str) -> Result<(Self, Resp), String> {
        for attempt in 0..2 {
            wait_for_login_budget(1).await;
            let (this, resp) = Self::attempt_login(base, user, pass).await?;
            if login_succeeded(&resp) {
                return Ok((this, resp));
            }
            // Someone outside this process already burned the budget.
            if attempt == 0 && login_throttled(&resp) {
                tokio::time::sleep(std::time::Duration::from_secs(LOGIN_WINDOW_SECS)).await;
                continue;
            }
            return Err(format!(
                "login did not succeed: HTTP {} {:?}",
                resp.status,
                resp.header("location")
            ));
        }
        Err("login did not succeed after waiting out the rate limit".into())
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

    /// POST exactly `fields`, with no authenticity token added.
    pub async fn post_form_raw(&self, path: &str, fields: &[(&str, &str)]) -> Resp {
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .form(fields)
            .send()
            .await
            .expect("POST form");
        self.finish(resp).await
    }

    /// Follow one redirect, or return `resp` when it is not a redirect.
    pub async fn page_after(&self, resp: &Resp) -> Resp {
        if matches!(resp.status, 301 | 302 | 303 | 307 | 308)
            && let Some(loc) = resp.header("location")
        {
            return self.get(&location_path(&self.base, loc)).await;
        }
        resp.clone()
    }
}

/// `location` as a path (and query) relative to `base`.
pub fn location_path(base: &str, location: &str) -> String {
    let loc = location.trim();
    if let Some(rest) = loc.strip_prefix(base) {
        if rest.is_empty() {
            "/".to_owned()
        } else {
            rest.to_owned()
        }
    } else {
        loc.to_owned()
    }
}

pub fn login_succeeded(resp: &Resp) -> bool {
    matches!(resp.status, 302 | 303)
        && resp
            .header("location")
            .is_some_and(|l| !l.contains("sign_in"))
}

/// Rails answers a throttled `POST /admins/sign_in` with a redirect back to
/// the form. A wrong password re-renders the form instead (no redirect).
pub fn login_throttled(resp: &Resp) -> bool {
    matches!(resp.status, 302 | 303)
        && resp
            .header("location")
            .is_some_and(|l| l.contains("sign_in"))
}

/// Block until `needed` more login attempts fit in the current window,
/// according to the attempts this suite recorded for this stack.
pub async fn wait_for_login_budget(needed: usize) {
    loop {
        let attempts = recent_login_attempts();
        if attempts.len() + needed <= LOGIN_BUDGET {
            return;
        }
        let now = epoch_secs();
        let oldest = attempts.iter().copied().min().unwrap_or(now);
        let wait = LOGIN_WINDOW_SECS
            .saturating_sub(now.saturating_sub(oldest))
            .max(1);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    }
}

fn record_login_attempt() {
    let mut attempts = recent_login_attempts();
    attempts.push(epoch_secs());
    let body = attempts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(login_log_path(), body + "\n");
}

fn recent_login_attempts() -> Vec<u64> {
    let now = epoch_secs();
    let text = std::fs::read_to_string(login_log_path()).unwrap_or_default();
    text.lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .filter(|t| now.saturating_sub(*t) < LOGIN_WINDOW_SECS)
        .collect()
}

fn login_log_path() -> std::path::PathBuf {
    let mut hash: u64 = 0;
    for b in stack().base_url.bytes() {
        hash = hash.wrapping_mul(131).wrapping_add(u64::from(b));
    }
    std::env::temp_dir().join(format!("ged-parity-logins-{hash:x}"))
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Exclusive file lock shared by every admin test process against one stack.
/// Closing the file releases it.
pub struct SuiteLock {
    file: Option<std::fs::File>,
}

impl SuiteLock {
    pub async fn acquire() -> Self {
        let path = suite_lock_path();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap_or_else(|e| panic!("open suite lock {}: {e}", path.display()));
        let file = tokio::task::spawn_blocking(move || {
            file.lock_exclusive()
                .unwrap_or_else(|e| panic!("lock suite {}: {e}", path.display()));
            file
        })
        .await
        .expect("suite lock task");
        Self { file: Some(file) }
    }
}

impl Drop for SuiteLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}

fn suite_lock_path() -> std::path::PathBuf {
    let mut hash: u64 = 0;
    for b in stack().base_url.bytes() {
        hash = hash.wrapping_mul(131).wrapping_add(u64::from(b));
    }
    std::env::temp_dir().join(format!("ged-parity-admin-{hash:x}.lock"))
}
