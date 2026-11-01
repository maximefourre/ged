//! HTTP ports of `test/controllers/admin/pagination_test.rb` and
//! `test/controllers/admin/param_guards_test.rb`.
//!
//! Indexes are scoped with a query (`department_id` or a unique status) so the
//! page size is the rows this test created. Non-scalar params are ignored:
//! the real row stays visible.

mod common;

use common::{AdminWorld, assert_see, name, tr_count};
use serde_json::json;

#[tokio::test]
async fn admin_documents_index_shows_50_per_page_with_navigation_links() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    for i in 0..51 {
        w.document(dept, &format!("doc-{i}.pdf"), json!({}), None)
            .await;
    }

    let page1 = w
        .admin
        .get(&format!("/admin/documents?department_id={dept}"))
        .await;
    assert_eq!(page1.status, 200, "{}", page1.text());
    assert_eq!(tr_count(&page1.text()), 51, "header + 50 rows");
    assert_see(&page1, "Older →");
    assert!(!page1.text().contains("← Newer"), "page 1 offered Newer");
    assert!(
        page1.text().contains(&format!("department_id={dept}")) && page1.text().contains("page=2"),
        "Older link should keep the department filter and point at page 2"
    );

    let page2 = w
        .admin
        .get(&format!("/admin/documents?department_id={dept}&page=2"))
        .await;
    assert_eq!(page2.status, 200, "{}", page2.text());
    assert_eq!(tr_count(&page2.text()), 2, "header + 1 row");
    assert_see(&page2, "← Newer");
    assert!(!page2.text().contains("Older →"), "last page offered Older");
}

#[tokio::test]
async fn admin_upload_requests_index_shows_50_per_page_and_keeps_the_status_filter() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let status = name("st");
    for _ in 0..51 {
        w.upload_request(dept, "report.pdf", &status, None).await;
    }

    let page = w
        .admin
        .get(&format!("/admin/upload_requests?status={status}"))
        .await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_eq!(tr_count(&page.text()), 51, "header + 50 rows");
    assert_see(&page, "Older →");
    assert!(
        page.text().contains(&format!("status={status}")) && page.text().contains("page=2"),
        "Older link should keep status={status} and page=2:\n{}",
        page.text()
    );
}

#[tokio::test]
async fn admin_non_numeric_or_non_scalar_page_params_fall_back_to_the_first_page() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("doc.pdf");
    w.document(dept, &filename, json!({}), None).await;

    for page in ["abc", "-1", "%5B%5D=2"] {
        let path = if page.starts_with("%5B") {
            format!("/admin/documents?department_id={dept}&page{page}")
        } else {
            format!("/admin/documents?department_id={dept}&page={page}")
        };
        let resp = w.admin.get(&path).await;
        assert_eq!(resp.status, 200, "{path}: {}", resp.text());
        assert_see(&resp, &filename);
        assert_eq!(tr_count(&resp.text()), 2, "{path} should be the first page");
    }
}

#[tokio::test]
async fn admin_a_page_far_beyond_bigint_offset_range_is_clamped_not_a_500() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("doc.pdf");
    w.document(dept, &filename, json!({}), None).await;

    let resp = w
        .admin
        .get(&format!(
            "/admin/documents?department_id={dept}&page=200000000000000000"
        ))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_see(&resp, "Filename");
    assert_see(&resp, "No documents match.");
    assert!(
        !resp.text().contains(&filename),
        "the clamped page should be past this document"
    );
}

#[tokio::test]
async fn admin_documents_index_ignores_an_array_filename_param() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("doc.pdf");
    w.document(dept, &filename, json!({}), None).await;

    let resp = w
        .admin
        .get(&format!(
            "/admin/documents?department_id={dept}&filename%5B%5D=x"
        ))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_see(&resp, &filename);
}

#[tokio::test]
async fn admin_documents_index_ignores_a_hash_department_id_param() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("doc.pdf");
    w.document(dept, &filename, json!({}), None).await;

    let resp = w.admin.get("/admin/documents?department_id%5Ba%5D=b").await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_see(&resp, &filename);
}

#[tokio::test]
async fn admin_documents_index_ignores_an_array_tag_param() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("doc.pdf");
    w.document(dept, &filename, json!({}), None).await;

    let resp = w
        .admin
        .get(&format!(
            "/admin/documents?department_id={dept}&tag%5B%5D=x"
        ))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_see(&resp, &filename);
}

#[tokio::test]
async fn admin_upload_requests_index_ignores_a_hash_status_param() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    w.upload_request(dept, &filename, "failed", None).await;

    let resp = w.admin.get("/admin/upload_requests?status%5Ba%5D=b").await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_see(&resp, &filename);
}
