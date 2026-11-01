//! HTTP ports of `test/controllers/admin/authentication_test.rb` and
//! `test/controllers/admin/login_rate_limit_test.rb`.
//!
//! Rails limits `POST /admins/sign_in` to 10 attempts per 3 minutes in one
//! process, and that counter cannot be cleared from outside the app. The
//! budget is shared with `AdminSession::cached` (one session for the rest of
//! the suite): these tests wait for a free window, and the exhausting test
//! holds the suite lock until the window is clear again.

mod common;

use common::settle_login_budget;
use parity::admin::{
    AdminSession, SuiteLock, login_succeeded, login_throttled, wait_for_login_budget,
};
use parity::stack::{admin_credentials, stack};

#[tokio::test]
async fn admin_unauthenticated_access_to_the_admin_area_redirects_to_login() {
    let _lock = SuiteLock::acquire().await;
    let anon = AdminSession::anonymous(&stack().base_url);
    let resp = anon.get("/admin").await;
    common::assert_login_redirect(&resp);
}

#[tokio::test]
async fn admin_authenticated_admin_reaches_the_admin_area() {
    let _lock = SuiteLock::acquire().await;
    let admin = AdminSession::cached(&stack().base_url)
        .await
        .expect("admin login");
    let resp = admin.get("/admin").await;
    assert!(
        matches!(resp.status, 302 | 303),
        "GET /admin: HTTP {} {:?}",
        resp.status,
        resp.header("location")
    );
    let loc = resp.header("location").unwrap_or("");
    assert!(
        loc.contains("/admin/documents"),
        "expected the documents list, got {loc}"
    );
    let page = admin.page_after(&resp).await;
    assert_eq!(page.status, 200, "{}", page.text());
    common::assert_see(&page, "Documents");
}

#[tokio::test]
async fn admin_failed_login_rerenders_the_form_with_the_devise_alert() {
    let _lock = SuiteLock::acquire().await;
    wait_for_login_budget(1).await;
    let (user, _) = admin_credentials();
    let (_session, resp) =
        AdminSession::attempt_login(&stack().base_url, &user, "definitely-wrong")
            .await
            .expect("login POST");
    assert_eq!(
        resp.status,
        422,
        "wrong password re-renders the form, got HTTP {} {:?}",
        resp.status,
        resp.header("location")
    );
    assert!(
        resp.text().contains("Invalid"),
        "devise alert missing:\n{}",
        resp.text()
    );
}

/// Budget: 10 attempts / 3 minutes per Rails process. One shared session afterwards.
#[tokio::test]
async fn admin_repeated_login_attempts_are_rate_limited() {
    let _lock = SuiteLock::acquire().await;
    // Ten failures must all land inside a fresh window, or the 11th is not
    // the request that crosses the limit.
    wait_for_login_budget(10).await;
    let base = stack().base_url.clone();
    let (user, pass) = admin_credentials();

    for i in 0..10 {
        let (_session, resp) = AdminSession::attempt_login(&base, &user, "definitely-wrong")
            .await
            .expect("login POST");
        assert_eq!(
            resp.status,
            422,
            "attempt {i} should still re-render, got HTTP {} {:?}",
            resp.status,
            resp.header("location")
        );
        assert!(
            resp.text().contains("Invalid"),
            "attempt {i} lost the devise alert"
        );
    }

    let (session, throttled) = AdminSession::attempt_login(&base, &user, "definitely-wrong")
        .await
        .expect("login POST");
    assert!(
        login_throttled(&throttled),
        "11th attempt should redirect to sign-in, got HTTP {} {:?}",
        throttled.status,
        throttled.header("location")
    );
    let page = session.page_after(&throttled).await;
    assert!(
        page.text()
            .to_lowercase()
            .contains("too many login attempts"),
        "throttle alert missing:\n{}",
        page.text()
    );

    let (_session, still) = AdminSession::attempt_login(&base, &user, &pass)
        .await
        .expect("login POST");
    assert!(
        login_throttled(&still) && !login_succeeded(&still),
        "the right password is still throttled, got HTTP {} {:?}",
        still.status,
        still.header("location")
    );

    // Hold the lock until the next admin test can log in again.
    settle_login_budget().await;
}

/// Budget: 10 attempts / 3 minutes per Rails process. One shared session afterwards.
#[tokio::test]
async fn admin_a_few_failed_attempts_do_not_block_a_valid_login() {
    let _lock = SuiteLock::acquire().await;
    wait_for_login_budget(4).await;
    let base = stack().base_url.clone();
    let (user, pass) = admin_credentials();

    for i in 0..3 {
        let (_session, resp) = AdminSession::attempt_login(&base, &user, "definitely-wrong")
            .await
            .expect("login POST");
        assert_eq!(resp.status, 422, "attempt {i}: HTTP {}", resp.status);
        assert!(
            resp.text().contains("Invalid"),
            "attempt {i} lost the devise alert"
        );
    }

    let (_session, resp) = AdminSession::attempt_login(&base, &user, &pass)
        .await
        .expect("login POST");
    assert!(
        login_succeeded(&resp),
        "a valid login after three failures should succeed, got HTTP {} {:?}",
        resp.status,
        resp.header("location")
    );
}
