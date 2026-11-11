//! Documents, their stored files and the purge, against a scratch database on
//! the worktree's Dockerised Postgres (`bin/worktree-stack db`).

mod support;

use chrono::{Duration, NaiveDateTime, Utc};
use ged::config::DEFAULT_MAX_UPLOAD_BYTES;
use ged::db::{self, PgTx};
use ged::domain::document::Document;
use ged::domain::document_creator::{CreateError, DocumentCreator, NewDocument};
use ged::jobs::env::PipelineEnv;
use ged::jobs::purge::PurgeDeletedDocumentsJob;
use ged::storage::{StagingStore, Storage, Stored};
use serde_json::json;
use sqlx::PgPool;
use support::ScratchDb;
use uuid::Uuid;

struct World {
    db: ScratchDb,
    pool: PgPool,
    storage: Storage,
    _dir: tempfile::TempDir,
    department: i64,
}

async fn world() -> World {
    let db = ScratchDb::create().await;
    let pool = db.pool().await;
    db::migrate(&pool).await.unwrap();
    let department = department(&pool, "Finance").await;
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().join("files"));
    World {
        db,
        pool,
        storage,
        _dir: dir,
        department,
    }
}

async fn department(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO departments (name, created_at, updated_at) \
         VALUES ($1, now() AT TIME ZONE 'utc', now() AT TIME ZONE 'utc') RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

impl World {
    async fn finish(self) {
        self.pool.close().await;
        self.db.drop_db().await;
    }

    /// The purge job, which deletes a row only while `purge_at` is still due.
    fn purge_job(&self) -> PurgeDeletedDocumentsJob {
        let staging = StagingStore::new(self._dir.path().join("staging"));
        PurgeDeletedDocumentsJob::new(PipelineEnv::new(
            self.pool.clone(),
            self.storage.clone(),
            staging,
            DEFAULT_MAX_UPLOAD_BYTES,
        ))
    }

    async fn stored(&self, bytes: &[u8]) -> Stored {
        self.storage.put(bytes, 1 << 20).await.unwrap()
    }

    fn input(&self, file: Stored) -> NewDocument {
        NewDocument {
            department_id: Some(self.department),
            filename: "rapport.pdf".into(),
            content_type: Some("application/pdf".into()),
            file,
            uploaded_by: "client-a".into(),
            tags: json!({}),
            uploaded_at: None,
            stored_async: false,
            max_byte_size: 1024,
        }
    }

    async fn create(&self, input: NewDocument) -> Result<Document, CreateError> {
        let mut tx: PgTx = self.pool.begin().await.unwrap();
        let doc = DocumentCreator::create(&mut tx, input).await?;
        tx.commit().await.unwrap();
        Ok(doc)
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn now() -> NaiveDateTime {
    Utc::now().naive_utc()
}

fn files_on_disk(storage: &Storage) -> usize {
    fn walk(dir: &std::path::Path) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.flatten()
            .map(|e| {
                if e.path().is_dir() {
                    walk(&e.path())
                } else {
                    1
                }
            })
            .sum()
    }
    walk(storage.root())
}

#[tokio::test]
async fn create_persists_the_document_and_its_stored_file() {
    let w = world().await;
    let file = w.stored(b"%PDF-1.4 hello").await;
    let key = file.key.clone();
    let mut input = w.input(file);
    input.content_type = Some("text/plain".into()); // a misleading declared type
    input.tags = json!({" Type ": " facture ", "URGENT": "  "});
    let doc = w.create(input).await.unwrap();

    assert_eq!(doc.filename, "rapport.pdf");
    // the declared type is what the document row keeps (as Rails)
    assert_eq!(doc.content_type.as_deref(), Some("text/plain"));
    assert_eq!(doc.byte_size, Some(14));
    assert_eq!(doc.tags, json!({"type": "facture", "urgent": null}));
    assert_eq!(doc.uploaded_by, "client-a");
    assert!(!doc.stored_async);
    assert_eq!(doc.uploaded_at, doc.created_at);
    assert!(doc.deleted_at.is_none() && doc.purge_at.is_none());

    // the blob records what the content says
    let (stored_key, kind, size): (String, String, i64) = sqlx::query_as(
        "SELECT key, content_type, byte_size FROM stored_files WHERE document_id = $1",
    )
    .bind(doc.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        (stored_key.as_str(), kind.as_str(), size),
        (key.as_str(), "application/pdf", 14)
    );
    assert!(w.storage.exists(&key).await);
    w.finish().await;
}

#[tokio::test]
async fn async_documents_keep_the_request_time_and_the_async_flag() {
    let w = world().await;
    let at = now() - Duration::hours(2);
    let mut input = w.input(w.stored(b"x").await);
    input.uploaded_at = Some(at);
    input.stored_async = true;
    let doc = w.create(input).await.unwrap();
    assert_eq!(doc.uploaded_at, at);
    assert!(doc.created_at > doc.uploaded_at);
    assert_eq!(doc.metadata_hash()["upload_mode"], "async");
    w.finish().await;
}

#[tokio::test]
async fn hostile_file_names_are_stored_normalized() {
    let w = world().await;
    for (raw, want) in [
        ("../../etc/passwd", "passwd"),
        ("..\\..\\win\\cmd.exe", "cmd.exe"),
        ("evil\r\nSet-Cookie: x=1", "evilSet-Cookie: x=1"),
        ("...", "unnamed"),
    ] {
        let mut input = w.input(w.stored(b"x").await);
        input.filename = raw.into();
        assert_eq!(w.create(input).await.unwrap().filename, want, "{raw:?}");
    }
    w.finish().await;
}

#[tokio::test]
async fn invalid_input_lists_rails_messages_in_order_and_writes_nothing() {
    let w = world().await;
    let mut input = w.input(w.stored(&[0u8; 2000]).await);
    input.department_id = Some(999_999);
    input.uploaded_by = "  ".into();
    input.tags = json!({"a": 1});
    let Err(CreateError::Invalid(messages)) = w.create(input).await else {
        panic!("expected a validation error");
    };
    assert_eq!(
        messages,
        vec![
            "Tags values must be strings of at most 500 characters, or null",
            "Department must exist",
            "Uploaded by can't be blank",
            "Byte size must be less than or equal to 1024",
        ]
    );
    assert_eq!(w.count("documents").await, 0);
    assert_eq!(w.count("stored_files").await, 0);
    w.finish().await;
}

#[tokio::test]
async fn a_trashed_or_missing_department_is_rejected() {
    let w = world().await;
    sqlx::query("UPDATE departments SET deleted_at = now() AT TIME ZONE 'utc' WHERE id = $1")
        .bind(w.department)
        .execute(&w.pool)
        .await
        .unwrap();
    let Err(CreateError::Invalid(messages)) = w.create(w.input(w.stored(b"x").await)).await else {
        panic!("trashed department accepted");
    };
    assert_eq!(messages, vec!["Department must exist"]);
    let mut none = w.input(w.stored(b"x").await);
    none.department_id = None;
    assert!(matches!(w.create(none).await, Err(CreateError::Invalid(_))));
    w.finish().await;
}

#[tokio::test]
async fn a_rolled_back_transaction_leaves_no_row() {
    let w = world().await;
    let mut tx = w.pool.begin().await.unwrap();
    DocumentCreator::create(&mut tx, w.input(w.stored(b"x").await))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(w.count("documents").await, 0);
    assert_eq!(w.count("stored_files").await, 0);
    w.finish().await;
}

#[tokio::test]
async fn soft_delete_hides_schedules_the_purge_and_restore_undoes_it() {
    let w = world().await;
    let doc = w.create(w.input(w.stored(b"x").await)).await.unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    let t = now();

    let trashed = Document::soft_delete(&mut conn, doc.id, t)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(trashed.deleted_at, Some(t));
    assert_eq!(trashed.purge_at, Some(t + Duration::days(30)));
    assert!(Document::find(&mut conn, doc.id).await.unwrap().is_none());
    assert!(
        Document::find_with_deleted(&mut conn, doc.id)
            .await
            .unwrap()
            .is_some()
    );
    // a trashed document cannot be trashed again
    assert!(
        Document::soft_delete(&mut conn, doc.id, t)
            .await
            .unwrap()
            .is_none()
    );

    let back = Document::restore(&mut conn, doc.id, now())
        .await
        .unwrap()
        .unwrap();
    assert!(back.deleted_at.is_none() && back.purge_at.is_none());
    assert!(Document::find(&mut conn, doc.id).await.unwrap().is_some());
    drop(conn);
    w.finish().await;
}

#[tokio::test]
async fn bulk_soft_delete_counts_only_live_rows_and_keeps_updated_at() {
    let w = world().await;
    let a = w.create(w.input(w.stored(b"a").await)).await.unwrap();
    let b = w.create(w.input(w.stored(b"b").await)).await.unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    let t = now();
    assert_eq!(
        Document::bulk_soft_delete(&mut conn, &[a.id, b.id, Uuid::new_v4()], t)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        Document::bulk_soft_delete(&mut conn, &[a.id], t)
            .await
            .unwrap(),
        0
    );
    let row = Document::find_with_deleted(&mut conn, a.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.purge_at, Some(t + Duration::days(30)));
    assert_eq!(row.updated_at, a.updated_at);
    drop(conn);
    w.finish().await;
}

/// The purge removes the row, its stored_files row AND the file on disk.
#[tokio::test]
async fn purge_physically_deletes_the_file() {
    let w = world().await;
    let file = w.stored(b"to be purged").await;
    let key = file.key.clone();
    let path = w.storage.path_for(&key).unwrap();
    let doc = w.create(w.input(file)).await.unwrap();
    assert!(path.exists());

    let mut conn = w.pool.acquire().await.unwrap();
    let t = now() - Duration::days(31);
    Document::soft_delete(&mut conn, doc.id, t).await.unwrap();
    let due = Document::purgeable_ids(&mut conn, now()).await.unwrap();
    assert_eq!(due, vec![doc.id]);
    drop(conn);

    assert!(
        w.purge_job()
            .purge_document(doc.id, Utc::now())
            .await
            .unwrap()
    );
    assert!(!path.exists(), "file still on disk after purge");
    assert_eq!(files_on_disk(&w.storage), 0);
    assert_eq!(w.count("documents").await, 0);
    assert_eq!(w.count("stored_files").await, 0);
    // purging twice is harmless
    assert!(
        !w.purge_job()
            .purge_document(doc.id, Utc::now())
            .await
            .unwrap()
    );
    w.finish().await;
}

#[tokio::test]
async fn documents_inside_the_window_are_not_purgeable() {
    let w = world().await;
    let doc = w.create(w.input(w.stored(b"x").await)).await.unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    Document::soft_delete(&mut conn, doc.id, now())
        .await
        .unwrap();
    assert!(
        Document::purgeable_ids(&mut conn, now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        Document::purgeable_ids(&mut conn, now() + Duration::days(31))
            .await
            .unwrap()
            .len()
            == 1
    );
    drop(conn);
    w.finish().await;
}

#[tokio::test]
async fn upload_requests_pointing_at_a_purged_document_are_unlinked() {
    let w = world().await;
    let doc = w.create(w.input(w.stored(b"x").await)).await.unwrap();
    sqlx::query(
        "INSERT INTO upload_requests (callback_url, department_id, document_id, filename, status, \
         uploaded_by, created_at, updated_at) \
         VALUES ('https://example.test/cb', $1, $2, 'a', 'completed', 'c', now(), now())",
    )
    .bind(w.department)
    .bind(doc.id)
    .execute(&w.pool)
    .await
    .unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    Document::soft_delete(&mut conn, doc.id, now() - Duration::days(31))
        .await
        .unwrap();
    drop(conn);
    assert!(
        w.purge_job()
            .purge_document(doc.id, Utc::now())
            .await
            .unwrap()
    );
    let linked: Option<Uuid> = sqlx::query_scalar("SELECT document_id FROM upload_requests")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert!(linked.is_none());
    w.finish().await;
}
