//! `Admin::DocumentsController`: the list, the upload, tag editing, preview,
//! download, and the bulk actions. The trash lives in [`super::trash`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use askama::Template;
use axum::body::Body;
use axum::extract::multipart::Field;
use axum::extract::{Extension, FromRequest, Multipart, Path, Request, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{NaiveDateTime, Utc};
use futures_util::StreamExt;
use serde_json::{Map, Value};
use sqlx::{Postgres, QueryBuilder};
use tokio_util::io::{ReaderStream, StreamReader};
use uuid::Uuid;

use super::departments::{self, TreeOption};
use super::upload_requests::tag_pairs;
use super::view::{AdminError, AdminResult, Chrome, Ctx, rails_id};
use super::zip_stream::{ZipFile, stream};
use crate::api::support::{id_param_text, integer_id, uuid_id};
use crate::domain::document::Document;
use crate::domain::document_creator::{CreateError, DocumentCreator, NewDocument};
use crate::domain::tags::{self, ruby_downcase, ruby_strip};
use crate::http::AppState;
use crate::http::params::{ParamValue, RackParams, RequestParams};
use crate::storage::disposition::{content_disposition, zip_entry_name};
use crate::storage::{StorageError, Stored};

/// `Admin::BaseController::PER_PAGE`.
pub(super) const PER_PAGE: i64 = 50;
/// `Admin::BaseController::MAX_PAGE`.
const MAX_PAGE: i64 = 20_000;

const DOCUMENTS: &str = "/admin/documents";
const NEW_DOCUMENT: &str = "/admin/documents/new";

/// Ruby `String#to_i` on a query value, clamped to `1..=MAX_PAGE`.
/// Anything that is not a plain string is page 1.
pub(super) fn page_of(raw: Option<&str>) -> i64 {
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
pub(super) fn page_href(path: &str, query: &str, page: i64) -> String {
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
    format!("{path}?{}", pairs.join("&"))
}

pub(super) fn format_time(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H:%M").to_string()
}

/// One `key=value` pair per line; a line without `=` is a key with no value.
/// Stripping matches Ruby `String#strip` (ASCII whitespace and NUL).
pub fn parse_tags_text(text: &str) -> Value {
    let mut tags = Map::new();
    for line in text.lines() {
        let line = ruby_strip(line);
        if line.is_empty() {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some((k, v)) => (k, Value::String(v.to_owned())),
            None => (line, Value::Null),
        };
        tags.insert(key.to_owned(), value);
    }
    Value::Object(tags)
}

fn tags_textarea(tags: &Value) -> String {
    let Some(map) = tags.as_object() else {
        return String::new();
    };
    map.iter()
        .map(|(key, value)| match value {
            Value::Null => key.clone(),
            Value::String(text) => format!("{key}={text}"),
            other => format!("{key}={other}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn like_pattern(filename: &str) -> String {
    let escaped = filename
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

async fn department_choices(
    db: &sqlx::PgPool,
    selected: Option<i64>,
) -> Result<Vec<TreeOption>, sqlx::Error> {
    Ok(departments::tree_options(
        &departments::tree(departments::live_departments(db).await?),
        selected,
    ))
}

// --- index -------------------------------------------------------------------

#[derive(Template)]
#[template(path = "admin/documents/index.html")]
struct IndexPage {
    chrome: Chrome,
    filename: String,
    tag: String,
    departments: Vec<TreeOption>,
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
    id: Uuid,
    filename: String,
    content_type: String,
    department: String,
    uploaded_by: String,
    date: String,
    tags: Vec<(String, Option<String>)>,
    confirm: String,
}

#[derive(sqlx::FromRow)]
struct Listed {
    id: Uuid,
    filename: String,
    content_type: Option<String>,
    uploaded_by: String,
    created_at: NaiveDateTime,
    tags: Value,
    department_name: String,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    params: RackParams,
    uri: axum::http::Uri,
) -> AdminResult {
    let filename = params.str("filename").filter(|s| !departments::blank(s));
    let tag = params.str("tag").filter(|s| !departments::blank(s));
    let department = params
        .str("department_id")
        .filter(|s| !departments::blank(s))
        .map(integer_id);
    let page = page_of(params.str("page"));

    let mut sql = QueryBuilder::<Postgres>::new(
        "SELECT d.id, d.filename, d.content_type, d.uploaded_by, d.created_at, d.tags, \
         dept.name AS department_name \
         FROM documents d \
         JOIN departments dept ON dept.id = d.department_id AND dept.deleted_at IS NULL \
         WHERE d.deleted_at IS NULL",
    );
    if let Some(name) = filename {
        sql.push(" AND d.filename ILIKE ");
        sql.push_bind(like_pattern(name));
    }
    if let Some(id) = department {
        sql.push(" AND d.department_id = ");
        sql.push_bind(id);
    }
    if let Some(key) = tag {
        sql.push(" AND jsonb_exists(d.tags, ");
        sql.push_bind(ruby_downcase(ruby_strip(key)));
        sql.push(")");
    }
    sql.push(" ORDER BY d.created_at DESC, d.id DESC OFFSET ");
    sql.push_bind((page - 1) * PER_PAGE);
    sql.push(" LIMIT ");
    sql.push_bind(PER_PAGE + 1);

    let listed: Vec<Listed> = sql.build_query_as().fetch_all(&state.db).await?;
    let has_older = i64::try_from(listed.len()).unwrap_or(i64::MAX) > PER_PAGE;
    let rows = listed
        .into_iter()
        .take(PER_PAGE as usize)
        .map(|doc| Row {
            confirm: format!("Delete {}?", doc.filename),
            tags: tag_pairs(&doc.tags),
            date: format_time(doc.created_at),
            content_type: doc.content_type.unwrap_or_default(),
            department: doc.department_name,
            id: doc.id,
            filename: doc.filename,
            uploaded_by: doc.uploaded_by,
        })
        .collect();

    let query = uri.query().unwrap_or("");
    let page = IndexPage {
        chrome: ctx.chrome("documents"),
        filename: params.str("filename").unwrap_or("").to_owned(),
        tag: params.str("tag").unwrap_or("").to_owned(),
        departments: department_choices(&state.db, params.str("department_id").map(integer_id))
            .await?,
        rows,
        newer: (page > 1).then(|| page_href(DOCUMENTS, query, page - 1)),
        older: has_older.then(|| page_href(DOCUMENTS, query, page + 1)),
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

// --- upload ------------------------------------------------------------------

#[derive(Template)]
#[template(path = "admin/documents/new.html")]
struct NewPage {
    chrome: Chrome,
    departments: Vec<TreeOption>,
}

pub async fn new(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    let page = NewPage {
        chrome: ctx.chrome("documents"),
        departments: department_choices(&state.db, None).await?,
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

struct Incoming {
    filename: String,
    content_type: Option<String>,
    file: HeldFile,
}

/// A stored upload that is deleted unless [`HeldFile::keep`] is called after
/// the document row commits. Drop is synchronous, so cancelling the request
/// (the client hung up) cannot leave the bytes behind.
struct HeldFile {
    storage: crate::storage::Storage,
    stored: Stored,
    keep: bool,
}

impl HeldFile {
    fn new(storage: crate::storage::Storage, stored: Stored) -> Self {
        Self {
            storage,
            stored,
            keep: false,
        }
    }

    fn snapshot(&self) -> Stored {
        Stored {
            key: self.stored.key.clone(),
            written: self.stored.written.clone(),
        }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for HeldFile {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let Ok(path) = self.storage.path_for(&self.stored.key) else {
            return;
        };
        let _ = std::fs::remove_file(&path);
        let mut part = path.into_os_string();
        part.push(".part");
        let _ = std::fs::remove_file(std::path::Path::new(&part));
    }
}

enum StoreFail {
    TooLarge,
    BadRequest,
    Internal(String),
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    req: Request,
) -> AdminResult {
    if is_multipart(req.headers()) {
        create_multipart(state, ctx, req).await
    } else {
        create_urlencoded(state, ctx, req).await
    }
}

fn is_multipart(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("multipart/form-data")
        })
}

async fn create_multipart(state: AppState, ctx: Ctx, req: Request) -> AdminResult {
    let mut form = match Multipart::from_request(req, &state).await {
        Ok(form) => form,
        Err(rejection) => {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
            }
            return Err(AdminError::BadRequest);
        }
    };

    let mut token = None;
    let mut department = None;
    let mut tags_text = String::new();
    let mut file: Option<Incoming> = None;
    let mut file_problem: Option<&'static str> = None;

    loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                file.take();
                return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
            }
            Err(_) => {
                file.take();
                return Err(AdminError::BadRequest);
            }
        };
        let name = field.name().unwrap_or("").to_owned();
        if name == "authenticity_token" {
            let text = match field.text().await {
                Ok(text) => text,
                Err(_) => {
                    file.take();
                    return Err(AdminError::BadRequest);
                }
            };
            // A bad token ahead of the file: refuse before reading the file.
            if !ctx.verify_csrf(Some(&text)) {
                file.take();
                return Ok(super::view::error_page(StatusCode::UNPROCESSABLE_ENTITY));
            }
            token = Some(text);
            continue;
        }
        if name == "document[file]" {
            match take_file(&state, field).await {
                Ok(Take::File(incoming)) => {
                    // Dropping the previous part deletes its bytes.
                    file.take();
                    file = Some(incoming);
                    file_problem = None;
                }
                Ok(Take::Problem(message)) => {
                    file.take();
                    file = None;
                    file_problem = Some(message);
                }
                Err(StoreFail::TooLarge) => {
                    file.take();
                    return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
                }
                Err(StoreFail::BadRequest) => {
                    file.take();
                    return Err(AdminError::BadRequest);
                }
                Err(StoreFail::Internal(cause)) => {
                    file.take();
                    return Err(AdminError::Internal(cause));
                }
            }
            continue;
        }
        let text = match field.text().await {
            Ok(text) => text,
            Err(_) => {
                file.take();
                return Err(AdminError::BadRequest);
            }
        };
        match name.as_str() {
            "document[department_id]" => department = Some(text),
            "document[tags_text]" => tags_text = text,
            _ => {}
        }
    }

    if !ctx.verify_csrf(token.as_deref()) {
        file.take();
        return Ok(super::view::error_page(StatusCode::UNPROCESSABLE_ENTITY));
    }
    if let Some(message) = file_problem {
        ctx.flash("alert", message);
        return Ok(ctx.redirect(NEW_DOCUMENT));
    }
    let Some(upload) = file else {
        ctx.flash("alert", "File is required.");
        return Ok(ctx.redirect(NEW_DOCUMENT));
    };
    finish_upload(&state, &ctx, upload, department.as_deref(), &tags_text).await
}

enum Take {
    File(Incoming),
    Problem(&'static str),
}

/// A part with a filename is an upload, empty bytes included. A part without
/// one is a plain field: blank means the file is missing, anything else is
/// not a file.
async fn take_file(state: &AppState, field: Field<'_>) -> Result<Take, StoreFail> {
    let filename = field.file_name().map(str::to_owned);
    let content_type = field.content_type().map(str::to_owned);
    let Some(filename) = filename else {
        let text = field.text().await.map_err(|_| StoreFail::BadRequest)?;
        let message = if departments::blank(&text) {
            "File is required."
        } else {
            "File must be a file upload."
        };
        return Ok(Take::Problem(message));
    };
    let file = store_field(state, field).await?;
    Ok(Take::File(Incoming {
        filename,
        content_type,
        file,
    }))
}

/// Streams the part into the store, one chunk at a time, in this task. The
/// bytes are never accumulated in a `Vec`. A failure or a cancelled request
/// removes the file before it is committed (`PartGuard` inside `put`, then
/// [`HeldFile`] until the row commits).
async fn store_field(state: &AppState, field: Field<'_>) -> Result<HeldFile, StoreFail> {
    let limit = state.config.max_upload_bytes;
    let too_large = AtomicBool::new(false);
    let bad_request = AtomicBool::new(false);
    let stream = field.map(|result| match result {
        Ok(chunk) => Ok(chunk),
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            too_large.store(true, Ordering::Relaxed);
            Err(std::io::Error::other("payload too large"))
        }
        Err(_) => {
            bad_request.store(true, Ordering::Relaxed);
            Err(std::io::Error::other("multipart"))
        }
    });
    let reader = StreamReader::new(stream);
    match state.storage.put(reader, limit).await {
        Ok(stored) => Ok(HeldFile::new(state.storage.clone(), stored)),
        Err(StorageError::TooLarge { .. }) => Err(StoreFail::TooLarge),
        Err(_) if too_large.load(Ordering::Relaxed) => Err(StoreFail::TooLarge),
        Err(_) if bad_request.load(Ordering::Relaxed) => Err(StoreFail::BadRequest),
        Err(e) => Err(StoreFail::Internal(e.to_string())),
    }
}

async fn finish_upload(
    state: &AppState,
    ctx: &Ctx,
    mut upload: Incoming,
    department: Option<&str>,
    tags_text: &str,
) -> AdminResult {
    let username = ctx.admin().map(|a| a.username).unwrap_or_default();
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => return Err(e.into()),
    };
    let created = DocumentCreator::create(
        &mut tx,
        NewDocument {
            department_id: department
                .filter(|s| !departments::blank(s))
                .and_then(rails_id),
            filename: upload.filename,
            content_type: upload.content_type,
            file: upload.file.snapshot(),
            uploaded_by: format!("admin:{username}"),
            tags: parse_tags_text(tags_text),
            uploaded_at: None,
            stored_async: false,
            max_byte_size: state.config.max_upload_bytes,
        },
    )
    .await;
    match created {
        Ok(_) => match tx.commit().await {
            Ok(()) => {
                upload.file.keep();
                ctx.flash("notice", "Document uploaded.");
                Ok(ctx.redirect(DOCUMENTS))
            }
            Err(e) => Err(e.into()),
        },
        Err(CreateError::Invalid(errors)) => {
            let _ = tx.rollback().await;
            ctx.flash("alert", errors.join(", "));
            Ok(ctx.redirect(NEW_DOCUMENT))
        }
        Err(CreateError::Db(e)) => {
            let _ = tx.rollback().await;
            Err(e.into())
        }
    }
}

async fn create_urlencoded(state: AppState, ctx: Ctx, req: Request) -> AdminResult {
    let RequestParams(params) = RequestParams::from_request(req, &state)
        .await
        .map_err(|_| AdminError::BadRequest)?;
    let fields = document_fields(&params)?;
    let file = fields.and_then(|m| m.get("file"));
    let message = match file {
        None | Some(ParamValue::Nil) => "File is required.",
        Some(ParamValue::Str(text)) if departments::blank(text) => "File is required.",
        Some(_) => "File must be a file upload.",
    };
    ctx.flash("alert", message);
    Ok(ctx.redirect(NEW_DOCUMENT))
}

/// `params.require(:document)`: missing is 400, a scalar is the Rails 500.
fn document_fields(
    params: &RackParams,
) -> Result<Option<&crate::http::params::ParamMap>, AdminError> {
    match params.get("document") {
        None | Some(ParamValue::Nil) => Err(AdminError::BadRequest),
        Some(ParamValue::Str(text)) if !departments::blank(text) => Err(AdminError::Internal(
            "params[document] is a string, not a hash".into(),
        )),
        Some(ParamValue::Map(map)) if map.is_empty() => Ok(None),
        Some(ParamValue::Map(map)) => Ok(Some(map)),
        Some(_) => Err(AdminError::BadRequest),
    }
}

// --- tags --------------------------------------------------------------------

#[derive(Template)]
#[template(path = "admin/documents/edit.html")]
struct EditPage {
    chrome: Chrome,
    id: Uuid,
    filename: String,
    tags_text: String,
    uploaded: String,
    indexed: String,
    updated: String,
    upload_mode: String,
    uploaded_by: String,
}

pub async fn edit(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let document = find_live(&state, &id).await?;
    let page = EditPage {
        chrome: ctx.chrome("documents"),
        tags_text: tags_textarea(&document.tags),
        uploaded: format_time(document.uploaded_at),
        indexed: format_time(document.created_at),
        updated: format_time(document.updated_at),
        upload_mode: document.upload_mode().to_owned(),
        uploaded_by: document.uploaded_by,
        filename: document.filename,
        id: document.id,
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let id = uuid_id(&id).ok_or(AdminError::NotFound)?;
    let fields = document_fields(&params)?.ok_or(AdminError::BadRequest)?;
    let text = fields
        .get("tags_text")
        .and_then(ParamValue::as_str)
        .unwrap_or("");
    let tags = tags::normalize(parse_tags_text(text));
    let errors = tags::full_messages(&tags::validate(&tags));
    if !errors.is_empty() {
        ctx.flash("alert", errors.join(", "));
        return Ok(ctx.redirect(&format!("{DOCUMENTS}/{id}/edit")));
    }
    let now = Utc::now().naive_utc();
    let updated = sqlx::query(
        "UPDATE documents SET tags = $1, updated_at = $2 WHERE id = $3 AND deleted_at IS NULL",
    )
    .bind(&tags)
    .bind(now)
    .bind(id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AdminError::NotFound);
    }
    ctx.flash("notice", "Tags updated.");
    Ok(ctx.redirect(DOCUMENTS))
}

pub async fn destroy(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let id = uuid_id(&id).ok_or(AdminError::NotFound)?;
    let mut conn = state.db.acquire().await?;
    Document::soft_delete(&mut conn, id, Utc::now().naive_utc())
        .await?
        .ok_or(AdminError::NotFound)?;
    ctx.flash("notice", "Document moved to trash.");
    Ok(ctx.redirect(DOCUMENTS))
}

// --- download and preview ----------------------------------------------------

pub async fn download(State(state): State<AppState>, Path(id): Path<String>) -> AdminResult {
    let document = find_live(&state, &id).await?;
    send_document(&state, &document, false).await
}

pub async fn preview(State(state): State<AppState>, Path(id): Path<String>) -> AdminResult {
    let document = find_live(&state, &id).await?;
    let mut response = send_document(&state, &document, true).await?;
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(preview_csp(document.content_type.as_deref())),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

/// PDF exactly: `frame-ancestors 'self'` and no `sandbox`, so Chrome's viewer
/// can render it. Every other type (including a missing one) is a scriptless
/// sandbox. The comparison is the stored type, not a sniffed one.
fn preview_csp(content_type: Option<&str>) -> &'static str {
    if content_type == Some("application/pdf") {
        "frame-ancestors 'self'"
    } else {
        "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox"
    }
}

async fn send_document(state: &AppState, document: &Document, inline: bool) -> AdminResult {
    let key: String = sqlx::query_scalar("SELECT key FROM stored_files WHERE document_id = $1")
        .bind(document.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| {
            AdminError::Internal(format!("document {} has no stored file", document.id))
        })?;
    let file = state
        .storage
        .open(&key)
        .await
        .map_err(|e| AdminError::Internal(format!("opening {key}: {e}")))?;
    let length = file
        .metadata()
        .await
        .map_err(|e| AdminError::Internal(format!("stat {key}: {e}")))?
        .len();
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, media_type(document.content_type.as_deref()));
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(inline, &document.filename))
            .map_err(|e| AdminError::Internal(format!("content-disposition: {e}")))?,
    );
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(
        axum::http::header::HeaderName::from_static("content-transfer-encoding"),
        HeaderValue::from_static("binary"),
    );
    Ok(response)
}

fn media_type(declared: Option<&str>) -> HeaderValue {
    declared
        .and_then(|value| HeaderValue::from_str(value).ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"))
}

async fn find_live(state: &AppState, id: &str) -> Result<Document, AdminError> {
    let id = uuid_id(id).ok_or(AdminError::NotFound)?;
    let mut conn = state.db.acquire().await?;
    Document::find(&mut conn, id)
        .await?
        .ok_or(AdminError::NotFound)
}

// --- bulk --------------------------------------------------------------------

pub async fn bulk_destroy(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let selection = document_ids(&params);
    if !selection.submitted {
        ctx.flash("alert", "No documents selected.");
        return Ok(ctx.redirect(DOCUMENTS));
    }
    let mut conn = state.db.acquire().await?;
    let n = Document::bulk_soft_delete(&mut conn, &selection.ids, Utc::now().naive_utc()).await?;
    ctx.flash("notice", format!("{n} documents moved to trash."));
    Ok(ctx.redirect(DOCUMENTS))
}

pub async fn bulk_download(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    params: RackParams,
) -> AdminResult {
    let selection = document_ids(&params);
    if !selection.submitted {
        ctx.flash("alert", "No documents selected.");
        return Ok(ctx.redirect(DOCUMENTS));
    }
    let ids = selection.ids;
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT d.id, d.filename, s.key FROM documents d \
         LEFT JOIN stored_files s ON s.document_id = d.id \
         WHERE d.deleted_at IS NULL AND d.id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    let by_id: HashMap<Uuid, (String, Option<String>)> = rows
        .into_iter()
        .map(|(id, filename, key)| (id, (filename, key)))
        .collect();

    let mut files = Vec::new();
    for id in &ids {
        let Some((filename, key)) = by_id.get(id) else {
            continue;
        };
        let Some(key) = key else {
            eprintln!("ged: admin: zip skip {id}: no stored file");
            continue;
        };
        match state.storage.path_for(key) {
            Ok(path) => files.push(ZipFile {
                name: zip_entry_name(&id.to_string(), filename),
                path,
            }),
            Err(e) => eprintln!("ged: admin: zip skip {id}: {e}"),
        }
    }

    let filename = format!("documents-{}.zip", Utc::now().format("%Y%m%d"));
    let mut response = Response::new(stream(files));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(false, &filename))
            .map_err(|e| AdminError::Internal(format!("content-disposition: {e}")))?,
    );
    Ok(response)
}

/// Non-blank `document_ids` values, and the UUIDs among them.
///
/// Rails keeps every non-blank string (`reject(&:blank?)`) and casts it.
/// An invalid UUID becomes nil and matches nothing; it is still a selection,
/// so the action runs (`0 documents moved to trash.`, or an empty ZIP) instead
/// of the empty-selection alert. Order is kept, duplicates dropped.
struct IdSelection {
    submitted: bool,
    ids: Vec<Uuid>,
}

fn document_ids(params: &RackParams) -> IdSelection {
    let values: Vec<ParamValue> = match params.get("document_ids") {
        Some(ParamValue::Array(items)) => items.clone(),
        Some(other) => vec![other.clone()],
        None => {
            return IdSelection {
                submitted: false,
                ids: Vec::new(),
            };
        }
    };
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut submitted = false;
    for value in values {
        let text = id_param_text(&value);
        if departments::blank(&text) {
            continue;
        }
        submitted = true;
        if let Some(id) = uuid_id(&text)
            && seen.insert(id)
        {
            ids.push(id);
        }
    }
    IdSelection { submitted, ids }
}

#[cfg(test)]
mod tests {
    use super::{document_ids, page_href, page_of, parse_tags_text, preview_csp};
    use crate::http::params::RackParams;
    use serde_json::json;

    #[test]
    fn a_non_uuid_is_still_a_selection() {
        let params = RackParams::parse("document_ids[]=not-a-uuid").unwrap();
        let selection = document_ids(&params);
        assert!(selection.submitted);
        assert!(selection.ids.is_empty());
    }

    #[test]
    fn blanks_are_not_a_selection() {
        let params = RackParams::parse("document_ids[]=&document_ids[]=").unwrap();
        let selection = document_ids(&params);
        assert!(!selection.submitted);
        assert!(selection.ids.is_empty());
        assert!(!document_ids(&RackParams::parse("").unwrap()).submitted);
    }

    #[test]
    fn a_real_uuid_is_kept_beside_a_non_uuid() {
        let params = RackParams::parse(
            "document_ids[]=not-a-uuid&document_ids[]=11111111-1111-4111-8111-111111111111&document_ids[]=11111111-1111-4111-8111-111111111111",
        )
        .unwrap();
        let selection = document_ids(&params);
        assert!(selection.submitted);
        assert_eq!(selection.ids.len(), 1);
    }

    #[test]
    fn tags_text_is_one_pair_per_line() {
        assert_eq!(
            parse_tags_text("type=facture\n\n  urgent \nnote=a=b\n"),
            json!({"type": "facture", "urgent": null, "note": "a=b"})
        );
        assert_eq!(parse_tags_text(""), json!({}));
    }

    #[test]
    fn page_falls_back_and_clamps() {
        assert_eq!(page_of(None), 1);
        assert_eq!(page_of(Some("abc")), 1);
        assert_eq!(page_of(Some("-4")), 1);
        assert_eq!(page_of(Some("3")), 3);
        assert_eq!(page_of(Some("200000000000000000")), 20_000);
    }

    #[test]
    fn page_links_keep_the_other_filters() {
        assert_eq!(
            page_href("/admin/documents", "department_id=4&filename=a", 2),
            "/admin/documents?department_id=4&filename=a&page=2"
        );
        assert_eq!(
            page_href("/admin/documents", "page%5B%5D=2&department_id=4", 1),
            "/admin/documents?department_id=4&page=1"
        );
    }

    #[test]
    fn pdf_preview_csp_has_no_sandbox() {
        assert_eq!(
            preview_csp(Some("application/pdf")),
            "frame-ancestors 'self'"
        );
        let text = preview_csp(Some("text/plain"));
        assert!(text.contains("sandbox"));
        assert!(!text.contains("allow-scripts"));
        assert!(preview_csp(Some("application/pdf; charset=binary")).contains("sandbox"));
        assert!(preview_csp(None).contains("sandbox"));
    }
}
