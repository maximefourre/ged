//! Request-time half of `test/security/ssrf_test.rb`: the four numeric
//! loopback encodings are rejected with 422 before an `upload_requests` row
//! exists. Pinning, DNS and the hostname negative control stay unit tests;
//! one public-hostname upload locks that a callback is not refused wholesale.

use parity::fixtures::{self};
use parity::json::{assert_error, assert_json_content_type, assert_status};
use parity::oauth::{self};
use parity::{Api, backend};
use serde_json::json;

const BLOCKED: &str = "Callback url must not target a private or internal address";

struct Ssrf {
    base: String,
    token: String,
    dept: i64,
}

impl Ssrf {
    /// A client bound to this test's runtime. A shared `reqwest::Client` dies
    /// with the runtime that first polled it.
    fn api(&self) -> Api {
        Api::new(&self.base)
    }
}

async fn ssrf() -> &'static Ssrf {
    static CELL: tokio::sync::OnceCell<Ssrf> = tokio::sync::OnceCell::const_new();
    CELL.get_or_init(build).await
}

async fn build() -> Ssrf {
    let backend = backend().await;
    let db = backend.db();
    let base = backend.base_url().to_owned();
    let dept = fixtures::create_department(db, &fixtures::unique("ssrf"), None).await;
    let creds = oauth::register_everywhere(db, "ssrf").await;
    let id = fixtures::app_client_id(db, &creds.client_id).await;
    fixtures::grant(db, id, dept, true, true).await;
    let token = oauth::fetch_token(&creds).await;
    Ssrf { base, token, dept }
}

async fn assert_blocked(host: &str) {
    let f = ssrf().await;
    let url = format!("http://{host}/{}", fixtures::unique("cb"));
    let dept = f.dept.to_string();
    let response = f
        .api()
        .upload_bearer(
            "/api/v1/documents",
            &f.token,
            &[
                ("department_id", dept.as_str()),
                ("callback_url", url.as_str()),
            ],
            "report.pdf",
            "application/pdf",
            b"PDF!",
        )
        .await;
    assert_error(&response, 422, BLOCKED);
    let backend = backend().await;
    assert_eq!(
        fixtures::upload_request_count(backend.db(), f.dept, &url).await,
        0,
        "blocked callback still inserted a row"
    );
}

#[tokio::test]
async fn security_ssrf_decimal_loopback_is_blocked() {
    assert_blocked("2130706433").await;
}

#[tokio::test]
async fn security_ssrf_hex_loopback_is_blocked() {
    assert_blocked("0x7f000001").await;
}

#[tokio::test]
async fn security_ssrf_octal_loopback_is_blocked() {
    assert_blocked("0177.0.0.1").await;
}

#[tokio::test]
async fn security_ssrf_short_loopback_is_blocked() {
    assert_blocked("127.1").await;
}

#[tokio::test]
async fn security_ssrf_public_hostname_is_accepted_at_request_time() {
    let f = ssrf().await;
    let url = format!("https://example.com/{}", fixtures::unique("cb"));
    let dept = f.dept.to_string();
    let response = f
        .api()
        .upload_bearer(
            "/api/v1/documents",
            &f.token,
            &[
                ("department_id", dept.as_str()),
                ("callback_url", url.as_str()),
            ],
            "report.pdf",
            "application/pdf",
            b"PDF!",
        )
        .await;
    assert_status(&response, 202);
    assert_json_content_type(&response);
    let body = response.json();
    assert_eq!(body["status"], json!("pending"));
    assert!(body["request_id"].as_str().is_some(), "{body}");
    let backend = backend().await;
    assert_eq!(
        fixtures::upload_request_count(backend.db(), f.dept, &url).await,
        1
    );
}
