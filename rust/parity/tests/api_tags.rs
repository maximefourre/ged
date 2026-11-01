//! Port of `test/controllers/api/v1/document_tags_test.rb`.

mod common;

use common::World;
use parity::json::{assert_error, assert_json_content_type, assert_status};
use serde_json::json;

async fn tagged_document(w: &World) -> (i64, String) {
    let t = w.read_write_tree().await;
    let id =
        parity::fixtures::insert_document(w.backend.db(), t.finance, "report.pdf", "seed").await;
    parity::fixtures::set_document_tags(w.backend.db(), &id, &json!({"draft": null})).await;
    (t.hr, id)
}

#[tokio::test]
async fn api_put_replaces_the_whole_tag_map() {
    let w = World::new().await;
    let (_hr, id) = tagged_document(&w).await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{id}/tags"),
            &w.token,
            &[("tags[type]", "facture"), ("tags[annee]", "2026")],
        )
        .await;
    assert_status(&resp, 200);
    assert_json_content_type(&resp);
    assert_eq!(
        resp.json()["tags"],
        json!({"type": "facture", "annee": "2026"})
    );
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &id).await,
        json!({"type": "facture", "annee": "2026"})
    );
}

#[tokio::test]
async fn api_put_without_tags_clears_the_map() {
    let w = World::new().await;
    let (_hr, id) = tagged_document(&w).await;
    let resp = w
        .api
        .put_form(&format!("/api/v1/documents/{id}/tags"), &w.token, &[])
        .await;
    assert_status(&resp, 200);
    assert_eq!(resp.json()["tags"], json!({}));
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &id).await,
        json!({})
    );
}

#[tokio::test]
async fn api_tag_keys_and_values_are_normalized() {
    let w = World::new().await;
    let (_hr, id) = tagged_document(&w).await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{id}/tags"),
            &w.token,
            &[("tags[  Type ]", "  Facture  ")],
        )
        .await;
    assert_status(&resp, 200);
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &id).await,
        json!({"type": "Facture"})
    );
}

#[tokio::test]
async fn api_rejects_a_non_map_tags_param_with_422_and_leaves_tags_untouched() {
    let w = World::new().await;
    let (_hr, id) = tagged_document(&w).await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{id}/tags"),
            &w.token,
            &[("tags", "flat")],
        )
        .await;
    assert_error(&resp, 422, "Tags must be a map of key/value pairs");
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &id).await,
        json!({"draft": null})
    );
}

#[tokio::test]
async fn api_rejects_nested_tag_values_with_422() {
    let w = World::new().await;
    let (_hr, id) = tagged_document(&w).await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{id}/tags"),
            &w.token,
            &[("tags[type][nested]", "x")],
        )
        .await;
    assert_error(
        &resp,
        422,
        "Tags values must be strings of at most 500 characters, or null",
    );
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &id).await,
        json!({"draft": null})
    );
}

#[tokio::test]
async fn api_tag_replace_is_forbidden_outside_granted_departments() {
    let w = World::new().await;
    let (hr, _id) = tagged_document(&w).await;
    let other = parity::fixtures::insert_document(w.backend.db(), hr, "other.pdf", "seed").await;
    let resp = w
        .api
        .put_form(
            &format!("/api/v1/documents/{other}/tags"),
            &w.token,
            &[("tags[type]", "facture")],
        )
        .await;
    assert_error(&resp, 403, "no write access to this department");
}

#[tokio::test]
async fn api_the_tag_registry_endpoints_are_gone() {
    let w = World::new().await;
    let resp = w.api.get_bearer("/api/v1/tags", &w.token).await;
    // Ruby only asserted 404. `JsonApiExceptions` renders Rack's status name
    // for an unrouted `/api/v1` path (`Not Found` → `not found`). The body is
    // part of the contract and is checked against the Rails stack.
    assert_error(&resp, 404, "not found");
}
