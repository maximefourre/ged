//! Port of the parity lines in `test/security/authz_idor_test.rb`, plus both
//! directions of read ≠ write (the reader in that file is never exercised).
//!
//! Cross-department access is refused with the exact `no read|write access`
//! body, a missing document is `not found`, and the two grants do not imply
//! each other. A read-only client is refused on delete, tag replace and
//! upload with no mutation. A write-only client gets an empty list, a 403 on
//! show, and a real 204 on delete. Documents are rows in the business tables.

use parity::fixtures::{self};
use parity::json::{assert_error, assert_json_content_type, assert_json_eq, assert_status};
use parity::oauth::{self};
use parity::{Api, backend};
use serde_json::{Value, json};

const NO_READ: &str = "no read access to this department";
const NO_WRITE: &str = "no write access to this department";
const NOT_FOUND: &str = "not found";

struct Idor {
    base: String,
    dept_child: i64,
    dept_b: i64,
    doc_a: String,
    doc_b: String,
    upload_b: String,
    client_a: String,
    token_a: String,
    token_writer: String,
    token_none: String,
    token_child: String,
}

impl Idor {
    /// A client bound to this test's runtime. A shared `reqwest::Client` dies
    /// with the runtime that first polled it.
    fn api(&self) -> Api {
        Api::new(&self.base)
    }
}

async fn idor() -> &'static Idor {
    static CELL: tokio::sync::OnceCell<Idor> = tokio::sync::OnceCell::const_new();
    CELL.get_or_init(build_idor).await
}

async fn build_idor() -> Idor {
    let backend = backend().await;
    let db = backend.db();
    let base = backend.base_url().to_owned();

    let dept_a = fixtures::create_department(db, &fixtures::unique("dept-a"), None).await;
    let dept_child =
        fixtures::create_department(db, &fixtures::unique("dept-a-child"), Some(dept_a)).await;
    let dept_b = fixtures::create_department(db, &fixtures::unique("dept-b"), None).await;

    let (client_a, token_a) = registered(db, "dept-a", Some((dept_a, true, true))).await;
    let (_, token_writer) = registered(db, "writer", Some((dept_a, false, true))).await;
    let (_, token_none) = registered(db, "noaccess", None).await;
    let (_, token_child) = registered(db, "child-only", Some((dept_child, true, true))).await;

    let doc_a = fixtures::insert_document(db, dept_a, "a-report.pdf", "seed").await;
    let doc_b = fixtures::insert_document(db, dept_b, "b-report.pdf", "seed").await;
    let upload_b = fixtures::insert_upload_request(
        db,
        dept_b,
        "https://example.com/hook",
        "b-report.pdf",
        "seed",
    )
    .await;

    Idor {
        base,
        dept_child,
        dept_b,
        doc_a,
        doc_b,
        upload_b,
        client_a,
        token_a,
        token_writer,
        token_none,
        token_child,
    }
}

async fn registered(
    db: &parity::PgPool,
    prefix: &str,
    grant: Option<(i64, bool, bool)>,
) -> (String, String) {
    let creds = oauth::register_everywhere(db, prefix).await;
    if let Some((dept, read, write)) = grant {
        let id = fixtures::app_client_id(db, &creds.client_id).await;
        fixtures::grant(db, id, dept, read, write).await;
    }
    let token = oauth::fetch_token(&creds).await;
    (creds.client_id, token)
}

fn document_ids(body: &Value) -> Vec<String> {
    body["documents"]
        .as_array()
        .expect("documents")
        .iter()
        .map(|d| d["id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test]
async fn security_authz_dept_a_cannot_show_a_document_in_dept_b() {
    let f = idor().await;
    let r = f
        .api()
        .get_bearer(&format!("/api/v1/documents/{}", f.doc_b), &f.token_a)
        .await;
    assert_error(&r, 403, NO_READ);
}

#[tokio::test]
async fn security_authz_dept_a_cannot_download_a_document_in_dept_b() {
    let f = idor().await;
    let r = f
        .api()
        .get_bearer(
            &format!("/api/v1/documents/{}/download", f.doc_b),
            &f.token_a,
        )
        .await;
    assert_error(&r, 403, NO_READ);
}

#[tokio::test]
async fn security_authz_dept_a_cannot_delete_a_document_in_dept_b() {
    let f = idor().await;
    let r = f
        .api()
        .delete_bearer(&format!("/api/v1/documents/{}", f.doc_b), &f.token_a)
        .await;
    assert_error(&r, 403, NO_WRITE);
    let backend = backend().await;
    assert!(
        fixtures::document_exists(backend.db(), &f.doc_b).await,
        "forbidden delete removed the row"
    );
    assert!(
        fixtures::retention_seconds(backend.db(), &f.doc_b)
            .await
            .is_none(),
        "forbidden delete set a purge window"
    );
}

#[tokio::test]
async fn security_authz_dept_a_cannot_replace_tags_on_a_document_in_dept_b() {
    let f = idor().await;
    let r = f
        .api()
        .put_form_bearer(
            &format!("/api/v1/documents/{}/tags", f.doc_b),
            &f.token_a,
            &[("tags[type]", "facture")],
        )
        .await;
    assert_error(&r, 403, NO_WRITE);
    let backend = backend().await;
    assert_eq!(
        fixtures::document_tags_opt(backend.db(), &f.doc_b)
            .await
            .expect("row"),
        json!({})
    );
}

#[tokio::test]
async fn security_authz_dept_a_cannot_filter_the_list_by_dept_b() {
    let f = idor().await;
    let dept = f.dept_b.to_string();
    let r = f
        .api()
        .get_bearer_query(
            "/api/v1/documents",
            &f.token_a,
            &[("department_id", dept.as_str())],
        )
        .await;
    assert_error(&r, 403, NO_READ);
}

#[tokio::test]
async fn security_authz_unfiltered_list_never_leaks_dept_b() {
    let f = idor().await;
    let r = f.api().get_bearer("/api/v1/documents", &f.token_a).await;
    assert_status(&r, 200);
    assert_json_content_type(&r);
    let ids = document_ids(&r.json());
    assert!(ids.contains(&f.doc_a), "missing own document: {ids:?}");
    assert!(!ids.contains(&f.doc_b), "leaked dept B: {ids:?}");
}

#[tokio::test]
async fn security_authz_grant_cascades_to_the_child_on_upload() {
    let f = idor().await;
    let dept = f.dept_child.to_string();
    let r = f
        .api()
        .upload_bearer(
            "/api/v1/documents",
            &f.token_a,
            &[("department_id", dept.as_str())],
            "child.pdf",
            "application/pdf",
            b"PDF!",
        )
        .await;
    assert_status(&r, 201);
    assert_json_content_type(&r);
    let body = r.json();
    assert_eq!(body["department_id"], json!(f.dept_child));
    assert_eq!(body["uploaded_by"], json!(f.client_a));
    assert_eq!(body["filename"], json!("child.pdf"));
}

#[tokio::test]
async fn security_authz_child_grant_does_not_reach_the_parent_document() {
    let f = idor().await;
    let r = f
        .api()
        .get_bearer(&format!("/api/v1/documents/{}", f.doc_a), &f.token_child)
        .await;
    assert_error(&r, 403, NO_READ);
}

#[tokio::test]
async fn security_authz_write_only_client_cannot_read() {
    let f = idor().await;
    let r = f
        .api()
        .get_bearer(&format!("/api/v1/documents/{}", f.doc_a), &f.token_writer)
        .await;
    assert_error(&r, 403, NO_READ);
}

#[tokio::test]
async fn security_authz_client_with_no_grants_cannot_access_documents() {
    let f = idor().await;
    let show = f
        .api()
        .get_bearer(&format!("/api/v1/documents/{}", f.doc_a), &f.token_none)
        .await;
    assert_error(&show, 403, NO_READ);

    let list = f.api().get_bearer("/api/v1/documents", &f.token_none).await;
    assert_json_eq(
        &list,
        &json!({
            "documents": [],
            "pagination": { "limit": 100, "offset": 0, "total": 0 }
        }),
    );
}

#[tokio::test]
async fn security_authz_unknown_document_uuid_is_404() {
    let f = idor().await;
    let r = f
        .api()
        .get_bearer(
            "/api/v1/documents/11111111-1111-4111-8111-111111111111",
            &f.token_a,
        )
        .await;
    assert_error(&r, 404, NOT_FOUND);
}

#[tokio::test]
async fn security_authz_dept_a_cannot_poll_an_upload_request_in_dept_b() {
    let f = idor().await;
    let r = f
        .api()
        .get_bearer(
            &format!("/api/v1/upload_requests/{}", f.upload_b),
            &f.token_a,
        )
        .await;
    assert_error(&r, 403, NO_READ);
}

/// Read and write grants on a fresh department. Each test builds its own so a
/// delete cannot race another test's row.
struct Pair {
    base: String,
    dept: i64,
    reader: String,
    writer: String,
}

impl Pair {
    fn api(&self) -> Api {
        Api::new(&self.base)
    }
}

async fn pair(prefix: &str) -> Pair {
    let backend = backend().await;
    let db = backend.db();
    let base = backend.base_url().to_owned();
    let dept = fixtures::create_department(db, &fixtures::unique(prefix), None).await;
    let (_, reader) = registered(db, &format!("{prefix}-r"), Some((dept, true, false))).await;
    let (_, writer) = registered(db, &format!("{prefix}-w"), Some((dept, false, true))).await;
    Pair {
        base,
        dept,
        reader,
        writer,
    }
}

#[tokio::test]
async fn security_authz_read_only_cannot_delete() {
    let p = pair("rw-del").await;
    let backend = backend().await;
    let doc = fixtures::insert_document(backend.db(), p.dept, "report.pdf", "seed").await;
    let r = p
        .api()
        .delete_bearer(&format!("/api/v1/documents/{doc}"), &p.reader)
        .await;
    assert_error(&r, 403, NO_WRITE);
    assert!(
        fixtures::document_exists(backend.db(), &doc).await,
        "read-only delete removed the row"
    );
    assert!(
        fixtures::retention_seconds(backend.db(), &doc)
            .await
            .is_none(),
        "read-only delete set a purge window"
    );
}

#[tokio::test]
async fn security_authz_read_only_cannot_replace_tags() {
    let p = pair("rw-tags").await;
    let backend = backend().await;
    let doc = fixtures::insert_document(backend.db(), p.dept, "report.pdf", "seed").await;
    let r = p
        .api()
        .put_form_bearer(
            &format!("/api/v1/documents/{doc}/tags"),
            &p.reader,
            &[("tags[type]", "facture")],
        )
        .await;
    assert_error(&r, 403, NO_WRITE);
    assert_eq!(
        fixtures::document_tags_opt(backend.db(), &doc)
            .await
            .expect("row"),
        json!({})
    );
}

#[tokio::test]
async fn security_authz_read_only_cannot_upload() {
    let p = pair("rw-up").await;
    let backend = backend().await;
    let before = fixtures::document_count(backend.db(), p.dept).await;
    let dept = p.dept.to_string();
    let r = p
        .api()
        .upload_bearer(
            "/api/v1/documents",
            &p.reader,
            &[("department_id", dept.as_str())],
            "report.pdf",
            "application/pdf",
            b"PDF!",
        )
        .await;
    assert_error(&r, 403, NO_WRITE);
    assert_eq!(
        fixtures::document_count(backend.db(), p.dept).await,
        before,
        "read-only upload persisted a document"
    );
}

#[tokio::test]
async fn security_authz_write_only_list_is_empty() {
    let p = pair("rw-list").await;
    let backend = backend().await;
    let doc = fixtures::insert_document(backend.db(), p.dept, "report.pdf", "seed").await;
    let listed = p.api().get_bearer("/api/v1/documents", &p.writer).await;
    assert_status(&listed, 200);
    assert_json_eq(
        &listed,
        &json!({
            "documents": [],
            "pagination": { "limit": 100, "offset": 0, "total": 0 }
        }),
    );
    let seen = p.api().get_bearer("/api/v1/documents", &p.reader).await;
    assert_status(&seen, 200);
    let ids = document_ids(&seen.json());
    assert!(
        ids.contains(&doc),
        "reader lost its document, so the empty writer list proved nothing: {ids:?}"
    );
}

#[tokio::test]
async fn security_authz_write_only_delete_is_effective() {
    let p = pair("rw-wipe").await;
    let backend = backend().await;
    let doc = fixtures::insert_document(backend.db(), p.dept, "report.pdf", "seed").await;
    let path = format!("/api/v1/documents/{doc}");

    let visible = p.api().get_bearer(&path, &p.reader).await;
    assert_status(&visible, 200);
    assert_eq!(visible.json()["id"], json!(doc));

    let hidden = p.api().get_bearer(&path, &p.writer).await;
    assert_error(&hidden, 403, NO_READ);

    let deleted = p.api().delete_bearer(&path, &p.writer).await;
    assert_eq!(deleted.status, 204, "{}", deleted.text());
    assert!(deleted.body.is_empty(), "204 body: {:?}", deleted.text());
    assert!(
        fixtures::document_exists(backend.db(), &doc).await,
        "write-only delete destroyed the row instead of soft-deleting it"
    );
    let window = fixtures::retention_seconds(backend.db(), &doc)
        .await
        .expect("write-only delete did not set a purge window");
    assert!(
        (window - 2_592_000.0).abs() < 2.0,
        "retention was {window}s, expected 30 days"
    );
    assert_error(&p.api().get_bearer(&path, &p.reader).await, 404, NOT_FOUND);
}
