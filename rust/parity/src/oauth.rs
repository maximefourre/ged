//! Client registration and token retrieval against `oauth-provider`.
//!
//! Registering a client takes two steps: create credentials in the provider
//! (`bin/register-client` inside its container), then register the same
//! client id in GED (an `app_clients` row — see [`crate::fixtures`]).

use std::sync::Mutex;

use crate::stack::{self, stack};

#[derive(Debug, Clone)]
pub struct Credentials {
    pub client_id: String,
    pub secret: String,
}

/// Register `client_id` in the oauth-provider and return its credentials.
pub fn register(client_id: &str) -> Credentials {
    assert!(
        client_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "client id {client_id:?} has characters unsafe for exec"
    );
    // The provider's SQLite store takes one writer at a time: serialize
    // registrations within this process and retry if another one holds the lock.
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut attempt = 0;
    let out = loop {
        match stack::exec("oauth-provider", &["bin/register-client", client_id]) {
            Ok(out) => break out,
            Err(e) if e.contains("locked") && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => panic!("register client: {e}"),
        }
    };
    let secret = out
        .lines()
        .find_map(|l| l.strip_prefix("client_secret:"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| panic!("no client_secret in register-client output: {out:?}"));
    Credentials {
        client_id: client_id.to_owned(),
        secret,
    }
}

/// Fetch a genuine RS256 access token (client_credentials grant).
///
/// Under parallel suite load the provider's SQLite store sometimes answers
/// the token endpoint with an HTML error page instead of JSON. That is a
/// harness failure, not an auth result: retry a few times, then still require
/// a successful JSON body that carries `access_token`.
pub async fn fetch_token(creds: &Credentials) -> String {
    let http = reqwest::Client::new();
    let mut last = String::new();
    for attempt in 0..5 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(
                50 * u64::try_from(attempt).unwrap_or(1),
            ))
            .await;
        }
        let resp = http
            .post(format!("{}/token", stack().oauth_url))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", creds.client_id.as_str()),
                ("client_secret", creds.secret.as_str()),
            ])
            .send()
            .await
            .expect("POST /token");
        let status = resp.status();
        let bytes = resp.bytes().await.expect("token body");
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(body) if status.is_success() => {
                return body["access_token"]
                    .as_str()
                    .expect("access_token")
                    .to_owned();
            }
            Ok(body) => last = format!("token endpoint answered {status}: {body}"),
            Err(_) => {
                let text = String::from_utf8_lossy(&bytes);
                let excerpt: String = text.chars().take(180).collect();
                last = format!("token endpoint answered {status} with a non-JSON body: {excerpt}");
            }
        }
    }
    panic!("no access token after 5 attempts: {last}");
}

/// Both registration steps: a fresh `prefix-xxxxxxxx` client in the provider
/// and the same id in GED's `app_clients`, with no department access yet.
pub async fn register_everywhere(db: &sqlx::PgPool, prefix: &str) -> Credentials {
    let creds = register(&crate::fixtures::unique(prefix));
    crate::fixtures::create_app_client(db, &creds.client_id).await;
    creds
}
