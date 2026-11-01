//! HTTP ports of `test/controllers/admin/upload_requests_controller_test.rb`
//! and the admin UI row "lists upload requests with status and error".

mod common;

use common::{AdminWorld, assert_see, name, td_texts, tr_containing};
use parity::admin::AdminSession;

#[tokio::test]
async fn admin_upload_requests_require_an_admin_session() {
    let w = AdminWorld::new().await;
    let anon = AdminSession::anonymous(w.backend.base_url());
    let resp = anon.get("/admin/upload_requests").await;
    common::assert_login_redirect(&resp);
}

#[tokio::test]
async fn admin_upload_requests_index_lists_requests_with_their_department_name() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let filename = name("report.pdf");
    w.upload_request(dept, &filename, "pending", None).await;

    let page = w.admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, &filename);
    assert_see(&page, &dept_name);
}

#[tokio::test]
async fn admin_upload_requests_index_renders_for_a_request_whose_department_is_trashed() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let filename = name("report.pdf");
    w.upload_request(dept, &filename, "pending", None).await;
    sqlx::query("UPDATE departments SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(dept)
        .execute(w.db())
        .await
        .expect("trash department");

    let page = w.admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, &filename);
    assert_see(&page, &dept_name);
    assert_see(&page, "(trashed)");
}

#[tokio::test]
async fn admin_upload_requests_index_shows_an_empty_state_when_there_are_none() {
    let w = AdminWorld::new().await;
    // The empty copy is the unfiltered index. AdminWorld holds SuiteLock, so
    // this reset is exclusive among admin tests and does not depend on a
    // freshly seeded stack. Other tests keep isolating with unique names.
    w.backend.reset().await.expect("reset business tables");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM upload_requests")
        .fetch_one(w.db())
        .await
        .expect("count");
    assert_eq!(n, 0, "reset left upload requests behind");
    let page = w.admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, "No upload requests match.");
}

#[tokio::test]
async fn admin_upload_requests_lists_status_and_error() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("big.pdf");
    w.upload_request(
        dept,
        &filename,
        "failed",
        Some("staged file lost before processing"),
    )
    .await;

    let page = w.admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    let body = page.text();
    // The status filter's <select> always lists "failed", so the word has to
    // come from this request's own row.
    let row =
        tr_containing(&body, &filename).unwrap_or_else(|| panic!("no row for {filename}:\n{body}"));
    let cells = td_texts(row);
    assert!(
        cells.iter().any(|c| c == "failed"),
        "status cell: {cells:?}"
    );
    assert!(
        cells
            .iter()
            .any(|c| c == "staged file lost before processing"),
        "error cell: {cells:?}"
    );
}
