//! `Document`: a file's metadata row. Port of `app/models/document.rb`
//! (soft delete with a 30-day purge window, readonly metadata block, file
//! name normalisation). Creation lives in `document_creator`.

use chrono::{Duration, NaiveDateTime};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::time::rails_json_naive;

/// Grace period between soft-deleting a document and the purge job
/// hard-deleting it (row + file).
pub const RETENTION_DAYS: i64 = 30;

pub fn retention() -> Duration {
    Duration::days(RETENTION_DAYS)
}

pub const COLUMNS: &str = "id, byte_size, content_type, created_at, deleted_at, department_id, \
     filename, purge_at, stored_async, tags, updated_at, uploaded_at, uploaded_by";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Document {
    pub id: Uuid,
    pub byte_size: Option<i64>,
    pub content_type: Option<String>,
    pub created_at: NaiveDateTime,
    pub deleted_at: Option<NaiveDateTime>,
    pub department_id: i64,
    pub filename: String,
    pub purge_at: Option<NaiveDateTime>,
    pub stored_async: bool,
    pub tags: Value,
    pub updated_at: NaiveDateTime,
    pub uploaded_at: NaiveDateTime,
    pub uploaded_by: String,
}

impl Document {
    /// Reduces an arbitrary string to a safe basename (defense in depth, port
    /// of `Document.normalize_filename`): backslashes become slashes, control
    /// bytes (incl. NUL) go, only the basename stays, leading dots are
    /// stripped, and `unnamed` replaces an empty result. Spaces are kept.
    pub fn normalize_filename(value: &str) -> String {
        let name: String = value
            .chars()
            .map(|c| if c == '\\' { '/' } else { c })
            .filter(|c| !matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'))
            .collect();
        let name = ruby_basename(&name);
        let name = name.trim_start_matches('.');
        // `presence`: blank (whitespace only) counts as missing
        if name.chars().all(char::is_whitespace) {
            "unnamed".to_owned()
        } else {
            name.to_owned()
        }
    }

    /// `"sync"` for inline uploads, `"async"` for documents stored by the job.
    pub fn upload_mode(&self) -> &'static str {
        if self.stored_async { "async" } else { "sync" }
    }

    /// Readonly system metadata surfaced in the API `metadata` block. The
    /// indexation date is the top-level `created_at`; the owner `uploaded_by`.
    pub fn metadata_hash(&self) -> Value {
        json!({
            "uploaded_at": rails_json_naive(self.uploaded_at),
            "updated_at": rails_json_naive(self.updated_at),
            "upload_mode": self.upload_mode(),
        })
    }

    /// A live (not trashed) document.
    pub async fn find(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<Document>> {
        sqlx::query_as(sql(&format!(
            "SELECT {COLUMNS} FROM documents WHERE id = $1 AND deleted_at IS NULL"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await
    }

    /// `Document.with_deleted.find`.
    pub async fn find_with_deleted(
        conn: &mut PgConnection,
        id: Uuid,
    ) -> sqlx::Result<Option<Document>> {
        sqlx::query_as(sql(&format!(
            "SELECT {COLUMNS} FROM documents WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await
    }

    /// `soft_delete!`: hides a live document and schedules its purge.
    /// `None` when there is no live document with that id.
    pub async fn soft_delete(
        conn: &mut PgConnection,
        id: Uuid,
        now: NaiveDateTime,
    ) -> sqlx::Result<Option<Document>> {
        sqlx::query_as(sql(&format!(
            "UPDATE documents SET deleted_at = $2, purge_at = $3, updated_at = $2 \
             WHERE id = $1 AND deleted_at IS NULL RETURNING {COLUMNS}"
        )))
        .bind(id)
        .bind(now)
        .bind(now + retention())
        .fetch_optional(conn)
        .await
    }

    /// `bulk_soft_delete`: same two columns as the single path through
    /// `update_all`, so `updated_at` is left alone. Returns the rows affected
    /// (already trashed ids are not counted).
    pub async fn bulk_soft_delete(
        conn: &mut PgConnection,
        ids: &[Uuid],
        now: NaiveDateTime,
    ) -> sqlx::Result<u64> {
        let done = sqlx::query(
            "UPDATE documents SET deleted_at = $2, purge_at = $3 \
             WHERE id = ANY($1) AND deleted_at IS NULL",
        )
        .bind(ids)
        .bind(now)
        .bind(now + retention())
        .execute(conn)
        .await?;
        Ok(done.rows_affected())
    }

    /// `restore!`: brings a *trashed* document back. Conditional on the row
    /// being trashed: restoring a live document (a double click, a stale page,
    /// a race with the purge) changes nothing — `updated_at` included — and
    /// answers `None`, as does an unknown id.
    pub async fn restore(
        conn: &mut PgConnection,
        id: Uuid,
        now: NaiveDateTime,
    ) -> sqlx::Result<Option<Document>> {
        sqlx::query_as(sql(&format!(
            "UPDATE documents SET deleted_at = NULL, purge_at = NULL, updated_at = $2 \
             WHERE id = $1 AND deleted_at IS NOT NULL RETURNING {COLUMNS}"
        )))
        .bind(id)
        .bind(now)
        .fetch_optional(conn)
        .await
    }

    /// `Document.purgeable`: past-due rows, trashed or not.
    pub async fn purgeable_ids(
        conn: &mut PgConnection,
        now: NaiveDateTime,
    ) -> sqlx::Result<Vec<Uuid>> {
        sqlx::query_scalar("SELECT id FROM documents WHERE purge_at <= $1 ORDER BY purge_at, id")
            .bind(now)
            .fetch_all(conn)
            .await
    }
}

/// Trusted, assembled-from-constants SQL.
pub(crate) fn sql(text: &str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(text.to_owned())
}

/// Ruby's `File.basename`: trailing slashes ignored; only slashes gives `/`.
fn ruby_basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "" } else { "/" };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[derive(serde::Deserialize)]
    struct Corpus {
        cases: Vec<Case>,
    }

    #[derive(serde::Deserialize)]
    struct Case {
        input: String,
        normalized: String,
    }

    /// Every hostile name, normalised exactly as Rails does.
    #[test]
    fn normalize_filename_matches_rails_on_hostile_names() {
        let corpus: Corpus =
            serde_json::from_str(include_str!("../../tests/fixtures/filename_corpus.json"))
                .unwrap();
        let mut failures = Vec::new();
        for case in &corpus.cases {
            let got = Document::normalize_filename(&case.input);
            if got != case.normalized {
                failures.push(format!(
                    "{:?}: rails={:?} rust={got:?}",
                    case.input, case.normalized
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn normalized_names_never_carry_separators_or_control_bytes() {
        for hostile in [
            "../../etc/passwd",
            "a\\b\\c",
            "x\r\ny",
            "\0",
            "..",
            "a/../b",
        ] {
            let name = Document::normalize_filename(hostile);
            assert!(
                !name.contains('\\') && !name.chars().any(char::is_control),
                "{name:?}"
            );
            assert!(!name.contains('/') || name == "/", "{name:?}");
            assert!(!name.starts_with('.'), "{name:?}");
            assert!(!name.is_empty());
        }
    }

    fn doc(stored_async: bool) -> Document {
        let at = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_micro_opt(21, 12, 39, 123_999)
            .unwrap();
        Document {
            id: Uuid::nil(),
            byte_size: Some(3),
            content_type: None,
            created_at: at,
            deleted_at: None,
            department_id: 1,
            filename: "a.pdf".into(),
            purge_at: None,
            stored_async,
            tags: json!({}),
            updated_at: at,
            uploaded_at: at,
            uploaded_by: "c".into(),
        }
    }

    #[test]
    fn metadata_hash_is_the_readonly_block_in_rails_order() {
        let meta = doc(false).metadata_hash();
        assert_eq!(
            meta.to_string(),
            r#"{"uploaded_at":"2026-11-01T21:12:39.123Z","updated_at":"2026-11-01T21:12:39.123Z","upload_mode":"sync"}"#
        );
        assert_eq!(doc(true).metadata_hash()["upload_mode"], "async");
    }

    #[test]
    fn retention_is_thirty_days() {
        assert_eq!(retention(), Duration::days(30));
    }
}
