//! `multipart/form-data` intake for `POST /api/v1/documents`.
//!
//! Rack parses the whole multipart body before the controller runs (a broken
//! one is a 400 even without a token), spooling file parts to temp files.
//! This does the same: text parts become ordinary parameters, and the `file`
//! part is streamed straight into the [`Storage`] under a fresh key — never
//! buffered in memory — so the caller either keeps it (sync upload), moves it
//! to staging (async upload) or deletes it. Whatever goes wrong here leaves no
//! file behind.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use futures_util::TryStreamExt;
use tokio_util::io::StreamReader;

use crate::http::params::{FORM_BYTES_LIMIT, RackParams};
use crate::storage::{Storage, StorageError, Stored};

/// Rack's `multipart_part_limit`.
pub const PART_LIMIT: usize = 128;

pub struct UploadedFile {
    pub stored: Stored,
    /// The name the client gave the part.
    pub original_filename: String,
    /// The part's `Content-Type` header as sent; `None` when absent.
    pub content_type: Option<String>,
}

pub struct Intake {
    pub params: RackParams,
    pub file: Option<UploadedFile>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum IntakeError {
    /// More than `GED_MAX_UPLOAD_BYTES` in the body: 413 with an empty body.
    TooLarge,
    /// Unparsable multipart or parameters: the generic 400.
    Malformed,
}

/// The `boundary` of a `multipart/*` content type (Rack's
/// `%r|\Amultipart/.*boundary=\"?([^\";,]+)\"?|ni`). `None` means the body is
/// not multipart as far as Rack is concerned.
pub fn boundary(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    let lower = value.to_ascii_lowercase();
    if !lower.starts_with("multipart/") {
        return None;
    }
    // `.*boundary=` is greedy: the last occurrence wins
    let at = lower.rfind("boundary=")? + "boundary=".len();
    let rest = value[at..].strip_prefix('"').unwrap_or(&value[at..]);
    let end = rest.find(['"', ';', ',']).unwrap_or(rest.len());
    let boundary = &rest[..end];
    (!boundary.is_empty()).then(|| boundary.to_owned())
}

/// Reads the whole multipart body. `max_body` is `GED_MAX_UPLOAD_BYTES`, the
/// limit on the *request body* (the proxy's `MAX_REQUEST_BODY`).
pub async fn read(
    boundary: String,
    body: Body,
    storage: &Storage,
    max_body: u64,
) -> Result<Intake, IntakeError> {
    let seen = Arc::new(AtomicU64::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let counted = {
        let seen = seen.clone();
        let exceeded = exceeded.clone();
        body.into_data_stream()
            .map_err(std::io::Error::other)
            .and_then(move |chunk| {
                let total =
                    seen.fetch_add(chunk.len() as u64, Ordering::SeqCst) + chunk.len() as u64;
                let over = total > max_body;
                if over {
                    exceeded.store(true, Ordering::SeqCst);
                }
                std::future::ready(if over {
                    Err(std::io::Error::other("body over the upload limit"))
                } else {
                    Ok(chunk)
                })
            })
    };
    let mut multipart = multer::Multipart::new(counted, boundary);

    let mut fields = String::new();
    let mut file: Option<UploadedFile> = None;
    let mut parts = 0usize;
    let mut text_bytes = 0usize;
    let result = loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break Ok(()),
            Err(_) => break Err(()),
        };
        parts += 1;
        if parts > PART_LIMIT {
            break Err(());
        }
        let Some(name) = field.name().map(str::to_owned) else {
            continue;
        };
        match field.file_name().map(str::to_owned) {
            // A file input left empty: Rack drops the part.
            Some(filename) if filename.is_empty() => {}
            Some(filename) if name == "file" => {
                let content_type = field
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let reader = StreamReader::new(field.map_err(std::io::Error::other));
                match storage.put(reader, max_body).await {
                    Ok(stored) => {
                        // a repeated `file` part: the last one wins
                        if let Some(previous) = file.replace(UploadedFile {
                            stored,
                            original_filename: filename,
                            content_type,
                        }) {
                            let _ = storage.delete(&previous.stored.key).await;
                        }
                    }
                    Err(StorageError::TooLarge { .. }) => {
                        exceeded.store(true, Ordering::SeqCst);
                        break Err(());
                    }
                    Err(_) => break Err(()),
                }
            }
            // A file under another name is not something the API reads.
            Some(_) => {}
            None => {
                let Ok(bytes) = field.bytes().await else {
                    break Err(());
                };
                text_bytes += bytes.len();
                if text_bytes > FORM_BYTES_LIMIT {
                    break Err(());
                }
                let Ok(value) = String::from_utf8(bytes.to_vec()) else {
                    break Err(());
                };
                if !fields.is_empty() {
                    fields.push('&');
                }
                fields.push_str(&urlencode(&name));
                fields.push('=');
                fields.push_str(&urlencode(&value));
            }
        }
    };

    let outcome = match result {
        Ok(()) => RackParams::parse(&fields)
            .map(|params| Intake { params, file: None })
            .map_err(|_| IntakeError::Malformed),
        Err(()) if exceeded.load(Ordering::SeqCst) => Err(IntakeError::TooLarge),
        Err(()) => Err(IntakeError::Malformed),
    };
    match outcome {
        Ok(mut intake) => {
            intake.file = file;
            Ok(intake)
        }
        Err(e) => {
            if let Some(file) = file {
                let _ = storage.delete(&file.stored.key).await;
            }
            Err(e)
        }
    }
}

/// Percent-encodes everything but unreserved characters, so that
/// [`RackParams::parse`] gets back exactly the bytes the part carried.
fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn the_boundary_is_read_like_rack() {
        assert_eq!(
            boundary(&headers("multipart/form-data; boundary=XX")).as_deref(),
            Some("XX")
        );
        assert_eq!(
            boundary(&headers("multipart/form-data; boundary=\"a b\"; x=1")).as_deref(),
            Some("a b")
        );
        assert_eq!(
            boundary(&headers("Multipart/Mixed;Boundary=zz,q")).as_deref(),
            Some("zz")
        );
        assert_eq!(boundary(&headers("multipart/form-data")), None);
        assert_eq!(boundary(&headers("multipart/form-data; boundary=")), None);
        assert_eq!(boundary(&headers("application/json; boundary=XX")), None);
        assert_eq!(boundary(&HeaderMap::new()), None);
    }

    fn body(boundary: &str, parts: &[(&str, Option<&str>, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, filename, data) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match filename {
                Some(f) => out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes(),
                ),
                None => out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            out.extend_from_slice(data.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    fn storage() -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("files"));
        (dir, storage)
    }

    fn files_under(root: &std::path::Path) -> usize {
        let mut count = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path)
                } else {
                    count += 1
                }
            }
        }
        count
    }

    #[tokio::test]
    async fn text_parts_become_parameters_and_the_file_is_stored() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("department_id", None, "7"),
                ("tags[type]", None, "facture"),
                ("file", Some("a.pdf"), "PDF!"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.params.str("department_id"), Some("7"));
        assert!(intake.params.get("tags").is_some());
        let file = intake.file.unwrap();
        assert_eq!(file.original_filename, "a.pdf");
        assert_eq!(file.content_type.as_deref(), Some("application/pdf"));
        assert_eq!(file.stored.written.byte_size, 4);
        assert_eq!(files_under(storage.root()), 1);
    }

    #[tokio::test]
    async fn an_empty_file_input_is_dropped_and_other_file_names_ignored() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[("file", Some(""), ""), ("other", Some("x.txt"), "zzz")],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert!(intake.file.is_none());
        assert_eq!(files_under(storage.root()), 0);
    }

    #[tokio::test]
    async fn a_repeated_file_part_keeps_the_last_and_leaves_one_file() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("file", Some("a.pdf"), "one"),
                ("file", Some("b.pdf"), "second"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.file.unwrap().original_filename, "b.pdf");
        assert_eq!(files_under(storage.root()), 1);
    }

    /// Review Focus 2: a body cut in the middle of the file part is a clean
    /// 400 and leaves no stored file.
    #[tokio::test]
    async fn a_body_cut_mid_file_is_malformed_and_leaves_nothing() {
        let (_dir, storage) = storage();
        let mut data = body(
            "XX",
            &[
                ("department_id", None, "7"),
                ("file", Some("a.pdf"), &"x".repeat(5000)),
            ],
        );
        data.truncate(data.len() - 2000);
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20).await;
        assert_eq!(intake.err(), Some(IntakeError::Malformed));
        assert_eq!(files_under(storage.root()), 0);
    }

    #[tokio::test]
    async fn a_connection_dropped_mid_body_leaves_nothing() {
        let (_dir, storage) = storage();
        let head = body("XX", &[("file", Some("a.pdf"), &"x".repeat(3000))]);
        let first = bytes::Bytes::from(head[..1500].to_vec());
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(first),
            Err(std::io::Error::other("connection reset")),
        ]);
        let intake = read("XX".into(), Body::from_stream(stream), &storage, 1 << 20).await;
        assert_eq!(intake.err(), Some(IntakeError::Malformed));
        assert_eq!(files_under(storage.root()), 0);
    }

    /// Review Focus 2: over the limit is 413 and nothing stays on disk —
    /// the limit is on the whole body, exactly at it is accepted.
    #[tokio::test]
    async fn a_body_over_the_limit_is_too_large_and_exactly_at_it_is_accepted() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("department_id", None, "7"),
                ("file", Some("a.pdf"), &"x".repeat(4000)),
            ],
        );
        let at_limit = data.len() as u64;
        let ok = read("XX".into(), Body::from(data.clone()), &storage, at_limit).await;
        assert!(ok.is_ok(), "{:?}", ok.err());
        let before = files_under(storage.root());
        let over = read("XX".into(), Body::from(data), &storage, at_limit - 1).await;
        assert_eq!(over.err(), Some(IntakeError::TooLarge));
        assert_eq!(files_under(storage.root()), before);
    }

    #[tokio::test]
    async fn invalid_utf8_in_a_text_part_and_too_many_parts_are_malformed() {
        let (_dir, storage) = storage();
        let mut raw = b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0xff, 0xfe]);
        raw.extend_from_slice(b"\r\n--XX--\r\n");
        let bad = read("XX".into(), Body::from(raw), &storage, 1 << 20).await;
        assert_eq!(bad.err(), Some(IntakeError::Malformed));

        let many: Vec<(String, Option<&str>, &str)> = (0..=PART_LIMIT)
            .map(|i| (format!("k{i}"), None, "v"))
            .collect();
        let borrowed: Vec<(&str, Option<&str>, &str)> =
            many.iter().map(|(n, f, d)| (n.as_str(), *f, *d)).collect();
        let data = body("XX", &borrowed);
        let over = read("XX".into(), Body::from(data), &storage, 1 << 20).await;
        assert_eq!(over.err(), Some(IntakeError::Malformed));
    }
}
