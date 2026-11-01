//! HTTP ports of `test/controllers/admin/upload_queue_pause_test.rb`.

mod common;

use common::{AdminWorld, assert_login_redirect, assert_see, name, queue_has_word, queue_words};
use parity::admin::{AdminSession, csrf_token};
use reqwest::Method;

#[tokio::test]
async fn admin_upload_queue_pauses_and_resumes() {
    let w = AdminWorld::new().await;
    let paused = w
        .admin
        .submit(Method::POST, "/admin/upload_queue_pause", &[])
        .await;
    let during = w.admin.page_after(&paused).await;
    let resumed = w
        .admin
        .submit(Method::DELETE, "/admin/upload_queue_pause", &[])
        .await;
    let after = w.admin.page_after(&resumed).await;

    assert!(
        matches!(paused.status, 302 | 303),
        "pause: HTTP {}",
        paused.status
    );
    assert!(
        paused
            .header("location")
            .unwrap_or("")
            .contains("/admin/upload_requests"),
        "pause redirected to {:?}",
        paused.header("location")
    );
    assert_see(&during, "Upload queue paused.");
    assert_banner(&during.text(), "paused", "Resume");

    assert!(
        matches!(resumed.status, 302 | 303),
        "resume: HTTP {}",
        resumed.status
    );
    assert!(
        resumed
            .header("location")
            .unwrap_or("")
            .contains("/admin/upload_requests"),
        "resume redirected to {:?}",
        resumed.header("location")
    );
    assert_see(&after, "Upload queue resumed.");
    assert_banner(&after.text(), "active", "Pause");
}

#[tokio::test]
async fn admin_upload_queue_banner_reflects_state_and_request_counts() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    w.upload_request(dept, "r.pdf", "pending", None).await;
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM upload_requests WHERE status = 'pending'")
            .fetch_one(w.db())
            .await
            .expect("pending count");
    assert!(pending >= 1, "the fixture request was not inserted");

    let page = w.admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_banner(&page.text(), "active", "Pause");
    assert_see(&page, &format!("{pending} pending"));

    let paused = w
        .admin
        .submit(Method::POST, "/admin/upload_queue_pause", &[])
        .await;
    let during = w.admin.get("/admin/upload_requests").await;
    let _ = w
        .admin
        .submit(Method::DELETE, "/admin/upload_queue_pause", &[])
        .await;

    assert!(
        matches!(paused.status, 302 | 303),
        "pause: HTTP {}",
        paused.status
    );
    assert_banner(&during.text(), "paused", "Resume");
}

#[tokio::test]
async fn admin_upload_queue_pause_requires_an_admin_session() {
    let w = AdminWorld::new().await;
    let anon = AdminSession::anonymous(w.backend.base_url());
    let form = anon.get("/admins/sign_in").await;
    assert_eq!(form.status, 200, "{}", form.text());
    let token = csrf_token(&form.text()).expect("csrf meta on the login form");
    // A real token for this anonymous session, and no admin login.
    let resp = anon
        .post_form_with_token("/admin/upload_queue_pause", &token, &[])
        .await;
    assert_login_redirect(&resp);

    let page = w.admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_banner(&page.text(), "active", "Pause");
    assert!(
        !queue_has_word(&page.text(), "Resume"),
        "anonymous POST paused the queue: {:?}",
        queue_words(&page.text())
    );
}

fn assert_banner(body: &str, state: &str, action: &str) {
    let words = queue_words(body);
    assert!(
        queue_has_word(body, state),
        "banner did not show {state}, words={words:?}"
    );
    assert!(
        queue_has_word(body, action),
        "banner did not offer {action}, words={words:?}"
    );
}
