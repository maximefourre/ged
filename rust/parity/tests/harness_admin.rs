//! Smoke tests of the admin session utilities (form login, CSRF), not ports of
//! Ruby tests: those arrive with the admin parity modules.

use parity::admin::AdminSession;
use parity::backend;

#[tokio::test]
async fn admin_form_login_reaches_the_admin_area() {
    let be = backend().await;
    let admin = AdminSession::cached(be.base_url()).await.expect("login");
    let r = admin.get("/admin/documents").await;
    assert_eq!(r.status, 200, "{}", r.text());
}

#[tokio::test]
async fn admin_login_with_a_wrong_password_fails() {
    let be = backend().await;
    let err = AdminSession::login_as(be.base_url(), "admin", "definitely-wrong").await;
    assert!(err.is_err(), "wrong password must not log in");
}

#[tokio::test]
async fn admin_post_without_a_valid_csrf_token_is_refused() {
    let be = backend().await;
    let admin = AdminSession::cached(be.base_url()).await.expect("login");
    let r = admin
        .post_form_with_token("/admin/upload_queue_pause", "not-a-token", &[])
        .await;
    println!(
        "bad-csrf status = {} location = {:?}",
        r.status,
        r.header("location")
    );
    assert_ne!(r.status, 200);
    // The queue must not have been paused by the forged request.
    let page = admin.get("/admin/upload_requests").await;
    assert!(
        !page.text().contains("Resume"),
        "queue was paused without a valid token"
    );
}

#[tokio::test]
async fn admin_submit_with_a_valid_token_is_accepted_and_logout_ends_the_session() {
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
