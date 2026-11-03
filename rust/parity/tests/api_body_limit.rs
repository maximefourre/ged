//! The edge limit (Thruster `MAX_REQUEST_BODY` = `GED_MAX_UPLOAD_BYTES`)
//! applies to every request, before routing and authentication, not only to
//! the upload: an empty 413 with no Content-Type and no Rails headers.

mod common;

use common::World;
use parity::json::assert_status;
use reqwest::Method;

const EDGE_LIMIT: usize = 52_428_800;

#[tokio::test]
async fn api_an_oversized_body_is_an_empty_413_on_every_endpoint_before_auth() {
    let w = World::new().await;
    let id = "00000000-0000-4000-8000-000000000000";
    let cases = [
        (Method::PUT, format!("/api/v1/documents/{id}/tags")),
        (Method::DELETE, "/api/v1/documents/bulk_destroy".to_owned()),
        (Method::POST, "/api/v1/departments".to_owned()),
        (Method::POST, "/api/v1/documents".to_owned()),
    ];
    for (method, path) in cases {
        let body = vec![b' '; EDGE_LIMIT + 1];
        let resp = w
            .api
            .send(
                method.clone(),
                &path,
                &[("content-type", "application/json")],
                Some(body),
            )
            .await;
        assert_status(&resp, 413);
        assert_eq!(resp.body, b"", "{method} {path}");
        assert_eq!(resp.header("content-type"), None, "{method} {path}");
        assert_eq!(resp.header("x-frame-options"), None, "{method} {path}");
    }
}

#[tokio::test]
async fn api_a_small_body_still_reaches_the_controller() {
    let w = World::new().await;
    let body = vec![b' '; 1024];
    let resp = w
        .api
        .send(
            Method::POST,
            "/api/v1/departments",
            &[("content-type", "application/json")],
            Some(body),
        )
        .await;
    assert_status(&resp, 401);
}
