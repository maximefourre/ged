//! `PurgeDeletedDocumentsJob` (queue `default`, daily at 03:00): hard-deletes
//! documents whose retention window elapsed, then the trashed departments
//! nothing references any more, then files no row points to. Port of
//! `app/jobs/purge_deleted_documents_job.rb`.
//!
//! Every destroy is isolated: a failure is logged, the pass carries on, and
//! the job reports how many failed once it is done.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use futures_util::future::BoxFuture;
use serde_json::Value;
use uuid::Uuid;

use super::env::PipelineEnv;
use super::{Job, JobError, timed};
use crate::domain::department::Department;
use crate::domain::document::Document;
use crate::telemetry::{JobClass, JobQueue};

pub const KIND: &str = "PurgeDeletedDocumentsJob";
/// `ORPHAN_BLOB_AGE`: 1 day. In-flight uploads (file written, row not yet
/// committed) are younger than this.
pub const ORPHAN_AGE: Duration = Duration::from_secs(86_400);

pub struct PurgeDeletedDocumentsJob {
    env: PipelineEnv,
}

impl PurgeDeletedDocumentsJob {
    pub fn new(env: PipelineEnv) -> Self {
        Self { env }
    }

    /// One pass as of `now`.
    pub async fn run(&self, now: DateTime<Utc>) -> Result<(), JobError> {
        let mut failures = Failures::default();
        self.purge_documents(now, &mut failures).await;
        self.purge_departments(&mut failures).await;
        self.purge_orphan_files(now, &mut failures).await;
        failures.into_result()
    }

    /// Hard-deletes one document if, and only if, it is still past due:
    /// restored (`purge_at` back to NULL) or rescheduled in the meantime, it
    /// stays. The row and its `stored_files` row go in one transaction; the
    /// file follows. `false` when there was nothing to purge.
    pub async fn purge_document(&self, id: Uuid, now: DateTime<Utc>) -> Result<bool, sqlx::Error> {
        let mut tx = self.env.db.begin().await?;
        let key: Option<String> = sqlx::query_scalar(
            "SELECT s.key FROM stored_files s JOIN documents d ON d.id = s.document_id \
             WHERE d.id = $1 AND d.purge_at <= $2 FOR UPDATE OF d",
        )
        .bind(id)
        .bind(now.naive_utc())
        .fetch_optional(&mut *tx)
        .await?;
        let deleted = sqlx::query("DELETE FROM documents WHERE id = $1 AND purge_at <= $2")
            .bind(id)
            .bind(now.naive_utc())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        if deleted == 0 {
            return Ok(false);
        }
        if let Some(key) = key
            && let Err(e) = self.env.storage.delete(&key).await
        {
            // The row is gone; the orphan sweep takes the file later.
            eprintln!(
                "PurgeDeletedDocumentsJob: could not delete file {key} of document {id}: {e}"
            );
        }
        Ok(true)
    }

    async fn purge_documents(&self, now: DateTime<Utc>, failures: &mut Failures) {
        let due = {
            let mut conn = match self.env.db.acquire().await {
                Ok(c) => c,
                Err(e) => return failures.note("listing due documents", &e),
            };
            Document::purgeable_ids(&mut conn, now.naive_utc()).await
        };
        match due {
            Err(e) => failures.note("listing due documents", &e),
            Ok(ids) => {
                for id in ids {
                    if let Err(e) = self.purge_document(id, now).await {
                        failures.note(&format!("document {id}"), &e);
                    }
                }
            }
        }
    }

    /// Trashed departments no document (trashed included), upload request or
    /// child (trashed included) references any more.
    async fn purge_departments(&self, failures: &mut Failures) {
        let trashed = match Department::deleted(&self.env.db).await {
            Ok(d) => d,
            Err(e) => return failures.note("listing trashed departments", &e),
        };
        for department in trashed {
            let result = async {
                if department.references_cleared(&self.env.db).await? {
                    self.purge_department(department.id).await?;
                }
                Ok::<_, sqlx::Error>(())
            }
            .await;
            if let Err(e) = result {
                failures.note(&format!("department {}", department.id), &e);
            }
        }
    }

    /// Hard-deletes one department (and its grants) if, and only if, it is
    /// still trashed: restored since the trash was listed, it stays. The row
    /// is locked and re-checked in the transaction. `false` = nothing purged.
    pub async fn purge_department(&self, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.env.db.begin().await?;
        let trashed: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM departments WHERE id = $1 AND deleted_at IS NOT NULL FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if trashed.is_none() {
            return Ok(false);
        }
        sqlx::query("DELETE FROM client_department_accesses WHERE department_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM departments WHERE id = $1 AND deleted_at IS NOT NULL")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Files under the storage root no `stored_files` row points to (a crash
    /// between writing the bytes and committing the row) and abandoned
    /// `.part` files, once older than [`ORPHAN_AGE`]. Staging is not ours.
    async fn purge_orphan_files(&self, now: DateTime<Utc>, failures: &mut Failures) {
        let cutoff = SystemTime::from(now) - ORPHAN_AGE;
        let files = list_files(self.env.storage.root(), self.env.staging.root()).await;
        for path in files {
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            let old = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|m| m < cutoff);
            if !old {
                continue;
            }
            let orphan = if name.ends_with(".part") {
                true
            } else if is_key(&name) {
                match sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM stored_files WHERE key = $1)",
                )
                .bind(&name)
                .fetch_one(&self.env.db)
                .await
                {
                    Ok(referenced) => !referenced,
                    Err(e) => {
                        failures.note(&format!("file {name}"), &e);
                        continue;
                    }
                }
            } else {
                false // not something this store wrote
            };
            if orphan
                && let Err(e) = tokio::fs::remove_file(&path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                failures.note(&format!("file {name}"), &e);
            }
        }
    }
}

impl Job for PurgeDeletedDocumentsJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn perform(&self, _args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        let job = Self::new(self.env.clone());
        Box::pin(timed(
            JobClass::PurgeDeletedDocumentsJob,
            JobQueue::Default,
            async move { job.run(Utc::now()).await },
        ))
    }
}

fn is_key(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Regular files below `root`, skipping the `skip` directory.
async fn list_files(root: &Path, skip: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path == skip {
                continue;
            }
            match entry.file_type().await {
                Ok(t) if t.is_dir() => dirs.push(path),
                Ok(t) if t.is_file() => files.push(path),
                _ => {}
            }
        }
    }
    files
}

#[derive(Default)]
struct Failures {
    count: usize,
    first: Option<String>,
}

impl Failures {
    fn note(&mut self, what: &str, error: &dyn std::fmt::Display) {
        eprintln!("PurgeDeletedDocumentsJob: failed to purge {what}: {error}");
        self.count += 1;
        self.first.get_or_insert_with(|| format!("{what}: {error}"));
    }

    fn into_result(self) -> Result<(), JobError> {
        match self.first {
            None => Ok(()),
            Some(first) => Err(JobError::Retry(format!(
                "{} purge(s) failed, first: {first}",
                self.count
            ))),
        }
    }
}
