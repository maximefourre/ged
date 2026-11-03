//! `Rack::ConditionalGet` + `Rack::ETag` as Rails 8.1 stacks them: a 200/201
//! response with a body gets a weak ETag (first 32 hex of the SHA-256 of the
//! body) and `Cache-Control: max-age=0, private, must-revalidate`; a matching
//! `If-None-Match` on GET/HEAD turns a 200 into a 304; a response without an
//! ETag gets `Cache-Control: no-cache`. Observed on the stack 2026-11-03.
//! Also: Redoc/OpenAPI carry `; charset=utf-8`, like any Rails render.

mod common;

use common::World;
use parity::json::assert_status;
use regex::Regex;

const REVALIDATE: &str = "max-age=0, private, must-revalidate";

fn etag(resp: &parity::Resp) -> String {
    let etag = resp.header("etag").expect("etag header").to_owned();
    let shape = Regex::new(r#"^W/"[0-9a-f]{32}"$"#).unwrap();
    assert!(shape.is_match(&etag), "not a weak 32-hex etag: {etag}");
    etag
}

#[tokio::test]
async fn api_json_get_200_has_a_weak_etag_and_revalidating_cache_control() {
    let w = World::new().await;
    let first = w.api.get_bearer("/api/v1/departments", &w.token).await;
    assert_status(&first, 200);
    let tag = etag(&first);
    assert_eq!(first.header("cache-control"), Some(REVALIDATE));
    let again = w.api.get_bearer("/api/v1/departments", &w.token).await;
    assert_eq!(etag(&again), tag, "same body, same etag");
}

#[tokio::test]
async fn api_matching_if_none_match_is_a_304_without_body_type_or_length() {
    let w = World::new().await;
    let first = w.api.get("/up", &[]).await;
    let tag = etag(&first);
    let resp = w.api.get("/up", &[("if-none-match", tag.as_str())]).await;
    assert_status(&resp, 304);
    assert_eq!(resp.body, b"");
    assert_eq!(resp.header("content-type"), None);
    assert_eq!(resp.header("content-length"), None);
    assert_eq!(resp.header("etag"), Some(tag.as_str()));
    assert_eq!(resp.header("cache-control"), Some(REVALIDATE));
    // Security headers ride along on the 304.
    assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn api_a_non_matching_if_none_match_is_a_plain_200() {
    let w = World::new().await;
    let resp = w
        .api
        .get("/up", &[("if-none-match", "W/\"0000000000000000\"")])
        .await;
    assert_status(&resp, 200);
    assert!(!resp.body.is_empty());
}

#[tokio::test]
async fn api_a_json_response_without_etag_is_no_cache() {
    let w = World::new().await;
    let unauthorized = w.api.get("/api/v1/departments", &[]).await;
    assert_status(&unauthorized, 401);
    assert_eq!(unauthorized.header("etag"), None);
    assert_eq!(unauthorized.header("cache-control"), Some("no-cache"));
}

#[tokio::test]
async fn api_exceptions_app_pages_have_no_cache_control_or_etag() {
    let w = World::new().await;
    for path in ["/nope", "/api/v1/nope"] {
        let resp = w.api.get(path, &[]).await;
        assert_status(&resp, 404);
        assert_eq!(resp.header("etag"), None, "{path}");
        assert_eq!(resp.header("cache-control"), None, "{path}");
    }
}

#[tokio::test]
async fn api_head_carries_the_etag_of_the_get() {
    let w = World::new().await;
    let get = w.api.get("/up", &[]).await;
    let head = w.api.send(reqwest::Method::HEAD, "/up", &[], None).await;
    assert_status(&head, 200);
    assert_eq!(head.header("etag"), get.header("etag"));
    assert_eq!(head.header("cache-control"), Some(REVALIDATE));
}

#[tokio::test]
async fn api_docs_content_types_carry_the_utf8_charset() {
    let w = World::new().await;
    let spec = w.api.get("/api/v1/openapi.yaml", &[]).await;
    assert_eq!(
        spec.header("content-type"),
        Some("application/yaml; charset=utf-8")
    );
    let page = w.api.get("/api/docs", &[]).await;
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );
}

/// `send_file` is a streamed `FileResponse`: no ETag, never buffered.
#[tokio::test]
async fn api_download_headers_match_rails_send_file() {
    let w = World::new().await;
    let tree = w.read_write_tree().await;
    let up = common::upload(&w.api, &w.token, tree.finance, "cache.pdf", &[]).await;
    assert_status(&up, 201);
    let id = up.json()["id"].as_str().unwrap().to_owned();
    let resp = w
        .api
        .get_bearer(&format!("/api/v1/documents/{id}/download"), &w.token)
        .await;
    assert_status(&resp, 200);
    assert_eq!(resp.header("etag"), None);
    let mut seen: Vec<String> = resp
        .headers
        .iter()
        .map(|(k, v)| format!("{k}: {}", v.to_str().unwrap_or("?")))
        .collect();
    seen.sort();
    // Observed on the Rails stack: `no-cache`, no ETag, security headers present.
    assert_eq!(
        resp.header("cache-control"),
        Some("no-cache"),
        "download headers: {seen:#?}"
    );
    assert_eq!(resp.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(resp.header("content-transfer-encoding"), Some("binary"));
}
