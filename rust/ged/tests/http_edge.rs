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

/// Pre-auth DoS guard, end to end: the guard buffers and parses an urlencoded
/// body before any handler or authentication, so many distinct keys must not
/// cost quadratic time (80 000 keys took ~4.9 s in release before the index).
/// Rack caps a form at 4096 parts, so the largest body that is still parsed
/// has 4096 keys and anything bigger is refused without being parsed at all.
#[tokio::test]
async fn a_form_body_of_many_distinct_keys_is_guarded_in_linear_time() {
    let form = Some("application/x-www-form-urlencoded");
    let keys = |n: usize| {
        (0..n)
            .map(|i| format!("k{i}=1"))
            .collect::<Vec<_>>()
            .join("&")
    };
    let started = std::time::Instant::now();
    let r = request(
        "POST",
        "/admin/probe",
        form,
        &format!("{}&name=x", keys(4095)),
    )
    .await;
    let refused = request("POST", "/admin/probe", form, &keys(80_000)).await;
    let elapsed = started.elapsed();
    eprintln!("guard of 4096 then 80000 distinct keys: {elapsed:?}");
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, r#"name=Some("x")"#);
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
}

const FORM: Option<&str> = Some("application/x-www-form-urlencoded");
const MIB4: usize = 4 * 1024 * 1024;

fn is_bad_request_json(r: &support::Reply) -> bool {
    r.status == StatusCode::BAD_REQUEST
        && r.content_type == JSON_CT
        && r.body == r#"{"error":"bad request"}"#
}

/// Rack 3.2 (measured on the Rails stack): 4096 `&`-separated parts are
/// parsed, 4097 are a 400 — every `&` counts, empty parts included.
#[tokio::test]
async fn a_form_body_is_capped_at_4096_parts() {
    let at = format!("name=x{}", "&a".repeat(4095));
    let r = request("POST", "/api/v1/probe", FORM, &at).await;
    assert_eq!(r.body, r#"name=Some("x")"#);
    let over = format!("name=x{}", "&a".repeat(4096));
    assert!(is_bad_request_json(
        &request("POST", "/api/v1/probe", FORM, &over).await
    ));
    // empty parts count; `;` does not separate
    let amps = "&".repeat(4095);
    assert_eq!(
        request("POST", "/api/v1/probe", FORM, &amps).await.status,
        StatusCode::OK
    );
    let amps = "&".repeat(4096);
    assert!(is_bad_request_json(
        &request("POST", "/api/v1/probe", FORM, &amps).await
    ));
    let semis = format!("name=x{}", ";a".repeat(5000));
    assert_eq!(
        request("POST", "/api/v1/probe", FORM, &semis).await.status,
        StatusCode::OK
    );
    // outside the API: the HTML 400, not JSON
    let r = request("POST", "/admin/probe", FORM, &over).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.content_type.starts_with("text/html"),
        "{}",
        r.content_type
    );
}

/// Rack 3.2 (measured): a form of exactly 4 MiB is parsed, one byte more is a
/// 400 (JSON under the API) — before the handler, on the guard and the extractor.
#[tokio::test]
async fn a_form_body_is_capped_at_4_mib() {
    let at = format!("name={}", "x".repeat(MIB4 - 5));
    assert_eq!(at.len(), MIB4);
    let r = request("POST", "/api/v1/probe", FORM, &at).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.starts_with("name=Some(\"xxx"), "{}", &r.body[..40]);
    let over = format!("{at}x");
    assert!(is_bad_request_json(
        &request("POST", "/api/v1/probe", FORM, &over).await
    ));
    assert_eq!(
        request("POST", "/admin/probe", FORM, &over).await.status,
        StatusCode::BAD_REQUEST
    );
}

/// The query is limited by the web server in front of Rails (Puma: 10 240
/// bytes), which answers before routing with an empty 400 — no content type,
/// whatever the path or verb. There is no part-count limit on a query.
#[tokio::test]
async fn a_query_is_capped_at_10240_bytes_with_an_empty_400() {
    let at = "k".repeat(10 * 1024);
    assert_eq!(get_(&format!("/up?{at}")).await.status, StatusCode::OK);
    let over = format!("{at}k");
    for (method, path) in [
        ("GET", "/up"),
        ("GET", "/api/v1/probe"),
        ("GET", "/nope"),
        ("GET", "/api/v1/nope"),
        ("POST", "/up"),
        ("DELETE", "/api/v1/probe"),
    ] {
        let r = request(method, &format!("{path}?{over}"), None, "").await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{method} {path}");
        assert_eq!(r.body, "", "{method} {path}");
        assert_eq!(r.content_type, "", "{method} {path}");
    }
    let many = "a&".repeat(5000);
    assert_eq!(get_(&format!("/up?{many}")).await.status, StatusCode::OK);
    let amps = "&".repeat(10 * 1024);
    assert_eq!(get_(&format!("/up?{amps}")).await.status, StatusCode::OK);
}

// --- Rails' controller-layer headers, ETag and the edge limit ----------------

mod rails_layer {
    use super::support::app_with_probes;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ged::config::Config;
    use ged::{AppState, router_with};
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    async fn call(req: Request<Body>) -> axum::http::Response<Body> {
        app_with_probes().oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn etag_is_the_weak_32_hex_sha256_of_the_body() {
        let response = call(Request::get("/up").body(Body::empty()).unwrap()).await;
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        assert_eq!(
            response.headers()["cache-control"],
            "max-age=0, private, must-revalidate"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let digest = Sha256::digest(&body);
        let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(etag, format!("W/\"{hex}\""));
    }

    #[tokio::test]
    async fn controller_answers_get_security_headers_and_the_exceptions_app_does_not() {
        let up = call(Request::get("/up").body(Body::empty()).unwrap()).await;
        assert_eq!(up.headers()["x-frame-options"], "SAMEORIGIN");
        assert_eq!(up.headers()["x-xss-protection"], "0");
        assert_eq!(up.headers()["x-content-type-options"], "nosniff");
        assert_eq!(up.headers()["x-permitted-cross-domain-policies"], "none");
        assert_eq!(
            up.headers()["referrer-policy"],
            "strict-origin-when-cross-origin"
        );
        assert!(up.headers().get("content-security-policy").is_none());
        let miss = call(Request::get("/nope").body(Body::empty()).unwrap()).await;
        assert!(miss.headers().get("x-frame-options").is_none());
        assert!(miss.headers().get("cache-control").is_none());
        let boom = call(Request::get("/api/v1/boom").body(Body::empty()).unwrap()).await;
        assert_eq!(boom.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(boom.headers().get("x-frame-options").is_none());
    }

    #[tokio::test]
    async fn a_matching_if_none_match_is_an_empty_304_compared_verbatim() {
        let first = call(Request::get("/up").body(Body::empty()).unwrap()).await;
        let etag = first.headers()["etag"].clone();
        let hit = call(
            Request::get("/up")
                .header("if-none-match", etag.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(hit.status(), StatusCode::NOT_MODIFIED);
        assert!(hit.headers().get("content-type").is_none());
        // (Over the wire hyper sends no Content-Length on a 304: parity-checked.)
        assert!(hit.headers().get("content-length").is_none_or(|v| v == "0"));
        assert_eq!(hit.headers()["etag"], etag);
        let strong = etag.to_str().unwrap().trim_start_matches("W/").to_owned();
        let miss = call(
            Request::get("/up")
                .header("if-none-match", strong)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(miss.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_chunked_body_past_the_limit_is_an_empty_bare_413_on_any_endpoint() {
        let config = Config::from_lookup(&|k| match k {
            "RAILS_ENV" => Some("test".to_owned()),
            "GED_MAX_UPLOAD_BYTES" => Some("1024".to_owned()),
            _ => None,
        })
        .unwrap();
        let app = router_with(
            AppState::without_database(config),
            axum::Router::new().route(
                "/api/v1/echo",
                axum::routing::post(
                    |body: axum::body::Bytes| async move { body.len().to_string() },
                ),
            ),
        );
        let chunks = futures_util::stream::iter(
            (0..4).map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b'x'; 512]))),
        );
        let response = app
            .oneshot(
                Request::post("/api/v1/echo")
                    .body(Body::from_stream(chunks))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(response.headers().get("content-type").is_none());
        assert!(response.headers().get("x-frame-options").is_none());
    }
}
