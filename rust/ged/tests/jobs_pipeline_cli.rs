//! T9: `ged run-job <Kind>`: one job, in the foreground (what the parity
//! `Backend::run_job` calls; the Rails equivalent is `Job.perform_now`).

mod pipeline_support;
mod support;

use std::process::{Command, Stdio};

use chrono::{Duration, Utc};
use pipeline_support::Fixture;

fn ged(url: &str, cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ged"))
        .arg("run-job")
        .args(args)
        .env_clear()
        .env("RAILS_ENV", "test")
        .env("DATABASE_URL", url)
        .current_dir(cwd)
        .output()
        .unwrap()
}

#[tokio::test]
async fn run_job_performs_the_named_job_against_the_cwd_storage() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let due = (Utc::now() - Duration::days(1)).naive_utc();
    let (id, key) = f.document(dept, Some(due)).await;
    // the binary looks for files under ./storage, like the Rails app root
    let cwd = tempfile::tempdir().unwrap();
    let dest = cwd.path().join("storage").join(&key[0..2]).join(&key[2..4]);
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::copy(f.env.storage.path_for(&key).unwrap(), dest.join(&key)).unwrap();
    let url = f.url();

    let out = tokio::task::spawn_blocking({
        let cwd = cwd.path().to_owned();
        move || ged(&url, &cwd, &["PurgeDeletedDocumentsJob"])
    })
    .await
    .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let left: i64 = f.count("SELECT count(*) FROM documents").await;
    assert_eq!(left, 0, "{id} purged");
    assert!(!dest.join(&key).exists());
    f.finish().await;
}

#[tokio::test]
async fn run_job_rejects_an_unknown_kind() {
    let f = Fixture::new().await;
    let url = f.url();
    let cwd = tempfile::tempdir().unwrap();
    let out = tokio::task::spawn_blocking({
        let cwd = cwd.path().to_owned();
        move || ged(&url, &cwd, &["NoSuchJob"])
    })
    .await
    .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown job NoSuchJob"));
    f.finish().await;
}

/// Review M1: a real crash between the document write and the `completed`
/// update. `ged run-job StoreDocumentJob` is SIGKILLed while its transaction
/// is open (and its backend dropped); the stuck `processing` request is reset
/// by `ReconcileUploadRequestsJob`, and the re-run yields exactly one document
/// and one callback.
#[tokio::test]
async fn a_sigkilled_store_run_is_recovered_by_reconcile_with_exactly_one_document() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(
            dept,
            "pending",
            "https://client.example/hook",
            Some(b"crash"),
        )
        .await;
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("storage/staging")).unwrap();
    std::fs::write(cwd.path().join(format!("storage/staging/{id}")), b"crash").unwrap();
    // The document insert hangs inside the transaction: the kill lands in it.
    sqlx::raw_sql(
        "CREATE FUNCTION hang_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_sleep(60); RETURN NEW; END $$; \
         CREATE TRIGGER hang_insert BEFORE INSERT ON documents \
         FOR EACH ROW EXECUTE FUNCTION hang_insert();",
    )
    .execute(&f.pool)
    .await
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_ged"))
        .args([
            "run-job",
            "StoreDocumentJob",
            &format!("{{\"upload_request_id\":\"{id}\"}}"),
        ])
        .env_clear()
        .env("RAILS_ENV", "test")
        .env("DATABASE_URL", f.url())
        .current_dir(cwd.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // wait until the child is inside the open transaction
    let mut hanging = 0;
    for _ in 0..200 {
        hanging = f
            .count("SELECT count(*) FROM pg_stat_activity WHERE wait_event = 'PgSleep'")
            .await;
        if hanging > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(hanging > 0, "the run never reached the document insert");
    child.kill().unwrap(); // SIGKILL
    child.wait().unwrap();
    // the dead process's connection is gone: release its backend
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE wait_event = 'PgSleep'",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::raw_sql("DROP TRIGGER hang_insert ON documents")
        .execute(&f.pool)
        .await
        .unwrap();

    assert_eq!(f.status(id).await.0, "processing", "stuck by the crash");
    assert_eq!(f.count("SELECT count(*) FROM documents").await, 0);
    assert!(f.jobs().await.is_empty());

    // 20 min later reconcile resets it (the staging file is still there)
    std::fs::copy(
        cwd.path().join(format!("storage/staging/{id}")),
        f.env.staging.path(id),
    )
    .unwrap();
    f.set_times(id, "20 minutes", "20 minutes").await;
    ged::jobs::reconcile::ReconcileUploadRequestsJob::new(f.env.clone())
        .run(Utc::now())
        .await
        .unwrap();
    assert_eq!(f.status(id).await.0, "pending");
    assert_eq!(f.jobs().await.len(), 1, "StoreDocumentJob re-enqueued");

    let url = f.url();
    let out = tokio::task::spawn_blocking({
        let cwd = cwd.path().to_owned();
        let arg = format!("{{\"upload_request_id\":\"{id}\"}}");
        move || ged(&url, &cwd, &["StoreDocumentJob", &arg])
    })
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(f.status(id).await.0, "completed");
    assert_eq!(f.count("SELECT count(*) FROM documents").await, 1);
    assert_eq!(f.count("SELECT count(*) FROM stored_files").await, 1);
    assert_eq!(
        f.count("SELECT count(*) FROM jobs WHERE kind = 'DeliverCallbackJob'")
            .await,
        1
    );
    f.finish().await;
}
