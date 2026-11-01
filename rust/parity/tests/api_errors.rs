//! The error edge, observable without a token or any business route: only
//! `/up` and paths that do not exist are used, so the module runs against a
//! backend that has no API yet. Every expectation was read off the Rails
//! stack first (`GED_BACKEND=rails cargo test -p parity api_errors`).
//!
//! What it fixes:
//! - an unrouted path (any verb, any query) under `/api/v1` is a JSON
//!   `404 not found`; elsewhere it is a 404 page;
//! - a verb the route does not serve is the same 404, not a 405;
//! - the query string is parsed before dispatch on every *routed* request,
//!   so an unparsable one is a 400 even on `/up`, and never on an unrouted
//!   path;
//! - shapes that are odd but parseable (`limit[]=1`, last duplicate wins, …)
//!   are not errors.
//!
//! Not asserted here, on purpose: a `%` that is not followed by two hex
//! digits. The Rails stack answers 200 because its front proxy drops the
//! parameter before Rails sees it; Rails alone answers 400
//! (`docs/migration/parity-gaps.md`).

use parity::json::{assert_error, assert_status};
use parity::{Api, backend};
use reqwest::Method;

const VERBS: [Method; 5] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
];

async fn api() -> Api {
    Api::new(backend().await.base_url())
}

fn nested(depth: usize) -> String {
    format!("a{}=1", "[x]".repeat(depth))
}

#[tokio::test]
async fn api_errors_unrouted_api_paths_are_a_json_404_for_every_verb() {
    let api = api().await;
    for path in [
        "/api/v1/nope",
        "/api/v1/documents/does/not/exist",
        "/api/v1",
    ] {
        for verb in VERBS {
            let resp = api.send(verb.clone(), path, &[], None).await;
            assert_error(&resp, 404, "not found");
        }
    }
}

#[tokio::test]
async fn api_errors_a_verb_the_route_does_not_serve_is_a_404_not_a_405() {
    let api = api().await;
    for verb in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let resp = api.send(verb.clone(), "/up", &[], None).await;
        assert_status(&resp, 404);
    }
}

#[tokio::test]
async fn api_errors_unrouted_paths_outside_the_api_are_a_404_page() {
    let api = api().await;
    let resp = api.get("/nope", &[]).await;
    assert_status(&resp, 404);
    let content_type = resp.header("content-type").unwrap_or_default();
    assert!(content_type.starts_with("text/html"), "{content_type}");
}

#[tokio::test]
async fn api_errors_an_unparsable_query_never_turns_an_unrouted_path_into_a_400() {
    let api = api().await;
    for query in ["a=%ff", "a=1&a[b]=2", &nested(100)] {
        let resp = api.get(&format!("/api/v1/nope?{query}"), &[]).await;
        assert_error(&resp, 404, "not found");
        assert_status(&api.get(&format!("/nope?{query}"), &[]).await, 404);
    }
}

#[tokio::test]
async fn api_errors_an_unparsable_query_is_a_400_on_a_routed_path() {
    let api = api().await;
    for query in [
        "a=%ff".to_owned(),
        "%ff=1".to_owned(),
        "a=1&a[b]=2".to_owned(),
        "a[]=1&a[b]=2".to_owned(),
        "a[b]=1&a[]=2".to_owned(),
        "a[b]=1&a[b][c]=2".to_owned(),
        "a=1&a[]=2".to_owned(),
        "ok=1&b[]=%ff".to_owned(),
        nested(100),
        nested(150),
    ] {
        let resp = api.get(&format!("/up?{query}"), &[]).await;
        assert_eq!(resp.status, 400, "/up?{query}");
    }
}

#[tokio::test]
async fn api_errors_odd_but_parseable_queries_are_not_errors() {
    let api = api().await;
    for query in [
        "limit[]=1".to_owned(),
        "limit[a]=1".to_owned(),
        "limit=5&limit=7".to_owned(),
        "limit[]=1&limit=5".to_owned(),
        "a[b]=1&a=2".to_owned(),
        "a[b][c]=1&a[b]=2".to_owned(),
        "a[]=1&a=2".to_owned(),
        "a[][b]=1&a[][c]=2".to_owned(),
        "tags[type]=facture&tags[urgent]".to_owned(),
        "a=1;b=2".to_owned(),
        "a=&b&=c&&".to_owned(),
        "a=%c3%a9".to_owned(),
        nested(99),
    ] {
        let resp = api.get(&format!("/up?{query}"), &[]).await;
        assert_eq!(resp.status, 200, "/up?{query}");
    }
}

#[tokio::test]
async fn api_errors_an_unparsable_form_body_on_an_unrouted_path_is_still_a_404() {
    // The form is only read once a route matches; the routed half
    // (`POST /api/v1/…` with a bad body is a 400) belongs to the API modules,
    // which need business routes.
    let api = api().await;
    let resp = api
        .send(
            Method::POST,
            "/api/v1/nope",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(b"a=%ff".to_vec()),
        )
        .await;
    assert_error(&resp, 404, "not found");
}
