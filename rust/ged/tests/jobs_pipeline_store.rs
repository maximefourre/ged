//! T9: `StoreDocumentJob` against a scratch database.

mod pipeline_support;
mod support;

use ged::jobs::store_document::StoreDocumentJob;
use ged::jobs::{Job, JobError};
use pipeline_support::Fixture;
use serde_json::json;
use uuid::Uuid;

fn args(id: Uuid) -> serde_json::Value {
    json!({ "upload_request_id": id.to_string() })
}

const CALLBACK: &str = "https://client.example/hook";

#[tokio::test]
async fn it_stores_the_document_and_completes_the_request_together() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", CALLBACK, Some(b"hello pipeline"))
        .await;

    StoreDocumentJob::new(f.env.clone())
        .perform(args(id))
        .await
        .unwrap();

    let (status, error, document_id) = f.status(id).await;
    assert_eq!((status.as_str(), error), ("completed", None));
    let document_id = document_id.expect("document_id set");
    let (filename, uploaded_by, stored_async, byte_size, tags, uploaded_at, created_at): (
        String, String, bool, Option<i64>, serde_json::Value, chrono::NaiveDateTime, chrono::NaiveDateTime,
    ) = sqlx::query_as(
        "SELECT d.filename, d.uploaded_by, d.stored_async, d.byte_size, d.tags, d.uploaded_at, r.created_at \
         FROM documents d, upload_requests r WHERE d.id = $1 AND r.id = $2",
    )
    .bind(document_id)
    .bind(id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(filename, "report.txt");
    assert_eq!(uploaded_by, "client-a");
    assert!(stored_async);
    assert_eq!(byte_size, Some(14));
    assert_eq!(tags, json!({"type": "facture", "urgent": null}));
    // uploaded_at is the originating request's created_at, to the microsecond
    assert_eq!(uploaded_at, created_at);

    assert!(!f.env.staging.exists(id).await, "staging file removed");
    assert_eq!(f.stored_files().len(), 1, "one stored file");
    assert_eq!(
        f.jobs().await,
        vec![("default".into(), "DeliverCallbackJob".into(), args(id))]
    );
    f.finish().await;
}

/// Plan T9: a crash between the document write and the `completed` update
/// leaves no effect (one transaction). The second write is made to fail at the
/// database; nothing of the first may survive, and the job still reports.
#[tokio::test]
async fn a_failure_between_the_two_writes_leaves_no_document() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", CALLBACK, Some(b"bytes"))
        .await;
    sqlx::raw_sql(
        "CREATE FUNCTION refuse_completed() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.status = 'completed' THEN RAISE EXCEPTION 'injected crash before completed'; END IF; \
         RETURN NEW; END $$; \
         CREATE TRIGGER refuse_completed BEFORE UPDATE ON upload_requests \
         FOR EACH ROW EXECUTE FUNCTION refuse_completed();",
    )
    .execute(&f.pool)
    .await
    .unwrap();

    StoreDocumentJob::new(f.env.clone())
        .perform(args(id))
        .await
        .unwrap();

    assert_eq!(f.count("SELECT count(*) FROM documents").await, 0);
    assert_eq!(f.count("SELECT count(*) FROM stored_files").await, 0);
    assert!(f.stored_files().is_empty(), "{:?}", f.stored_files());
    let (status, error, document_id) = f.status(id).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("injected crash before completed"));
    assert_eq!(document_id, None);
    assert!(!f.env.staging.exists(id).await);
    assert_eq!(
        f.jobs().await.len(),
        1,
        "failure is still reported by callback"
    );
    f.finish().await;
}

#[tokio::test]
async fn a_rerun_or_a_race_stores_the_document_once() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", CALLBACK, Some(b"once"))
        .await;
    let job = StoreDocumentJob::new(f.env.clone());

    let runs = futures_util::future::join_all((0..6).map(|_| job.perform(args(id)))).await;
    assert!(runs.iter().all(Result::is_ok));
    job.perform(args(id)).await.unwrap();

    assert_eq!(f.count("SELECT count(*) FROM documents").await, 1);
    assert_eq!(f.stored_files().len(), 1);
    assert_eq!(f.jobs().await.len(), 1, "one callback only");
    f.finish().await;
}

#[tokio::test]
async fn a_validation_failure_marks_the_request_failed_with_the_rails_message() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", CALLBACK, Some(b"bytes"))
        .await;
    sqlx::query("UPDATE departments SET deleted_at = now() WHERE id = $1")
        .bind(dept)
        .execute(&f.pool)
        .await
        .unwrap();

    StoreDocumentJob::new(f.env.clone())
        .perform(args(id))
        .await
        .unwrap();

    let (status, error, _) = f.status(id).await;
    assert_eq!(status, "failed");
    assert_eq!(
        error.as_deref(),
        Some("Validation failed: Department must exist")
    );
    assert!(
        f.stored_files().is_empty(),
        "file written before validation is removed"
    );
    assert!(!f.env.staging.exists(id).await);
    f.finish().await;
}

#[tokio::test]
async fn an_oversized_staged_file_is_rejected_like_the_model_validation() {
    let f = Fixture::with_max_bytes(4).await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", CALLBACK, Some(b"12345"))
        .await;

    StoreDocumentJob::new(f.env.clone())
        .perform(args(id))
        .await
        .unwrap();

    let (status, error, _) = f.status(id).await;
    assert_eq!(status, "failed");
    assert_eq!(
        error.as_deref(),
        Some("Validation failed: Byte size must be less than or equal to 4")
    );
    assert!(f.stored_files().is_empty());
    f.finish().await;
}

#[tokio::test]
async fn a_missing_staging_file_fails_the_request() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f.upload_request(dept, "pending", CALLBACK, None).await;

    StoreDocumentJob::new(f.env.clone())
        .perform(args(id))
        .await
        .unwrap();

    let (status, error, _) = f.status(id).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().starts_with("No such file or directory"));
    assert_eq!(f.jobs().await.len(), 1);
    f.finish().await;
}

#[tokio::test]
async fn it_leaves_requests_that_are_not_pending_alone() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    for status in ["processing", "completed", "failed"] {
        let id = f.upload_request(dept, status, CALLBACK, Some(b"x")).await;
        StoreDocumentJob::new(f.env.clone())
            .perform(args(id))
            .await
            .unwrap();
        assert_eq!(f.status(id).await.0, status);
        assert!(
            f.env.staging.exists(id).await,
            "{status}: staging untouched"
        );
    }
    assert_eq!(f.count("SELECT count(*) FROM documents").await, 0);
    assert!(f.jobs().await.is_empty());
    f.finish().await;
}

#[tokio::test]
async fn an_unknown_request_or_bad_args_is_discarded() {
    let f = Fixture::new().await;
    let job = StoreDocumentJob::new(f.env.clone());
    assert!(matches!(
        job.perform(args(Uuid::new_v4())).await,
        Err(JobError::Discard(_))
    ));
    assert!(matches!(
        job.perform(json!({})).await,
        Err(JobError::Discard(_))
    ));
    assert_eq!(job.kind(), "StoreDocumentJob");
    f.finish().await;
}

/// Reconcile gave up on a slow run and reset the request to pending: the slow
/// run must not complete it behind the new run's back (double document).
#[tokio::test]
async fn a_run_overtaken_by_reconcile_stores_nothing_and_keeps_the_staging_file() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", CALLBACK, Some(b"slow"))
        .await;
    sqlx::raw_sql(
        "CREATE FUNCTION slow_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_sleep(1); RETURN NEW; END $$; \
         CREATE TRIGGER slow_insert BEFORE INSERT ON documents \
         FOR EACH ROW EXECUTE FUNCTION slow_insert();",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let job = StoreDocumentJob::new(f.env.clone());
    let running = tokio::spawn(job.perform(args(id)));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    sqlx::query("UPDATE upload_requests SET status = 'pending' WHERE id = $1")
        .bind(id)
        .execute(&f.pool)
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    assert_eq!(f.count("SELECT count(*) FROM documents").await, 0);
    assert!(f.stored_files().is_empty());
    assert_eq!(f.status(id).await.0, "pending");
    assert!(f.env.staging.exists(id).await, "the new run still needs it");
    assert!(f.jobs().await.is_empty());
    f.finish().await;
}
