//! Port of the parity rows in `test/integration/api_docs_test.rb`.
//! The "every routed path is documented" check stays a Rust unit test: it
//! reads the router's table, which is not an HTTP observation.

mod common;

use common::World;
use parity::json::assert_status;

#[tokio::test]
async fn api_openapi_yaml_is_served_without_authentication() {
    let w = World::new().await;
    let resp = w.api.get("/api/v1/openapi.yaml", &[]).await;
    assert_status(&resp, 200);
    let ct = resp.header("content-type").unwrap_or_default();
    assert!(ct.starts_with("application/yaml"), "content-type is {ct:?}");
    let body = resp.text();
    assert!(
        body.starts_with("openapi: 3.1.") || body.contains("\nopenapi: 3.1."),
        "spec does not declare OpenAPI 3.1: {}",
        &body[..body.len().min(80)]
    );
}

#[tokio::test]
async fn api_docs_renders_the_redoc_page_pointing_at_the_spec() {
    let w = World::new().await;
    let resp = w.api.get("/api/docs", &[]).await;
    assert_status(&resp, 200);
    let ct = resp.header("content-type").unwrap_or_default();
    assert!(ct.starts_with("text/html"), "content-type is {ct:?}");
    let body = resp.text();
    assert!(body.contains("redoc"), "page does not mention redoc");
    assert!(body.contains("/api/v1/openapi.yaml"));
}

#[tokio::test]
async fn api_documents_list_response_conforms_to_envelope_schema() {
    let w = World::new().await;
    let resp = w.api.get("/api/v1/openapi.yaml", &[]).await;
    assert_status(&resp, 200);
    let body = resp.text();
    assert!(
        body.contains("$ref: \"#/components/schemas/DocumentList\""),
        "documents list 200 does not $ref DocumentList"
    );
    let list = section_after(&body, "    DocumentList:");
    assert!(
        list.contains("\n        documents:"),
        "DocumentList.documents"
    );
    assert!(
        list.contains("\n        pagination:"),
        "DocumentList.pagination"
    );
    let pagination = section_after(&body, "    Pagination:");
    assert!(pagination.contains("\n        limit:"), "Pagination.limit");
    assert!(
        pagination.contains("\n        offset:"),
        "Pagination.offset"
    );
    assert!(pagination.contains("\n        total:"), "Pagination.total");
}

/// Body of one 4-space schema heading, up to the next schema heading.
fn section_after<'a>(body: &'a str, heading: &str) -> &'a str {
    let start = body
        .find(heading)
        .unwrap_or_else(|| panic!("missing {heading}"));
    let after = body[start + heading.len()..].trim_start_matches('\n');
    let bytes = after.as_bytes();
    let mut i = 0;
    while i + 5 < bytes.len() {
        let at_schema = bytes[i..].starts_with(b"\n    ")
            && !bytes[i..].starts_with(b"\n     ")
            && bytes[i + 5].is_ascii_alphabetic();
        if at_schema {
            return &after[..i];
        }
        i += 1;
    }
    after
}
