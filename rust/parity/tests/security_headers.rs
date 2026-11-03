//! Rails 8.1 `default_headers` (`load_defaults 7.0+`), observed on the stack
//! (2026-11-03): set by `ActionDispatch::Response`, so on every response a
//! *controller* produces — JSON success and error, `/up`, Redoc, OpenAPI — and
//! on none produced elsewhere: the exceptions app (routing miss, unparsable
//! params), `public/*.html`, and the 413 of the edge (Thruster) carry none.
//! No global CSP, no HSTS.

mod common;

use common::World;
use parity::json::assert_status;
use parity::{Api, Resp};

const SECURITY: [(&str, &str); 5] = [
    ("x-frame-options", "SAMEORIGIN"),
    ("x-xss-protection", "0"),
    ("x-content-type-options", "nosniff"),
    ("x-permitted-cross-domain-policies", "none"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
];
const EDGE_LIMIT: usize = 52_428_800;

fn assert_security_headers(resp: &Resp, what: &str) {
    for (name, value) in SECURITY {
        assert_eq!(resp.header(name), Some(value), "{what}: {name}");
    }
    for absent in ["content-security-policy", "strict-transport-security"] {
        assert_eq!(resp.header(absent), None, "{what}: {absent}");
    }
}

fn assert_no_security_headers(resp: &Resp, what: &str) {
    for (name, _) in SECURITY {
        assert_eq!(resp.header(name), None, "{what}: {name}");
    }
}

#[tokio::test]
async fn security_headers_on_a_json_200() {
    let w = World::new().await;
    let resp = w.api.get_bearer("/api/v1/departments", &w.token).await;
    assert_status(&resp, 200);
    assert_security_headers(&resp, "200 JSON");
}

#[tokio::test]
async fn security_headers_on_a_json_4xx_from_a_controller() {
    let w = World::new().await;
    let unauthorized = w.api.get("/api/v1/departments", &[]).await;
    assert_status(&unauthorized, 401);
    assert_security_headers(&unauthorized, "401 JSON");
    let missing = w
        .api
        .get_bearer(
            "/api/v1/documents/00000000-0000-4000-8000-000000000000",
            &w.token,
        )
        .await;
    assert_status(&missing, 404);
    assert_security_headers(&missing, "404 JSON from the controller");
}

#[tokio::test]
async fn security_headers_on_up_and_the_docs_pages() {
    let w = World::new().await;
    for path in ["/up", "/api/docs", "/api/v1/openapi.yaml"] {
        let resp = w.api.get(path, &[]).await;
        assert_status(&resp, 200);
        assert_security_headers(&resp, path);
    }
}

#[tokio::test]
async fn security_headers_are_absent_from_errors_the_exceptions_app_renders() {
    let w = World::new().await;
    let html = w.api.get("/nope", &[]).await;
    assert_status(&html, 404);
    assert_no_security_headers(&html, "404 HTML");
    let json = w.api.get("/api/v1/nope", &[]).await;
    assert_status(&json, 404);
    assert_no_security_headers(&json, "routing 404 JSON");
    let bad = w.api.get("/api/v1/departments?a=%ff", &[]).await;
    assert_status(&bad, 400);
    assert_no_security_headers(&bad, "400 unparsable query");
}

#[tokio::test]
async fn security_headers_are_absent_from_the_edge_413() {
    let w = World::new().await;
    let api = Api::new(w.backend.base_url());
    let body = vec![b'a'; EDGE_LIMIT + 1];
    let resp = api
        .upload_bearer(
            "/api/v1/documents",
            &w.token,
            &[("department_id", "1")],
            "huge.pdf",
            "application/pdf",
            &body,
        )
        .await;
    assert_status(&resp, 413);
    assert_eq!(resp.body, b"");
    assert_no_security_headers(&resp, "413");
}
