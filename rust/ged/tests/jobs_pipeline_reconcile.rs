//! T9: `ReconcileUploadRequestsJob` (STUCK_AFTER 600 s, 30-day retention).

mod pipeline_support;
mod support;

use chrono::Utc;
use ged::jobs::queue::{self, QueueName};
use ged::jobs::reconcile::ReconcileUploadRequestsJob;
use ged::jobs::scheduler::{self, Cadence};
use pipeline_support::{Fixture, age_file};
use serde_json::json;
use uuid::Uuid;

const CALLBACK: &str = "https://client.example/hook";

fn job(f: &Fixture) -> ReconcileUploadRequestsJob {
    ReconcileUploadRequestsJob::new(f.env.clone())
}

fn store_job(id: Uuid) -> (String, String, serde_json::Value) {
    (
        "uploads".into(),
        "StoreDocumentJob".into(),
        json!({"upload_request_id": id.to_string()}),
    )
}

fn callback_job(id: Uuid) -> (String, String, serde_json::Value) {
    (
        "default".into(),
        "DeliverCallbackJob".into(),
        json!({"upload_request_id": id.to_string()}),
    )
}

#[tokio::test]
async fn a_stuck_pending_request_with_its_file_is_requeued() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let stuck = f
        .upload_request(dept, "pending", CALLBACK, Some(b"x"))
        .await;
    let fresh = f
        .upload_request(dept, "pending", CALLBACK, Some(b"x"))
        .await;
    f.set_times(stuck, "11 minutes", "11 minutes").await;
    f.set_times(fresh, "9 minutes", "9 minutes").await;

    job(&f).run(Utc::now()).await.unwrap();

    assert_eq!(f.jobs().await, vec![store_job(stuck)]);
    assert_eq!(f.status(stuck).await.0, "pending");
    f.finish().await;
}

#[tokio::test]
async fn a_stuck_pending_request_without_its_file_fails_and_notifies() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f.upload_request(dept, "pending", CALLBACK, None).await;
    f.set_times(id, "11 minutes", "11 minutes").await;

    job(&f).run(Utc::now()).await.unwrap();

    let (status, error, _) = f.status(id).await;
    assert_eq!(
        (status.as_str(), error.as_deref()),
        ("failed", Some("staged file lost before processing"))
    );
    assert_eq!(f.jobs().await, vec![callback_job(id)]);
    f.finish().await;
}

#[tokio::test]
async fn a_paused_uploads_queue_keeps_pending_requests_waiting() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let with_file = f
        .upload_request(dept, "pending", CALLBACK, Some(b"x"))
        .await;
    let without = f.upload_request(dept, "pending", CALLBACK, None).await;
    for id in [with_file, without] {
        f.set_times(id, "20 minutes", "20 minutes").await;
    }
    queue::pause(&f.pool, QueueName::Uploads).await.unwrap();

    job(&f).run(Utc::now()).await.unwrap();

    assert!(
        f.jobs().await.is_empty(),
        "nothing piles into a paused queue"
    );
    assert_eq!(f.status(with_file).await.0, "pending");
    assert_eq!(f.status(without).await.0, "pending", "not failed either");
    f.finish().await;
}

#[tokio::test]
async fn a_stuck_processing_request_is_reset_or_failed() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let with_file = f
        .upload_request(dept, "processing", CALLBACK, Some(b"x"))
        .await;
    let without = f.upload_request(dept, "processing", CALLBACK, None).await;
    let busy = f
        .upload_request(dept, "processing", CALLBACK, Some(b"x"))
        .await;
    // created long ago: only updated_at (the claim time) counts
    f.set_times(with_file, "1 hour", "11 minutes").await;
    f.set_times(without, "1 hour", "11 minutes").await;
    f.set_times(busy, "1 hour", "2 minutes").await;

    job(&f).run(Utc::now()).await.unwrap();

    assert_eq!(f.status(with_file).await.0, "pending");
    assert_eq!(f.status(without).await.0, "failed");
    assert_eq!(f.status(busy).await.0, "processing");
    let mut jobs = f.jobs().await;
    jobs.sort_by_key(|j| j.1.clone());
    assert_eq!(jobs, vec![callback_job(without), store_job(with_file)]);
    f.finish().await;
}

#[tokio::test]
async fn processing_requests_are_recovered_even_while_uploads_are_paused() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "processing", CALLBACK, Some(b"x"))
        .await;
    f.set_times(id, "1 hour", "11 minutes").await;
    queue::pause(&f.pool, QueueName::Uploads).await.unwrap();

    job(&f).run(Utc::now()).await.unwrap();

    assert_eq!(f.status(id).await.0, "pending");
    f.finish().await;
}

#[tokio::test]
async fn terminal_requests_older_than_thirty_days_go_with_their_staging_file() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let old_done = f
        .upload_request(dept, "completed", CALLBACK, Some(b"x"))
        .await;
    let old_failed = f.upload_request(dept, "failed", CALLBACK, None).await;
    let young = f
        .upload_request(dept, "completed", CALLBACK, Some(b"x"))
        .await;
    let old_pending = f.upload_request(dept, "pending", CALLBACK, None).await;
    f.set_times(old_done, "31 days", "31 days").await;
    f.set_times(old_failed, "31 days", "31 days").await;
    f.set_times(young, "29 days", "29 days").await;
    f.set_times(old_pending, "31 days", "31 days").await;
    queue::pause(&f.pool, QueueName::Uploads).await.unwrap();

    job(&f).run(Utc::now()).await.unwrap();

    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM upload_requests ORDER BY created_at")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    assert_eq!(left.len(), 2);
    assert!(left.contains(&young) && left.contains(&old_pending));
    assert!(!f.env.staging.exists(old_done).await);
    assert!(f.env.staging.exists(young).await);
    f.finish().await;
}

#[tokio::test]
async fn staging_files_older_than_thirty_days_are_swept() {
    let f = Fixture::new().await;
    let old = Uuid::new_v4();
    let recent = Uuid::new_v4();
    for id in [old, recent] {
        std::fs::write(f.env.staging.path(id), b"x").unwrap();
    }
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(f.env.staging.path(old))
        .unwrap()
        .set_modified(long_ago)
        .unwrap();

    job(&f).run(Utc::now()).await.unwrap();

    assert!(!f.env.staging.exists(old).await);
    assert!(f.env.staging.exists(recent).await);
    f.finish().await;
}

/// C2 (T7 review): a crash between the staging write and the row insert leaves
/// `<uuid>` or `<uuid>.part` with no request; swept after a safe margin (1 day).
#[tokio::test]
async fn orphan_staging_files_are_swept_after_a_day_but_not_before() {
    let f = Fixture::new().await;
    let root = f.env.staging.root().to_owned();
    let (old, young, old_part, young_part) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let files = [
        (old, "", 2),
        (young, "", 0),
        (old_part, ".part", 2),
        (young_part, ".part", 0),
    ];
    for (id, ext, days) in files {
        let path = root.join(format!("{id}{ext}"));
        std::fs::write(&path, b"x").unwrap();
        if days > 0 {
            age_file(&path, days);
        }
    }

    job(&f).run(Utc::now()).await.unwrap();

    assert!(!root.join(old.to_string()).exists());
    assert!(!root.join(format!("{old_part}.part")).exists());
    assert!(root.join(young.to_string()).exists());
    assert!(root.join(format!("{young_part}.part")).exists());
    f.finish().await;
}

#[tokio::test]
async fn staging_of_live_requests_is_never_swept_but_terminal_ones_are() {
    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let mut ids = Vec::new();
    for status in ["pending", "processing", "completed", "failed"] {
        let id = f.upload_request(dept, status, CALLBACK, Some(b"x")).await;
        age_file(&f.env.staging.path(id), 2);
        ids.push((status, id));
    }
    // keep pending/processing from being recovered: they are young rows
    queue::pause(&f.pool, QueueName::Uploads).await.unwrap();

    job(&f).run(Utc::now()).await.unwrap();

    for (status, id) in ids {
        let live = matches!(status, "pending" | "processing");
        assert_eq!(f.env.staging.exists(id).await, live, "{status}");
    }
    f.finish().await;
}

/// `config/recurring.yml`, production block, is the source of truth.
#[test]
fn the_schedule_matches_config_recurring_yml() {
    let yml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/recurring.yml"
    ))
    .unwrap();
    let find = |class: &str| {
        let after = yml
            .split(&format!("class: {class}"))
            .nth(1)
            .unwrap_or_else(|| panic!("{class}"));
        after
            .lines()
            .find(|l| l.trim_start().starts_with("schedule:"))
            .unwrap()
            .trim()
            .to_owned()
    };
    assert_eq!(
        find("ReconcileUploadRequestsJob"),
        "schedule: every 10 minutes"
    );
    assert_eq!(
        find("PurgeDeletedDocumentsJob"),
        "schedule: every day at 3am"
    );

    let tasks = scheduler::default_schedule();
    let by_key = |key: &str| tasks.iter().find(|t| t.key == key).unwrap();
    let reconcile = by_key("reconcile_upload_requests");
    assert_eq!(
        (reconcile.kind.as_str(), reconcile.queue, reconcile.cadence),
        (
            "ReconcileUploadRequestsJob",
            QueueName::Default,
            Cadence::EveryMinutes(10)
        )
    );
    let purge = by_key("purge_deleted_documents");
    assert_eq!(
        (purge.kind.as_str(), purge.queue, purge.cadence),
        (
            "PurgeDeletedDocumentsJob",
            QueueName::Default,
            Cadence::DailyAt(3)
        )
    );
}
