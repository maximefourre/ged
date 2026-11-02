//! Process-local JWKS cache (port of `Auth::JwksCache`).
//!
//! Refreshes every [`TTL`]; keeps serving the last good key set when a refresh
//! fails; fails closed ([`FetchError`]) only if no key set was ever fetched.
//!
//! Refreshes are single-flight: while one task fetches, concurrent requests
//! keep being served the cached (possibly stale) keys instead of queueing.
//! Fetches use short timeouts, back off after a failure, and a forced refresh
//! (unknown `kid`, i.e. IdP key rotation) is rate-limited so garbage tokens
//! cannot hammer the IdP.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;

pub const TTL: Duration = Duration::from_secs(300);
/// No retry after a failed fetch for this long.
pub const FAILURE_BACKOFF: Duration = Duration::from_secs(30);
/// Minimum gap between unknown-`kid` refreshes.
pub const FORCED_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const OPEN_TIMEOUT: Duration = Duration::from_secs(2);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError(pub String);

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FetchError {}

/// Read-only snapshot for the admin status page. `failing` is set by a failed
/// refresh and cleared by the next success, so fetched + failing means
/// "serving stale keys".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health {
    pub fetched: bool,
    pub age_seconds: Option<f64>,
    pub failing: bool,
}

#[derive(Default)]
struct State {
    key_set: Option<Arc<JwkSet>>,
    fetched_at: Option<Instant>,
    failed_at: Option<Instant>,
    forced_at: Option<Instant>,
}

pub struct JwksCache {
    url: String,
    http: reqwest::Client,
    state: Mutex<State>,
    /// Held across a fetch: the single-flight gate.
    refresh_gate: tokio::sync::Mutex<()>,
}

impl JwksCache {
    pub fn new(url: String) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(OPEN_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            // Like Net::HTTP: a redirect is a failed fetch, never followed.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            url,
            http,
            state: Mutex::new(State::default()),
            refresh_gate: tokio::sync::Mutex::new(()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The cached key set, refreshing first when due. `force` is the
    /// unknown-`kid` path.
    pub async fn key_set(&self, force: bool) -> Result<Arc<JwkSet>, FetchError> {
        if self.wants_refresh(force) {
            self.refresh_single_flight(force).await;
        }
        self.state()
            .key_set
            .clone()
            .ok_or_else(|| FetchError("no JWKS available".into()))
    }

    /// Never fetches.
    pub fn health(&self) -> Health {
        let state = self.state();
        Health {
            fetched: state.key_set.is_some(),
            age_seconds: state.fetched_at.map(|t| t.elapsed().as_secs_f64()),
            failing: state.failed_at.is_some(),
        }
    }

    /// Makes the cached keys look older than [`TTL`].
    #[doc(hidden)]
    pub fn force_expire_for_tests(&self) {
        let mut state = self.state();
        state.fetched_at = Instant::now().checked_sub(TTL + Duration::from_secs(1));
    }

    /// Makes the last failure look older than [`FAILURE_BACKOFF`].
    #[doc(hidden)]
    pub fn expire_backoff_for_tests(&self) {
        let mut state = self.state();
        if state.failed_at.is_some() {
            state.failed_at = Instant::now().checked_sub(FAILURE_BACKOFF + Duration::from_secs(1));
        }
    }

    fn wants_refresh(&self, force: bool) -> bool {
        let state = self.state();
        if state
            .failed_at
            .is_some_and(|t| t.elapsed() < FAILURE_BACKOFF)
        {
            return false;
        }
        if force {
            state
                .forced_at
                .is_none_or(|t| t.elapsed() >= FORCED_REFRESH_INTERVAL)
        } else {
            state.key_set.is_none() || state.fetched_at.is_none_or(|t| t.elapsed() > TTL)
        }
    }

    /// With no keys there is nothing to serve, so callers wait for the
    /// in-flight fetch. Once a key set exists, whoever wins the gate refreshes
    /// and everyone else is served the cached keys immediately.
    async fn refresh_single_flight(&self, force: bool) {
        let cold = self.state().key_set.is_none();
        let _gate = if cold {
            self.refresh_gate.lock().await
        } else {
            match self.refresh_gate.try_lock() {
                Ok(gate) => gate,
                Err(_) => return,
            }
        };
        if self.wants_refresh(force) {
            self.refresh(force).await;
        }
    }

    async fn refresh(&self, force: bool) {
        if force {
            self.state().forced_at = Some(Instant::now());
        }
        match self.fetch().await {
            Ok(keys) => {
                let mut state = self.state();
                state.key_set = Some(Arc::new(keys));
                state.fetched_at = Some(Instant::now());
                state.failed_at = None;
            }
            Err(e) => {
                self.state().failed_at = Some(Instant::now());
                // Keep stale keys if we have them; `key_set` fails if never.
                eprintln!("ged: JWKS refresh failed: {e}");
            }
        }
    }

    async fn fetch(&self) -> Result<JwkSet, FetchError> {
        let response = self
            .http
            .get(&self.url)
            .send()
            .await
            .map_err(|e| FetchError(format!("JWKS request failed: {e}")))?;
        if !response.status().is_success() {
            return Err(FetchError(format!(
                "JWKS endpoint returned {}",
                response.status().as_u16()
            )));
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| FetchError(format!("JWKS body unreadable: {e}")))?;
        serde_json::from_slice(&body).map_err(|e| FetchError(format!("JWKS unparsable: {e}")))
    }
}
