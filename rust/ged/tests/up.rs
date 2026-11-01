use axum::{body::Body, http::Request};
use tower::ServiceExt;

#[tokio::test]
async fn up_returns_200() {
    let response = ged::router()
        .oneshot(Request::get("/up").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}
