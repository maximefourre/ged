//! Port of the parity lines in `test/security/input_fuzz_test.rb`, plus the
//! Rack parameter matrix the migration treats as the parity oracle:
//! `tags[type]=x`, `limit[]=1` → 200, duplicate keys, empty versus absent.
//!
//! Two Ruby tests stay unit-only: a nil `byte_size`, and the proof that an
//! oversized async upload never writes a staging file. On the compose stack
//! the edge and the model share `GED_MAX_UPLOAD_BYTES`, so an oversized body
//! is rejected by Thruster before Rails; that observed rejection is what the
//! oversized tests lock.

use parity::fixtures::{self};
use parity::json::{assert_error, assert_json_content_type, assert_status};
use parity::oauth::{self};
use parity::{Api, Resp, backend};
use serde_json::{Value, json};

const DOCS: &str = "/api/v1/documents";
const MISSING_DEPARTMENT: &str = "param is missing or the value is empty or invalid: department_id";
const TAGS_MAP: &str = "Tags must be a map of key/value pairs";
const TAGS_COUNT: &str = "Tags cannot have more than 50 entries";
const TAGS_KEY: &str = "Tags keys must be non-empty strings of at most 100 characters";
const TAGS_VALUE: &str = "Tags values must be strings of at most 500 characters, or null";
/// Thruster `MAX_REQUEST_BODY`, same default as `GED_MAX_UPLOAD_BYTES`.
const EDGE_LIMIT: usize = 52_428_800;

struct Fuzz {
    base: String,
    token: String,
    dept: i64,
}

impl Fuzz {
    /// A client bound to this test's runtime. A shared `reqwest::Client` dies
    /// with the runtime that first polled it.
    fn api(&self) -> Api {
        Api::new(&self.base)
    }
}

async fn fuzz() -> &'static Fuzz {
    static CELL: tokio::sync::OnceCell<Fuzz> = tokio::sync::OnceCell::const_new();
    CELL.get_or_init(build_fuzz).await
}

async fn build_fuzz() -> Fuzz {
    let backend = backend().await;
    let db = backend.db();
    let base = backend.base_url().to_owned();
    let dept = fixtures::create_department(db, &fixtures::unique("fuzz"), None).await;
    let creds = oauth::register_everywhere(db, "fuzz").await;
    let id = fixtures::app_client_id(db, &creds.client_id).await;
    fixtures::grant(db, id, dept, true, true).await;
    let token = oauth::fetch_token(&creds).await;
    fixtures::insert_document(db, dept, "report.pdf", &creds.client_id).await;
    Fuzz { base, token, dept }
}

async fn upload(f: &Fuzz, extra: &[(&str, &str)], filename: &str, bytes: &[u8]) -> Resp {
    let dept = f.dept.to_string();
    let mut fields = vec![("department_id", dept.as_str())];
    fields.extend_from_slice(extra);
    f.api()
        .upload_bearer(DOCS, &f.token, &fields, filename, "application/pdf", bytes)
        .await
}

fn assert_window(resp: &Resp, limit: i64, offset: i64) {
    assert_status(resp, 200);
    assert_json_content_type(resp);
    let page = resp.json();
    assert_eq!(page["pagination"]["limit"], json!(limit), "{}", resp.text());
    assert_eq!(
        page["pagination"]["offset"],
        json!(offset),
        "{}",
        resp.text()
    );
    assert!(page["documents"].is_array(), "{}", resp.text());
}

fn ids_of(body: &Value) -> Vec<String> {
    body["documents"]
        .as_array()
        .expect("documents")
        .iter()
        .map(|d| d["id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test]
async fn security_fuzz_nested_hash_where_a_scalar_is_expected_is_400() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("department_id[a]", "b")])
        .await;
    assert_error(&r, 400, "department_id must be a string");
}

#[tokio::test]
async fn security_fuzz_array_where_a_scalar_is_expected_is_400() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("filename[]", "x")])
        .await;
    assert_error(&r, 400, "filename must be a string");
}

#[tokio::test]
async fn security_fuzz_tag_value_without_tag_is_400() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("tag_value", "x")])
        .await;
    assert_error(&r, 400, "tag_value requires tag");
}

#[tokio::test]
async fn security_fuzz_unparsable_date_filter_is_400() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("uploaded_after", "not-a-date")])
        .await;
    assert_error(&r, 400, "uploaded_after must be an ISO8601 datetime");
}

#[tokio::test]
async fn security_fuzz_invalid_upload_mode_is_400() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("upload_mode", "bogus")])
        .await;
    assert_error(&r, 400, "upload_mode must be sync or async");
}

#[tokio::test]
async fn security_fuzz_negative_limit_is_clamped() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("limit", "-5")])
        .await;
    assert_window(&r, 100, 0);
}

#[tokio::test]
async fn security_fuzz_huge_limit_is_clamped() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("limit", "9999999")])
        .await;
    assert_window(&r, 500, 0);
}

#[tokio::test]
async fn security_fuzz_negative_offset_is_clamped() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("offset", "-1")])
        .await;
    assert_window(&r, 100, 0);
}

#[tokio::test]
async fn security_fuzz_offset_beyond_bigint_is_clamped() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("offset", "9223372036854775808")])
        .await;
    assert_window(&r, 100, 1_000_000);
}

#[tokio::test]
async fn security_fuzz_more_than_50_tags_is_422() {
    let f = fuzz().await;
    let owned: Vec<(String, String)> = (0..51)
        .map(|i| (format!("tags[k{i}]"), "value".to_owned()))
        .collect();
    let fields: Vec<(&str, &str)> = owned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let r = upload(f, &fields, "report.pdf", b"PDF!").await;
    assert_error(&r, 422, TAGS_COUNT);
}

#[tokio::test]
async fn security_fuzz_tag_key_longer_than_100_is_422() {
    let f = fuzz().await;
    let name = format!("tags[{}]", "k".repeat(200));
    let r = upload(f, &[(name.as_str(), "value")], "report.pdf", b"PDF!").await;
    assert_error(&r, 422, TAGS_KEY);
}

#[tokio::test]
async fn security_fuzz_tag_value_longer_than_500_is_422() {
    let f = fuzz().await;
    let value = "v".repeat(600);
    let r = upload(f, &[("tags[type]", value.as_str())], "report.pdf", b"PDF!").await;
    assert_error(&r, 422, TAGS_VALUE);
}

#[tokio::test]
async fn security_fuzz_malformed_json_body_is_a_json_400() {
    let f = fuzz().await;
    let r = f
        .api()
        .post_bytes_bearer(DOCS, &f.token, "application/json", br#"{"tags":"#.to_vec())
        .await;
    assert_error(&r, 400, "bad request");
}

#[tokio::test]
async fn security_fuzz_hostile_filename_is_reduced_to_its_basename() {
    let f = fuzz().await;
    let r = upload(f, &[], "a b/../../etc/passwd.pdf", b"PDF!").await;
    assert_status(&r, 201);
    let name = r.json()["filename"].clone();
    assert_eq!(name, json!("passwd.pdf"));
    let text = name.as_str().unwrap_or_default();
    assert!(!text.contains("../"));
    assert!(!text.contains('/'));
}

#[tokio::test]
async fn security_fuzz_filename_with_a_space_is_kept() {
    let f = fuzz().await;
    let r = upload(f, &[], "mon rapport.pdf", b"PDF!").await;
    assert_status(&r, 201);
    assert_eq!(r.json()["filename"], json!("mon rapport.pdf"));
}

#[tokio::test]
async fn security_fuzz_backslash_path_is_reduced_to_its_basename() {
    let f = fuzz().await;
    let r = upload(f, &[], r"..\..\windows\system32\evil.exe", b"PDF!").await;
    assert_status(&r, 201);
    assert_eq!(r.json()["filename"], json!("evil.exe"));
}

#[tokio::test]
async fn security_fuzz_filename_that_sanitizes_to_empty_becomes_unnamed() {
    let f = fuzz().await;
    let r = upload(f, &[], "../../", b"PDF!").await;
    assert_status(&r, 201);
    assert_eq!(r.json()["filename"], json!("unnamed"));
}

#[tokio::test]
async fn security_fuzz_oversized_sync_upload_is_rejected_at_the_edge() {
    let (api, token, dept) = isolated_writer("fuzz-big").await;
    let backend = backend().await;
    let before = fixtures::document_count(backend.db(), dept).await;
    let dept_s = dept.to_string();
    let body = vec![b'a'; EDGE_LIMIT + 1];
    let r = api
        .upload_bearer(
            DOCS,
            &token,
            &[("department_id", dept_s.as_str())],
            "huge.pdf",
            "application/pdf",
            &body,
        )
        .await;
    assert_eq!(r.status, 413, "oversized sync body: {:?}", r.text());
    assert_eq!(r.body, b"", "unexpected 413 body: {:?}", r.text());
    assert_eq!(
        fixtures::document_count(backend.db(), dept).await,
        before,
        "oversized sync upload persisted a document"
    );
}

#[tokio::test]
async fn security_fuzz_oversized_async_upload_is_rejected_without_a_request_row() {
    let (api, token, dept) = isolated_writer("fuzz-async-big").await;
    let backend = backend().await;
    let url = format!("https://example.com/{}", fixtures::unique("too-big"));
    let dept_s = dept.to_string();
    let body = vec![b'a'; EDGE_LIMIT + 1];
    let r = api
        .upload_bearer(
            DOCS,
            &token,
            &[
                ("department_id", dept_s.as_str()),
                ("callback_url", url.as_str()),
            ],
            "huge.pdf",
            "application/pdf",
            &body,
        )
        .await;
    assert_eq!(r.status, 413, "oversized async body: {:?}", r.text());
    assert_eq!(r.body, b"", "unexpected 413 body: {:?}", r.text());
    assert_eq!(
        fixtures::upload_request_count(backend.db(), dept, &url).await,
        0
    );
    assert_eq!(fixtures::document_count(backend.db(), dept).await, 0);
}

#[tokio::test]
async fn security_params_tags_bracket_form_is_stored() {
    let f = fuzz().await;
    let r = upload(f, &[("tags[type]", "x")], "tagged.pdf", b"PDF!").await;
    assert_status(&r, 201);
    assert_eq!(r.json()["tags"], json!({ "type": "x" }));
}

#[tokio::test]
async fn security_params_limit_array_is_200_with_the_default_window() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("limit[]", "1")])
        .await;
    assert_window(&r, 100, 0);
}

#[tokio::test]
async fn security_params_duplicate_limit_keeps_the_last_value() {
    let f = fuzz().await;
    let r = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("limit", "1"), ("limit", "7")])
        .await;
    assert_window(&r, 7, 0);
}

#[tokio::test]
async fn security_params_duplicate_tag_filter_keeps_the_last_key() {
    let f = fuzz().await;
    let alpha = upload(f, &[("tags[alpha]", "1")], "alpha.pdf", b"PDF!").await;
    let beta = upload(f, &[("tags[beta]", "1")], "beta.pdf", b"PDF!").await;
    assert_status(&alpha, 201);
    assert_status(&beta, 201);
    let alpha_id = alpha.json()["id"].as_str().expect("id").to_owned();
    let beta_id = beta.json()["id"].as_str().expect("id").to_owned();
    let listed = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("tag", "alpha"), ("tag", "beta")])
        .await;
    assert_status(&listed, 200);
    let ids = ids_of(&listed.json());
    assert!(ids.contains(&beta_id), "last tag key missing: {ids:?}");
    assert!(
        !ids.contains(&alpha_id),
        "first tag key still matched: {ids:?}"
    );
}

#[tokio::test]
async fn security_params_duplicate_tag_value_on_upload_keeps_the_last() {
    let f = fuzz().await;
    let r = upload(
        f,
        &[("tags[type]", "alpha"), ("tags[type]", "beta")],
        "dup.pdf",
        b"PDF!",
    )
    .await;
    assert_status(&r, 201);
    assert_eq!(r.json()["tags"], json!({ "type": "beta" }));
}

#[tokio::test]
async fn security_params_absent_tags_are_an_empty_map() {
    let f = fuzz().await;
    let r = upload(f, &[], "notags.pdf", b"PDF!").await;
    assert_status(&r, 201);
    assert_eq!(r.json()["tags"], json!({}));
}

#[tokio::test]
async fn security_params_empty_tags_field_is_422() {
    let f = fuzz().await;
    let r = upload(f, &[("tags", "")], "empty-tags.pdf", b"PDF!").await;
    assert_error(&r, 422, TAGS_MAP);
}

#[tokio::test]
async fn security_params_empty_tag_value_is_stored_as_null() {
    let f = fuzz().await;
    let r = upload(f, &[("tags[type]", "")], "null-tag.pdf", b"PDF!").await;
    assert_status(&r, 201);
    assert_eq!(r.json()["tags"], json!({ "type": null }));
}

#[tokio::test]
async fn security_params_empty_limit_matches_an_absent_limit() {
    let f = fuzz().await;
    let absent = f.api().get_bearer(DOCS, &f.token).await;
    let empty = f
        .api()
        .get_bearer_query(DOCS, &f.token, &[("limit", "")])
        .await;
    assert_window(&absent, 100, 0);
    assert_window(&empty, 100, 0);
}

#[tokio::test]
async fn security_params_empty_department_id_matches_a_missing_one() {
    let f = fuzz().await;
    let missing = f
        .api()
        .upload_bearer(DOCS, &f.token, &[], "r.pdf", "application/pdf", b"PDF!")
        .await;
    assert_error(&missing, 400, MISSING_DEPARTMENT);
    let empty = f
        .api()
        .upload_bearer(
            DOCS,
            &f.token,
            &[("department_id", "")],
            "r.pdf",
            "application/pdf",
            b"PDF!",
        )
        .await;
    assert_error(&empty, 400, MISSING_DEPARTMENT);
}

/// Writer with its own department, so a rejection cannot race the shared fixture.
async fn isolated_writer(prefix: &str) -> (Api, String, i64) {
    let backend = backend().await;
    let db = backend.db();
    let api = Api::new(backend.base_url());
    let dept = fixtures::create_department(db, &fixtures::unique(prefix), None).await;
    let creds = oauth::register_everywhere(db, prefix).await;
    let id = fixtures::app_client_id(db, &creds.client_id).await;
    fixtures::grant(db, id, dept, true, true).await;
    let token = oauth::fetch_token(&creds).await;
    (api, token, dept)
}
