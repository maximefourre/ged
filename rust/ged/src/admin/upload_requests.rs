//! `Admin::UploadRequestsController` and `Admin::UploadQueuePausesController`:
//! the async-upload requests list, and the Pause/Resume of the uploads queue.

use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use chrono::NaiveDateTime;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

use super::view::{AdminResult, Chrome, Ctx};
use crate::http::{AppState, RackParams};
use crate::jobs::queue::{self, QueueName};

pub const STATUSES: [&str; 4] = ["pending", "processing", "completed", "failed"];

const PER_PAGE: i64 = 50;
/// Keeps `(page - 1) * 50` far below the bigint range (the Rails
/// `Admin::BaseController::MAX_PAGE`).
const MAX_PAGE: i64 = 20_000;

/// `AdminHelper#status_badge_class`.
pub fn status_badge_class(status: &str) -> &'static str {
    match status {
        "completed" => "ok",
        "processing" => "info",
        "pending" => "warn",
        "failed" => "bad",
        _ => "",
    }
}

#[derive(sqlx::FromRow)]
struct Request {
    id: Uuid,
    department_id: i64,
    filename: String,
    tags: Value,
    status: String,
    error_message: Option<String>,
    uploaded_by: String,
    created_at: NaiveDateTime,
}

struct Line {
    id: Uuid,
    department: String,
    trashed: bool,
    filename: String,
    tags: Vec<(String, Option<String>)>,
    status: String,
    badge: &'static str,
    error: String,
    uploaded_by: String,
    date: String,
}

#[derive(Template)]
#[template(path = "admin/upload_requests/index.html")]
struct IndexPage {
    chrome: Chrome,
    paused: bool,
    pending: i64,
    processing: i64,
    statuses: Vec<(&'static str, bool)>,
    lines: Vec<Line>,
    newer: Option<String>,
    older: Option<String>,
}

/// Ruby `String#to_i` on a query value, clamped to `1..=MAX_PAGE`;
/// anything that is not a plain string is page 1.
fn page_of(raw: Option<&str>) -> i64 {
    let n = raw.map_or(0, |s| {
        let t = s.trim_start();
        let (neg, digits) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
        let n: i64 = digits
            .parse()
            .unwrap_or(if digits.is_empty() { 0 } else { i64::MAX });
        if neg { -n } else { n }
    });
    n.clamp(1, MAX_PAGE)
}

/// The current query string with `page` replaced (`url_for(query.merge(page:))`).
fn page_link(query: &str, page: i64) -> String {
    let mut pairs: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let key = p.split('=').next().unwrap_or("");
            key != "page" && !key.starts_with("page%5B") && !key.starts_with("page[")
        })
        .collect();
    let page = format!("page={page}");
    pairs.push(&page);
    format!("/admin/upload_requests?{}", pairs.join("&"))
}

fn format_time(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H:%M").to_string()
}

pub async fn index(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    params: RackParams,
    uri: axum::http::Uri,
) -> AdminResult {
    // Non-scalar values are ignored, not rejected (`scalar_param`).
    let filter = params.str("status").filter(|s| !s.trim().is_empty());
    let page = page_of(params.str("page"));

    let rows: Vec<Request> = sqlx::query_as(
        "SELECT id, department_id, filename, tags, status, error_message, uploaded_by, created_at \
         FROM upload_requests WHERE ($1::text IS NULL OR status = $1) \
         ORDER BY created_at DESC, id DESC OFFSET $2 LIMIT $3",
    )
    .bind(filter)
    .bind((page - 1) * PER_PAGE)
    .bind(PER_PAGE + 1)
    .fetch_all(&state.db)
    .await?;
    let has_older = i64::try_from(rows.len()).unwrap_or(i64::MAX) > PER_PAGE;
    let rows: Vec<Request> = rows.into_iter().take(PER_PAGE as usize).collect();

    // Trashed departments included: the request outlives the soft delete.
    let ids: Vec<i64> = rows.iter().map(|r| r.department_id).collect();
    let depts: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT id, name, deleted_at IS NOT NULL FROM departments WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    let depts: HashMap<i64, (String, bool)> = depts
        .into_iter()
        .map(|(id, name, t)| (id, (name, t)))
        .collect();

    let lines = rows
        .into_iter()
        .map(|r| {
            let (department, trashed) = depts.get(&r.department_id).cloned().unwrap_or_default();
            Line {
                tags: tag_pairs(&r.tags),
                badge: status_badge_class(&r.status),
                date: format_time(r.created_at),
                error: r.error_message.unwrap_or_default(),
                id: r.id,
                department,
                trashed,
                filename: r.filename,
                status: r.status,
                uploaded_by: r.uploaded_by,
            }
        })
        .collect();

    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM upload_requests WHERE status = 'pending'")
            .fetch_one(&state.db)
            .await?;
    let processing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM upload_requests WHERE status = 'processing'")
            .fetch_one(&state.db)
            .await?;
    let query = uri.query().unwrap_or("");
    let page = IndexPage {
        chrome: ctx.chrome("upload_requests"),
        paused: queue::is_paused(&state.db, QueueName::Uploads).await?,
        pending,
        processing,
        statuses: STATUSES.iter().map(|s| (*s, filter == Some(*s))).collect(),
        lines,
        newer: (page > 1).then(|| page_link(query, page - 1)),
        older: has_older.then(|| page_link(query, page + 1)),
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

/// `{"type": "facture", "urgent": null}` → ordered `(key, value)` pairs.
pub fn tag_pairs(tags: &Value) -> Vec<(String, Option<String>)> {
    tags.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let value = match v {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        other => Some(other.to_string()),
                    };
                    (k.clone(), value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `POST /admin/upload_queue_pause`.
pub async fn pause(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    queue::pause(&state.db, QueueName::Uploads).await?;
    ctx.flash("notice", "Upload queue paused.");
    Ok(ctx.redirect("/admin/upload_requests"))
}

/// `DELETE /admin/upload_queue_pause`.
pub async fn resume(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    queue::resume(&state.db, QueueName::Uploads).await?;
    ctx.flash("notice", "Upload queue resumed.");
    Ok(ctx.redirect("/admin/upload_requests"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_is_clamped_like_rails() {
        for (raw, want) in [
            (None, 1),
            (Some("abc"), 1),
            (Some("0"), 1),
            (Some("-3"), 1),
            (Some("2"), 2),
            (Some(" 7x"), 7),
            (Some("99999999999999999999"), 20_000),
            (Some("20001"), 20_000),
        ] {
            assert_eq!(page_of(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn page_links_keep_the_other_filters() {
        assert_eq!(page_link("", 2), "/admin/upload_requests?page=2");
        assert_eq!(
            page_link("status=failed&page=3&x=1", 4),
            "/admin/upload_requests?status=failed&x=1&page=4"
        );
        assert_eq!(
            page_link("page[a]=1&status=pending", 2),
            "/admin/upload_requests?status=pending&page=2"
        );
    }

    #[test]
    fn status_badges_match_the_helper() {
        assert_eq!(status_badge_class("completed"), "ok");
        assert_eq!(status_badge_class("processing"), "info");
        assert_eq!(status_badge_class("pending"), "warn");
        assert_eq!(status_badge_class("failed"), "bad");
        assert_eq!(status_badge_class("weird"), "");
    }

    #[test]
    fn tags_keep_valueless_keys() {
        let t = serde_json::json!({"type": "facture", "urgent": null});
        assert_eq!(
            tag_pairs(&t),
            [
                ("type".into(), Some("facture".into())),
                ("urgent".into(), None)
            ]
        );
    }
}
