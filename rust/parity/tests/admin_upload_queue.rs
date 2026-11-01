//! HTTP ports of `test/controllers/admin/upload_queue_pause_test.rb`.

mod common;

use common::{AdminWorld, assert_login_redirect, assert_see, name};
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
    assert!(
        during.text().contains(">paused<"),
        "banner did not show paused"
    );
    assert!(
        during.text().contains(">Resume<"),
        "banner did not offer Resume"
    );

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
    assert!(
        after.text().contains(">active<"),
        "banner did not show active"
    );
    assert!(
        after.text().contains(">Pause<"),
        "banner did not offer Pause"
    );
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
    assert!(page.text().contains(">active<"), "banner was not active");
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
    assert!(
        during.text().contains(">paused<"),
        "banner did not show paused"
    );
    assert!(
        during.text().contains(">Resume<"),
        "banner did not offer Resume"
    );
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
    assert!(
        page.text().contains(">Pause<") && !page.text().contains(">Resume<"),
        "anonymous POST paused the queue:\n{}",
        page.text()
    );
}
