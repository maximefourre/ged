//! Print the cached admin session cookie (`name=value`) on stdout.
//!
//! Uses [`parity::admin::AdminSession::cached`] (one login per process) and
//! the Rust backend, so Playwright does not spend the limit of 10 sign-in
//! attempts per 3 minutes.

#[tokio::main]
async fn main() -> Result<(), String> {
    let backend = parity::backend().await;
    let session = parity::admin::AdminSession::cached(backend.base_url()).await?;
    let cookie = session.session_cookie().await?;
    if cookie.is_empty() || !cookie.contains('=') {
        return Err(format!("session cookie is not name=value: {cookie:?}"));
    }
    println!("{cookie}");
    Ok(())
}
