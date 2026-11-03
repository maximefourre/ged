//! `Admin::DeletedDocumentsController`: the trash index and restore.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use chrono::{NaiveDateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::documents::{PER_PAGE, format_time, page_href, page_of};
use super::upload_requests::tag_pairs;
use super::view::{AdminError, AdminResult, Chrome, Ctx};
use crate::api::support::uuid_id;
use crate::domain::department::Department;
use crate::domain::document::Document;
use crate::http::AppState;
use crate::http::params::RackParams;

const TRASH: &str = "/admin/deleted_documents";

#[derive(Template)]
#[template(path = "admin/deleted_documents/index.html")]
struct IndexPage {
    chrome: Chrome,
    rows: Vec<Row>,
    newer: Option<String>,
    older: Option<String>,
}

impl IndexPage {
    fn has_pager(&self) -> bool {
        self.newer.is_some() || self.older.is_some()
    }
}

struct Row {
    filename: String,
    department: String,
    trashed: bool,
    tags: Vec<(String, Option<String>)>,
    deleted: String,
    purge: String,
    restore: String,
}

#[derive(sqlx::FromRow)]
struct Trashed {
    id: Uuid,
    filename: String,
    tags: Value,
    deleted_at: Option<NaiveDateTime>,
    purge_at: Option<NaiveDateTime>,
    department_id: i64,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    params: RackParams,
    uri: axum::http::Uri,
) -> AdminResult {
    let page = page_of(params.str("page"));
    let docs: Vec<Trashed> = sqlx::query_as(
        "SELECT id, filename, tags, deleted_at, purge_at, department_id FROM documents \
         WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC OFFSET $1 LIMIT $2",
    )
    .bind((page - 1) * PER_PAGE)
    .bind(PER_PAGE + 1)
    .fetch_all(&state.db)
    .await?;
    let has_older = i64::try_from(docs.len()).unwrap_or(i64::MAX) > PER_PAGE;
    let docs: Vec<Trashed> = docs.into_iter().take(PER_PAGE as usize).collect();

    let ids: Vec<i64> = docs.iter().map(|d| d.department_id).collect();
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

    let rows = docs
        .into_iter()
        .map(|doc| {
            let (department, trashed) = depts.get(&doc.department_id).cloned().unwrap_or_default();
            Row {
                restore: format!("{TRASH}/{}/restore", doc.id),
                tags: tag_pairs(&doc.tags),
                deleted: doc.deleted_at.map(format_time).unwrap_or_default(),
                purge: doc
                    .purge_at
                    .map(|t| t.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
                filename: doc.filename,
                department,
                trashed,
            }
        })
        .collect();

    let query = uri.query().unwrap_or("");
    let page = IndexPage {
        chrome: ctx.chrome("deleted_documents"),
        rows,
        newer: (page > 1).then(|| page_href(TRASH, query, page - 1)),
        older: has_older.then(|| page_href(TRASH, query, page + 1)),
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

/// Restores the document, then un-hides its department only. Siblings stay
/// trashed. The two writes are sequential, as in Rails (not one transaction).
pub async fn restore(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let id = uuid_id(&id).ok_or(AdminError::NotFound)?;
    let mut conn = state.db.acquire().await?;
    let document = Document::restore(&mut conn, id, Utc::now().naive_utc())
        .await?
        .ok_or(AdminError::NotFound)?;
    drop(conn);
    let department = Department::find_with_deleted(&state.db, document.department_id)
        .await?
        .ok_or(AdminError::NotFound)?;
    if department.deleted_at.is_some() {
        department.clear_soft_delete(&state.db).await?;
    }
    ctx.flash("notice", "Document restored.");
    Ok(ctx.redirect(TRASH))
}
