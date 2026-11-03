//! `Admin::StatusController`: system health, the upload pipeline and the
//! document/storage counts. Everything is read from the application database
//! (the job queue lives in it), never from live HTTP.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};

use super::system_health::{self, Check};
use super::upload_requests::STATUSES;
use super::view::{AdminResult, Chrome, Ctx, human_size};
use crate::http::AppState;
use crate::jobs::queue::{self, QueueName};

const RECENT_FAILURES: i64 = 5;

struct Failure {
    kind: String,
    error: String,
    at: String,
}

struct Top {
    name: String,
    volume: String,
    documents: i64,
}

#[derive(Template)]
#[template(path = "admin/status/index.html")]
struct Page {
    chrome: Chrome,
    checks: Vec<Check>,
    paused: bool,
    by_status: Vec<(&'static str, i64)>,
    depths: Vec<(&'static str, i64)>,
    scheduled: i64,
    in_flight: i64,
    failed: i64,
    failures: Vec<Failure>,
    live: i64,
    trashed: i64,
    purgeable: i64,
    volume: String,
    sync: i64,
    asynchronous: i64,
    uploads_24h: i64,
    uploads_7d: i64,
    top: Vec<Top>,
}

/// ActionView's `truncate(text, length: 120)`.
fn truncate(text: &str, length: usize) -> String {
    if text.chars().count() <= length {
        return text.to_owned();
    }
    let head: String = text.chars().take(length.saturating_sub(3)).collect();
    format!("{head}...")
}

async fn count(db: &sqlx::PgPool, sql: &'static str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(sql).fetch_one(db).await
}

pub async fn index(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    let db = &state.db;
    let alive = crate::jobs::worker::WorkerSettings::from_process_env()
        .heartbeat
        .expiry;
    let checks = system_health::checks(db, &state.jwks, &super::storage_root(), alive).await;

    let ready: Vec<(String, i64)> = sqlx::query_as(
        "SELECT queue, count(*) FROM jobs WHERE finished_at IS NULL AND failed_at IS NULL \
         AND claimed_by IS NULL AND run_at <= now() GROUP BY 1",
    )
    .fetch_all(db)
    .await?;
    let ready: HashMap<String, i64> = ready.into_iter().collect();
    let depths = QueueName::ALL
        .iter()
        .map(|q| (q.as_str(), ready.get(q.as_str()).copied().unwrap_or(0)))
        .collect();

    let requests: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, count(*) FROM upload_requests GROUP BY 1")
            .fetch_all(db)
            .await?;
    let requests: HashMap<String, i64> = requests.into_iter().collect();
    let by_status = STATUSES
        .iter()
        .map(|s| (*s, requests.get(*s).copied().unwrap_or(0)))
        .collect();

    let failures: Vec<(String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT kind, last_error, failed_at FROM jobs WHERE failed_at IS NOT NULL \
         ORDER BY failed_at DESC LIMIT $1",
    )
    .bind(RECENT_FAILURES)
    .fetch_all(db)
    .await?;

    let split: Vec<(bool, i64)> = sqlx::query_as(
        "SELECT stored_async, count(*) FROM documents WHERE deleted_at IS NULL GROUP BY 1",
    )
    .fetch_all(db)
    .await?;
    let split: HashMap<bool, i64> = split.into_iter().collect();

    let top: Vec<(Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT d.name, s.bytes, s.docs FROM ( \
            SELECT doc.department_id, COALESCE(sum(f.byte_size), 0)::bigint AS bytes, \
                   count(*) AS docs \
            FROM documents doc JOIN stored_files f ON f.document_id = doc.id \
            WHERE doc.deleted_at IS NULL GROUP BY doc.department_id) s \
         LEFT JOIN departments d ON d.id = s.department_id AND d.deleted_at IS NULL \
         ORDER BY s.bytes DESC, s.department_id LIMIT 5",
    )
    .fetch_all(db)
    .await?;

    let total_bytes: i64 = count(
        db,
        "SELECT COALESCE(sum(byte_size), 0)::bigint FROM stored_files",
    )
    .await?;
    let page = Page {
        chrome: ctx.chrome("status"),
        checks,
        paused: queue::is_paused(db, QueueName::Uploads).await?,
        by_status,
        depths,
        scheduled: count(db, "SELECT count(*) FROM jobs WHERE finished_at IS NULL AND failed_at IS NULL AND claimed_by IS NULL AND run_at > now()").await?,
        in_flight: count(db, "SELECT count(*) FROM jobs WHERE finished_at IS NULL AND failed_at IS NULL AND claimed_by IS NOT NULL").await?,
        failed: count(db, "SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL").await?,
        failures: failures
            .into_iter()
            .map(|(kind, error, at)| Failure {
                kind,
                error: truncate(&error.unwrap_or_default(), 120),
                at: at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect(),
        live: count(db, "SELECT count(*) FROM documents WHERE deleted_at IS NULL").await?,
        trashed: count(db, "SELECT count(*) FROM documents WHERE deleted_at IS NOT NULL").await?,
        purgeable: count(db, "SELECT count(*) FROM documents WHERE deleted_at IS NOT NULL AND purge_at <= (now() AT TIME ZONE 'utc')").await?,
        volume: human_size(total_bytes),
        sync: split.get(&false).copied().unwrap_or(0),
        asynchronous: split.get(&true).copied().unwrap_or(0),
        uploads_24h: count(db, "SELECT count(*) FROM documents WHERE uploaded_at >= (now() AT TIME ZONE 'utc') - interval '24 hours'").await?,
        uploads_7d: count(db, "SELECT count(*) FROM documents WHERE uploaded_at >= (now() AT TIME ZONE 'utc') - interval '7 days'").await?,
        top: top
            .into_iter()
            .map(|(name, bytes, documents)| Top {
                name: name.unwrap_or_default(),
                volume: human_size(bytes),
                documents,
            })
            .collect(),
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_matches_action_view() {
        assert_eq!(truncate("short", 120), "short");
        let long = "x".repeat(130);
        let t = truncate(&long, 120);
        assert_eq!(t.chars().count(), 120);
        assert!(t.ends_with("..."));
    }
}
