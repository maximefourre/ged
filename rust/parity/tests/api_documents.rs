//! Port of `test/controllers/api/v1/documents_test.rb`.
//!
//! Documents that must stream bytes are created through HTTP. Rows that only
//! exist so a list or an authz check can see them are inserted in `documents`.

mod common;

use common::{
    PDF, Tree, World, assert_document_keys, assert_rails_time, document_id, documents_of,
    filenames, pagination_of, query, upload,
};
use parity::http::FilePart;
use parity::json::{assert_error, assert_json_content_type, assert_json_includes, assert_status};
use serde_json::{Value, json};

async fn tree(w: &World) -> Tree {
    w.read_write_tree().await
}

fn dept(id: i64) -> String {
    id.to_string()
}

#[tokio::test]
async fn api_sync_upload_returns_201_with_the_document_uuid() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(&w.api, &w.token, t.finance, "report.pdf", &[]).await;
    assert_status(&resp, 201);
    assert_json_content_type(&resp);
    let body = resp.json();
    assert_document_keys(&body);
    let id = body["id"].as_str().expect("id");
    assert_eq!(id.len(), 36);
    assert!(id.contains('-'), "{id}");
    assert_json_includes(
        &body,
        &json!({
            "filename": "report.pdf",
            "content_type": "application/pdf",
            "byte_size": 4,
            "department_id": t.finance,
            "uploaded_by": w.client.client_id,
            "tags": {}
        }),
    );
    let got = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}/download"), &w.token)
        .await;
    assert_eq!(got.body, PDF, "uploaded bytes were not stored");
}

#[tokio::test]
async fn api_documents_expose_a_readonly_metadata_block() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(&w.api, &w.token, t.finance, "report.pdf", &[]).await;
    assert_status(&resp, 201);
    let body = resp.json();
    let meta = &body["metadata"];
    let mut keys: Vec<&str> = meta
        .as_object()
        .expect("metadata")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["updated_at", "upload_mode", "uploaded_at"]);
    assert_eq!(meta["upload_mode"], json!("sync"));
    assert_rails_time(&meta["uploaded_at"]);
    assert_rails_time(&meta["updated_at"]);
    assert_rails_time(&body["created_at"]);
    assert!(meta.get("indexed_at").is_none());
}

#[tokio::test]
async fn api_upload_into_a_subfolder_of_a_granted_department_is_allowed() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(&w.api, &w.token, t.invoices, "report.pdf", &[]).await;
    assert_status(&resp, 201);
    assert_eq!(resp.json()["department_id"], json!(t.invoices));
}

#[tokio::test]
async fn api_sync_upload_accepts_tags_and_returns_them_in_the_map() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(
        &w.api,
        &w.token,
        t.finance,
        "report.pdf",
        &[("tags[Type]", " Facture "), ("tags[urgent]", "")],
    )
    .await;
    assert_status(&resp, 201);
    let body = resp.json();
    assert_eq!(body["tags"], json!({"type": "Facture", "urgent": null}));
    let id = document_id(&resp);
    assert_eq!(
        parity::fixtures::document_tags(w.backend.db(), &id).await,
        json!({"type": "Facture", "urgent": null})
    );
}

#[tokio::test]
async fn api_sync_upload_with_a_non_map_tags_param_returns_422() {
    let w = World::new().await;
    let t = tree(&w).await;
    let id = dept(t.finance);
    let resp = w
        .api
        .post_multipart(
            "/api/v1/documents",
            &w.token,
            &[("department_id", id.as_str()), ("tags", "flat")],
            Some(pdf_part()),
        )
        .await;
    assert_error(&resp, 422, "Tags must be a map of key/value pairs");
    assert_eq!(
        parity::fixtures::count_documents(w.backend.db(), t.finance).await,
        0
    );
}

#[tokio::test]
async fn api_upload_with_a_non_file_file_param_returns_400() {
    let w = World::new().await;
    let t = tree(&w).await;
    let id = dept(t.finance);
    let resp = w
        .api
        .post_form(
            "/api/v1/documents",
            &w.token,
            &[("department_id", id.as_str()), ("file", "hello")],
        )
        .await;
    assert_error(&resp, 400, "file must be a multipart file upload");
    assert_eq!(
        parity::fixtures::count_documents(w.backend.db(), t.finance).await,
        0
    );
}

#[tokio::test]
async fn api_upload_into_a_forbidden_department_returns_403() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(&w.api, &w.token, t.hr, "report.pdf", &[]).await;
    assert_error(&resp, 403, "no write access to this department");
    assert_eq!(
        parity::fixtures::count_documents(w.backend.db(), t.hr).await,
        0
    );
}

#[tokio::test]
async fn api_upload_without_file_returns_400() {
    let w = World::new().await;
    let t = tree(&w).await;
    let id = dept(t.finance);
    let resp = w
        .api
        .post_form(
            "/api/v1/documents",
            &w.token,
            &[("department_id", id.as_str())],
        )
        .await;
    assert_error(
        &resp,
        400,
        "param is missing or the value is empty or invalid: file",
    );
}

#[tokio::test]
async fn api_list_returns_only_accessible_documents_with_filters() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(
        &w.api,
        &w.token,
        t.invoices,
        "facture-2026.pdf",
        &[("tags[invoice]", "2026")],
    )
    .await;
    assert_status(&resp, 201);
    parity::fixtures::insert_document(w.backend.db(), t.hr, "salaries.pdf", "other").await;

    let listed = w.api.get_bearer("/api/v1/documents", &w.token).await;
    assert_status(&listed, 200);
    assert_eq!(filenames(&documents_of(&listed)), vec!["facture-2026.pdf"]);

    let by_name = get_docs(&w, &query(&[("filename", "facture")])).await;
    assert_eq!(filenames(&by_name), vec!["facture-2026.pdf"]);

    let by_tag = get_docs(&w, &query(&[("tag", "invoice")])).await;
    assert_eq!(by_tag.len(), 1);

    let missing = get_docs(&w, &query(&[("tag", "nope")])).await;
    assert!(missing.is_empty());
}

#[tokio::test]
async fn api_documents_carry_tags_and_tag_value_filters_on_the_pair() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(
        &w.api,
        &w.token,
        t.invoices,
        "facture-2026.pdf",
        &[("tags[type]", "facture"), ("tags[urgent]", "")],
    )
    .await;
    assert_status(&resp, 201);

    let matched = get_docs(&w, &query(&[("tag", "type"), ("tag_value", "facture")])).await;
    assert_eq!(matched.len(), 1);
    assert_eq!(
        matched[0]["tags"],
        json!({"type": "facture", "urgent": null})
    );

    let normalized = get_docs(&w, &query(&[("tag", " Type "), ("tag_value", " facture ")])).await;
    assert_eq!(normalized.len(), 1);

    let other = get_docs(&w, &query(&[("tag", "type"), ("tag_value", "devis")])).await;
    assert!(other.is_empty());
}

#[tokio::test]
async fn api_tag_value_without_tag_returns_400() {
    let w = World::new().await;
    let _t = tree(&w).await;
    let resp = w
        .api
        .get_bearer(
            &format!("/api/v1/documents?{}", query(&[("tag_value", "facture")])),
            &w.token,
        )
        .await;
    assert_error(&resp, 400, "tag_value requires tag");
}

#[tokio::test]
async fn api_list_paginates_with_limit_and_offset() {
    let w = World::new().await;
    let t = tree(&w).await;
    for i in 0..3 {
        let resp = upload(&w.api, &w.token, t.finance, &format!("doc-{i}.pdf"), &[]).await;
        assert_status(&resp, 201);
    }

    let page = get_ok(&w, &query(&[("limit", "2")])).await;
    assert_eq!(documents_of(&page).len(), 2);
    assert_eq!(
        pagination_of(&page),
        json!({"limit": 2, "offset": 0, "total": 3})
    );

    let next = get_docs(&w, &query(&[("limit", "2"), ("offset", "2")])).await;
    assert_eq!(next.len(), 1);

    for bad in ["abc", "0", "-5"] {
        let resp = get_ok(&w, &query(&[("limit", bad), ("offset", "junk")])).await;
        assert_eq!(documents_of(&resp).len(), 3, "limit={bad}");
        assert_eq!(
            pagination_of(&resp),
            json!({"limit": 100, "offset": 0, "total": 3})
        );
    }

    let capped = get_ok(&w, &query(&[("limit", "999999")])).await;
    assert_status(&capped, 200);
    assert_eq!(documents_of(&capped).len(), 3);
    assert_eq!(pagination_of(&capped)["limit"], json!(500));
}

#[tokio::test]
async fn api_the_list_is_a_documents_envelope_carrying_the_applied_window() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(&w.api, &w.token, t.finance, "doc.pdf", &[]).await;
    assert_status(&resp, 201);
    let listed = w.api.get_bearer("/api/v1/documents", &w.token).await;
    assert_status(&listed, 200);
    let body = listed.json();
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["documents", "pagination"]);
    assert_eq!(
        body["pagination"],
        json!({"limit": 100, "offset": 0, "total": 1})
    );
}

#[tokio::test]
async fn api_pagination_total_reflects_the_filters_not_the_whole_scope() {
    let w = World::new().await;
    let t = tree(&w).await;
    for i in 0..3 {
        let resp = upload(
            &w.api,
            &w.token,
            t.finance,
            &format!("match-{i}.pdf"),
            &[("tags[type]", "facture")],
        )
        .await;
        assert_status(&resp, 201);
    }
    for i in 0..2 {
        let resp = upload(&w.api, &w.token, t.finance, &format!("other-{i}.pdf"), &[]).await;
        assert_status(&resp, 201);
    }
    let resp = get_ok(
        &w,
        &query(&[("tag", "type"), ("tag_value", "facture"), ("limit", "1")]),
    )
    .await;
    assert_eq!(documents_of(&resp).len(), 1);
    assert_eq!(pagination_of(&resp)["total"], json!(3));
}

#[tokio::test]
async fn api_an_empty_list_still_carries_a_pagination_block() {
    let w = World::new().await;
    let _t = tree(&w).await;
    let resp = w.api.get_bearer("/api/v1/documents", &w.token).await;
    assert_status(&resp, 200);
    assert!(documents_of(&resp).is_empty());
    assert_eq!(
        pagination_of(&resp),
        json!({"limit": 100, "offset": 0, "total": 0})
    );
}

#[tokio::test]
async fn api_an_offset_above_the_cap_is_echoed_clamped() {
    let w = World::new().await;
    let _t = tree(&w).await;
    let resp = get_ok(&w, &query(&[("offset", "5000000")])).await;
    assert_eq!(pagination_of(&resp)["offset"], json!(1_000_000));
}

#[tokio::test]
async fn api_departments_list_paginates_too() {
    let w = World::new().await;
    let t = tree(&w).await;
    let ops = parity::fixtures::unique("ops");
    let ops_id = parity::fixtures::create_department(w.backend.db(), &ops, Some(t.finance)).await;
    parity::fixtures::grant(w.backend.db(), w.db_id().await, ops_id, true, true).await;

    let all = w.api.get_bearer("/api/v1/departments", &w.token).await;
    assert_status(&all, 200);
    assert!(all.json().as_array().expect("array").len() >= 2);

    let page = w
        .api
        .get_bearer(
            &format!("/api/v1/departments?{}", query(&[("limit", "1")])),
            &w.token,
        )
        .await;
    assert_status(&page, 200);
    assert_eq!(page.json().as_array().expect("array").len(), 1);
}

#[tokio::test]
async fn api_non_scalar_limit_and_offset_fall_back_to_the_default_page_size() {
    let w = World::new().await;
    let t = tree(&w).await;
    for i in 0..3 {
        let resp = upload(&w.api, &w.token, t.finance, &format!("doc-{i}.pdf"), &[]).await;
        assert_status(&resp, 201);
    }
    let resp = get_ok(&w, &query(&[("limit[]", "2"), ("offset[page]", "1")])).await;
    assert_status(&resp, 200);
    assert_eq!(documents_of(&resp).len(), 3);
    assert_eq!(
        pagination_of(&resp),
        json!({"limit": 100, "offset": 0, "total": 3})
    );
}

#[tokio::test]
async fn api_non_string_filter_params_return_400_instead_of_500() {
    let w = World::new().await;
    let _t = tree(&w).await;
    let cases = [
        ("filename[]", "x", "filename must be a string"),
        ("tag[]", "invoice", "tag must be a string"),
        ("tag_value[]", "x", "tag_value must be a string"),
        ("department_id[a]", "b", "department_id must be a string"),
    ];
    for (key, value, message) in cases {
        let resp = w
            .api
            .get_bearer(
                &format!("/api/v1/documents?{}", query(&[(key, value)])),
                &w.token,
            )
            .await;
        assert_error(&resp, 400, message);
    }
}

#[tokio::test]
async fn api_non_string_department_id_or_callback_url_on_upload_returns_400() {
    let w = World::new().await;
    let t = tree(&w).await;
    let id = dept(t.finance);
    let bad_dept = w
        .api
        .post_multipart(
            "/api/v1/documents",
            &w.token,
            &[("department_id[]", id.as_str())],
            Some(pdf_part()),
        )
        .await;
    assert_error(&bad_dept, 400, "department_id must be a string");

    let bad_callback = w
        .api
        .post_multipart(
            "/api/v1/documents",
            &w.token,
            &[
                ("department_id", id.as_str()),
                ("callback_url[]", "https://caller.test/cb"),
            ],
            Some(pdf_part()),
        )
        .await;
    assert_error(&bad_callback, 400, "callback_url must be a string");
    assert_eq!(
        parity::fixtures::count_upload_requests(w.backend.db(), t.finance).await,
        0
    );
}

#[tokio::test]
async fn api_filtering_by_a_forbidden_department_returns_403() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = w
        .api
        .get_bearer(
            &format!(
                "/api/v1/documents?{}",
                query(&[("department_id", &t.hr.to_string())])
            ),
            &w.token,
        )
        .await;
    assert_error(&resp, 403, "no read access to this department");
}

#[tokio::test]
async fn api_download_streams_the_file_content() {
    let w = World::new().await;
    let t = tree(&w).await;
    let resp = upload(&w.api, &w.token, t.finance, "report.pdf", &[]).await;
    assert_status(&resp, 201);
    let id = document_id(&resp);
    let got = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}/download"), &w.token)
        .await;
    assert_status(&got, 200);
    assert_eq!(got.body, PDF);
}

#[tokio::test]
async fn api_show_download_delete_of_a_forbidden_document_return_403() {
    let w = World::new().await;
    let t = tree(&w).await;
    let id = parity::fixtures::insert_document(w.backend.db(), t.hr, "salaries.pdf", "other").await;
    let show = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}"), &w.token)
        .await;
    assert_error(&show, 403, "no read access to this department");
    let download = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}/download"), &w.token)
        .await;
    assert_error(&download, 403, "no read access to this department");
    let delete = w
        .api
        .delete_bearer(&format!("/api/v1/documents/{id}"), &w.token)
        .await;
    assert_error(&delete, 403, "no write access to this department");
    assert!(parity::fixtures::document_is_live(w.backend.db(), &id).await);
}

#[tokio::test]
async fn api_delete_soft_deletes_the_document_and_keeps_it_for_the_retention_window() {
    let w = World::new().await;
    let t = tree(&w).await;
    let created = upload(&w.api, &w.token, t.finance, "report.pdf", &[]).await;
    assert_status(&created, 201);
    let id = document_id(&created);

    let deleted = w
        .api
        .delete_bearer(&format!("/api/v1/documents/{id}"), &w.token)
        .await;
    assert_status(&deleted, 204);

    assert!(!parity::fixtures::document_is_live(w.backend.db(), &id).await);
    let show = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}"), &w.token)
        .await;
    assert_error(&show, 404, "not found");
    assert!(
        parity::fixtures::document_retained_for_30_days(w.backend.db(), &id).await,
        "soft-deleted row must stay, with purge_at = deleted_at + 30 days"
    );
}

#[tokio::test]
async fn api_unknown_document_returns_404() {
    let w = World::new().await;
    let _t = tree(&w).await;
    let id = parity::fixtures::random_uuid();
    let resp = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}"), &w.token)
        .await;
    assert_error(&resp, 404, "not found");
}

#[tokio::test]
async fn api_filters_documents_by_owner_and_upload_mode() {
    let w = World::new().await;
    let t = tree(&w).await;
    let sync = upload(&w.api, &w.token, t.finance, "sync.pdf", &[]).await;
    assert_status(&sync, 201);
    let async_doc = upload(&w.api, &w.token, t.finance, "async.pdf", &[]).await;
    assert_status(&async_doc, 201);
    parity::fixtures::set_upload_origin(
        w.backend.db(),
        &document_id(&async_doc),
        true,
        "other-app",
    )
    .await;

    assert_eq!(
        filenames(&get_docs(&w, &query(&[("upload_mode", "async")])).await),
        vec!["async.pdf"]
    );
    assert_eq!(
        filenames(&get_docs(&w, &query(&[("upload_mode", "sync")])).await),
        vec!["sync.pdf"]
    );
    assert_eq!(
        filenames(&get_docs(&w, &query(&[("uploaded_by", "other-app")])).await),
        vec!["async.pdf"]
    );
    let bad = w
        .api
        .get_bearer(
            &format!(
                "/api/v1/documents?{}",
                query(&[("upload_mode", "sideways")])
            ),
            &w.token,
        )
        .await;
    assert_error(&bad, 400, "upload_mode must be sync or async");
}

#[tokio::test]
async fn api_filters_documents_by_uploaded_indexed_and_updated_ranges() {
    let w = World::new().await;
    let t = tree(&w).await;
    let old = upload(&w.api, &w.token, t.finance, "old.pdf", &[]).await;
    assert_status(&old, 201);
    let recent = upload(&w.api, &w.token, t.finance, "recent.pdf", &[]).await;
    assert_status(&recent, 201);
    parity::fixtures::backdate_document(w.backend.db(), &document_id(&old), "2020-01-01T00:00:00Z")
        .await;
    parity::fixtures::backdate_document(
        w.backend.db(),
        &document_id(&recent),
        "2026-06-01T00:00:00Z",
    )
    .await;
    let cutoff = "2025-01-01T00:00:00Z";
    let cases = [
        ("uploaded_after", "recent.pdf"),
        ("indexed_after", "recent.pdf"),
        ("updated_after", "recent.pdf"),
        ("uploaded_before", "old.pdf"),
        ("indexed_before", "old.pdf"),
        ("updated_before", "old.pdf"),
    ];
    for (param, expected) in cases {
        assert_eq!(
            filenames(&get_docs(&w, &query(&[(param, cutoff)])).await),
            vec![expected],
            "filter {param}"
        );
    }
    let bad = w
        .api
        .get_bearer(
            &format!(
                "/api/v1/documents?{}",
                query(&[("uploaded_after", "not-a-date")])
            ),
            &w.token,
        )
        .await;
    assert_error(&bad, 400, "uploaded_after must be an ISO8601 datetime");
}

#[tokio::test]
async fn api_sync_upload_targeting_trashed_department_returns_404() {
    let w = World::new().await;
    let name = parity::fixtures::unique("trashed");
    let id = parity::fixtures::create_department(w.backend.db(), &name, None).await;
    parity::fixtures::trash_department(w.backend.db(), id).await;
    let resp = upload(&w.api, &w.token, id, "report.pdf", &[]).await;
    assert_error(&resp, 404, "not found");
}

fn pdf_part() -> FilePart<'static> {
    FilePart {
        field: "file",
        filename: "report.pdf",
        content_type: "application/pdf",
        bytes: PDF,
    }
}

async fn get_ok(w: &World, qs: &str) -> parity::Resp {
    let resp = w
        .api
        .get_bearer(&format!("/api/v1/documents?{qs}"), &w.token)
        .await;
    assert_status(&resp, 200);
    assert_json_content_type(&resp);
    resp
}

async fn get_docs(w: &World, qs: &str) -> Vec<Value> {
    documents_of(&get_ok(w, qs).await)
}
