//! The Rails `default_headers` also cover the admin UI: every answer one of
//! its controllers produces (page, login form) carries
//! them; the exceptions app (unknown admin path) and Devise's failure app (redirect to
//! the login) do not. Same contract as
//! `security_headers.rs`, observed on the stack and asserted on both backends.

mod common;

use common::World;
use parity::Resp;
use parity::admin::AdminSession;

const SECURITY: [(&str, &str); 5] = [
    ("x-frame-options", "SAMEORIGIN"),
    ("x-xss-protection", "0"),
    ("x-content-type-options", "nosniff"),
    ("x-permitted-cross-domain-policies", "none"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
];

fn assert_security_headers(resp: &Resp, what: &str) {
    for (name, value) in SECURITY {
        assert_eq!(resp.header(name), Some(value), "{what}: {name}");
    }
    for absent in ["content-security-policy", "strict-transport-security"] {
        assert_eq!(resp.header(absent), None, "{what}: {absent}");
    }
}

#[tokio::test]
async fn admin_pages_and_redirects_carry_the_security_headers() {
    let w = World::new().await;
    let base = w.backend.base_url();

    let anonymous = AdminSession::anonymous(base);
    let login = anonymous.get("/admins/sign_in").await;
    assert_eq!(login.status, 200, "login form");
    assert_security_headers(&login, "login form");

    let redirected = anonymous.get("/admin/departments").await;
    assert!(
        matches!(redirected.status, 302 | 303),
        "{}",
        redirected.status
    );
    // Devise's failure app (Warden middleware) answers, not a controller.
    for (name, _) in SECURITY {
        assert_eq!(redirected.header(name), None, "redirect to login: {name}");
    }

    let admin = AdminSession::cached(base).await.expect("admin session");
    for path in ["/admin/departments", "/admin/app_clients", "/admin/status"] {
        let page = admin.get(path).await;
        assert_eq!(page.status, 200, "{path}");
        assert_security_headers(&page, path);
    }
}

#[tokio::test]
async fn admin_unknown_path_is_a_bare_exceptions_app_page() {
    let w = World::new().await;
    let resp = AdminSession::anonymous(w.backend.base_url())
        .get("/admin/nope")
        .await;
    assert_eq!(resp.status, 404);
    for (name, _) in SECURITY {
        assert_eq!(resp.header(name), None, "{name}");
    }
}

#[tokio::test]
async fn admin_missing_record_and_csrf_refusal_are_bare_exceptions_app_pages() {
    let w = World::new().await;
    let admin = AdminSession::cached(w.backend.base_url())
        .await
        .expect("admin session");
    let missing = admin.get("/admin/app_clients/999999/edit").await;
    assert_eq!(missing.status, 404);
    for (name, _) in SECURITY {
        assert_eq!(missing.header(name), None, "RecordNotFound: {name}");
    }
    let forged = admin.post_form_raw("/admin/upload_queue_pause", &[]).await;
    assert_eq!(forged.status, 422);
    for (name, _) in SECURITY {
        assert_eq!(
            forged.header(name),
            None,
            "InvalidAuthenticityToken: {name}"
        );
    }
}
