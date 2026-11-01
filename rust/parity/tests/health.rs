//! Port of `test/integration/health_check_test.rb`.

mod common;

use common::World;
use parity::json::assert_status;

#[tokio::test]
async fn api_up_returns_200() {
    let w = World::new().await;
    let resp = w.api.get("/up", &[]).await;
    assert_status(&resp, 200);
}
