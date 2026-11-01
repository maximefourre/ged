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
//! - that routing miss comes before the parameter check: an unparsable query
//!   on a verb the route does not serve is the 404, not a 400;
//! - shapes that are odd but parseable (`limit[]=1`, last duplicate wins, …)
//!   are not errors;
//! - input size, measured at both edges of each limit: an urlencoded body is
//!   capped at 4 MiB and 4096 `&`-separated parts (Rack 3.2 defaults), the
//!   query string at 10 240 bytes (the web server's own line limit, which
//!   answers before Rails with an empty 400) and has no part-count limit.
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
async fn api_errors_a_verb_the_route_does_not_serve_is_a_404_even_with_an_unparsable_query() {
    let api = api().await;
    for query in ["a=%ff", "a=1&a[b]=2", &nested(100)] {
        for verb in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            let resp = api
                .send(verb.clone(), &format!("/up?{query}"), &[], None)
                .await;
            assert_status(&resp, 404);
        }
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

// --- input size ------------------------------------------------------------
//
// Observed on the Rails stack (2026-11-01, through Thruster):
//   body, 4 MiB exactly            -> parsed        (4 MiB + 1 byte -> 400)
//   body, 4096 `&`-separated parts -> parsed        (4097 parts     -> 400)
//   query, 10 240 bytes            -> parsed        (10 241 bytes   -> 400,
//                                     empty body, no content-type)
//   query, 5000 parts in < 10 KiB  -> parsed (no part-count limit)
// A body is only read once a route matches, so `GET /up` with a form body
// stands in for a routed business endpoint (Rack parses a GET form too).

const FORM: [(&str, &str); 1] = [("Content-Type", "application/x-www-form-urlencoded")];
const MIB4: usize = 4 * 1024 * 1024;
const QUERY_MAX: usize = 10 * 1024;

fn parts(n: usize) -> String {
    vec!["a"; n].join("&")
}

async fn form_get(api: &Api, path: &str, body: String) -> parity::http::Resp {
    api.send(Method::GET, path, &FORM, Some(body.into_bytes()))
        .await
}

#[tokio::test]
async fn api_errors_a_form_body_of_exactly_4_mib_is_parsed_and_one_more_byte_is_a_400() {
    let api = api().await;
    assert_status(&form_get(&api, "/up", "k".repeat(MIB4)).await, 200);
    let over = form_get(&api, "/up", "k".repeat(MIB4 + 1)).await;
    assert_status(&over, 400);
    let content_type = over.header("content-type").unwrap_or_default();
    assert!(content_type.starts_with("text/html"), "{content_type}");
    // `a=` + value, so the limit is on bytes, not on the key.
    assert_status(
        &form_get(&api, "/up", format!("a={}", "x".repeat(MIB4 - 2))).await,
        200,
    );
    assert_status(
        &form_get(&api, "/up", format!("a={}", "x".repeat(MIB4 - 1))).await,
        400,
    );
}

#[tokio::test]
async fn api_errors_a_form_body_of_4096_parts_is_parsed_and_4097_is_a_400() {
    let api = api().await;
    for (body, expected, what) in [
        (parts(4095), 200, "4095 parts"),
        (parts(4096), 200, "4096 parts"),
        (parts(4097), 400, "4097 parts"),
        // every `&` counts, empty parts and duplicates included
        ("&".repeat(4095), 200, "4095 `&`"),
        ("&".repeat(4096), 400, "4096 `&`"),
        (vec!["a=1"; 4096].join("&"), 200, "4096 duplicate keys"),
        (vec!["a=1"; 4097].join("&"), 400, "4097 duplicate keys"),
        (vec!["a[]=1"; 4097].join("&"), 400, "4097 array items"),
        // only `&` separates for this count: `;` does not
        (vec!["a"; 4097].join(";"), 200, "4097 `;`-separated"),
    ] {
        let resp = form_get(&api, "/up", body).await;
        assert_eq!(resp.status, expected, "{what}");
    }
}

#[tokio::test]
async fn api_errors_the_form_size_limits_never_turn_an_unrouted_path_into_a_400() {
    let api = api().await;
    for path in ["/nope", "/api/v1/nope"] {
        for body in ["k".repeat(MIB4 + 1), parts(4097)] {
            assert_status(&form_get(&api, path, body).await, 404);
        }
    }
    // and only a *form* is subject to them
    let json = api
        .send(
            Method::GET,
            "/up",
            &[("Content-Type", "application/json")],
            Some(vec![b' '; MIB4 + 1]),
        )
        .await;
    assert_status(&json, 200);
}

#[tokio::test]
async fn api_errors_a_query_of_10240_bytes_is_parsed_and_one_more_byte_is_an_empty_400() {
    let api = api().await;
    assert_status(
        &api.get(&format!("/up?{}", "k".repeat(QUERY_MAX)), &[])
            .await,
        200,
    );
    for path in ["/up", "/nope", "/api/v1/nope", "/api/v1/documents"] {
        let resp = api
            .get(&format!("{path}?{}", "k".repeat(QUERY_MAX + 1)), &[])
            .await;
        assert_status(&resp, 400);
        assert!(resp.body.is_empty(), "{path}: {:?}", resp.text());
        assert_eq!(resp.header("content-type"), None, "{path}");
    }
    // the server rejects before any routing, so the verb does not matter
    for verb in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let resp = api
            .send(
                verb.clone(),
                &format!("/up?{}", "k".repeat(QUERY_MAX + 1)),
                &[],
                None,
            )
            .await;
        assert_status(&resp, 400);
        assert!(resp.body.is_empty(), "{verb}");
    }
}

#[tokio::test]
async fn api_errors_a_query_has_no_part_count_limit_below_its_byte_limit() {
    let api = api().await;
    for n in [4096, 4097, 5000] {
        assert_status(&api.get(&format!("/up?{}", parts(n)), &[]).await, 200);
    }
    assert_status(
        &api.get(&format!("/up?{}", "&".repeat(QUERY_MAX)), &[])
            .await,
        200,
    );
}
