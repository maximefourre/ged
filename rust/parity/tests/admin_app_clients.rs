//! HTTP ports of the app-client journeys in `test/controllers/admin`.

mod common;

use common::{
    AdminWorld, access_flags, access_id, assert_login_redirect, assert_see,
    client_id_by_identifier, name,
};
use parity::admin::AdminSession;
use reqwest::Method;

#[tokio::test]
async fn admin_app_clients_require_a_session() {
    let _w = AdminWorld::new().await;
    let anon = AdminSession::anonymous(_w.backend.base_url());
    let resp = anon.get("/admin/app_clients").await;
    assert_login_redirect(&resp);
}

#[tokio::test]
async fn admin_app_clients_index_lists_the_granted_department_name() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, true).await;
    let page = w.admin.get("/admin/app_clients").await;
    assert_eq!(page.status, 200);
    assert_see(&page, &ident);
    assert_see(&page, &dept_name);
}

#[tokio::test]
async fn admin_app_clients_index_renders_a_grant_to_a_trashed_department() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, false).await;
    trash_department(w.db(), dept).await;
    let page = w.admin.get("/admin/app_clients").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, &ident);
    assert_see(&page, &dept_name);
    assert_see(&page, "(trashed)");
}

#[tokio::test]
async fn admin_app_client_edit_form_marks_a_trashed_grant() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, false).await;
    trash_department(w.db(), dept).await;
    let page = w
        .admin
        .get(&format!("/admin/app_clients/{client}/edit"))
        .await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_see(&page, &dept_name);
    assert_see(&page, "(trashed)");
}

#[tokio::test]
async fn admin_can_remove_a_grant_to_a_trashed_department() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, false).await;
    trash_department(w.db(), dept).await;
    let read = format!("app_client[access][{dept}][read]");
    let write = format!("app_client[access][{dept}][write]");
    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/app_clients/{client}"),
            &[
                ("app_client[client_identifier]", &ident),
                (&read, "0"),
                (&write, "0"),
            ],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Client updated.");
    assert!(access_flags(w.db(), client, dept).await.is_none());
}

#[tokio::test]
async fn admin_resaving_keeps_an_untouched_grant_to_a_trashed_department() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, false).await;
    trash_department(w.db(), dept).await;
    let read = format!("app_client[access][{dept}][read]");
    let write = format!("app_client[access][{dept}][write]");
    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/app_clients/{client}"),
            &[
                ("app_client[client_identifier]", &ident),
                (&read, "1"),
                (&write, "1"),
            ],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    let flags = access_flags(w.db(), client, dept)
        .await
        .expect("grant kept");
    assert_eq!(flags, (true, true));
}

#[tokio::test]
async fn admin_app_clients_index_shows_an_empty_state_when_there_are_no_clients() {
    let w = AdminWorld::new().await;
    // The empty copy is the unfiltered index. AdminWorld holds SuiteLock, so
    // this reset is exclusive among admin tests and does not depend on a
    // freshly seeded stack. Other tests keep isolating with unique names.
    w.backend.reset().await.expect("reset business tables");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_clients")
        .fetch_one(w.db())
        .await
        .unwrap();
    assert_eq!(n, 0, "reset left app clients behind");
    let page = w.admin.get("/admin/app_clients").await;
    assert_eq!(page.status, 200);
    assert_see(&page, "No registered clients.");
}

#[tokio::test]
async fn admin_registers_an_app_client_with_full_read_write_access() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("erp");
    register(&w, &ident, dept, "1", "1", Some("ERP")).await;
    let client = client_id_by_identifier(w.db(), &ident).await;
    w.track_client(client);
    assert_eq!(access_flags(w.db(), client, dept).await, Some((true, true)));
}

#[tokio::test]
async fn admin_registers_a_read_only_app_client() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("reader");
    register(&w, &ident, dept, "1", "0", None).await;
    let client = client_id_by_identifier(w.db(), &ident).await;
    w.track_client(client);
    assert_eq!(
        access_flags(w.db(), client, dept).await,
        Some((true, false))
    );
}

#[tokio::test]
async fn admin_registers_a_write_only_app_client() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("writer");
    register(&w, &ident, dept, "0", "1", None).await;
    let client = client_id_by_identifier(w.db(), &ident).await;
    w.track_client(client);
    assert_eq!(
        access_flags(w.db(), client, dept).await,
        Some((false, true))
    );
}

#[tokio::test]
async fn admin_unchecking_both_boxes_removes_the_department_access_row() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, true).await;
    let read = format!("app_client[access][{dept}][read]");
    let write = format!("app_client[access][{dept}][write]");
    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/app_clients/{client}"),
            &[
                ("app_client[client_identifier]", &ident),
                (&read, "0"),
                (&write, "0"),
            ],
        )
        .await;
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Client updated.");
    assert!(access_flags(w.db(), client, dept).await.is_none());
}

#[tokio::test]
async fn admin_updating_access_flags_edits_the_existing_row_in_place() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("erp");
    let client = w.client(&ident).await;
    w.grant(client, dept, true, true).await;
    let before = access_id(w.db(), client, dept).await.expect("row");
    let read = format!("app_client[access][{dept}][read]");
    let write = format!("app_client[access][{dept}][write]");
    let resp = w
        .admin
        .submit(
            Method::PATCH,
            &format!("/admin/app_clients/{client}"),
            &[
                ("app_client[client_identifier]", &ident),
                (&read, "1"),
                (&write, "0"),
            ],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    assert_eq!(access_id(w.db(), client, dept).await, Some(before));
    assert_eq!(
        access_flags(w.db(), client, dept).await,
        Some((true, false))
    );
}

#[tokio::test]
async fn admin_app_client_form_shows_the_read_write_grid() {
    let mut w = AdminWorld::new().await;
    let dept_name = name("finance");
    let dept = w.department(&dept_name, None).await;
    let page = w.admin.get("/admin/app_clients/new").await;
    assert_eq!(page.status, 200);
    assert_see(&page, "Department access");
    assert_see(&page, &dept_name);
    assert_see(&page, &format!("app_client[access][{dept}][read]"));
    assert_see(&page, &format!("app_client[access][{dept}][write]"));
}

#[tokio::test]
async fn admin_legacy_department_ids_param_no_longer_grants_access() {
    let mut w = AdminWorld::new().await;
    let dept = w.department(&name("finance"), None).await;
    let ident = name("legacy");
    let dept_s = dept.to_string();
    let resp = w
        .admin
        .submit(
            Method::POST,
            "/admin/app_clients",
            &[
                ("app_client[client_identifier]", &ident),
                ("app_client[department_ids][]", &dept_s),
            ],
        )
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    let client = client_id_by_identifier(w.db(), &ident).await;
    w.track_client(client);
    assert!(access_flags(w.db(), client, dept).await.is_none());
}

async fn register(
    w: &AdminWorld,
    ident: &str,
    dept: i64,
    read: &str,
    write: &str,
    description: Option<&str>,
) {
    let read_key = format!("app_client[access][{dept}][read]");
    let write_key = format!("app_client[access][{dept}][write]");
    let mut fields = vec![
        ("app_client[client_identifier]", ident),
        (&read_key, read),
        (&write_key, write),
    ];
    if let Some(desc) = description {
        fields.push(("app_client[description]", desc));
    }
    let resp = w
        .admin
        .submit(Method::POST, "/admin/app_clients", &fields)
        .await;
    assert!(matches!(resp.status, 302 | 303), "{}", resp.text());
    let page = w.admin.page_after(&resp).await;
    assert_see(&page, "Client registered.");
}

async fn trash_department(db: &sqlx::PgPool, id: i64) {
    sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .unwrap();
}
