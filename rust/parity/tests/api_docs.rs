//! Port of the parity rows in `test/integration/api_docs_test.rb`.
//! The "every routed path is documented" check stays a Rust unit test: it
//! reads the router's table, which is not an HTTP observation.

mod common;

use common::World;
use parity::json::assert_status;
use serde_yaml::Value;

#[tokio::test]
async fn api_openapi_yaml_is_served_without_authentication() {
    let w = World::new().await;
    let resp = w.api.get("/api/v1/openapi.yaml", &[]).await;
    assert_status(&resp, 200);
    assert_eq!(
        media_type(resp.header("content-type").unwrap_or_default()),
        "application/yaml"
    );
    let spec = parse_spec(&resp.text());
    let version = spec["openapi"]
        .as_str()
        .unwrap_or_else(|| panic!("openapi is not a string: {:?}", spec["openapi"]));
    assert!(
        version.starts_with("3.1."),
        "spec does not declare OpenAPI 3.1: {version}"
    );
}

#[tokio::test]
async fn api_docs_renders_the_redoc_page_pointing_at_the_spec() {
    let w = World::new().await;
    let resp = w.api.get("/api/docs", &[]).await;
    assert_status(&resp, 200);
    assert_eq!(
        media_type(resp.header("content-type").unwrap_or_default()),
        "text/html"
    );
    let body = resp.text();
    assert!(body.contains("redoc"), "page does not mention redoc");
    assert!(body.contains("/api/v1/openapi.yaml"));
}

#[tokio::test]
async fn api_documents_list_response_conforms_to_envelope_schema() {
    let w = World::new().await;
    let resp = w.api.get("/api/v1/openapi.yaml", &[]).await;
    assert_status(&resp, 200);
    let spec = parse_spec(&resp.text());

    let list_response = dig(
        &spec,
        &["paths", "/api/v1/documents", "get", "responses", "200"],
    );
    let schema_ref = dig(
        list_response,
        &["content", "application/json", "schema", "$ref"],
    );
    assert_eq!(
        schema_ref.as_str(),
        Some("#/components/schemas/DocumentList"),
        "documents list 200 response should $ref DocumentList"
    );

    let document_list = dig(&spec, &["components", "schemas", "DocumentList"]);
    assert_eq!(
        mapping_keys(dig(document_list, &["properties"])),
        ["documents", "pagination"],
        "DocumentList should have documents and pagination properties"
    );

    let pagination = dig(&spec, &["components", "schemas", "Pagination"]);
    assert_eq!(
        mapping_keys(dig(pagination, &["properties"])),
        ["limit", "offset", "total"],
        "Pagination should have limit, offset, and total properties"
    );
}

/// Rails `response.media_type`: the type, without parameters such as charset.
fn media_type(header: &str) -> &str {
    header.split(';').next().unwrap_or("").trim()
}

fn parse_spec(body: &str) -> Value {
    serde_yaml::from_str(body).unwrap_or_else(|e| panic!("openapi body is not YAML ({e})"))
}

fn dig<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .unwrap_or_else(|| panic!("missing {key} under {path:?}"));
    }
    current
}

fn mapping_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_mapping()
        .unwrap_or_else(|| panic!("expected a mapping, got {value:?}"))
        .keys()
        .map(|key| {
            key.as_str()
                .unwrap_or_else(|| panic!("non-string key in mapping: {key:?}"))
        })
        .collect();
    keys.sort_unstable();
    keys
}
