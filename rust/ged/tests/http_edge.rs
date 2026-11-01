//! The HTTP edge contract: JSON errors under `/api/v1`, parameter parsing
//! done before dispatch, nothing internal in a response. Each expectation
//! was observed on the Rails stack (see `rust/parity/tests/api_errors.rs`).

mod support;

use axum::http::StatusCode;
use support::{JSON_CT, get_, request};

#[tokio::test]
async fn up_is_the_green_page() {
    let r = get_("/up").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.content_type, "text/html; charset=utf-8");
    assert_eq!(
        r.body,
        "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>"
    );
}

#[tokio::test]
async fn unknown_api_routes_are_json_404s_for_every_verb() {
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        let r = request(method, "/api/v1/nope", None, "").await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(r.content_type, JSON_CT, "{method}");
        assert_eq!(r.body, r#"{"error":"not found"}"#, "{method}");
    }
}

#[tokio::test]
async fn wrong_verb_on_a_known_api_path_is_the_same_404() {
    let r = request("DELETE", "/api/v1/probe", None, "").await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (StatusCode::NOT_FOUND, r#"{"error":"not found"}"#)
    );
}

#[tokio::test]
async fn unknown_paths_outside_the_api_are_not_json() {
    let r = get_("/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(
        r.content_type.starts_with("text/html"),
        "{}",
        r.content_type
    );
}

#[tokio::test]
async fn a_bad_query_is_a_json_400_on_a_matched_api_route() {
    for qs in ["a=%ff", "a=%zz", "a=1&a[b]=2", "a[]=1&a[b]=2"] {
        let r = get_(&format!("/api/v1/probe?{qs}")).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{qs}");
        assert_eq!(r.content_type, JSON_CT, "{qs}");
        assert_eq!(r.body, r#"{"error":"bad request"}"#, "{qs}");
    }
}

#[tokio::test]
async fn a_bad_query_is_a_400_even_on_up_but_never_on_an_unmatched_path() {
    assert_eq!(get_("/up?a=%ff").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        get_("/api/v1/nope?a=%ff").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_unparsable_urlencoded_body_is_a_400_before_the_handler() {
    let ct = Some("application/x-www-form-urlencoded");
    let r = request("POST", "/api/v1/probe", ct, "name=%ff").await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (StatusCode::BAD_REQUEST, r#"{"error":"bad request"}"#)
    );
    let r = request("POST", "/api/v1/probe", ct, "a=1&a[b]=2").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_valid_urlencoded_body_reaches_the_handler_intact() {
    let ct = Some("application/x-www-form-urlencoded");
    let r = request("POST", "/api/v1/probe", ct, "name=Compta+%C3%A9").await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (StatusCode::OK, r#"name=Some("Compta é")"#)
    );
}

#[tokio::test]
async fn malformed_json_is_a_json_400_when_the_handler_reads_params() {
    for body in ["{bad", "{\"name\":", "", " ", "{\"a\":1,}"] {
        let r = request("POST", "/api/v1/probe", Some("application/json"), body).await;
        if body.is_empty() {
            // An empty body is "no parameters", not malformed.
            assert_eq!(r.status, StatusCode::OK, "{body:?}");
            continue;
        }
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body:?}");
        assert_eq!(r.content_type, JSON_CT);
        assert_eq!(r.body, r#"{"error":"bad request"}"#);
    }
}

#[tokio::test]
async fn json_bodies_parse_for_every_json_content_type_rails_knows() {
    for ct in [
        "application/json",
        "Application/JSON; charset=utf-8",
        "text/x-json",
        "application/jsonrequest",
    ] {
        let r = request("POST", "/api/v1/probe", Some(ct), r#"{"name":"x"}"#).await;
        assert_eq!(
            (r.status, r.body.as_str()),
            (StatusCode::OK, r#"name=Some("x")"#),
            "{ct}"
        );
    }
    // Not a JSON type to Rails: the body is ignored, not rejected.
    let r = request(
        "POST",
        "/api/v1/probe",
        Some("application/vnd.api+json"),
        "{bad",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn array_limit_is_present_but_ignored() {
    let r = get_("/api/v1/probe?limit[]=1").await;
    assert_eq!(r.body, "limit=None present=true");
    let r = get_("/api/v1/probe?limit=5&limit=7").await;
    assert_eq!(r.body, r#"limit=Some("7") present=true"#);
    let r = get_("/api/v1/probe").await;
    assert_eq!(r.body, "limit=None present=false");
}

#[tokio::test]
async fn a_panic_is_a_generic_json_500_with_no_detail() {
    let r = get_("/api/v1/boom").await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(r.content_type, JSON_CT);
    assert_eq!(r.body, r#"{"error":"internal server error"}"#);
    assert!(!r.body.contains("SELECT"));
}

#[tokio::test]
async fn a_panic_outside_the_api_is_not_json_and_leaks_nothing() {
    let r = get_("/admin/probe").await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(r.content_type.starts_with("text/html"));
    assert!(!r.body.contains("SELECT"));
}

#[tokio::test]
async fn framework_rejections_under_the_api_use_the_json_contract() {
    let r = get_("/api/v1/items/abc").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.content_type, JSON_CT);
    assert_eq!(r.body, r#"{"error":"bad request"}"#);
    assert_eq!(get_("/api/v1/items/12").await.body, "12");
}

#[tokio::test]
async fn wrong_verb_outside_the_api_is_a_404_page_not_a_405() {
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let r = request(method, "/up", None, "").await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{method}");
        assert!(r.content_type.starts_with("text/html"), "{method}");
    }
}
