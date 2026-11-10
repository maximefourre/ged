//! Port of `test/integration/api_docs_test.rb`.
//!
//! The spec file, the public Redoc page and the routed `/api/v1` paths stay
//! in lockstep. Parameter names (`{id}` vs `{document_id}`) compare equal.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use ged::config::Config;
use ged::http::AppState;
use ged::router;
use http_body_util::BodyExt;
use serde_yaml::Value;
use tower::ServiceExt;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn normalize(path: &str) -> String {
    // Same two substitutions as `ApiDocsTest#normalize`: Rails `:id`, then
    // OpenAPI `{id}`.
    let colon = regex::Regex::new(r":[A-Za-z_]+").expect("colon");
    let brace = regex::Regex::new(r"\{[^}]+\}").expect("brace");
    brace
        .replace_all(&colon.replace_all(path, "{}"), "{}")
        .into_owned()
}

fn spec_value() -> Value {
    serde_yaml::from_str(ged::api::docs::SPEC).expect("parse embedded openapi.yaml")
}

fn dig<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |cur, key| cur.get(*key))
}

fn app() -> axum::Router {
    let config = Config::from_lookup(&|key| (key == "RAILS_ENV").then(|| "test".to_owned()))
        .expect("test config");
    router(AppState::without_database(config))
}

async fn send(uri: &str) -> (StatusCode, String, String) {
    let response = app()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[test]
fn normalize_collapses_both_parameter_styles() {
    assert_eq!(normalize("/api/v1/documents/{id}"), "/api/v1/documents/{}");
    assert_eq!(
        normalize("/api/v1/documents/:id/tags"),
        "/api/v1/documents/{}/tags"
    );
    assert_eq!(normalize("/api/v1/ping"), "/api/v1/ping");
}

#[tokio::test]
async fn openapi_yaml_is_served_without_authentication() {
    let (status, content_type, body) = send("/api/v1/openapi.yaml").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("application/yaml"),
        "content type {content_type}"
    );
    let spec: Value = serde_yaml::from_str(&body).expect("response is yaml");
    let version = spec
        .get("openapi")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(version.starts_with("3.1."), "openapi version {version}");
}

#[tokio::test]
async fn api_docs_renders_redoc_pointing_at_the_spec() {
    let (status, content_type, body) = send("/api/docs").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("text/html"),
        "content type {content_type}"
    );
    assert!(body.contains("redoc"), "redoc missing");
    assert!(body.contains("/api/v1/openapi.yaml"));
}

#[test]
fn every_routed_api_v1_path_is_documented() {
    let spec = spec_value();
    let documented: BTreeSet<String> = spec
        .get("paths")
        .and_then(Value::as_mapping)
        .expect("paths")
        .keys()
        .filter_map(Value::as_str)
        .map(normalize)
        .collect();

    let api = root().join("rust/ged/src/api");
    let route = regex::Regex::new(r#"\.route\(\s*"(/api/v1/[^"]+)""#).expect("regex");
    let mut routed = BTreeSet::new();
    for entry in fs::read_dir(&api).expect("read api dir").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read route source");
        for found in route.captures_iter(&source) {
            let raw = found.get(1).expect("path").as_str();
            if raw == "/api/v1/openapi.yaml" {
                continue;
            }
            routed.insert(normalize(raw));
        }
    }
    assert!(!routed.is_empty(), "no /api/v1 routes found");
    let missing: Vec<_> = routed.difference(&documented).cloned().collect();
    assert!(
        missing.is_empty(),
        "undocumented API paths: {}",
        missing.join(", ")
    );
}

#[test]
fn documents_list_response_conforms_to_the_envelope_schema() {
    let spec = spec_value();
    let schema_ref = dig(
        &spec,
        &[
            "paths",
            "/api/v1/documents",
            "get",
            "responses",
            "200",
            "content",
            "application/json",
            "schema",
            "$ref",
        ],
    )
    .and_then(Value::as_str);
    assert_eq!(
        schema_ref,
        Some("#/components/schemas/DocumentList"),
        "documents list 200 response should $ref DocumentList"
    );

    let properties = dig(
        &spec,
        &["components", "schemas", "DocumentList", "properties"],
    )
    .and_then(Value::as_mapping)
    .expect("DocumentList schema");
    let mut keys: Vec<_> = properties.keys().filter_map(Value::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["documents", "pagination"]);

    let pagination = dig(
        &spec,
        &["components", "schemas", "Pagination", "properties"],
    )
    .and_then(Value::as_mapping)
    .expect("Pagination schema");
    let mut page_keys: Vec<_> = pagination.keys().filter_map(Value::as_str).collect();
    page_keys.sort_unstable();
    assert_eq!(page_keys, ["limit", "offset", "total"]);
}
