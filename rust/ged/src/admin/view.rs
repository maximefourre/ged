//! What every admin handler shares: the request context (session, flash,
//! CSRF), the page chrome, redirects and the Rails error pages.

use std::sync::{Arc, Mutex};

use askama::Template;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue, LOCATION};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::assets;
use super::csrf;
use super::session::SessionData;

/// The signed-in admin of this request.
#[derive(Debug, Clone)]
pub struct CurrentAdmin {
    pub id: i64,
    pub username: String,
}

struct Inner {
    session: Mutex<SessionData>,
    /// Last request's flash, until something reads or writes the flash: a
    /// request that only redirects leaves it for the next one.
    incoming: Mutex<Vec<(String, String)>>,
    /// `flash.now` messages of this page.
    now: Mutex<Vec<(String, String)>>,
    admin: Mutex<Option<CurrentAdmin>>,
    /// `scheme://host` of this request, for absolute redirects and the Origin check.
    base: String,
}

/// Per-request state, put in the request extensions by the admin guard.
#[derive(Clone)]
pub struct Ctx(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ctx {
    pub(super) fn new(mut data: SessionData, base: String) -> Self {
        let incoming = std::mem::take(&mut data.flash);
        Self(Arc::new(Inner {
            session: Mutex::new(data),
            incoming: Mutex::new(incoming),
            now: Mutex::new(Vec::new()),
            admin: Mutex::new(None),
            base,
        }))
    }

    /// The session to store: the messages set during this request, plus last
    /// request's if nothing consumed them.
    pub(super) fn session(&self) -> SessionData {
        let mut data = lock(&self.0.session).clone();
        data.flash.extend(lock(&self.0.incoming).iter().cloned());
        data
    }

    pub(super) fn with_session<R>(&self, f: impl FnOnce(&mut SessionData) -> R) -> R {
        f(&mut lock(&self.0.session))
    }

    pub(super) fn set_admin(&self, admin: Option<CurrentAdmin>) {
        *lock(&self.0.admin) = admin;
    }

    pub fn admin(&self) -> Option<CurrentAdmin> {
        lock(&self.0.admin).clone()
    }

    pub(super) fn base(&self) -> &str {
        &self.0.base
    }

    /// A message for the next page (`flash[:notice] = …` before a redirect).
    pub fn flash(&self, kind: &str, text: impl Into<String>) {
        // Touching the flash sweeps the old messages (Rails).
        lock(&self.0.incoming).clear();
        self.with_session(|s| s.flash.push((kind.to_owned(), text.into())));
    }

    /// A message for this page only (`flash.now[:alert] = …`).
    pub fn flash_now(&self, kind: &str, text: impl Into<String>) {
        lock(&self.0.now).push((kind.to_owned(), text.into()));
    }

    fn secret(&self) -> csrf::Secret {
        self.with_session(|s| {
            if let Some(secret) = s.csrf.as_deref().and_then(csrf::decode_secret) {
                return secret;
            }
            let secret = csrf::new_secret();
            s.csrf = Some(csrf::encode_secret(&secret));
            secret
        })
    }

    /// A fresh masked token for a form or the `csrf-token` meta tag.
    pub fn csrf_token(&self) -> String {
        csrf::mask(&self.secret())
    }

    /// Whether `submitted` is a token of this session. A session without a
    /// secret has issued no token, so nothing can match.
    pub fn verify_csrf(&self, submitted: Option<&str>) -> bool {
        let Some(submitted) = submitted else {
            return false;
        };
        let secret = self.with_session(|s| s.csrf.as_deref().and_then(csrf::decode_secret));
        secret.is_some_and(|secret| csrf::verify(&secret, submitted))
    }

    pub fn chrome(&self, active: &'static str) -> Chrome {
        let (css, js) = assets::urls();
        Chrome {
            flashes: {
                let mut shown = std::mem::take(&mut *lock(&self.0.incoming));
                shown.extend(lock(&self.0.now).iter().cloned());
                shown
            },
            csrf: self.csrf_token(),
            username: self.admin().map(|a| a.username),
            active,
            css,
            js,
        }
    }

    /// `redirect_to path` (302), with an absolute `Location` like Rails.
    pub fn redirect(&self, path: &str) -> Response {
        self.redirect_with(StatusCode::FOUND, path)
    }

    pub fn redirect_with(&self, status: StatusCode, path: &str) -> Response {
        let target = if path.starts_with('/') && !path.starts_with("//") {
            format!("{}{path}", self.0.base)
        } else {
            "/".to_owned()
        };
        let mut response = status.into_response();
        if let Ok(value) = HeaderValue::from_str(&target) {
            response.headers_mut().insert(LOCATION, value);
        }
        html_type(&mut response);
        response
    }

    /// Renders a page.
    pub fn page<T: Template>(&self, status: StatusCode, page: &T) -> Response {
        match page.render() {
            Ok(body) => {
                let mut response = (status, body).into_response();
                html_type(&mut response);
                response
            }
            Err(e) => {
                eprintln!("ged: admin: rendering a page: {e}");
                error_page(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

fn html_type(response: &mut Response) {
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
}

/// `max-age=0, private, must-revalidate` on a page, `no-cache` on anything
/// else (redirects, errors): what Rails sends.
pub(super) fn cache_headers(response: &mut Response) {
    if response.headers().contains_key(CACHE_CONTROL) {
        return;
    }
    let value = if response.status() == StatusCode::OK {
        "max-age=0, private, must-revalidate"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(value));
}

/// `scheme://host` from the request headers, as `request.base_url` sees it.
pub(super) fn base_url(headers: &HeaderMap) -> String {
    let first = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let scheme = match first("x-forwarded-proto") {
        Some("https") => "https",
        _ => "http",
    };
    let host = first("x-forwarded-host")
        .or_else(|| first("host"))
        .filter(|h| {
            h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
        })
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

/// Everything a page needs around its own content: the admin layout.
#[derive(Debug, Clone)]
pub struct Chrome {
    pub flashes: Vec<(String, String)>,
    pub csrf: String,
    pub username: Option<String>,
    /// The nav entry to highlight (Rails' `controller_name`).
    pub active: &'static str,
    pub css: String,
    pub js: String,
}

impl Chrome {
    pub fn initial(&self) -> String {
        self.username
            .as_deref()
            .and_then(|u| u.chars().next())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }

    pub fn nav(&self) -> [(&'static str, &'static str, &'static str, bool); 6] {
        let item = |label, path, key| (label, path, key, key == self.active);
        [
            item("Documents", "/admin/documents", "documents"),
            item("Departments", "/admin/departments", "departments"),
            item("Clients", "/admin/app_clients", "app_clients"),
            item(
                "Upload requests",
                "/admin/upload_requests",
                "upload_requests",
            ),
            item("Trash", "/admin/deleted_documents", "deleted_documents"),
            item("Status", "/admin/status", "status"),
        ]
    }
}

/// The static pages of `public/`, as Rails serves them in production.
pub fn error_page(status: StatusCode) -> Response {
    let body = match status {
        StatusCode::BAD_REQUEST => include_str!("../../templates/errors/400.html"),
        StatusCode::NOT_FOUND => include_str!("../../templates/errors/404.html"),
        StatusCode::UNPROCESSABLE_ENTITY => include_str!("../../templates/errors/422.html"),
        _ => include_str!("../../templates/errors/500.html"),
    };
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=UTF-8"),
    );
    response
}

/// How a handler fails.
#[derive(Debug)]
pub enum AdminError {
    /// `ActiveRecord::RecordNotFound`.
    NotFound,
    /// `ActionController::ParameterMissing`.
    BadRequest,
    /// An unhandled error: a 500 page, the cause only in the log.
    Internal(String),
}

impl From<sqlx::Error> for AdminError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            other => Self::Internal(other.to_string()),
        }
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => error_page(StatusCode::NOT_FOUND),
            Self::BadRequest => error_page(StatusCode::BAD_REQUEST),
            Self::Internal(cause) => {
                eprintln!("ged: admin: {cause}");
                error_page(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

pub type AdminResult = Result<Response, AdminError>;

/// Ruby `String#to_i` for a path id: `None` when it cannot address a row
/// (no digits would be 0, which never exists; too big for a bigint is a
/// `RecordNotFound` in Rails as well).
pub fn rails_id(raw: &str) -> Option<i64> {
    let s = raw.trim_start();
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    let n: i64 = digits.parse().ok()?;
    Some(if negative { -n } else { n })
}

/// ActiveSupport's `number_to_human_size`: three significant digits, trailing
/// zeros dropped, 1024-based.
pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["Bytes", "KB", "MB", "GB", "TB"];
    if bytes == 1 {
        return "1 Byte".to_owned();
    }
    if bytes < 1024 {
        return format!("{bytes} Bytes");
    }
    let mut exp = 0usize;
    let mut value = bytes as f64;
    while value >= 1024.0 && exp < UNITS.len() - 1 {
        value /= 1024.0;
        exp += 1;
    }
    let magnitude = value.log10().floor() as i32;
    // Three significant digits: 2 decimals under 10, 1 under 100, then none
    // and (from 1000 up) the tens.
    let shift = 2 - magnitude;
    let factor = 10f64.powi(shift);
    let rounded = (value * factor + 0.5).floor() / factor;
    let mut text = format!("{rounded:.*}", usize::try_from(shift.max(0)).unwrap_or(0));
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    format!("{text} {}", UNITS[exp])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_follows_active_support() {
        for (bytes, want) in [
            (0, "0 Bytes"),
            (1, "1 Byte"),
            (3, "3 Bytes"),
            (123, "123 Bytes"),
            (1024, "1 KB"),
            (1234, "1.21 KB"),
            (12345, "12.1 KB"),
            (1234567, "1.18 MB"),
            (483989, "473 KB"),
            (1024 * 1024, "1 MB"),
            (1_048_575, "1020 KB"),
            (1_234_567_890, "1.15 GB"),
            (10 * 1024, "10 KB"),
        ] {
            assert_eq!(human_size(bytes), want, "{bytes}");
        }
    }

    #[test]
    fn path_ids_parse_like_to_i() {
        assert_eq!(rails_id("42"), Some(42));
        assert_eq!(rails_id("5abc"), Some(5));
        assert_eq!(rails_id("abc"), None);
        assert_eq!(rails_id(""), None);
        assert_eq!(rails_id("99999999999999999999"), None);
        assert_eq!(rails_id(" 7"), Some(7));
    }

    #[test]
    fn the_base_url_honours_the_proxy_headers() {
        let mut h = HeaderMap::new();
        h.insert("host", HeaderValue::from_static("ged.test:3122"));
        assert_eq!(base_url(&h), "http://ged.test:3122");
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert_eq!(base_url(&h), "https://ged.test:3122");
        h.insert("host", HeaderValue::from_static("bad host/x"));
        assert_eq!(base_url(&h), "https://localhost");
    }
}
