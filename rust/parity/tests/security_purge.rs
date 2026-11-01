//! Soft-delete and purge. Time is moved in the business tables, then
//! `Backend::run_job("PurgeDeletedDocumentsJob")` runs the real job.
//! Storage and queue tables are never read or written by the suite.
//!
//! The job purges every due row, not one caller's, so these tests hold a
//! process-wide lock across the mutation and the job.

use parity::fixtures::{self};
use parity::json::{assert_error, assert_json_content_type, assert_status};
use parity::oauth::{self};
use parity::{Api, backend};

const NOT_FOUND: &str = "not found";
const JOB: &str = "PurgeDeletedDocumentsJob";
/// `Document::RETENTION` is `30.days`, exactly 30 * 24 hours.
const RETENTION_SECONDS: f64 = 2_592_000.0;

async fn gate() -> tokio::sync::MutexGuard<'static, ()> {
    static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    GATE.lock().await
}

async fn run_purge(backend: &dyn parity::Backend) {
    backend.run_job(JOB).await.expect("purge job");
}

#[tokio::test]
async fn security_purge_http_soft_delete_hides_the_document_until_the_job() {
    let backend = backend().await;
    let db = backend.db();
    let api = Api::new(backend.base_url());
    let dept = fixtures::create_department(db, &fixtures::unique("purge-http"), None).await;
    let creds = oauth::register_everywhere(db, "purge-http").await;
    let client = fixtures::app_client_id(db, &creds.client_id).await;
    fixtures::grant(db, client, dept, true, true).await;
    let token = oauth::fetch_token(&creds).await;
    let dept_s = dept.to_string();

    let created = api
        .upload_bearer(
            "/api/v1/documents",
            &token,
            &[("department_id", dept_s.as_str())],
            "report.pdf",
            "application/pdf",
            b"PDF!",
        )
        .await;
    assert_status(&created, 201);
    assert_json_content_type(&created);
    let id = created.json()["id"].as_str().expect("id").to_owned();
    let path = format!("/api/v1/documents/{id}");

    let deleted = api.delete_bearer(&path, &token).await;
    assert_eq!(deleted.status, 204, "{}", deleted.text());
    assert!(deleted.body.is_empty(), "204 body: {:?}", deleted.text());

    assert_error(&api.get_bearer(&path, &token).await, 404, NOT_FOUND);
    assert_error(
        &api.get_bearer(&format!("{path}/download"), &token).await,
        404,
        NOT_FOUND,
    );
    assert_error(&api.delete_bearer(&path, &token).await, 404, NOT_FOUND);

    let listed = api
        .get_bearer_query(
            "/api/v1/documents",
            &token,
            &[("department_id", dept_s.as_str())],
        )
        .await;
    assert_status(&listed, 200);
    let listed_body = listed.json();
    let ids: Vec<&str> = listed_body["documents"]
        .as_array()
        .expect("documents")
        .iter()
        .filter_map(|d| d["id"].as_str())
        .collect();
    assert!(
        !ids.contains(&id.as_str()),
        "soft-deleted document still listed"
    );

    let window = fixtures::retention_seconds(db, &id)
        .await
        .expect("purge window");
    assert!(
        (window - RETENTION_SECONDS).abs() < 2.0,
        "retention was {window}s, expected {RETENTION_SECONDS}"
    );

    let _lock = gate().await;
    fixtures::make_purgeable_id(db, &id).await;
    run_purge(backend.as_ref()).await;
    assert!(
        !fixtures::document_exists(db, &id).await,
        "purge left the document row"
    );
}

#[tokio::test]
async fn security_purge_hard_deletes_documents_past_their_purge_at() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-due"), None).await;
    let doc = fixtures::insert_document(db, dept, "report.pdf", "erp-app").await;
    fixtures::make_purgeable_id(db, &doc).await;
    run_purge(backend.as_ref()).await;
    assert!(!fixtures::document_exists(db, &doc).await);
}

#[tokio::test]
async fn security_purge_keeps_documents_still_inside_the_retention_window() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-window"), None).await;
    let doc = fixtures::insert_document(db, dept, "report.pdf", "erp-app").await;
    fixtures::soft_delete_document(db, &doc).await;
    run_purge(backend.as_ref()).await;
    assert!(fixtures::document_exists(db, &doc).await);
}

#[tokio::test]
async fn security_purge_keeps_live_documents() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-live"), None).await;
    let doc = fixtures::insert_document(db, dept, "report.pdf", "erp-app").await;
    run_purge(backend.as_ref()).await;
    assert!(fixtures::document_exists(db, &doc).await);
    assert!(fixtures::department_exists(db, dept).await);
}

#[tokio::test]
async fn security_purge_hard_deletes_a_leaf_department_once_its_documents_are_purged() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-leaf"), None).await;
    let doc = fixtures::insert_document(db, dept, "report.pdf", "erp-app").await;
    fixtures::mark_department_deleted(db, dept).await;
    fixtures::make_purgeable_id(db, &doc).await;
    run_purge(backend.as_ref()).await;
    assert!(!fixtures::document_exists(db, &doc).await);
    assert!(!fixtures::department_exists(db, dept).await);
}

#[tokio::test]
async fn security_purge_keeps_a_department_while_a_trashed_document_remains() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-held"), None).await;
    let doc = fixtures::insert_document(db, dept, "report.pdf", "erp-app").await;
    fixtures::mark_department_deleted(db, dept).await;
    fixtures::soft_delete_document(db, &doc).await;
    run_purge(backend.as_ref()).await;
    assert!(fixtures::document_exists(db, &doc).await);
    assert!(fixtures::department_exists(db, dept).await);
}

#[tokio::test]
async fn security_purge_keeps_a_department_while_an_upload_request_references_it() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-upload"), None).await;
    let callback = format!("https://caller.test/{}", fixtures::unique("cb"));
    fixtures::insert_upload_request(db, dept, &callback, "r.pdf", "erp-app").await;
    fixtures::mark_department_deleted(db, dept).await;
    run_purge(backend.as_ref()).await;
    assert!(fixtures::department_exists(db, dept).await);
    assert_eq!(fixtures::upload_request_count(db, dept, &callback).await, 1);
}

#[tokio::test]
async fn security_purge_never_touches_an_active_department() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-active"), None).await;
    let doc = fixtures::insert_document(db, dept, "report.pdf", "erp-app").await;
    run_purge(backend.as_ref()).await;
    assert!(fixtures::department_exists(db, dept).await);
    assert!(fixtures::document_exists(db, &doc).await);
}

#[tokio::test]
async fn security_purge_hard_delete_of_a_department_destroys_its_grants() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let dept = fixtures::create_department(db, &fixtures::unique("purge-grant"), None).await;
    let client = fixtures::create_app_client(db, &fixtures::unique("purge-client")).await;
    fixtures::grant(db, client, dept, true, false).await;
    assert_eq!(fixtures::grant_count(db, dept).await, 1);
    fixtures::mark_department_deleted(db, dept).await;
    run_purge(backend.as_ref()).await;
    assert!(!fixtures::department_exists(db, dept).await);
    assert_eq!(fixtures::grant_count(db, dept).await, 0);
}

#[tokio::test]
async fn security_purge_purges_a_trashed_leaf_under_a_live_parent() {
    let _lock = gate().await;
    let backend = backend().await;
    let db = backend.db();
    let parent = fixtures::create_department(db, &fixtures::unique("purge-parent"), None).await;
    let child =
        fixtures::create_department(db, &fixtures::unique("purge-child"), Some(parent)).await;
    fixtures::mark_department_deleted(db, child).await;
    run_purge(backend.as_ref()).await;
    assert!(!fixtures::department_exists(db, child).await);
    assert!(fixtures::department_exists(db, parent).await);
}
