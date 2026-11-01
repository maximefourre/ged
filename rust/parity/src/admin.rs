//! Admin web session: form login, CSRF token handling, form submissions.
//!
//! Login attempts are rate limited per Rails process (10 per 3 minutes), so
//! use [`AdminSession::login`] sparingly and prefer [`AdminSession::cached`].

use regex::Regex;
use reqwest::{Client, Method, redirect};

use crate::http::{Resp, read};
use crate::stack::admin_credentials;

pub struct AdminSession {
    base: String,
    http: Client,
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

/// Browser-like client: cookie jar, no redirects followed, and an `Accept`
/// header like a browser's (Devise answers 204 instead of redirecting when the
/// client does not ask for HTML).
fn new_client(cookie: Option<&str>) -> Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(
        reqwest::header::ACCEPT,
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
            .parse()
            .expect("accept"),
    );
    if let Some(c) = cookie {
        h.insert(reqwest::header::COOKIE, c.parse().expect("cookie header"));
    }
    Client::builder()
        .redirect(redirect::Policy::none())
        .cookie_store(true)
        .default_headers(h)
        .build()
        .expect("build http client")
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
        let this = Self {
            base: base.trim_end_matches('/').to_owned(),
            http: new_client(None),
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

    /// One login per process, then every caller reuses the session cookie.
    pub async fn cached(base: &str) -> Result<Self, String> {
        static COOKIE: tokio::sync::Mutex<Option<String>> = tokio::sync::Mutex::const_new(None);
        let mut slot = COOKIE.lock().await;
        if let Some(c) = slot.as_ref() {
            return Ok(Self {
                base: base.trim_end_matches('/').to_owned(),
                http: new_client(Some(c)),
            });
        }
        let this = Self::login(base).await?;
        *slot = Some(this.session_cookie().await?);
        Ok(this)
    }

    /// `name=value` of the session cookie, so a browser suite can reuse this
    /// login. Rails allows 10 `POST /admins/sign_in` attempts per 3 minutes.
    pub async fn session_cookie(&self) -> Result<String, String> {
        // reqwest's jar is opaque; recover the cookie from the Set-Cookie of a
        // fresh authenticated page instead of re-implementing a jar.
        let resp = self
            .http
            .get(format!("{}/admin", self.base))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let from_set_cookie = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("_ged_session="))
            .map(|v| v.split(';').next().unwrap_or_default().to_owned());
        from_set_cookie.ok_or_else(|| "no session Set-Cookie on /admin".into())
    }

    pub async fn get(&self, path: &str) -> Resp {
        read(
            self.http
                .get(format!("{}{path}", self.base))
                .send()
                .await
                .expect("GET"),
        )
        .await
    }

    /// Fresh CSRF token, read from an authenticated (or login) page.
    pub async fn csrf(&self) -> String {
        let page = self.get("/admin/documents").await;
        csrf_token(&page.text())
            .unwrap_or_else(|| panic!("no csrf-token on /admin/documents (HTTP {})", page.status))
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
        read(resp).await
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
        read(resp).await
    }

    /// Sign out (`DELETE /admins/sign_out`).
    pub async fn logout(&self) -> Resp {
        self.submit(Method::DELETE, "/admins/sign_out", &[]).await
    }
}
