//! T9: `PurgeDeletedDocumentsJob`: due documents, gated departments, orphans.

mod pipeline_support;
mod support;

use chrono::{Duration, Utc};
use ged::jobs::JobError;
use ged::jobs::purge::PurgeDeletedDocumentsJob;
use pipeline_support::{Fixture, age_file};

fn job(f: &Fixture) -> PurgeDeletedDocumentsJob {
    PurgeDeletedDocumentsJob::new(f.env.clone())
}

fn ago(days: i64) -> chrono::NaiveDateTime {
    (Utc::now() - Duration::days(days)).naive_utc()
}

fn in_days(days: i64) -> chrono::NaiveDateTime {
    ago(-days)
}

#[tokio::test]
async fn due_documents_are_deleted_with_their_physical_file() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let (due, due_key) = f.document(dept, Some(ago(1))).await;
    let (waiting, waiting_key) = f.document(dept, Some(in_days(5))).await;
    let (live, live_key) = f.document(dept, None).await;

    job(&f).run(Utc::now()).await.unwrap();

    let left: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM documents ORDER BY created_at, id")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert!(!left.contains(&due) && left.contains(&waiting) && left.contains(&live));
    assert!(!f.file_exists(&due_key), "file removed from disk");
    assert!(f.file_exists(&waiting_key) && f.file_exists(&live_key));
    assert_eq!(f.count("SELECT count(*) FROM stored_files").await, 2);
    f.finish().await;
}

/// A document restored between listing and purging (purge_at back to NULL)
/// must survive: the delete itself is guarded by `purge_at`.
#[tokio::test]
async fn the_purge_itself_is_guarded_by_purge_at() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let (restored, key_restored) = f.document(dept, None).await;
    let (early, key_early) = f.document(dept, Some(in_days(2))).await;
    let (due, key_due) = f.document(dept, Some(ago(1))).await;
    let j = job(&f);

    assert!(!j.purge_document(restored, Utc::now()).await.unwrap());
    assert!(!j.purge_document(early, Utc::now()).await.unwrap());
    assert!(f.file_exists(&key_restored) && f.file_exists(&key_early));
    assert!(j.purge_document(due, Utc::now()).await.unwrap());
    assert!(!f.file_exists(&key_due));
    assert!(
        !j.purge_document(due, Utc::now()).await.unwrap(),
        "already gone is not an error"
    );
    f.finish().await;
}

#[tokio::test]
async fn one_failing_destroy_does_not_stop_the_others_and_is_reported() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let (bad, bad_key) = f.document(dept, Some(ago(1))).await;
    let (good, good_key) = f.document(dept, Some(ago(1))).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION refuse_delete() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF OLD.id = '{bad}' THEN RAISE EXCEPTION 'injected delete failure'; END IF; RETURN OLD; END $$; \
         CREATE TRIGGER refuse_delete BEFORE DELETE ON documents \
         FOR EACH ROW EXECUTE FUNCTION refuse_delete();"
    )))
    .execute(&f.pool)
    .await
    .unwrap();

    let result = job(&f).run(Utc::now()).await;

    assert!(
        matches!(&result, Err(JobError::Retry(m)) if m.contains("injected delete failure")),
        "{result:?}"
    );
    assert!(
        f.file_exists(&bad_key),
        "a row that failed to delete keeps its file"
    );
    assert!(!f.file_exists(&good_key));
    let left: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM documents")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    assert_eq!(left, vec![bad]);
    assert!(!left.contains(&good));
    f.finish().await;
}

#[tokio::test]
async fn a_trashed_department_waits_for_every_reference_seen_unscoped() {
    let f = Fixture::new().await;
    let with_trashed_doc = f.department("A trashed doc", None).await;
    let with_request = f.department("B request", None).await;
    let with_trashed_child_parent = f.department("C parent", None).await;
    let trashed_child = f
        .department("C child", Some(with_trashed_child_parent))
        .await;
    let free = f.department("D free", None).await;
    f.document(with_trashed_doc, Some(in_days(10))).await;
    f.upload_request(with_request, "completed", "https://x.example/h", None)
        .await;
    for d in [
        with_trashed_doc,
        with_request,
        with_trashed_child_parent,
        trashed_child,
        free,
    ] {
        f.trash_department(d).await;
    }

    job(&f).run(Utc::now()).await.unwrap();

    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM departments ORDER BY id")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    assert!(!ids.contains(&free), "free leaf purged");
    assert!(!ids.contains(&trashed_child), "child leaf purged");
    assert!(
        ids.contains(&with_trashed_doc),
        "trashed document still references it"
    );
    assert!(
        ids.contains(&with_request),
        "upload request still references it"
    );
    assert!(
        ids.contains(&with_trashed_child_parent),
        "its child was still there when checked"
    );

    // next pass: the parent's last child is gone
    job(&f).run(Utc::now()).await.unwrap();
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM departments")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    assert!(!ids.contains(&with_trashed_child_parent));
    f.finish().await;
}

#[tokio::test]
async fn a_department_is_purged_in_the_run_that_purges_its_last_document() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    f.document(dept, Some(ago(1))).await;
    f.trash_department(dept).await;
    sqlx::query(
        "INSERT INTO app_clients (client_identifier, created_at, updated_at) VALUES ('c', now(), now())",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let client: i64 = f.count("SELECT id FROM app_clients").await;
    sqlx::query(
        "INSERT INTO client_department_accesses (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
         VALUES ($1, $2, true, false, now(), now())",
    )
    .bind(client)
    .bind(dept)
    .execute(&f.pool)
    .await
    .unwrap();

    job(&f).run(Utc::now()).await.unwrap();

    assert_eq!(f.count("SELECT count(*) FROM departments").await, 0);
    assert_eq!(
        f.count("SELECT count(*) FROM client_department_accesses")
            .await,
        0,
        "grants cascade"
    );
    f.finish().await;
}

#[tokio::test]
async fn live_departments_are_never_purged() {
    let f = Fixture::new().await;
    f.department("Compta", None).await;
    job(&f).run(Utc::now()).await.unwrap();
    assert_eq!(f.count("SELECT count(*) FROM departments").await, 1);
    f.finish().await;
}

#[tokio::test]
async fn orphan_files_and_part_files_are_swept_after_a_day() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let (_, referenced) = f.document(dept, None).await;
    age_file(&f.env.storage.path_for(&referenced).unwrap(), 5);

    let write = |name: &str, age_days: u64| {
        let key = name.to_owned();
        let path = f.env.storage.path_for(&key).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"o").unwrap();
        age_file(&path, age_days);
        path
    };
    let old_orphan = write(&"a".repeat(32), 2);
    let fresh_orphan = write(&"b".repeat(32), 0);
    let mut old_part = write(&"c".repeat(32), 2);
    let mut fresh_part = write(&"d".repeat(32), 0);
    for (part, age) in [(&mut old_part, 2), (&mut fresh_part, 0)] {
        let mut renamed = part.as_os_str().to_owned();
        renamed.push(".part");
        std::fs::rename(&*part, &renamed).unwrap();
        *part = renamed.into();
        age_file(part, age);
    }
    let staged = uuid::Uuid::new_v4();
    std::fs::write(f.env.staging.path(staged), b"s").unwrap();
    age_file(&f.env.staging.path(staged), 2);

    job(&f).run(Utc::now()).await.unwrap();

    assert!(!old_orphan.exists() && !old_part.exists());
    assert!(
        fresh_orphan.exists() && fresh_part.exists(),
        "in-flight uploads are left alone"
    );
    assert!(
        f.file_exists(&referenced),
        "referenced files are never orphans, however old"
    );
    assert!(
        f.env.staging.exists(staged).await,
        "staging belongs to reconcile"
    );
    f.finish().await;
}
