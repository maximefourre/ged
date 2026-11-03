//! T9: `ged run-job <Kind>`: one job, in the foreground (what the parity
//! `Backend::run_job` calls; the Rails equivalent is `Job.perform_now`).

mod pipeline_support;
mod support;

use std::process::Command;

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
