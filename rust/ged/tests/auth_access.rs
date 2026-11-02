//! Permissions against a real database: `AppClient::can_access`,
//! `accessible_department_ids`, `Department` tree helpers, and the
//! registered/unregistered split of `/api/v1/ping`. Port of
//! `test/models/app_client_test.rb` and `department_test.rb`, plus plan
//! Review Focus 4 (trashed ancestor between grant and access, deep trees).

mod auth_support;
mod support;

use ged::domain::app_client::{AccessMode, AppClient};
use ged::domain::department::Department;
use sqlx::PgPool;
use support::ScratchDb;

struct Db {
    scratch: ScratchDb,
    pool: PgPool,
}

async fn db() -> Db {
    let scratch = ScratchDb::create().await;
    let pool = scratch.pool().await;
    ged::db::migrate(&pool).await.unwrap();
    Db { scratch, pool }
}

impl Db {
    async fn finish(self) {
        self.pool.close().await;
        self.scratch.drop_db().await;
    }

    async fn dept(&self, name: &str, parent: Option<i64>) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO departments (name, parent_id, created_at, updated_at) \
             VALUES ($1, $2, now(), now()) RETURNING id",
        )
        .bind(name)
        .bind(parent)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn client(&self, identifier: &str) -> AppClient {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO app_clients (client_identifier, created_at, updated_at) \
             VALUES ($1, now(), now()) RETURNING id",
        )
        .bind(identifier)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        AppClient {
            id,
            client_identifier: identifier.to_owned(),
            description: None,
        }
    }

    async fn grant(&self, client: &AppClient, dept: i64, read: bool, write: bool) {
        sqlx::query(
            "INSERT INTO client_department_accesses \
             (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, now(), now())",
        )
        .bind(client.id)
        .bind(dept)
        .bind(read)
        .bind(write)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn trash(&self, dept: i64) {
        sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
            .bind(dept)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn can(&self, c: &AppClient, dept: i64, mode: AccessMode) -> bool {
        c.can_access(&self.pool, dept, mode).await.unwrap()
    }
}

#[tokio::test]
async fn a_grant_covers_the_department_and_its_subtree_for_both_modes() {
    let db = db().await;
    let finance = db.dept("Finance", None).await;
    let invoices = db.dept("Invoices", Some(finance)).await;
    let hr = db.dept("HR", None).await;
    let c = db.client("erp-app").await;
    db.grant(&c, finance, true, true).await;
    for mode in [AccessMode::Read, AccessMode::Write] {
        assert!(db.can(&c, finance, mode).await);
        assert!(db.can(&c, invoices, mode).await);
        assert!(!db.can(&c, hr, mode).await);
    }
    db.finish().await;
}

#[tokio::test]
async fn read_and_write_are_independent() {
    let db = db().await;
    let finance = db.dept("Finance", None).await;
    let invoices = db.dept("Invoices", Some(finance)).await;
    let reader = db.client("reader").await;
    let writer = db.client("writer").await;
    db.grant(&reader, finance, true, false).await;
    db.grant(&writer, finance, false, true).await;
    assert!(db.can(&reader, finance, AccessMode::Read).await);
    assert!(!db.can(&reader, finance, AccessMode::Write).await);
    assert!(!db.can(&reader, invoices, AccessMode::Write).await);
    assert!(db.can(&writer, finance, AccessMode::Write).await);
    assert!(!db.can(&writer, finance, AccessMode::Read).await);
    db.finish().await;
}

#[tokio::test]
async fn read_and_write_cascade_independently_and_never_escalate_upward() {
    let db = db().await;
    let finance = db.dept("Finance", None).await;
    let invoices = db.dept("Invoices", Some(finance)).await;
    let mixed = db.client("mixed").await;
    db.grant(&mixed, finance, true, false).await;
    db.grant(&mixed, invoices, false, true).await;
    assert!(db.can(&mixed, invoices, AccessMode::Read).await);
    assert!(db.can(&mixed, invoices, AccessMode::Write).await);
    assert!(!db.can(&mixed, finance, AccessMode::Write).await);
    db.finish().await;
}

#[tokio::test]
async fn a_client_never_sees_another_clients_grant() {
    let db = db().await;
    let finance = db.dept("Finance", None).await;
    let a = db.client("a").await;
    let b = db.client("b").await;
    db.grant(&a, finance, true, true).await;
    assert!(!db.can(&b, finance, AccessMode::Read).await);
    assert!(
        b.accessible_department_ids(&db.pool, AccessMode::Read)
            .await
            .unwrap()
            .is_empty()
    );
    db.finish().await;
}

#[tokio::test]
async fn an_unknown_department_is_not_accessible() {
    let db = db().await;
    let c = db.client("c").await;
    assert!(!db.can(&c, 999_999, AccessMode::Read).await);
    db.finish().await;
}

#[tokio::test]
async fn accessible_department_ids_expand_granted_subtrees_per_mode() {
    let db = db().await;
    let finance = db.dept("Finance", None).await;
    let invoices = db.dept("Invoices", Some(finance)).await;
    let hr = db.dept("HR", None).await;
    let partial = db.client("partial").await;
    db.grant(&partial, finance, true, false).await;
    db.grant(&partial, hr, false, true).await;
    let mut read = partial
        .accessible_department_ids(&db.pool, AccessMode::Read)
        .await
        .unwrap();
    read.sort();
    assert_eq!(read, vec![finance, invoices]);
    let write = partial
        .accessible_department_ids(&db.pool, AccessMode::Write)
        .await
        .unwrap();
    assert_eq!(write, vec![hr]);
    db.finish().await;
}

#[tokio::test]
async fn overlapping_grants_do_not_duplicate_ids() {
    let db = db().await;
    let finance = db.dept("Finance", None).await;
    let invoices = db.dept("Invoices", Some(finance)).await;
    let c = db.client("c").await;
    db.grant(&c, finance, true, false).await;
    db.grant(&c, invoices, true, false).await;
    let ids = c
        .accessible_department_ids(&db.pool, AccessMode::Read)
        .await
        .unwrap();
    assert_eq!(ids.len(), 2, "{ids:?}");
    db.finish().await;
}

// --- Review Focus 4 --------------------------------------------------------

#[tokio::test]
async fn a_trashed_ancestor_cuts_the_chain_no_escalation_through_it() {
    // Rails walks `parent` through the default scope: a trashed ancestor is
    // not found, so the walk stops there and grants above it do not apply.
    let db = db().await;
    let root = db.dept("Root", None).await;
    let mid = db.dept("Mid", Some(root)).await;
    let leaf = db.dept("Leaf", Some(mid)).await;
    let c = db.client("c").await;
    db.grant(&c, root, true, true).await;
    assert!(db.can(&c, leaf, AccessMode::Read).await, "before the trash");
    db.trash(mid).await;
    assert!(
        !db.can(&c, leaf, AccessMode::Read).await,
        "trashed Mid cuts the chain"
    );
    assert!(!db.can(&c, leaf, AccessMode::Write).await);
    db.finish().await;
}

#[tokio::test]
async fn a_grant_on_a_trashed_department_grants_nothing() {
    let db = db().await;
    let root = db.dept("Root", None).await;
    let child = db.dept("Child", Some(root)).await;
    let c = db.client("c").await;
    db.grant(&c, root, true, true).await;
    db.trash(child).await;
    assert!(!db.can(&c, child, AccessMode::Read).await, "trashed itself");
    let ids = c
        .accessible_department_ids(&db.pool, AccessMode::Read)
        .await
        .unwrap();
    assert_eq!(ids, vec![root], "trashed child leaves the accessible set");
    db.trash(root).await;
    assert!(
        c.accessible_department_ids(&db.pool, AccessMode::Read)
            .await
            .unwrap()
            .is_empty()
    );
    db.finish().await;
}

#[tokio::test]
async fn a_deep_tree_neither_overflows_nor_errors() {
    let db = db().await;
    let mut parent = db.dept("d0", None).await;
    let top = parent;
    for i in 1..=2000 {
        parent = db.dept(&format!("d{i}"), Some(parent)).await;
    }
    let c = db.client("c").await;
    db.grant(&c, top, true, false).await;
    assert!(db.can(&c, parent, AccessMode::Read).await);
    assert!(!db.can(&c, parent, AccessMode::Write).await);
    assert_eq!(
        c.accessible_department_ids(&db.pool, AccessMode::Read)
            .await
            .unwrap()
            .len(),
        2001
    );
    let chain = Department::find(&db.pool, parent)
        .await
        .unwrap()
        .unwrap()
        .self_and_ancestors(&db.pool)
        .await
        .unwrap();
    assert_eq!(chain.len(), 2001);
    assert_eq!(chain[0].id, parent, "self first");
    assert_eq!(chain[2000].id, top, "root last");
    db.finish().await;
}

#[tokio::test]
async fn a_parent_cycle_terminates() {
    // Impossible through the app (the parent validation forbids it) but
    // `self_and_ancestors` must stay finite on corrupt data, like Rails' `seen`.
    let db = db().await;
    let a = db.dept("A", None).await;
    let b = db.dept("B", Some(a)).await;
    sqlx::query("UPDATE departments SET parent_id = $1 WHERE id = $2")
        .bind(b)
        .bind(a)
        .execute(&db.pool)
        .await
        .unwrap();
    let c = db.client("c").await;
    db.grant(&c, a, true, false).await;
    assert!(db.can(&c, b, AccessMode::Read).await);
    assert!(!db.can(&c, b, AccessMode::Write).await);
    let chain = Department::find(&db.pool, b)
        .await
        .unwrap()
        .unwrap()
        .self_and_ancestors(&db.pool)
        .await
        .unwrap();
    assert_eq!(chain.iter().map(|d| d.id).collect::<Vec<_>>(), vec![b, a]);
    assert_eq!(
        c.accessible_department_ids(&db.pool, AccessMode::Read)
            .await
            .unwrap()
            .len(),
        2
    );
    db.finish().await;
}

// --- Department soft delete ------------------------------------------------

async fn document(db: &Db, dept: i64, trashed: bool) -> String {
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO documents (department_id, filename, uploaded_by, uploaded_at, created_at, updated_at, \
         deleted_at, purge_at) VALUES ($1, 'f.txt', 'u', now(), now(), CASE WHEN $2 THEN now() - interval '1 day' ELSE now() END, \
         CASE WHEN $2 THEN now() END, CASE WHEN $2 THEN now() + interval '30 days' END) RETURNING id",
    )
    .bind(dept)
    .bind(trashed)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    id.to_string()
}

#[tokio::test]
async fn soft_delete_is_leaf_only_even_for_trashed_children() {
    let db = db().await;
    let parent = db.dept("P", None).await;
    let child = db.dept("C", Some(parent)).await;
    let p = Department::find(&db.pool, parent).await.unwrap().unwrap();
    assert!(!p.is_leaf(&db.pool).await.unwrap());
    assert!(p.soft_delete(&db.pool).await.is_err());
    db.trash(child).await;
    assert!(
        !p.is_leaf(&db.pool).await.unwrap(),
        "a trashed child still blocks"
    );
    assert!(p.soft_delete(&db.pool).await.is_err());
    db.finish().await;
}

#[tokio::test]
async fn soft_delete_hides_the_department_and_trashes_its_live_documents_together() {
    let db = db().await;
    let d = db.dept("Solo", None).await;
    document(&db, d, false).await;
    document(&db, d, false).await;
    document(&db, d, true).await;
    let dept = Department::find(&db.pool, d).await.unwrap().unwrap();
    dept.soft_delete(&db.pool).await.unwrap();
    assert!(
        Department::find(&db.pool, d).await.unwrap().is_none(),
        "hidden"
    );
    assert!(
        Department::find_with_deleted(&db.pool, d)
            .await
            .unwrap()
            .is_some()
    );
    let rows: Vec<(bool, bool, bool)> = sqlx::query_as(
        "SELECT deleted_at IS NOT NULL, purge_at IS NOT NULL, \
         purge_at = deleted_at + interval '30 days' FROM documents WHERE department_id = $1",
    )
    .bind(d)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.0 && r.1));
    // the identical purge_at in bulk: one distinct value among the 2 live ones
    let distinct: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT purge_at) FROM documents WHERE department_id = $1 AND updated_at = deleted_at",
    )
    .bind(d)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(distinct, 1);
    db.finish().await;
}

#[tokio::test]
async fn restore_brings_back_the_department_and_all_its_trashed_documents() {
    let db = db().await;
    let d = db.dept("Solo", None).await;
    document(&db, d, false).await;
    document(&db, d, true).await;
    let dept = Department::find(&db.pool, d).await.unwrap().unwrap();
    dept.soft_delete(&db.pool).await.unwrap();
    let trashed = Department::find_with_deleted(&db.pool, d)
        .await
        .unwrap()
        .unwrap();
    trashed.restore(&db.pool).await.unwrap();
    assert!(Department::find(&db.pool, d).await.unwrap().is_some());
    let still: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE department_id = $1 AND (deleted_at IS NOT NULL OR purge_at IS NOT NULL)",
    )
    .bind(d)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(still, 0);
    db.finish().await;
}

#[tokio::test]
async fn clear_soft_delete_un_greys_the_department_but_leaves_documents_alone() {
    let db = db().await;
    let d = db.dept("Solo", None).await;
    document(&db, d, false).await;
    Department::find(&db.pool, d)
        .await
        .unwrap()
        .unwrap()
        .soft_delete(&db.pool)
        .await
        .unwrap();
    let trashed = Department::find_with_deleted(&db.pool, d)
        .await
        .unwrap()
        .unwrap();
    trashed.clear_soft_delete(&db.pool).await.unwrap();
    assert!(Department::find(&db.pool, d).await.unwrap().is_some());
    let trashed_docs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE department_id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(d)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(trashed_docs, 1);
    db.finish().await;
}

#[tokio::test]
async fn a_trashed_department_is_purgeable_only_once_every_reference_is_gone() {
    let db = db().await;
    let parent = db.dept("P", None).await;
    let child = db.dept("C", Some(parent)).await;
    let doc = document(&db, child, false).await;
    Department::find(&db.pool, child)
        .await
        .unwrap()
        .unwrap()
        .soft_delete(&db.pool)
        .await
        .unwrap();
    let c = Department::find_with_deleted(&db.pool, child)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !c.references_cleared(&db.pool).await.unwrap(),
        "trashed doc still references it"
    );
    sqlx::query("DELETE FROM documents WHERE id = $1::uuid")
        .bind(&doc)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(c.references_cleared(&db.pool).await.unwrap());
    // an upload request is a reference too
    sqlx::query(
        "INSERT INTO upload_requests (callback_url, department_id, filename, uploaded_by, created_at, updated_at) \
         VALUES ('http://x.example/cb', $1, 'f', 'u', now(), now())",
    )
    .bind(child)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(!c.references_cleared(&db.pool).await.unwrap());
    sqlx::query("DELETE FROM upload_requests")
        .execute(&db.pool)
        .await
        .unwrap();
    // grants cascade with the hard delete
    let client = db.client("g").await;
    db.grant(&client, child, true, true).await;
    c.purge(&db.pool).await.unwrap();
    assert!(
        Department::find_with_deleted(&db.pool, child)
            .await
            .unwrap()
            .is_none()
    );
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM client_department_accesses")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(grants, 0);
    db.finish().await;
}

// --- /api/v1/ping, registered or not --------------------------------------

#[tokio::test]
async fn ping_is_200_for_a_registered_client_and_403_for_an_unregistered_one() {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let db = db().await;
    db.client("erp-app").await;
    let idp = auth_support::Idp::start().await;
    let config = ged::config::Config::from_lookup(&|k| match k {
        "RAILS_ENV" => Some("test".to_owned()),
        "GED_OIDC_ISSUER" => Some(auth_support::ISSUER.to_owned()),
        "GED_OIDC_AUDIENCE" => Some(auth_support::AUDIENCE.to_owned()),
        "GED_OIDC_JWKS_URL" => Some(idp.url.clone()),
        _ => None,
    })
    .unwrap();
    let app = ged::router(ged::AppState::new(config, db.pool.clone()));
    for (client, status, body) in [
        ("erp-app", 200, r#"{"client":"erp-app"}"#),
        ("ghost", 403, r#"{"error":"client not registered"}"#),
    ] {
        let token = auth_support::sign(&auth_support::claims(client));
        let req = Request::get("/api/v1/ping")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{client}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8_lossy(&bytes), body, "{client}");
    }
    db.finish().await;
}
