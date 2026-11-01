//! Smoke tests of the admin session utilities (form login, CSRF), not ports of
//! Ruby tests: those arrive with the admin parity modules.

mod common;

use common::ensure_queue_active;
use parity::admin::{AdminSession, SuiteLock};
use parity::backend;

#[tokio::test]
async fn admin_form_login_reaches_the_admin_area() {
    let _lock = SuiteLock::acquire().await;
    let be = backend().await;
    let admin = AdminSession::cached(be.base_url()).await.expect("login");
    let r = admin.get("/admin/documents").await;
    assert_eq!(r.status, 200, "{}", r.text());
}

#[tokio::test]
async fn admin_login_with_a_wrong_password_fails() {
    let _lock = SuiteLock::acquire().await;
    let be = backend().await;
    let err = AdminSession::login_as(be.base_url(), "admin", "definitely-wrong").await;
    assert!(err.is_err(), "wrong password must not log in");
}

#[tokio::test]
async fn admin_post_without_a_valid_csrf_token_is_refused() {
    let _lock = SuiteLock::acquire().await;
    let be = backend().await;
    let admin = AdminSession::cached(be.base_url()).await.expect("login");
    ensure_queue_active(&admin).await;

    // No token at all.
    let missing = admin
        .post_form_raw("/admin/upload_queue_pause", &[("commit", "Pause")])
        .await;
    assert_csrf_refused(&missing);

    // A token that is not the session's.
    let forged = admin
        .post_form_with_token(
            "/admin/upload_queue_pause",
            "not-a-token",
            &[("commit", "Pause")],
        )
        .await;
    assert_csrf_refused(&forged);

    let page = admin.get("/admin/upload_requests").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert!(
        !page.text().contains(">Resume<"),
        "queue was paused without a valid token:\n{}",
        page.text()
    );
    assert!(
        page.text().contains(">Pause<"),
        "queue banner lost its Pause action:\n{}",
        page.text()
    );
}

fn assert_csrf_refused(resp: &parity::http::Resp) {
    assert_eq!(
        resp.status,
        422,
        "a POST without a valid CSRF token must be refused, got HTTP {} location {:?} body {}",
        resp.status,
        resp.header("location"),
        resp.text()
    );
    assert!(
        resp.text().contains("The change you wanted was rejected"),
        "refusal page should say the change was rejected:\n{}",
        resp.text()
    );
}

#[tokio::test]
async fn admin_submit_with_a_valid_token_is_accepted_and_logout_ends_the_session() {
    let _lock = SuiteLock::acquire().await;
    let be = backend().await;
    let admin = AdminSession::login(be.base_url()).await.expect("login");
    let r = admin.logout().await;
    assert!(
        matches!(r.status, 302 | 303),
        "logout: {} {:?}",
        r.status,
        r.header("location")
    );
    let after = admin.get("/admin/documents").await;
    assert!(matches!(after.status, 302 | 303));
    assert!(
        after
            .header("location")
            .unwrap_or_default()
            .contains("sign_in")
    );
}
