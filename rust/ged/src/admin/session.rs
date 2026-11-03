//! The admin session: a signed cookie (`_ged_session`) holding who is signed
//! in, the CSRF secret and the flash messages, plus the Devise login
//! throttle (`Admins::SessionsController`: 10 creates per 3 minutes).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const COOKIE_NAME: &str = "_ged_session";

/// What the cookie carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionData {
    /// The signed-in admin, with a fingerprint of the password hash so that a
    /// password change ends the session (Devise's `authenticatable_salt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<(i64, String)>,
    /// Base64 of the CSRF secret ([`super::csrf`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csrf: Option<String>,
    /// Messages for the next page, `(kind, text)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flash: Vec<(String, String)>,
    /// Where to go after signing in (the GET that was refused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_to: Option<String>,
}

/// Signs and verifies cookie values: `base64url(json) "." base64url(hmac)`.
#[derive(Clone)]
pub struct Codec {
    key: [u8; 32],
}

impl Codec {
    /// The key is derived from `SECRET_KEY_BASE`, so the same variable signs
    /// the same way on every replica of `web`.
    pub fn new(secret_key_base: &str) -> Self {
        use sha2::Digest as _;
        let mut hasher = Sha256::new();
        hasher.update(b"ged admin session v1\0");
        hasher.update(secret_key_base.as_bytes());
        Self {
            key: hasher.finalize().into(),
        }
    }

    fn mac(&self, payload: &str) -> Hmac<Sha256> {
        // HMAC accepts any key length: this cannot fail.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key).expect("hmac key");
        mac.update(payload.as_bytes());
        mac
    }

    pub fn encode(&self, data: &SessionData) -> String {
        let json = serde_json::to_vec(data).unwrap_or_default();
        let payload = URL_SAFE_NO_PAD.encode(json);
        let tag = URL_SAFE_NO_PAD.encode(self.mac(&payload).finalize().into_bytes());
        format!("{payload}.{tag}")
    }

    /// `None` for anything not signed with this key (tampered, truncated,
    /// signed elsewhere): the caller then starts a fresh session.
    pub fn decode(&self, value: &str) -> Option<SessionData> {
        let (payload, tag) = value.split_once('.')?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        self.mac(payload).verify_slice(&tag).ok()?;
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
    }
}

/// Fingerprint of a stored password hash (not the hash itself).
pub fn salt_of(encrypted_password: &str) -> String {
    use sha2::Digest as _;
    let digest = Sha256::digest(encrypted_password.as_bytes());
    URL_SAFE_NO_PAD.encode(&digest[..12])
}

/// A fixed-window counter per client, like Rails' `rate_limit` on a
/// `MemoryStore`: the window starts at the first attempt and the counter
/// resets when it ends. Per process, which is all brute-force protection needs.
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    hits: Mutex<HashMap<String, (u32, Instant)>>,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one attempt for `key`; `true` when it is over the limit.
    pub fn throttled(&self, key: &str) -> bool {
        self.throttled_at(key, Instant::now())
    }

    fn throttled_at(&self, key: &str, now: Instant) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        // Entries of finished windows are dead weight: drop them so a flood of
        // distinct keys cannot grow the map without bound.
        hits.retain(|_, (_, start)| now.duration_since(*start) < self.window);
        let entry = hits.entry(key.to_owned()).or_insert((0, now));
        entry.0 += 1;
        entry.0 > self.limit
    }
}

/// The login throttle of `Admins::SessionsController`.
pub fn login_limiter() -> RateLimiter {
    RateLimiter::new(10, Duration::from_secs(180))
}

// --- the Devise screens -------------------------------------------------

use argon2::Argon2;
use argon2::password_hash::PasswordVerifier;
use argon2::password_hash::phc::PasswordHash;
use askama::Template;
use axum::extract::{Extension, Request, State};
use axum::http::header::ACCEPT;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;

use super::csrf;
use super::view::{AdminError, AdminResult, Chrome, Ctx, CurrentAdmin};
use crate::http::{AppState, RequestParams};

pub const UNAUTHENTICATED: &str = "You need to sign in or sign up before continuing.";
pub const ALREADY_SIGNED_IN: &str = "You are already signed in.";
pub const INVALID: &str = "Invalid username or password.";
pub const THROTTLED: &str = "Too many login attempts. Try again in a few minutes.";

/// The admin a session belongs to, if its fingerprint still matches the
/// stored password hash (a changed password ends the session).
pub async fn current_admin(
    db: &PgPool,
    data: &SessionData,
) -> Result<Option<CurrentAdmin>, sqlx::Error> {
    let Some((id, salt)) = &data.admin else {
        return Ok(None);
    };
    let row: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, username, encrypted_password FROM admins WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(row
        .filter(|(_, _, hash)| salt_of(hash) == *salt)
        .map(|(id, username, _)| CurrentAdmin { id, username }))
}

/// An anonymous visitor asked for a screen that needs an admin: Devise's
/// failure app. A refused GET is remembered so that signing in comes back.
pub fn refuse(ctx: &Ctx, method: &Method, req: &Request) -> Response {
    ctx.flash("alert", UNAUTHENTICATED);
    if method == Method::GET {
        let target = req
            .uri()
            .path_and_query()
            .map_or_else(|| req.uri().path().to_owned(), |pq| pq.as_str().to_owned());
        ctx.with_session(|s| s.return_to = Some(target));
    }
    ctx.redirect("/admins/sign_in")
}

/// `GET /admin`: the dashboard sends you to the documents.
pub async fn root(Extension(ctx): Extension<Ctx>) -> Response {
    ctx.redirect("/admin/documents")
}

#[derive(Template)]
#[template(path = "admin/sessions_new.html")]
struct NewPage {
    chrome: Chrome,
}

/// `GET /admins/sign_in`.
pub async fn new(Extension(ctx): Extension<Ctx>) -> Response {
    if ctx.admin().is_some() {
        ctx.flash("alert", ALREADY_SIGNED_IN);
        return ctx.redirect("/admin");
    }
    ctx.page(
        StatusCode::OK,
        &NewPage {
            chrome: ctx.chrome("sessions"),
        },
    )
}

/// The throttling key: the client address as the nearest proxy saw it — the
/// right-most `X-Forwarded-For` entry that is not a private address. Without
/// one, all clients share a bucket (stricter, never looser).
fn client_key(headers: &HeaderMap) -> String {
    use std::net::IpAddr;
    let private = |ip: &IpAddr| match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    };
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|v| v.trim().parse::<IpAddr>().ok())
        .rfind(|ip| !private(ip))
        .map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
}

fn password_matches(stored: &str, candidate: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(candidate.as_bytes(), &parsed)
            .is_ok()
    })
}

/// A valid hash that matches nothing: verifying against it keeps an unknown
/// username as slow as a wrong password.
fn decoy() -> &'static str {
    use argon2::password_hash::PasswordHasher;
    static DECOY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DECOY.get_or_init(|| {
        Argon2::default()
            .hash_password(b"decoy")
            .map(|h| h.to_string())
            .unwrap_or_default()
    })
}

/// `POST /admins/sign_in`.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    headers: HeaderMap,
    RequestParams(params): RequestParams,
) -> AdminResult {
    if ctx.admin().is_some() {
        ctx.flash("alert", ALREADY_SIGNED_IN);
        return Ok(ctx.redirect("/admin"));
    }
    if state.admin.login_limiter.throttled(&client_key(&headers)) {
        ctx.flash("alert", THROTTLED);
        return Ok(ctx.redirect("/admins/sign_in"));
    }
    let creds = params.get("admin").and_then(|v| v.as_map());
    let field = |name: &str| creds.and_then(|m| m.get(name)).and_then(|v| v.as_str());
    // Devise: `strip_whitespace_keys` and `case_insensitive_keys` on username.
    let username = field("username").unwrap_or("").trim().to_lowercase();
    let password = field("password").unwrap_or("");

    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id, encrypted_password FROM admins WHERE lower(username) = $1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await
            .map_err(AdminError::from)?;
    let ok = match &row {
        Some((_, hash)) => password_matches(hash, password),
        None => {
            let _ = password_matches(decoy(), password);
            false
        }
    };
    let Some((id, hash)) = row.filter(|_| ok) else {
        ctx.flash_now("alert", INVALID);
        return Ok(ctx.page(
            StatusCode::UNPROCESSABLE_ENTITY,
            &NewPage {
                chrome: ctx.chrome("sessions"),
            },
        ));
    };

    // A new session: nothing from before the login (including its CSRF
    // secret) survives it.
    let return_to = ctx.with_session(|s| s.return_to.take());
    ctx.with_session(|s| {
        *s = SessionData {
            admin: Some((id, salt_of(&hash))),
            csrf: Some(csrf::encode_secret(&csrf::new_secret())),
            flash: vec![("notice".to_owned(), "Signed in successfully.".to_owned())],
            return_to: None,
        };
    });
    let target = return_to
        .filter(|t| t.starts_with('/') && !t.starts_with("//"))
        .unwrap_or_else(|| "/admin".to_owned());
    Ok(ctx.redirect_with(StatusCode::SEE_OTHER, &target))
}

/// `DELETE /admins/sign_out`: Devise answers a browser with a redirect and
/// anything else with `204 No Content`.
pub async fn destroy(Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Response {
    let signed_in = ctx.admin().is_some();
    ctx.with_session(|s| {
        *s = SessionData {
            csrf: Some(csrf::encode_secret(&csrf::new_secret())),
            flash: if signed_in {
                vec![("notice".to_owned(), "Signed out successfully.".to_owned())]
            } else {
                Vec::new()
            },
            ..SessionData::default()
        };
    });
    let wants_html = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    if wants_html {
        ctx.redirect_with(StatusCode::SEE_OTHER, "/")
    } else {
        StatusCode::NO_CONTENT.into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> SessionData {
        SessionData {
            admin: Some((7, salt_of("hash"))),
            csrf: Some("c".into()),
            flash: vec![("notice".into(), "Hi <b>".into())],
            return_to: Some("/admin/departments".into()),
        }
    }

    #[test]
    fn a_signed_session_round_trips() {
        let codec = Codec::new("secret");
        assert_eq!(codec.decode(&codec.encode(&data())), Some(data()));
    }

    #[test]
    fn a_cookie_signed_with_another_key_is_refused() {
        let cookie = Codec::new("one").encode(&data());
        assert_eq!(Codec::new("two").decode(&cookie), None);
    }

    #[test]
    fn a_tampered_payload_is_refused() {
        let codec = Codec::new("secret");
        let cookie = codec.encode(&data());
        let (_, tag) = cookie.split_once('.').unwrap();
        let forged = SessionData {
            admin: Some((1, "x".into())),
            ..SessionData::default()
        };
        let forged_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        assert_eq!(codec.decode(&format!("{forged_payload}.{tag}")), None);
    }

    #[test]
    fn garbage_cookies_are_refused() {
        let codec = Codec::new("secret");
        for bad in ["", ".", "a.b", "no-dot", "..", "e30.e30"] {
            assert_eq!(codec.decode(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_eleventh_attempt_in_a_window_is_throttled() {
        let limiter = login_limiter();
        let t0 = Instant::now();
        for i in 0..10 {
            assert!(!limiter.throttled_at("ip", t0), "attempt {i}");
        }
        assert!(limiter.throttled_at("ip", t0));
        assert!(limiter.throttled_at("ip", t0 + Duration::from_secs(179)));
    }

    #[test]
    fn the_counter_resets_when_the_window_ends() {
        let limiter = login_limiter();
        let t0 = Instant::now();
        for _ in 0..11 {
            limiter.throttled_at("ip", t0);
        }
        assert!(!limiter.throttled_at("ip", t0 + Duration::from_secs(181)));
    }

    #[test]
    fn clients_are_counted_apart() {
        let limiter = login_limiter();
        let t0 = Instant::now();
        for _ in 0..11 {
            limiter.throttled_at("a", t0);
        }
        assert!(!limiter.throttled_at("b", t0));
    }

    #[test]
    fn the_throttle_key_is_the_nearest_public_forwarded_address() {
        let mut h = HeaderMap::new();
        assert_eq!(client_key(&h), "unknown");
        h.insert("x-forwarded-for", "203.0.113.9, 10.0.0.2".parse().unwrap());
        assert_eq!(client_key(&h), "203.0.113.9");
        // A spoofed prefix does not move the key: the proxy appended the real one.
        h.insert("x-forwarded-for", "1.1.1.1, 198.51.100.4".parse().unwrap());
        assert_eq!(client_key(&h), "198.51.100.4");
        h.insert("x-forwarded-for", "garbage, 192.168.1.1".parse().unwrap());
        assert_eq!(client_key(&h), "unknown");
    }
}
