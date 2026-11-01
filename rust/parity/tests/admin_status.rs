//! HTTP ports of the parity rows in `test/controllers/admin/status_controller_test.rb`.
//!
//! "lists recent failed jobs" is a Rust unit test (Solid Queue tables), not
//! this suite. The JWKS line is whatever the live process last observed:
//! not fetched yet, keys cached, or serving stale keys.

mod common;

use common::{
    AdminWorld, admin_upload as upload, assert_login_redirect, assert_see, name, pdf_bytes,
    queue_has_word, queue_words, td_texts, tr_containing, visible_text,
};
use parity::admin::AdminSession;
use reqwest::Method;

#[tokio::test]
async fn admin_status_requires_an_admin_session() {
    let w = AdminWorld::new().await;
    let anon = AdminSession::anonymous(w.backend.base_url());
    let resp = anon.get("/admin/status").await;
    assert_login_redirect(&resp);
}

#[tokio::test]
async fn admin_status_renders_the_three_sections_with_live_counts() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    w.upload_request(dept, "r.pdf", "pending", None).await;
    let created = upload(
        &w.admin,
        dept,
        &name("a.pdf"),
        b"pdf bytes",
        "application/pdf",
        None,
    )
    .await;
    assert!(
        matches!(created.status, 302 | 303),
        "upload: HTTP {}\n{}",
        created.status,
        created.text()
    );

    let page = w.admin.get("/admin/status").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, "<h1>Status</h1>");
    assert_see(&page, "Database");
    assert_see(&page, "Job workers");
    assert_see(&page, "OIDC JWKS");
    assert_see(&page, "Storage disk");
    let body = page.text();
    assert!(
        body.contains("not fetched yet")
            || body.contains("keys cached")
            || body.contains("serving stale keys"),
        "JWKS detail is outside the closed set SystemHealth emits:\n{body}"
    );
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM upload_requests WHERE status = 'pending'")
            .fetch_one(w.db())
            .await
            .expect("pending count");
    assert!(pending >= 1, "the pending fixture is missing");
    let visible = visible_text(&body);
    assert!(
        visible.contains(&format!("{pending} pending requests")),
        "pipeline count is not the live pending total {pending}: {visible}"
    );
    assert_see(&page, &dept_name);
}

#[tokio::test]
async fn admin_status_reflects_the_uploads_queue_pause_state() {
    let w = AdminWorld::new().await;
    let before = w.admin.get("/admin/status").await;
    assert_eq!(before.status, 200, "{}", before.text());

    let paused = w
        .admin
        .submit(Method::POST, "/admin/upload_queue_pause", &[])
        .await;
    let during = w.admin.get("/admin/status").await;
    // Resume before asserting, so a failed assertion does not leave the queue paused.
    let resumed = w
        .admin
        .submit(Method::DELETE, "/admin/upload_queue_pause", &[])
        .await;
    let after = w.admin.get("/admin/status").await;

    assert_queue_state(&before.text(), "active");
    assert!(
        matches!(paused.status, 302 | 303),
        "pause: HTTP {}",
        paused.status
    );
    assert_queue_state(&during.text(), "paused");
    assert!(
        matches!(resumed.status, 302 | 303),
        "resume: HTTP {}",
        resumed.status
    );
    assert_queue_state(&after.text(), "active");
}

fn assert_queue_state(body: &str, state: &str) {
    assert!(
        queue_has_word(body, state),
        "queue banner did not show {state}, words={:?}",
        queue_words(body)
    );
}

#[tokio::test]
async fn admin_status_top_departments_lists_stored_volume_and_document_count() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    for i in 0..2 {
        let created = upload(
            &w.admin,
            dept,
            &format!("doc-{i}.pdf"),
            pdf_bytes(),
            "application/pdf",
            None,
        )
        .await;
        assert!(
            matches!(created.status, 302 | 303),
            "upload {i}: HTTP {}\n{}",
            created.status,
            created.text()
        );
    }

    let page = w.admin.get("/admin/status").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, "Documents");
    let body = page.text();
    let row = tr_containing(&body, &dept_name)
        .unwrap_or_else(|| panic!("no top-departments row for {dept_name}:\n{body}"));
    let cells = td_texts(row);
    assert_eq!(
        cells.last().map(String::as_str),
        Some("2"),
        "document count should be the last column, got {cells:?}"
    );
}
