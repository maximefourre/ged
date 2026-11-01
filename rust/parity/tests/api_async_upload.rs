//! Port of `test/controllers/api/v1/async_upload_test.rb` and
//! `test/controllers/api/v1/upload_requests_test.rb`.
//!
//! Callback POSTs are not asserted here: loopback targets are rejected by the
//! SSRF guard, and the inventory sends those assertions to the `deliver_callback`
//! unit tests. The same payload is checked through the polling endpoint.
//!
//! Two Ruby tests force a failure by deleting the staging file
//! (`an async failure is surfaced…`, `a failed async upload reports null tags…`).
//! That file is storage, not a business table, so those lines are unit tests
//! (`store_document`, `deliver_callback`), not parity rows.
//!
//! The success path checks stored bytes through the download endpoint only.
//! Asserting that `storage/staging/<id>` was removed is the same unit-level
//! observation. Pausing the uploads queue (to see `pending`) calls the admin
//! route `POST`/`DELETE /admin/upload_queue_pause`; the Rust port must serve
//! it as soon as these tests run against Rust.

mod common;

use std::sync::OnceLock;

use common::{World, poll_upload};
use parity::admin::AdminSession;
use parity::http::FilePart;
use parity::json::{assert_error, assert_json_content_type, assert_status};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const PDF: &[u8] = b"PDF!";

fn async_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Pauses the uploads queue and resumes it when dropped, including on panic,
/// so a failed assertion cannot leave the queue paused for later tests.
struct ResumeUploads {
    base: String,
}

impl Drop for ResumeUploads {
    fn drop(&mut self) {
        let base = self.base.clone();
        let started = std::thread::Builder::new()
            .name("resume-uploads".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("resume runtime");
                runtime.block_on(async move {
                    match AdminSession::cached(&base).await {
                        Ok(admin) => {
                            let _ = admin.resume_upload_queue().await;
                        }
                        Err(error) => eprintln!("resume-on-drop: {error}"),
                    }
                });
            });
        if let Ok(handle) = started {
            let _ = handle.join();
        }
    }
}

async fn pause_uploads(base: &str) -> ResumeUploads {
    let guard = ResumeUploads {
        base: base.to_owned(),
    };
    let admin = AdminSession::cached(base).await.expect("admin session");
    let resp = admin.pause_upload_queue().await;
    assert!(
        matches!(resp.status, 302 | 303),
        "pause answered {} {:?}",
        resp.status,
        resp.header("location")
    );
    guard
}

async fn resume_uploads(base: &str) {
    let admin = AdminSession::cached(base).await.expect("admin session");
    let resp = admin.resume_upload_queue().await;
    assert!(
        matches!(resp.status, 302 | 303),
        "resume answered {} {:?}",
        resp.status,
        resp.header("location")
    );
}

fn callback_url() -> String {
    format!(
        "https://example.com/ged-callback-{}",
        &parity::fixtures::random_uuid()[..8]
    )
}

async fn post_async(
    w: &World,
    department_id: i64,
    url: &str,
    extra: &[(&str, &str)],
) -> parity::Resp {
    let dept = department_id.to_string();
    let mut fields = vec![("department_id", dept.as_str()), ("callback_url", url)];
    fields.extend_from_slice(extra);
    w.api
        .post_multipart(
            "/api/v1/documents",
            &w.token,
            &fields,
            Some(FilePart {
                field: "file",
                filename: "report.pdf",
                content_type: "application/pdf",
                bytes: PDF,
            }),
        )
        .await
}

fn assert_pending_body(body: &Value, request_id: &str) {
    assert_eq!(
        body,
        &json!({
            "request_id": request_id,
            "status": "pending",
            "document_id": null,
            "error_message": null,
            "tags": null,
            "metadata": null
        })
    );
}

#[tokio::test]
async fn api_upload_with_a_callback_url_targeting_an_internal_address_returns_422() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let resp = post_async(
        &w,
        t.finance,
        "http://169.254.169.254/latest/meta-data",
        &[],
    )
    .await;
    assert_error(
        &resp,
        422,
        "Callback url must not target a private or internal address",
    );
    assert_eq!(
        parity::fixtures::count_upload_requests(w.backend.db(), t.finance).await,
        0
    );
}

#[tokio::test]
async fn api_upload_with_callback_url_returns_202_and_processes_in_background() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    resume_uploads(w.backend.base_url()).await;
    let t = w.read_write_tree().await;
    let url = callback_url();
    let resp = post_async(&w, t.finance, &url, &[]).await;
    assert_status(&resp, 202);
    assert_json_content_type(&resp);
    let body = resp.json();
    let request_id = body["request_id"].as_str().expect("request_id");
    assert_eq!(body, json!({"request_id": request_id, "status": "pending"}));

    let done = poll_upload(&w.api, &w.token, request_id, "completed").await;
    let document_id = done["document_id"].as_str().expect("document_id");
    assert!(done["error_message"].is_null());
    let download = w
        .api
        .get_bearer(
            &format!("/api/v1/documents/{document_id}/download"),
            &w.token,
        )
        .await;
    assert_status(&download, 200);
    assert_eq!(download.body, PDF);
}

#[tokio::test]
async fn api_rejects_callback_url_values_that_are_not_http_urls_with_422() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    for bad in ["notaurl", "file:///etc/passwd", "ftp://caller.test/cb"] {
        let resp = post_async(&w, t.finance, bad, &[]).await;
        assert_error(
            &resp,
            422,
            "Callback url must be an http(s) URL with a host",
        );
    }
    assert_eq!(
        parity::fixtures::count_upload_requests(w.backend.db(), t.finance).await,
        0
    );
}

#[tokio::test]
async fn api_upload_request_status_can_be_polled_through_pending_and_completed() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let base = w.backend.base_url().to_owned();
    resume_uploads(&base).await;
    let _resume_on_drop = pause_uploads(&base).await;
    let t = w.read_write_tree().await;
    let url = callback_url();
    let resp = post_async(&w, t.finance, &url, &[]).await;
    assert_status(&resp, 202);
    let request_id = resp.json()["request_id"].as_str().expect("id").to_owned();

    let pending = w
        .api
        .get_bearer(&format!("/api/v1/upload_requests/{request_id}"), &w.token)
        .await;
    assert_status(&pending, 200);
    assert_pending_body(&pending.json(), &request_id);

    resume_uploads(&base).await;
    let done = poll_upload(&w.api, &w.token, &request_id, "completed").await;
    assert!(done["document_id"].is_string());
    assert!(done["error_message"].is_null());
}

#[tokio::test]
async fn api_async_upload_applies_tags_and_returns_them_when_polled() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    resume_uploads(w.backend.base_url()).await;
    let t = w.read_write_tree().await;
    let url = callback_url();
    let resp = post_async(
        &w,
        t.finance,
        &url,
        &[("tags[type]", "facture"), ("tags[urgent]", "")],
    )
    .await;
    assert_status(&resp, 202);
    let request_id = resp.json()["request_id"].as_str().expect("id").to_owned();
    let done = poll_upload(&w.api, &w.token, &request_id, "completed").await;
    assert_eq!(done["tags"], json!({"type": "facture", "urgent": null}));
    let document_id = done["document_id"].as_str().expect("document_id");
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), document_id).await,
        json!({"type": "facture", "urgent": null})
    );
}

#[tokio::test]
async fn api_async_upload_with_invalid_tags_is_rejected_with_422_before_the_202() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let url = callback_url();
    let resp = post_async(&w, t.finance, &url, &[("tags", "flat")]).await;
    assert_error(&resp, 422, "Tags must be a map of key/value pairs");
    assert_eq!(
        parity::fixtures::count_upload_requests(w.backend.db(), t.finance).await,
        0
    );
}

#[tokio::test]
async fn api_async_metadata_is_null_while_pending_and_populated_once_completed() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let base = w.backend.base_url().to_owned();
    resume_uploads(&base).await;
    let _resume_on_drop = pause_uploads(&base).await;
    let t = w.read_write_tree().await;
    let url = callback_url();
    let resp = post_async(&w, t.finance, &url, &[]).await;
    assert_status(&resp, 202);
    let request_id = resp.json()["request_id"].as_str().expect("id").to_owned();

    let pending = w
        .api
        .get_bearer(&format!("/api/v1/upload_requests/{request_id}"), &w.token)
        .await;
    assert!(pending.json()["metadata"].is_null());

    resume_uploads(&base).await;
    let done = poll_upload(&w.api, &w.token, &request_id, "completed").await;
    let meta = &done["metadata"];
    assert_eq!(meta["upload_mode"], json!("async"));
    common::assert_rails_time(&meta["uploaded_at"]);
    common::assert_rails_time(&meta["updated_at"]);
}

#[tokio::test]
async fn api_polling_payload_carries_the_document_metadata() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    resume_uploads(w.backend.base_url()).await;
    let t = w.read_write_tree().await;
    let url = callback_url();
    let resp = post_async(&w, t.finance, &url, &[]).await;
    assert_status(&resp, 202);
    let request_id = resp.json()["request_id"].as_str().expect("id").to_owned();
    let done = poll_upload(&w.api, &w.token, &request_id, "completed").await;
    assert_eq!(done["metadata"]["upload_mode"], json!("async"));
    common::assert_rails_time(&done["metadata"]["uploaded_at"]);
    common::assert_rails_time(&done["metadata"]["updated_at"]);
}

#[tokio::test]
async fn api_polling_an_upload_request_of_a_forbidden_department_returns_403() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let request_id = parity::fixtures::insert_upload_request(
        w.backend.db(),
        t.hr,
        "https://example.com/ged-callback",
        "report.pdf",
        "other-app",
    )
    .await;
    let resp = w
        .api
        .get_bearer(&format!("/api/v1/upload_requests/{request_id}"), &w.token)
        .await;
    assert_error(&resp, 403, "no read access to this department");
}

#[tokio::test]
async fn api_async_upload_into_a_forbidden_department_returns_403_and_enqueues_nothing() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let resp = post_async(&w, t.hr, &callback_url(), &[]).await;
    assert_error(&resp, 403, "no write access to this department");
    assert_eq!(
        parity::fixtures::count_upload_requests(w.backend.db(), t.hr).await,
        0
    );
}

#[tokio::test]
async fn api_upload_request_for_trashed_dept_returns_404() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let request_id = parity::fixtures::insert_upload_request(
        w.backend.db(),
        t.finance,
        "https://example.com/callback",
        "report.pdf",
        &w.client.client_id,
    )
    .await;
    parity::fixtures::trash_department(w.backend.db(), t.finance).await;
    let resp = w
        .api
        .get_bearer(&format!("/api/v1/upload_requests/{request_id}"), &w.token)
        .await;
    assert_error(&resp, 404, "not found");
}

#[tokio::test]
async fn api_upload_request_for_live_dept_returns_200() {
    let _guard = async_lock().lock().await;
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let request_id = parity::fixtures::insert_upload_request(
        w.backend.db(),
        t.finance,
        "https://example.com/callback",
        "report.pdf",
        &w.client.client_id,
    )
    .await;
    let resp = w
        .api
        .get_bearer(&format!("/api/v1/upload_requests/{request_id}"), &w.token)
        .await;
    assert_status(&resp, 200);
    assert_json_content_type(&resp);
    assert_eq!(resp.json()["request_id"], json!(request_id));
    assert_eq!(resp.json()["status"], json!("pending"));
}
