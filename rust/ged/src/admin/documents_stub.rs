//! PROVISIONAL — replaced wholesale by T11 (`admin/documents.rs`).
//!
//! T10's parity suites need two things from the documents screen: a page
//! that carries the layout and its CSRF meta tag (`GET /admin/documents` is
//! where `/admin` leads and where the harness reads its token), and a bare
//! upload (`POST /admin/documents`) that the status tests use to put files
//! in the store. Nothing else of the documents screen exists here: no list,
//! filters, edit, preview, bulk actions, download or trash.

use askama::Template;
use axum::extract::{Extension, Multipart, State};
use axum::http::StatusCode;
use serde_json::{Map, Value};

use super::view::{AdminError, AdminResult, Chrome, Ctx, rails_id};
use crate::domain::document_creator::{CreateError, DocumentCreator, NewDocument};
use crate::http::AppState;
use crate::storage::Storage;

#[derive(Template)]
#[template(path = "admin/documents_stub.html")]
struct Page {
    chrome: Chrome,
}

pub async fn index(Extension(ctx): Extension<Ctx>) -> AdminResult {
    Ok(ctx.page(
        StatusCode::OK,
        &Page {
            chrome: ctx.chrome("documents"),
        },
    ))
}

/// One `key=value` pair per line; a line without `=` is a key with no value.
pub fn parse_tags_text(text: &str) -> Value {
    let mut tags = Map::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (key, value) = match line.split_once('=') {
            Some((k, v)) => (k, Value::String(v.to_owned())),
            None => (line, Value::Null),
        };
        tags.insert(key.to_owned(), value);
    }
    Value::Object(tags)
}

struct Upload {
    filename: String,
    content_type: Option<String>,
    bytes: Vec<u8>,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    mut form: Multipart,
) -> AdminResult {
    let limit = usize::try_from(state.config.max_upload_bytes).unwrap_or(usize::MAX);
    let (mut token, mut department, mut tags_text) = (None, None, String::new());
    let mut file: Option<Upload> = None;
    // The token may follow the file in the body: read everything first.
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response_());
            }
            Err(_) => return Err(AdminError::BadRequest),
        };
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "document[file]" => {
                let filename = field.file_name().map(str::to_owned);
                let content_type = field.content_type().map(str::to_owned);
                let mut bytes = Vec::new();
                let mut field = field;
                loop {
                    let chunk = match field.chunk().await {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => break,
                        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                            return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response_());
                        }
                        Err(_) => return Err(AdminError::BadRequest),
                    };
                    if bytes.len() + chunk.len() > limit {
                        return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response_());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if let Some(filename) = filename {
                    file = Some(Upload {
                        filename,
                        content_type,
                        bytes,
                    });
                }
            }
            other => {
                let text = field.text().await.map_err(|_| AdminError::BadRequest)?;
                match other {
                    "authenticity_token" => {
                        // A bad token ahead of the file: refuse before reading it.
                        if !ctx.verify_csrf(Some(&text)) {
                            return Ok(super::view::error_page(StatusCode::UNPROCESSABLE_ENTITY));
                        }
                        token = Some(text);
                    }
                    "document[department_id]" => department = Some(text),
                    "document[tags_text]" => tags_text = text,
                    _ => {}
                }
            }
        }
    }
    if !ctx.verify_csrf(token.as_deref()) {
        return Ok(super::view::error_page(StatusCode::UNPROCESSABLE_ENTITY));
    }
    let Some(upload) = file.filter(|u| !u.bytes.is_empty()) else {
        ctx.flash("alert", "File is required.");
        return Ok(ctx.redirect("/admin/documents"));
    };

    let storage = Storage::new(super::storage_root());
    let stored = storage
        .put(upload.bytes.as_slice(), state.config.max_upload_bytes)
        .await
        .map_err(|e| AdminError::Internal(format!("storing an upload: {e}")))?;
    let key = stored.key.clone();
    let username = ctx.admin().map(|a| a.username).unwrap_or_default();
    let mut tx = state.db.begin().await?;
    let created = DocumentCreator::create(
        &mut tx,
        NewDocument {
            department_id: department.as_deref().and_then(rails_id),
            filename: upload.filename,
            content_type: upload.content_type,
            file: stored,
            uploaded_by: format!("admin:{username}"),
            tags: parse_tags_text(&tags_text),
            uploaded_at: None,
            stored_async: false,
            max_byte_size: state.config.max_upload_bytes,
        },
    )
    .await;
    match created {
        Ok(_) => {
            tx.commit().await?;
            ctx.flash("notice", "Document uploaded.");
        }
        Err(CreateError::Invalid(errors)) => {
            let _ = tx.rollback().await;
            let _ = storage.delete(&key).await;
            ctx.flash("alert", errors.join(", "));
        }
        Err(CreateError::Db(e)) => {
            let _ = tx.rollback().await;
            let _ = storage.delete(&key).await;
            return Err(e.into());
        }
    }
    Ok(ctx.redirect("/admin/documents"))
}

trait IntoResponseExt {
    fn into_response_(self) -> axum::response::Response;
}

impl IntoResponseExt for StatusCode {
    fn into_response_(self) -> axum::response::Response {
        axum::response::IntoResponse::into_response(self)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_tags_text;
    use serde_json::json;

    #[test]
    fn tags_text_is_one_pair_per_line() {
        assert_eq!(
            parse_tags_text("type=facture\n\n  urgent \nnote=a=b\n"),
            json!({"type": "facture", "urgent": null, "note": "a=b"})
        );
        assert_eq!(parse_tags_text(""), json!({}));
    }
}
