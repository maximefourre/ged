//! HTTP ports of `test/controllers/admin/deleted_documents_controller_test.rb`.

mod common;

use common::{
    AdminWorld, assert_absent, assert_login_redirect, assert_see, deleted_at_document,
    department_trashed, name,
};
use parity::admin::AdminSession;
use reqwest::Method;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn admin_trash_index_lists_soft_deleted_documents_and_not_live_ones() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let gone = name("gone.pdf");
    let live = name("live.pdf");
    let gone_id = w.document(dept, &gone, json!({}), Some(4)).await;
    w.document(dept, &live, json!({}), Some(4)).await;
    soft_delete_document(w.db(), gone_id).await;

    let page = w.admin.get("/admin/deleted_documents").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, "Trash");
    assert_see(&page, &gone);
    assert_absent(&page, &live);
}

#[tokio::test]
async fn admin_trash_restore_brings_a_soft_deleted_document_back() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("gone.pdf");
    let id = w.document(dept, &filename, json!({}), Some(4)).await;
    soft_delete_document(w.db(), id).await;

    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/deleted_documents/{id}/restore"),
            &[],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    let loc = resp.header("location").unwrap_or("");
    assert!(
        loc.contains("/admin/deleted_documents"),
        "restore redirected to {loc}"
    );
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Document restored.");
    assert!(
        !deleted_at_document(w.db(), id).await,
        "document still trashed"
    );

    let index = w.admin.get("/admin/documents").await;
    assert_eq!(index.status, 200, "{}", index.text());
    assert_see(&index, &filename);
}

#[tokio::test]
async fn admin_trash_requires_an_admin_session() {
    let w = AdminWorld::new().await;
    let anon = AdminSession::anonymous(w.backend.base_url());
    let resp = anon.get("/admin/deleted_documents").await;
    assert_login_redirect(&resp);
}

#[tokio::test]
async fn admin_single_doc_restore_ungreys_its_department_and_leaves_siblings_trashed() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let id1 = w
        .document(dept, &name("sibling1.pdf"), json!({}), Some(4))
        .await;
    let id2 = w
        .document(dept, &name("sibling2.pdf"), json!({}), Some(4))
        .await;
    soft_delete_department_and_live_docs(w.db(), dept).await;
    assert!(deleted_at_document(w.db(), id1).await);
    assert!(deleted_at_document(w.db(), id2).await);
    assert!(department_trashed(w.db(), dept).await);

    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/deleted_documents/{id1}/restore"),
            &[],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("/admin/deleted_documents"),
        "restore redirected to {:?}",
        resp.header("location")
    );
    assert!(
        !deleted_at_document(w.db(), id1).await,
        "restored doc still trashed"
    );
    assert!(
        deleted_at_document(w.db(), id2).await,
        "sibling was restored too"
    );
    assert!(
        !department_trashed(w.db(), dept).await,
        "department stayed grey"
    );
}

#[tokio::test]
async fn admin_trash_page_renders_for_a_doc_whose_department_is_trashed() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let filename = name("gone.pdf");
    let id = w.document(dept, &filename, json!({}), Some(4)).await;
    soft_delete_document(w.db(), id).await;
    // The document is already trashed, so only the department goes grey.
    sqlx::query("UPDATE departments SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(dept)
        .execute(w.db())
        .await
        .expect("trash department");

    let page = w.admin.get("/admin/deleted_documents").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, &filename);
    assert_see(&page, &dept_name);
    assert_see(&page, "(trashed)");
}

async fn soft_delete_document(db: &sqlx::PgPool, id: Uuid) {
    sqlx::query(
        "UPDATE documents SET deleted_at = now(), purge_at = now() + interval '30 days', \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(db)
    .await
    .expect("soft-delete document");
}

async fn soft_delete_department_and_live_docs(db: &sqlx::PgPool, id: i64) {
    sqlx::query("UPDATE departments SET deleted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .expect("soft-delete department");
    sqlx::query(
        "UPDATE documents SET deleted_at = now(), purge_at = now() + interval '30 days', \
         updated_at = now() WHERE department_id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(db)
    .await
    .expect("trash live documents");
}
