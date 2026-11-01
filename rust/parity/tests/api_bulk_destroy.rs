//! Port of `test/controllers/api/v1/documents_bulk_destroy_test.rb`.

mod common;

use common::World;
use parity::json::{assert_error, assert_json_content_type, assert_json_eq, assert_status};

async fn destroy(w: &World, ids: &[String]) -> parity::Resp {
    let fields: Vec<(&str, &str)> = ids
        .iter()
        .map(|id| ("document_ids[]", id.as_str()))
        .collect();
    w.api
        .delete_form("/api/v1/documents/bulk_destroy", &w.token, &fields)
        .await
}

#[tokio::test]
async fn api_authorized_batch_across_a_department_and_its_subtree_soft_deletes_both() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let db = w.backend.db();
    let one = parity::fixtures::insert_document(db, t.finance, "one.pdf", "seed").await;
    let two = parity::fixtures::insert_document(db, t.invoices, "two.pdf", "seed").await;
    let resp = destroy(&w, &[one.clone(), two.clone()]).await;
    assert_status(&resp, 200);
    assert_json_content_type(&resp);
    assert_json_eq(&resp, &serde_json::json!({"deleted": 2}));
    assert!(!parity::fixtures::document_is_live(db, &one).await);
    assert!(!parity::fixtures::document_is_live(db, &two).await);
    assert!(parity::fixtures::document_retained_for_30_days(db, &one).await);
    assert!(parity::fixtures::document_retained_for_30_days(db, &two).await);
}

#[tokio::test]
async fn api_a_document_outside_write_access_forbids_the_whole_batch() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let db = w.backend.db();
    let granted = parity::fixtures::insert_document(db, t.finance, "granted.pdf", "seed").await;
    let forbidden = parity::fixtures::insert_document(db, t.hr, "forbidden.pdf", "seed").await;
    let resp = destroy(&w, &[granted.clone(), forbidden.clone()]).await;
    assert_error(&resp, 403, "no write access to one or more documents");
    assert!(parity::fixtures::document_is_live(db, &granted).await);
    assert!(parity::fixtures::document_is_live(db, &forbidden).await);
}

#[tokio::test]
async fn api_an_unknown_id_mixed_with_a_valid_id_returns_404_and_deletes_nothing() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let db = w.backend.db();
    let valid = parity::fixtures::insert_document(db, t.finance, "valid.pdf", "seed").await;
    let unknown = parity::fixtures::random_uuid();
    let resp = destroy(&w, &[valid.clone(), unknown]).await;
    assert_error(&resp, 404, "not found");
    assert!(parity::fixtures::document_is_live(db, &valid).await);
}

#[tokio::test]
async fn api_an_empty_document_ids_array_returns_400() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let resp = w
        .api
        .delete_form(
            "/api/v1/documents/bulk_destroy",
            &w.token,
            &[("document_ids[]", "")],
        )
        .await;
    assert_error(&resp, 400, "document_ids is required");
}

#[tokio::test]
async fn api_a_missing_document_ids_param_returns_400() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let resp = w
        .api
        .delete_form("/api/v1/documents/bulk_destroy", &w.token, &[])
        .await;
    assert_error(&resp, 400, "document_ids is required");
}

#[tokio::test]
async fn api_a_read_only_client_is_forbidden_from_bulk_destroying() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let reader = w.extra_client("reader-app").await;
    parity::fixtures::grant(w.backend.db(), reader.id, t.finance, true, false).await;
    let doc =
        parity::fixtures::insert_document(w.backend.db(), t.finance, "readonly.pdf", "seed").await;
    let resp = w
        .api
        .delete_form(
            "/api/v1/documents/bulk_destroy",
            &reader.token,
            &[("document_ids[]", doc.as_str())],
        )
        .await;
    assert_error(&resp, 403, "no write access to one or more documents");
    assert!(parity::fixtures::document_is_live(w.backend.db(), &doc).await);
}

#[tokio::test]
async fn api_more_than_500_document_ids_returns_400_before_any_existence_check() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let ids: Vec<String> = (0..501).map(|_| parity::fixtures::random_uuid()).collect();
    let resp = destroy(&w, &ids).await;
    assert_error(&resp, 400, "too many document_ids (max 500)");
}
