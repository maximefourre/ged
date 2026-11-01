//! Smoke tests of the admin session utilities (form login, CSRF), not ports of
//! Ruby tests: those arrive with the admin parity modules.

mod common;

use common::{ensure_queue_active, queue_has_word, queue_words};
use parity::admin::{AdminSession, SuiteLock, csrf_token};
use parity::backend;
use parity::http::Resp;

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

    // A real token from a second session. It has a valid shape, so a check
    // that only rejects malformed tokens would accept it.
    let other = AdminSession::anonymous(be.base_url());
    let login = other.get("/admins/sign_in").await;
    assert_eq!(login.status, 200, "{}", login.text());
    let foreign = csrf_token(&login.text()).expect("csrf token on the other session");

    let dept_name = parity::fixtures::unique("csrf-dept");
    let ident = parity::fixtures::unique("csrf-client");
    let probe_name = parity::fixtures::unique("csrf-probe");
    let probe = parity::fixtures::create_department(be.db(), &probe_name, None).await;

    let missing_pause = admin
        .post_form_raw("/admin/upload_queue_pause", &[("commit", "Pause")])
        .await;
    let forged_pause = admin
        .post_form_with_token(
            "/admin/upload_queue_pause",
            "not-a-token",
            &[("commit", "Pause")],
        )
        .await;
    let foreign_pause = admin
        .post_form_with_token(
            "/admin/upload_queue_pause",
            &foreign,
            &[("commit", "Pause")],
        )
        .await;
    let missing_dept = admin
        .post_form_raw(
            "/admin/departments",
            &[("department[name]", dept_name.as_str())],
        )
        .await;
    let still_active = admin.get("/admin/upload_requests").await;

    let paused = admin
        .submit(reqwest::Method::POST, "/admin/upload_queue_pause", &[])
        .await;
    let missing_resume = admin
        .post_form_raw("/admin/upload_queue_pause", &[("_method", "delete")])
        .await;
    let forged_resume = admin
        .post_form_with_token(
            "/admin/upload_queue_pause",
            "not-a-token",
            &[("_method", "delete")],
        )
        .await;
    let foreign_resume = admin
        .post_form_with_token(
            "/admin/upload_queue_pause",
            &foreign,
            &[("_method", "delete")],
        )
        .await;
    let still_paused = admin.get("/admin/upload_requests").await;
    let resumed = admin
        .submit(reqwest::Method::DELETE, "/admin/upload_queue_pause", &[])
        .await;

    let forged_client = admin
        .post_form_with_token(
            "/admin/app_clients",
            "not-a-token",
            &[
                ("app_client[client_identifier]", ident.as_str()),
                ("app_client[description]", "csrf"),
            ],
        )
        .await;
    let foreign_dept = admin
        .post_form_with_token(
            "/admin/departments",
            &foreign,
            &[("department[name]", dept_name.as_str())],
        )
        .await;
    let forged_patch = admin
        .post_form_with_token(
            &format!("/admin/departments/{probe}"),
            "not-a-token",
            &[
                ("_method", "patch"),
                ("department[name]", "renamed-by-csrf"),
            ],
        )
        .await;
    let foreign_delete = admin
        .post_form_with_token(
            &format!("/admin/departments/{probe}"),
            &foreign,
            &[("_method", "delete")],
        )
        .await;
    let after = admin.get("/admin/upload_requests").await;

    let dept_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM departments WHERE name = $1")
        .bind(&dept_name)
        .fetch_one(be.db())
        .await
        .expect("count department");
    let client_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_clients WHERE client_identifier = $1")
            .bind(&ident)
            .fetch_one(be.db())
            .await
            .expect("count client");
    let probe_still: i64 =
        sqlx::query_scalar("SELECT count(*) FROM departments WHERE id = $1 AND name = $2")
            .bind(probe)
            .bind(&probe_name)
            .fetch_one(be.db())
            .await
            .expect("probe department");

    sqlx::query("DELETE FROM departments WHERE id = $1 OR name = $2")
        .bind(probe)
        .bind(&dept_name)
        .execute(be.db())
        .await
        .expect("delete probe department");
    sqlx::query(
        "DELETE FROM client_department_accesses WHERE app_client_id IN \
         (SELECT id FROM app_clients WHERE client_identifier = $1)",
    )
    .bind(&ident)
    .execute(be.db())
    .await
    .expect("delete probe grants");
    sqlx::query("DELETE FROM app_clients WHERE client_identifier = $1")
        .bind(&ident)
        .execute(be.db())
        .await
        .expect("delete probe client");

    for resp in [
        &missing_pause,
        &forged_pause,
        &foreign_pause,
        &missing_dept,
        &missing_resume,
        &forged_resume,
        &foreign_resume,
        &forged_client,
        &foreign_dept,
        &forged_patch,
        &foreign_delete,
    ] {
        assert_csrf_refused(resp);
    }
    assert!(
        matches!(paused.status, 302 | 303),
        "pause: HTTP {}",
        paused.status
    );
    assert!(
        matches!(resumed.status, 302 | 303),
        "resume: HTTP {}",
        resumed.status
    );
    assert_eq!(still_active.status, 200, "{}", still_active.text());
    assert_banner(&still_active.text(), "Pause");
    assert_eq!(still_paused.status, 200, "{}", still_paused.text());
    assert_banner(&still_paused.text(), "Resume");
    assert_eq!(after.status, 200, "{}", after.text());
    assert_banner(&after.text(), "Pause");
    assert_eq!(
        dept_rows, 0,
        "department create ran without a valid CSRF token"
    );
    assert_eq!(
        client_rows, 0,
        "client create ran without a valid CSRF token"
    );
    assert_eq!(
        probe_still, 1,
        "PATCH or DELETE changed the probe department without a valid CSRF token"
    );
}

fn assert_banner(body: &str, action: &str) {
    let words = queue_words(body);
    assert!(
        queue_has_word(body, action),
        "queue banner did not offer {action}, words={words:?}"
    );
}

fn assert_csrf_refused(resp: &Resp) {
    assert_eq!(
        resp.status,
        422,
        "a mutation without a valid CSRF token must be refused, got HTTP {} location {:?} body {}",
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
