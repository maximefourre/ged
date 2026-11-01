//! Port of `test/controllers/api/v1/read_write_permissions_test.rb`.
//!
//! Read and write are independent. The document the reader downloads is
//! created over HTTP by a client that holds both flags, so the bytes exist
//! without this suite writing storage tables.

mod common;

use common::{PDF, World, documents_of, filenames, upload};
use parity::json::{assert_error, assert_status};
use serde_json::json;

struct Matrix {
    finance: i64,
    doc_id: String,
    reader_token: String,
    writer_token: String,
}

async fn matrix(w: &World) -> Matrix {
    let t = w.read_write_tree().await;
    let reader = w.extra_client("reader-app").await;
    let writer = w.extra_client("writer-app").await;
    let db = w.backend.db();
    parity::fixtures::grant(db, reader.id, t.finance, true, false).await;
    parity::fixtures::grant(db, writer.id, t.finance, false, true).await;
    let created = upload(&w.api, &w.token, t.finance, "report.pdf", &[]).await;
    assert_status(&created, 201);
    Matrix {
        finance: t.finance,
        doc_id: created.json()["id"].as_str().expect("id").to_owned(),
        reader_token: reader.token,
        writer_token: writer.token,
    }
}

#[tokio::test]
async fn api_read_only_client_can_list_show_and_download() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let list = w.api.get_bearer("/api/v1/documents", &m.reader_token).await;
    assert_status(&list, 200);
    assert_eq!(filenames(&documents_of(&list)), vec!["report.pdf"]);

    let show = w
        .api
        .get_bearer(&format!("/api/v1/documents/{}", m.doc_id), &m.reader_token)
        .await;
    assert_status(&show, 200);

    let download = w
        .api
        .get_bearer(
            &format!("/api/v1/documents/{}/download", m.doc_id),
            &m.reader_token,
        )
        .await;
    assert_status(&download, 200);
    assert_eq!(download.body, PDF);
}

#[tokio::test]
async fn api_read_only_client_cannot_upload() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = upload(&w.api, &m.reader_token, m.finance, "report.pdf", &[]).await;
    assert_error(&resp, 403, "no write access to this department");
}

#[tokio::test]
async fn api_read_only_client_cannot_delete() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = w
        .api
        .delete_bearer(&format!("/api/v1/documents/{}", m.doc_id), &m.reader_token)
        .await;
    assert_error(&resp, 403, "no write access to this department");
    assert!(parity::fixtures::document_is_live(w.backend.db(), &m.doc_id).await);
}

#[tokio::test]
async fn api_read_only_client_cannot_replace_tags() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{}/tags", m.doc_id),
            &m.reader_token,
            &[("tags[type]", "facture")],
        )
        .await;
    assert_error(&resp, 403, "no write access to this department");
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &m.doc_id).await,
        json!({})
    );
}

#[tokio::test]
async fn api_read_only_client_cannot_create_a_sub_department() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let parent = m.finance.to_string();
    let resp = w
        .api
        .post_form(
            "/api/v1/departments",
            &m.reader_token,
            &[("parent_id", parent.as_str()), ("name", "Archives")],
        )
        .await;
    assert_error(&resp, 403, "no write access to this department");
}

#[tokio::test]
async fn api_write_only_client_can_upload_and_gets_the_created_document_back() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = upload(&w.api, &m.writer_token, m.finance, "report.pdf", &[]).await;
    assert_status(&resp, 201);
    assert_eq!(resp.json()["filename"], json!("report.pdf"));
}

#[tokio::test]
async fn api_write_only_client_cannot_list_show_or_download() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let list = w.api.get_bearer("/api/v1/documents", &m.writer_token).await;
    assert_status(&list, 200);
    assert!(documents_of(&list).is_empty());

    let show = w
        .api
        .get_bearer(&format!("/api/v1/documents/{}", m.doc_id), &m.writer_token)
        .await;
    assert_error(&show, 403, "no read access to this department");

    let download = w
        .api
        .get_bearer(
            &format!("/api/v1/documents/{}/download", m.doc_id),
            &m.writer_token,
        )
        .await;
    assert_error(&download, 403, "no read access to this department");
}

#[tokio::test]
async fn api_write_only_client_cannot_filter_the_list_by_department() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = w
        .api
        .get_bearer(
            &format!("/api/v1/documents?department_id={}", m.finance),
            &m.writer_token,
        )
        .await;
    assert_error(&resp, 403, "no read access to this department");
}

#[tokio::test]
async fn api_write_only_client_sees_no_departments_in_the_read_scoped_index() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = w
        .api
        .get_bearer("/api/v1/departments", &m.writer_token)
        .await;
    assert_status(&resp, 200);
    assert_eq!(resp.json(), json!([]));
}

#[tokio::test]
async fn api_write_only_client_can_create_a_sub_department_and_gets_it_back() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let parent = m.finance.to_string();
    let name = parity::fixtures::unique("archives");
    let resp = w
        .api
        .post_form(
            "/api/v1/departments",
            &m.writer_token,
            &[("parent_id", parent.as_str()), ("name", name.as_str())],
        )
        .await;
    assert_status(&resp, 201);
    let body = resp.json();
    assert_eq!(body["name"], json!(name));
    assert_eq!(body["parent_id"], json!(m.finance));
}

#[tokio::test]
async fn api_write_only_client_can_delete() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = w
        .api
        .delete_bearer(&format!("/api/v1/documents/{}", m.doc_id), &m.writer_token)
        .await;
    assert_status(&resp, 204);
    assert!(!parity::fixtures::document_is_live(w.backend.db(), &m.doc_id).await);
}

#[tokio::test]
async fn api_write_only_client_can_replace_tags() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{}/tags", m.doc_id),
            &m.writer_token,
            &[("tags[type]", "facture")],
        )
        .await;
    assert_status(&resp, 200);
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &m.doc_id).await,
        json!({"type": "facture"})
    );
}

#[tokio::test]
async fn api_write_only_client_cannot_poll_an_upload_request() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let request_id = parity::fixtures::insert_upload_request(
        w.backend.db(),
        m.finance,
        "https://example.com/hook",
        "report.pdf",
        "writer-app",
    )
    .await;
    let denied = w
        .api
        .get_bearer(
            &format!("/api/v1/upload_requests/{request_id}"),
            &m.writer_token,
        )
        .await;
    assert_error(&denied, 403, "no read access to this department");
    let allowed = w
        .api
        .get_bearer(
            &format!("/api/v1/upload_requests/{request_id}"),
            &m.reader_token,
        )
        .await;
    assert_status(&allowed, 200);
    assert_eq!(allowed.json()["request_id"], json!(request_id));
    assert_eq!(allowed.json()["status"], json!("pending"));
}

#[tokio::test]
async fn api_destroy_requires_write_even_though_it_shares_the_finder_with_show() {
    let w = World::new().await;
    let m = matrix(&w).await;
    let deleted = w
        .api
        .delete_bearer(&format!("/api/v1/documents/{}", m.doc_id), &m.reader_token)
        .await;
    assert_error(&deleted, 403, "no write access to this department");
    let show = w
        .api
        .get_bearer(&format!("/api/v1/documents/{}", m.doc_id), &m.reader_token)
        .await;
    assert_status(&show, 200);
}
