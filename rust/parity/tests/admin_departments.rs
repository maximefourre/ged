//! HTTP ports of the department journeys in `test/controllers/admin`.
//! Status codes and visible labels; rows checked in the business tables.

mod common;

use common::{
    AdminWorld, assert_see, department_gone, department_id_by_name, department_trashed,
    doc_department, name, request_department, td_texts, tr_containing,
};
use reqwest::Method;
use uuid::Uuid;

#[tokio::test]
async fn admin_creates_a_top_level_department() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("hr");
    let resp = w
        .admin
        .submit(
            Method::POST,
            "/admin/departments",
            &[("department[name]", &dept_name)],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    let page = w.admin.page_after(&resp).await;
    assert_eq!(page.status, 200);
    assert_see(&page, "Department created.");
    assert_see(&page, &dept_name);
    let id = department_id_by_name(w.db(), &dept_name).await;
    w.track_department(id);
    let parent: Option<i64> = sqlx::query_scalar("SELECT parent_id FROM departments WHERE id = $1")
        .bind(id)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert!(parent.is_none());
}

#[tokio::test]
async fn admin_creates_a_subfolder() {
    let mut w = AdminWorld::new().await;
    let parent = w.department(&name("finance"), None).await;
    let child_name = name("invoices");
    let parent_s = parent.to_string();
    let resp = w
        .admin
        .submit(
            Method::POST,
            "/admin/departments",
            &[
                ("department[name]", &child_name),
                ("department[parent_id]", &parent_s),
            ],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    let id = department_id_by_name(w.db(), &child_name).await;
    w.track_department(id);
    let stored: Option<i64> = sqlx::query_scalar("SELECT parent_id FROM departments WHERE id = $1")
        .bind(id)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(stored, Some(parent));
}

#[tokio::test]
async fn admin_renames_a_department() {
    let mut w = AdminWorld::new().await;
    let id = w.department(&name("finance"), None).await;
    let edit = w.admin.get(&format!("/admin/departments/{id}/edit")).await;
    assert_eq!(edit.status, 200);
    assert_see(&edit, "Rename");
    let renamed = name("finance-ops");
    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/departments/{id}"),
            &[("department[name]", &renamed)],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Department renamed.");
    assert_see(&page, &renamed);
}

#[tokio::test]
async fn admin_rejects_renaming_a_department_to_a_blank_name() {
    let mut w = AdminWorld::new().await;
    let original = name("finance");
    let id = w.department(&original, None).await;
    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/departments/{id}"),
            &[("department[name]", "")],
        )
        .await;
    // The edit form re-renders with 422. It does not print the validation
    // sentence; the name field and the Rename action are what stay on screen.
    assert_eq!(resp.status, 422, "{}", resp.text());
    assert_see(&resp, "Rename department");
    assert_see(&resp, "Rename");
    let stored: String = sqlx::query_scalar("SELECT name FROM departments WHERE id = $1")
        .bind(id)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(stored, original);
}

#[tokio::test]
async fn admin_deletes_a_department_granted_to_an_app_client() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let client = w.client(&name("erp")).await;
    w.grant(client, dept, true, true).await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Department deleted.");
    assert!(department_gone(w.db(), dept).await);
}

#[tokio::test]
async fn admin_deleting_a_department_with_upload_requests_opens_resolution() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    w.upload_request(dept, &name("report.pdf"), "pending", None)
        .await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    let loc = resp.header("location").unwrap_or("");
    assert!(
        loc.contains(&format!("/admin/departments/{dept}/confirm_deletion")),
        "{loc}"
    );
    assert!(!department_gone(w.db(), dept).await);
    let page = w.admin.page_after(&resp).await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, "pending upload request");
}

#[tokio::test]
async fn admin_deleting_a_department_with_only_soft_deleted_documents_opens_resolution() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let doc = w
        .document(dept, &name("trashed.pdf"), serde_json::json!({}), None)
        .await;
    soft_delete_document(w.db(), doc).await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("confirm_deletion")
    );
    assert!(!department_gone(w.db(), dept).await);
    assert!(row_exists_document(w.db(), doc).await);
}

#[tokio::test]
async fn admin_destroy_with_live_documents_opens_resolution() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    w.document(dept, &name("invoice.pdf"), serde_json::json!({}), None)
        .await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("confirm_deletion")
    );
    assert!(!department_gone(w.db(), dept).await);
}

#[tokio::test]
async fn admin_destroy_with_all_documents_trashed_opens_resolution() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let doc = w
        .document(dept, &name("invoice.pdf"), serde_json::json!({}), None)
        .await;
    soft_delete_document(w.db(), doc).await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("confirm_deletion")
    );
    assert!(!department_gone(w.db(), dept).await);
}

#[tokio::test]
async fn admin_destroy_with_pending_upload_requests_opens_resolution() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    w.upload_request(dept, "pending.pdf", "pending", None).await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("confirm_deletion")
    );
    assert!(!department_gone(w.db(), dept).await);
}

#[tokio::test]
async fn admin_destroy_with_children_alerts_and_keeps_the_department() {
    let mut w = AdminWorld::new().await;
    let parent_name = name("parent");
    let parent = w.department(&parent_name, None).await;
    w.department(&name("child"), Some(parent)).await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{parent}"), &[])
        .await;
    assert!(matches!(resp.status, 302 | 303), "{resp:?}");
    let page = w.admin.page_after(&resp).await;
    assert_see(
        &page,
        &format!("Cannot delete {parent_name}: it has sub-departments. Delete or move them first."),
    );
    assert!(!department_gone(w.db(), parent).await);
}

#[tokio::test]
async fn admin_destroy_with_a_trashed_child_alerts_and_keeps_the_department() {
    let mut w = AdminWorld::new().await;
    let parent_name = name("parent");
    let parent = w.department(&parent_name, None).await;
    let child = w.department(&name("child"), Some(parent)).await;
    sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
        .bind(child)
        .execute(w.db())
        .await
        .unwrap();
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{parent}"), &[])
        .await;
    let page = w.admin.page_after(&resp).await;
    assert_see(
        &page,
        &format!("Cannot delete {parent_name}: it has sub-departments. Delete or move them first."),
    );
    assert!(!department_gone(w.db(), parent).await);
}

#[tokio::test]
async fn admin_destroy_on_an_empty_department_hard_deletes() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let resp = w
        .admin
        .submit(Method::DELETE, &format!("/admin/departments/{dept}"), &[])
        .await;
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Department deleted.");
    assert!(department_gone(w.db(), dept).await);
}

#[tokio::test]
async fn admin_move_documents_reassigns_live_and_trashed_docs_and_requests() {
    let mut w = AdminWorld::new().await;
    let source_name = name("source");
    let target_name = name("target");
    let source = w.department(&source_name, None).await;
    let target = w.department(&target_name, None).await;
    let live = w
        .document(source, &name("live.pdf"), serde_json::json!({}), None)
        .await;
    let trashed = w
        .document(source, &name("trashed.pdf"), serde_json::json!({}), None)
        .await;
    soft_delete_document(w.db(), trashed).await;
    let request = w
        .upload_request(source, "pending.pdf", "pending", None)
        .await;
    let target_s = target.to_string();
    let resp = w
        .admin
        .submit(
            Method::POST,
            &format!("/admin/departments/{source}/move_documents"),
            &[("target_department_id", &target_s)],
        )
        .await;
    let page = w.admin.page_after(&resp).await;
    assert_see(
        &page,
        &format!("Moved documents to {target_name} and deleted {source_name}."),
    );
    assert!(department_gone(w.db(), source).await);
    assert_eq!(doc_department(w.db(), live).await, target);
    assert_eq!(doc_department(w.db(), trashed).await, target);
    assert_eq!(request_department(w.db(), request).await, target);
}

#[tokio::test]
async fn admin_move_documents_rejects_the_source_as_target() {
    let mut w = AdminWorld::new().await;
    let source = w.department(&name("source"), None).await;
    let source_s = source.to_string();
    let resp = w
        .admin
        .submit(
            Method::POST,
            &format!("/admin/departments/{source}/move_documents"),
            &[("target_department_id", &source_s)],
        )
        .await;
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("confirm_deletion")
    );
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Choose a different destination department.");
    assert!(!department_gone(w.db(), source).await);
}

#[tokio::test]
async fn admin_move_documents_with_a_trashed_target_returns_404() {
    let mut w = AdminWorld::new().await;
    let source = w.department(&name("source"), None).await;
    let target = w.department(&name("target"), None).await;
    sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
        .bind(target)
        .execute(w.db())
        .await
        .unwrap();
    let target_s = target.to_string();
    let resp = w
        .admin
        .submit(
            Method::POST,
            &format!("/admin/departments/{source}/move_documents"),
            &[("target_department_id", &target_s)],
        )
        .await;
    assert_eq!(resp.status, 404, "{}", resp.text());
    assert_see(&resp, "doesn't exist");
}

#[tokio::test]
async fn admin_trash_then_restore_round_trips() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let doc1 = w
        .document(dept, &name("doc1.pdf"), serde_json::json!({}), None)
        .await;
    let doc2 = w
        .document(dept, &name("doc2.pdf"), serde_json::json!({}), None)
        .await;
    let trashed = w
        .admin
        .submit(
            Method::POST,
            &format!("/admin/departments/{dept}/trash"),
            &[],
        )
        .await;
    let page = w.admin.page_after(&trashed).await;
    assert_see(&page, &format!("{dept_name} moved to trash."));
    assert!(department_trashed(w.db(), dept).await);
    assert!(common::deleted_at_document(w.db(), doc1).await);
    assert!(common::deleted_at_document(w.db(), doc2).await);

    let restored = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/departments/{dept}/restore"),
            &[],
        )
        .await;
    let page = w.admin.page_after(&restored).await;
    assert_see(&page, &format!("{dept_name} restored."));
    assert!(!department_trashed(w.db(), dept).await);
    assert!(!common::deleted_at_document(w.db(), doc1).await);
    assert!(!common::deleted_at_document(w.db(), doc2).await);
}

#[tokio::test]
async fn admin_departments_index_renders_the_documents_count() {
    let mut w = AdminWorld::new().await;
    let finance = name("finance");
    let hr = name("hr");
    let dept1 = w.department(&finance, None).await;
    let dept2 = w.department(&hr, None).await;
    w.document(dept1, &name("f1.pdf"), serde_json::json!({}), None)
        .await;
    w.document(dept1, &name("f2.pdf"), serde_json::json!({}), None)
        .await;
    w.document(dept2, &name("h1.pdf"), serde_json::json!({}), None)
        .await;
    let page = w.admin.get("/admin/departments").await;
    assert_eq!(page.status, 200);
    assert_see(&page, "Documents");
    let body = page.text();
    let finance_row = tr_containing(&body, &finance).expect("finance row");
    let hr_row = tr_containing(&body, &hr).expect("hr row");
    assert_eq!(td_texts(finance_row).get(1).map(String::as_str), Some("2"));
    assert_eq!(td_texts(hr_row).get(1).map(String::as_str), Some("1"));
}

#[tokio::test]
async fn admin_departments_index_greys_a_trashed_department_and_offers_restore() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
        .bind(dept)
        .execute(w.db())
        .await
        .unwrap();
    let page = w.admin.get("/admin/departments").await;
    assert_eq!(page.status, 200);
    let body = page.text();
    let row = tr_containing(&body, &dept_name).expect("row");
    assert!(row.contains("class=\"trashed\""), "{row}");
    assert!(row.contains("Restore"), "{row}");
}

#[tokio::test]
async fn admin_departments_index_shows_an_empty_state_when_there_are_no_departments() {
    let w = AdminWorld::new().await;
    // The empty copy is the unfiltered index. AdminWorld holds SuiteLock, so
    // this reset is exclusive among admin tests and does not depend on a
    // freshly seeded stack. Other tests keep isolating with unique names.
    w.backend.reset().await.expect("reset business tables");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM departments")
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(n, 0, "reset left departments behind");
    let page = w.admin.get("/admin/departments").await;
    assert_eq!(page.status, 200);
    assert_see(&page, "No departments yet.");
}

#[tokio::test]
async fn admin_departments_index_lists_depth_first_with_tree_guides() {
    let mut w = AdminWorld::new().await;
    let parent_name = name("compta");
    let child_name = name("factures");
    let grand_name = name("y2026");
    let parent = w.department(&parent_name, None).await;
    let child = w.department(&child_name, Some(parent)).await;
    w.department(&grand_name, Some(child)).await;
    let page = w.admin.get("/admin/departments").await;
    assert_eq!(page.status, 200);
    let body = page.text();
    let i_parent = body.find(&parent_name).expect("parent");
    let i_child = body.find(&child_name).expect("child");
    let i_grand = body.find(&grand_name).expect("grandchild");
    assert!(i_parent < i_child && i_child < i_grand);
    assert_see(&page, "tree-guide");
}

#[tokio::test]
async fn admin_departments_index_renders_a_tree_toggle_for_a_parent_only() {
    let mut w = AdminWorld::new().await;
    let parent_name = name("compta");
    let child_name = name("factures");
    let parent = w.department(&parent_name, None).await;
    let child = w.department(&child_name, Some(parent)).await;
    let page = w.admin.get("/admin/departments").await;
    assert_eq!(page.status, 200);
    let body = page.text();
    let parent_row = tr_containing(&body, &format!("data-dept-id=\"{parent}\"")).unwrap();
    let child_row = tr_containing(&body, &format!("data-dept-id=\"{child}\"")).unwrap();
    assert!(parent_row.contains("data-tree-toggle"), "{parent_row}");
    assert!(!child_row.contains("data-tree-toggle"), "{child_row}");
    assert!(
        child_row.contains(&format!("data-parent-id=\"{parent}\"")),
        "{child_row}"
    );
}

async fn soft_delete_document(db: &sqlx::PgPool, id: Uuid) {
    sqlx::query(
        "UPDATE documents SET deleted_at = now(), purge_at = now() + interval '30 days' \
         WHERE id = $1",
    )
    .bind(id)
    .execute(db)
    .await
    .unwrap();
}

async fn row_exists_document(db: &sqlx::PgPool, id: Uuid) -> bool {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap();
    n == 1
}
