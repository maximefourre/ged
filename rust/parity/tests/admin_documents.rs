//! HTTP ports of the document journeys in `test/controllers/admin`.

mod common;

use common::{
    AdminWorld, admin_document_id as document_id, admin_upload as upload, assert_absent,
    assert_login_redirect, assert_see, deleted_at_document, name, owner, pdf_bytes, tags_of,
    zip_entries,
};
use parity::admin::AdminSession;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn admin_manages_a_documents_tags_through_the_textarea() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    let created = upload(&w.admin, dept, &filename, b"PDF!", "application/pdf", None).await;
    assert!(matches!(created.status, 302 | 303), "{}", created.text());
    let id = document_id(w.db(), dept, &filename).await;

    let edit = w.admin.get(&format!("/admin/documents/{id}/edit")).await;
    assert_eq!(edit.status, 200);
    assert_see(&edit, "Tags");

    let updated = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/documents/{id}"),
            &[("document[tags_text]", "Type=Facture\nurgent")],
        )
        .await;
    assert!(matches!(updated.status, 302 | 303), "{}", updated.text());
    let page = w.admin.page_after(&updated).await;
    assert_see(&page, "Tags updated.");
    assert_eq!(
        tags_of(w.db(), id).await,
        json!({"type": "Facture", "urgent": null})
    );

    let cleared = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/documents/{id}"),
            &[("document[tags_text]", "")],
        )
        .await;
    assert!(matches!(cleared.status, 302 | 303));
    assert_eq!(tags_of(w.db(), id).await, json!({}));

    let long_key = "k".repeat(101);
    let invalid_text = format!("{long_key}=x");
    let rejected = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/documents/{id}"),
            &[("document[tags_text]", &invalid_text)],
        )
        .await;
    assert!(matches!(rejected.status, 302 | 303), "{}", rejected.text());
    assert!(
        rejected
            .header("location")
            .unwrap_or("")
            .contains(&format!("/admin/documents/{id}/edit")),
        "{:?}",
        rejected.header("location")
    );
    let form = w.admin.page_after(&rejected).await;
    assert_see(&form, "at most 100 characters");
    assert_eq!(tags_of(w.db(), id).await, json!({}));
}

#[tokio::test]
async fn admin_tags_edit_page_shows_readonly_document_metadata() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let id = w
        .document(dept, &name("report.pdf"), json!({}), Some(4))
        .await;
    let page = w.admin.get(&format!("/admin/documents/{id}/edit")).await;
    assert_eq!(page.status, 200);
    assert_see(&page, "Upload mode");
    assert_see(&page, "sync");
    assert_see(&page, "Indexed");
}

#[tokio::test]
async fn admin_filters_documents_by_tag_key() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("tagged.pdf");
    w.document(dept, &filename, json!({"type": "facture"}), Some(4))
        .await;
    let matched = w
        .admin
        .get(&format!("/admin/documents?department_id={dept}&tag=type"))
        .await;
    assert_eq!(matched.status, 200);
    assert_see(&matched, &filename);
    assert_see(&matched, "facture");

    let absent = w
        .admin
        .get(&format!("/admin/documents?department_id={dept}&tag=absent"))
        .await;
    assert_eq!(absent.status, 200);
    assert_absent(&absent, &filename);
}

#[tokio::test]
async fn admin_uploads_a_document_then_soft_deletes_it_to_the_trash() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    let created = upload(
        &w.admin,
        dept,
        &filename,
        b"%PDF-1.1\n",
        "application/pdf",
        None,
    )
    .await;
    assert!(matches!(created.status, 302 | 303), "{}", created.text());
    let page = w.admin.page_after(&created).await;
    assert_see(&page, "Document uploaded.");
    assert_see(&page, &filename);
    let id = document_id(w.db(), dept, &filename).await;
    let uploaded_by: String = sqlx::query_scalar("SELECT uploaded_by FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(uploaded_by, owner());

    let deleted = w
        .admin
        .submit(Method::DELETE, &format!("/admin/documents/{id}"), &[])
        .await;
    let page = w.admin.page_after(&deleted).await;
    assert_see(&page, "Document moved to trash.");
    assert_absent(&page, &filename);
    assert!(deleted_at_document(w.db(), id).await);
    let still: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(still, 1);
}

#[tokio::test]
async fn admin_uploads_a_document_with_tags() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("tagged.pdf");
    let created = upload(
        &w.admin,
        dept,
        &filename,
        b"PDF!",
        "application/pdf",
        Some("type=facture\nurgent"),
    )
    .await;
    assert!(matches!(created.status, 302 | 303), "{}", created.text());
    let page = w.admin.page_after(&created).await;
    assert_see(&page, "Document uploaded.");
    let id = document_id(w.db(), dept, &filename).await;
    assert_eq!(
        tags_of(w.db(), id).await,
        json!({"type": "facture", "urgent": null})
    );
}

#[tokio::test]
async fn admin_uploading_a_non_file_param_shows_an_alert() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let dept_s = dept.to_string();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE department_id = $1")
        .bind(dept)
        .fetch_one(w.db())
        .await
        .unwrap();
    let resp = w
        .admin
        .submit(
            Method::POST,
            "/admin/documents",
            &[
                ("document[department_id]", &dept_s),
                ("document[file]", "hello"),
            ],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    assert!(
        resp.header("location")
            .unwrap_or("")
            .contains("/admin/documents/new"),
        "{:?}",
        resp.header("location")
    );
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "File must be a file upload.");
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE department_id = $1")
        .bind(dept)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn admin_downloads_a_document() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    let bytes = b"PDF!";
    let created = upload(&w.admin, dept, &filename, bytes, "application/pdf", None).await;
    assert!(matches!(created.status, 302 | 303), "{}", created.text());
    let id = document_id(w.db(), dept, &filename).await;
    let resp = w
        .admin
        .get(&format!("/admin/documents/{id}/download"))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.body, bytes);
}

#[tokio::test]
async fn admin_bulk_destroy_soft_deletes_the_selected_documents() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let d1 = w.document(dept, &name("one.pdf"), json!({}), None).await;
    let d2 = w.document(dept, &name("two.pdf"), json!({}), None).await;
    let a = d1.to_string();
    let b = d2.to_string();
    let resp = w
        .admin
        .submit(
            Method::POST,
            "/admin/documents/bulk_destroy",
            &[("document_ids[]", &a), ("document_ids[]", &b)],
        )
        .await;
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "2 documents moved to trash.");
    assert!(deleted_at_document(w.db(), d1).await);
    assert!(deleted_at_document(w.db(), d2).await);
}

#[tokio::test]
async fn admin_bulk_destroy_with_an_empty_selection_deletes_nothing() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("kept.pdf");
    let id = w.document(dept, &filename, json!({}), None).await;
    let resp = w
        .admin
        .submit(Method::POST, "/admin/documents/bulk_destroy", &[])
        .await;
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "No documents selected.");
    assert!(!deleted_at_document(w.db(), id).await);
}

#[tokio::test]
async fn admin_bulk_download_streams_a_zip_with_id_prefixed_names() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let first = name("alpha.pdf");
    let second = name("beta.pdf");
    upload(&w.admin, dept, &first, b"AAAA", "application/pdf", None).await;
    upload(&w.admin, dept, &second, b"BBBB", "application/pdf", None).await;
    let d1 = document_id(w.db(), dept, &first).await;
    let d2 = document_id(w.db(), dept, &second).await;
    let resp = w
        .admin
        .get(&format!(
            "/admin/documents/bulk_download?document_ids%5B%5D={d1}&document_ids%5B%5D={d2}"
        ))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let ct = resp.header("content-type").unwrap_or("");
    assert!(ct.starts_with("application/zip"), "{ct}");
    let entries = zip_entries(&resp.body);
    let map: std::collections::HashMap<_, _> = entries.into_iter().collect();
    assert_eq!(
        map.get(&format!("{d1}-{first}")).map(Vec::as_slice),
        Some(b"AAAA".as_slice())
    );
    assert_eq!(
        map.get(&format!("{d2}-{second}")).map(Vec::as_slice),
        Some(b"BBBB".as_slice())
    );
}

#[tokio::test]
async fn admin_bulk_download_dedupes_same_filenames_by_id_prefix() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    upload(&w.admin, dept, &filename, b"AAAA", "application/pdf", None).await;
    let d1 = document_id(w.db(), dept, &filename).await;
    upload(&w.admin, dept, &filename, b"BBBB", "application/pdf", None).await;
    let d2: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM documents WHERE department_id = $1 AND filename = $2 AND id <> $3 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(dept)
    .bind(&filename)
    .bind(d1)
    .fetch_one(w.db())
    .await
    .unwrap();
    let resp = w
        .admin
        .get(&format!(
            "/admin/documents/bulk_download?document_ids%5B%5D={d1}&document_ids%5B%5D={d2}"
        ))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let entries = zip_entries(&resp.body);
    assert_eq!(entries.len(), 2);
    let map: std::collections::HashMap<_, _> = entries.into_iter().collect();
    assert_eq!(
        map.get(&format!("{d1}-{filename}")).map(Vec::as_slice),
        Some(b"AAAA".as_slice())
    );
    assert_eq!(
        map.get(&format!("{d2}-{filename}")).map(Vec::as_slice),
        Some(b"BBBB".as_slice())
    );
}

#[tokio::test]
async fn admin_preview_streams_a_pdf_inline_with_a_viewer_compatible_csp() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    let bytes = pdf_bytes();
    upload(&w.admin, dept, &filename, bytes, "application/pdf", None).await;
    let id = document_id(w.db(), dept, &filename).await;
    let resp = w.admin.get(&format!("/admin/documents/{id}/preview")).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let disposition = resp.header("content-disposition").unwrap_or("");
    assert!(disposition.starts_with("inline"), "{disposition}");
    assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
    let csp = resp.header("content-security-policy").unwrap_or("");
    assert!(csp.contains("frame-ancestors 'self'"), "{csp}");
    assert!(!csp.contains("sandbox"), "{csp}");
    assert_eq!(resp.body, bytes);
}

#[tokio::test]
async fn admin_preview_hardens_non_pdf_types_with_a_scriptless_sandbox_csp() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("notes"), None).await;
    let filename = name("note.txt");
    let bytes = b"hello";
    upload(&w.admin, dept, &filename, bytes, "text/plain", None).await;
    let id = document_id(w.db(), dept, &filename).await;
    let stored: String = sqlx::query_scalar("SELECT content_type FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(
        stored, "text/plain",
        "the stored content type differs from the uploaded one"
    );
    let resp = w.admin.get(&format!("/admin/documents/{id}/preview")).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let csp = resp.header("content-security-policy").unwrap_or("");
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert!(csp.contains("sandbox"), "{csp}");
    assert!(!csp.contains("allow-scripts"), "{csp}");
}

#[tokio::test]
async fn admin_preview_redirects_to_login_when_signed_out() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let id = w
        .document(dept, &name("report.pdf"), json!({}), Some(4))
        .await;
    let anon = AdminSession::anonymous(w.backend.base_url());
    let resp = anon.get(&format!("/admin/documents/{id}/preview")).await;
    assert_login_redirect(&resp);
}

#[tokio::test]
async fn admin_download_still_streams_as_an_attachment() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let filename = name("report.pdf");
    let bytes = pdf_bytes();
    upload(&w.admin, dept, &filename, bytes, "application/pdf", None).await;
    let id = document_id(w.db(), dept, &filename).await;
    let resp = w
        .admin
        .get(&format!("/admin/documents/{id}/download"))
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let disposition = resp.header("content-disposition").unwrap_or("");
    assert!(disposition.starts_with("attachment"), "{disposition}");
    assert_eq!(resp.body, bytes);
}
