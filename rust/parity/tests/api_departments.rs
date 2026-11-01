//! Port of `test/controllers/api/v1/departments_test.rb`.
//!
//! The unique-index race is real concurrent creates (Review Focus 5): the
//! Ruby test stubbed `Department.create!`, which a black-box suite cannot do.

mod common;

use common::World;
use parity::json::{assert_error, assert_json_content_type, assert_status};
use serde_json::json;

#[tokio::test]
async fn api_list_returns_only_accessible_departments_subtree_included() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let resp = w.api.get_bearer("/api/v1/departments", &w.token).await;
    assert_status(&resp, 200);
    assert_json_content_type(&resp);
    let mut names = names(&resp.json());
    names.sort();
    let mut expected = vec![t.finance_name.clone(), t.invoices_name.clone()];
    expected.sort();
    assert_eq!(names, expected);
}

#[tokio::test]
async fn api_list_omits_trashed_departments() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    parity::fixtures::trash_department(w.backend.db(), t.invoices).await;
    let resp = w.api.get_bearer("/api/v1/departments", &w.token).await;
    assert_status(&resp, 200);
    assert_eq!(names(&resp.json()), vec![t.finance_name]);
}

#[tokio::test]
async fn api_create_a_subfolder_under_an_accessible_department() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let name = parity::fixtures::unique("folder");
    let parent = t.invoices.to_string();
    let resp = w
        .api
        .post_form(
            "/api/v1/departments",
            &w.token,
            &[("name", name.as_str()), ("parent_id", parent.as_str())],
        )
        .await;
    assert_status(&resp, 201);
    assert_json_content_type(&resp);
    let body = resp.json();
    assert_eq!(body["name"], json!(name));
    assert_eq!(body["parent_id"], json!(t.invoices));
    assert!(body["id"].is_number());
}

#[tokio::test]
async fn api_create_under_a_forbidden_parent_returns_403() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let parent = t.hr.to_string();
    let resp = w
        .api
        .post_form(
            "/api/v1/departments",
            &w.token,
            &[("name", "Payroll"), ("parent_id", parent.as_str())],
        )
        .await;
    assert_error(&resp, 403, "no write access to this department");
}

#[tokio::test]
async fn api_create_without_parent_id_returns_400() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let resp = w
        .api
        .post_form("/api/v1/departments", &w.token, &[("name", "Rogue")])
        .await;
    assert_error(
        &resp,
        400,
        "parent_id is required (top-level departments are created by an admin)",
    );
}

#[tokio::test]
async fn api_duplicate_sibling_name_returns_422() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let parent = t.finance.to_string();
    let resp = w
        .api
        .post_form(
            "/api/v1/departments",
            &w.token,
            &[
                ("name", t.invoices_name.as_str()),
                ("parent_id", parent.as_str()),
            ],
        )
        .await;
    assert_error(&resp, 422, "Name has already been taken");
}

#[tokio::test]
async fn api_non_string_name_or_parent_id_returns_400_instead_of_a_garbage_row() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let parent = t.finance.to_string();
    let bad_name = w
        .api
        .post_form(
            "/api/v1/departments",
            &w.token,
            &[("name[a]", "b"), ("parent_id", parent.as_str())],
        )
        .await;
    assert_error(&bad_name, 400, "name must be a string");

    let bad_parent = w
        .api
        .post_form(
            "/api/v1/departments",
            &w.token,
            &[("name", "Ok"), ("parent_id[]", parent.as_str())],
        )
        .await;
    assert_error(&bad_parent, 400, "parent_id must be a string or an integer");
    assert_eq!(
        parity::fixtures::child_department_count(w.backend.db(), t.finance).await,
        1,
        "no garbage child was created"
    );
}

#[tokio::test]
async fn api_create_with_an_integer_parent_id_in_a_json_body_returns_201() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let name = parity::fixtures::unique("json-folder");
    let resp = w
        .api
        .post_json(
            "/api/v1/departments",
            &w.token,
            &json!({"name": name, "parent_id": t.invoices}),
        )
        .await;
    assert_status(&resp, 201);
    let body = resp.json();
    assert_eq!(body["name"], json!(name));
    assert_eq!(body["parent_id"], json!(t.invoices));
}

#[tokio::test]
async fn api_create_with_a_null_parent_id_in_a_json_body_returns_400() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let resp = w
        .api
        .post_json(
            "/api/v1/departments",
            &w.token,
            &json!({"name": "Rogue", "parent_id": null}),
        )
        .await;
    assert_error(
        &resp,
        400,
        "parent_id is required (top-level departments are created by an admin)",
    );
}

#[tokio::test]
async fn api_create_with_a_non_scalar_parent_id_in_a_json_body_returns_400() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let resp = w
        .api
        .post_json(
            "/api/v1/departments",
            &w.token,
            &json!({"name": "Rogue", "parent_id": {"a": "b"}}),
        )
        .await;
    assert_error(&resp, 400, "parent_id must be a string or an integer");
}

#[tokio::test]
async fn api_create_with_a_float_parent_id_in_a_json_body_returns_400() {
    let w = World::new().await;
    let _t = w.read_write_tree().await;
    let resp = w
        .api
        .post_json(
            "/api/v1/departments",
            &w.token,
            &json!({"name": "Rogue", "parent_id": 1.5}),
        )
        .await;
    assert_error(&resp, 400, "parent_id must be a string or an integer");
}

#[tokio::test]
async fn api_concurrent_create_of_the_same_sibling_returns_422_not_500() {
    let w = World::new().await;
    let t = w.read_write_tree().await;
    let name = parity::fixtures::unique("race");
    let body = json!({"name": name, "parent_id": t.finance});
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let api = w.api.clone();
        let token = w.token.clone();
        let body = body.clone();
        tasks.push(tokio::spawn(async move {
            api.post_json("/api/v1/departments", &token, &body).await
        }));
    }
    let mut created = 0;
    let mut rejected = 0;
    for task in tasks {
        let resp = task.await.expect("join");
        match resp.status {
            201 => created += 1,
            422 => {
                assert_error(&resp, 422, "Name has already been taken");
                rejected += 1;
            }
            other => panic!("unexpected HTTP {other}: {}", resp.text()),
        }
    }
    assert_eq!(created, 1, "one create wins");
    assert_eq!(
        rejected, 7,
        "the losers are validation or unique-index 422s"
    );
}

fn names(body: &serde_json::Value) -> Vec<String> {
    body.as_array()
        .expect("department list")
        .iter()
        .map(|d| d["name"].as_str().expect("name").to_owned())
        .collect()
}
