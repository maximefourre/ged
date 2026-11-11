//! One generated API call, and the bytes both stacks receive.
//!
//! Path ids that the servers allocate (`CreatedDoc`, `CreatedUpload`, `SeedDoc`)
//! are filled in per side. Everything else is identical on the wire.

use crate::http::form_encode;

pub const DOC_FIXTURE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";
pub const DOC_ASYNC: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2";
pub const DOC_READONLY: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3";
pub const UPLOAD_PENDING: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1";
pub const UPLOAD_FAILED: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2";
pub const UNKNOWN_UUID: &str = "cccccccc-cccc-4ccc-8ccc-ccccccccccc1";
pub const CLIENT_ID: &str = "fuzz-client";
pub const CLIENT_UNKNOWN: &str = "fuzz-unknown";
pub const CLIENT_NOGRANT: &str = "fuzz-nogrant";
pub const CLIENT_OTHER: &str = "fuzz-other";
pub const DOC_DELETED: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub category: &'static str,
    pub method: &'static str,
    pub route: Route,
    /// When set, this path is sent as-is (no escaping). Used for a raw `%`.
    pub raw_path: Option<String>,
    pub query: Vec<(String, String)>,
    /// When set, this is the query string, without `?` and without encoding.
    pub raw_query: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub auth: Auth,
    pub body: Body,
}

impl Request {
    pub fn get(category: &'static str, route: Route) -> Self {
        Self {
            category,
            method: "GET",
            route,
            raw_path: None,
            query: Vec::new(),
            raw_query: None,
            extra_headers: Vec::new(),
            auth: Auth::Valid,
            body: Body::None,
        }
    }

    pub fn summary(&self) -> String {
        let path = self.raw_path.clone().unwrap_or_else(|| self.route.label());
        let query = self
            .raw_query
            .clone()
            .unwrap_or_else(|| pairs_preview(&self.query));
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        };
        format!(
            "{} {path}{query} auth={:?} body={}",
            self.method,
            self.auth,
            self.body.kind()
        )
    }
}

fn pairs_preview(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Ping,
    OpenApi,
    Documents,
    Document { action: DocAction, id: IdRef },
    Bulk,
    Tags { id: IdRef },
    Departments,
    Upload { id: IdRef },
    Literal(String),
}

impl Route {
    fn label(&self) -> String {
        match self {
            Self::Ping => "/api/v1/ping".into(),
            Self::OpenApi => "/api/v1/openapi.yaml".into(),
            Self::Documents => "/api/v1/documents".into(),
            Self::Document { action, id } => match action {
                DocAction::Show => format!("/api/v1/documents/{}", id.label()),
                DocAction::Download => format!("/api/v1/documents/{}/download", id.label()),
            },
            Self::Bulk => "/api/v1/documents/bulk_destroy".into(),
            Self::Tags { id } => format!("/api/v1/documents/{}/tags", id.label()),
            Self::Departments => "/api/v1/departments".into(),
            Self::Upload { id } => format!("/api/v1/upload_requests/{}", id.label()),
            Self::Literal(path) => path.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocAction {
    Show,
    Download,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdRef {
    Doc,
    DocAsync,
    DocReadonly,
    UploadPending,
    UploadFailed,
    Unknown,
    SeedDoc,
    CreatedDoc(u8),
    CreatedUpload(u8),
    Raw(String),
}

impl IdRef {
    fn label(&self) -> String {
        match self {
            Self::Doc => DOC_FIXTURE.into(),
            Self::DocAsync => DOC_ASYNC.into(),
            Self::DocReadonly => DOC_READONLY.into(),
            Self::UploadPending => UPLOAD_PENDING.into(),
            Self::UploadFailed => UPLOAD_FAILED.into(),
            Self::Unknown => UNKNOWN_UUID.into(),
            Self::SeedDoc => "<seed>".into(),
            Self::CreatedDoc(n) => format!("<created-doc-{n}>"),
            Self::CreatedUpload(n) => format!("<created-upload-{n}>"),
            Self::Raw(s) => s.clone(),
        }
    }

    pub fn resolve(&self, ids: &Ids<'_>) -> String {
        match self {
            Self::Doc => DOC_FIXTURE.into(),
            Self::DocAsync => DOC_ASYNC.into(),
            Self::DocReadonly => DOC_READONLY.into(),
            Self::UploadPending => UPLOAD_PENDING.into(),
            Self::UploadFailed => UPLOAD_FAILED.into(),
            Self::Unknown => UNKNOWN_UUID.into(),
            Self::SeedDoc => {
                if ids.seed_doc.is_empty() {
                    DOC_FIXTURE.into()
                } else {
                    ids.seed_doc.to_owned()
                }
            }
            Self::CreatedDoc(n) => ids
                .created_docs
                .get(*n as usize)
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if ids.seed_doc.is_empty() {
                        DOC_FIXTURE.into()
                    } else {
                        ids.seed_doc.to_owned()
                    }
                }),
            Self::CreatedUpload(n) => ids
                .created_uploads
                .get(*n as usize)
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| UPLOAD_PENDING.into()),
            Self::Raw(s) => s.clone(),
        }
    }
}

/// Per-side ids. Tokens are shared when both stacks trust the same key.
pub struct Ids<'a> {
    pub seed_doc: &'a str,
    pub created_docs: &'a [String],
    pub created_uploads: &'a [String],
    pub token_valid: &'a str,
    pub token_expired: &'a str,
    pub token_bad_aud: &'a str,
    pub token_unknown: &'a str,
    pub token_nogrant: &'a str,
    pub token_other: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    None,
    Valid,
    Expired,
    BadAud,
    /// `Authorization: Bearer` with nothing after the scheme.
    EmptyBearer,
    Garbage,
    Basic,
    /// Signed token whose `sub` is not a registered client.
    Unknown,
    /// Registered client with no department grant.
    NoGrant,
    /// Second registered client, read and write on department 1.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePart {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    None,
    Raw {
        content_type: String,
        bytes: Vec<u8>,
    },
    Form(Vec<(String, String)>),
    Multipart {
        boundary: String,
        fields: Vec<(String, String)>,
        file: Option<FilePart>,
    },
}

impl Body {
    fn kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Raw { .. } => "raw",
            Self::Form(_) => "form",
            Self::Multipart { file: None, .. } => "multipart",
            Self::Multipart { .. } => "multipart+file",
        }
    }
}

/// Bytes ready to send. Built once per side so only the path id changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub method: &'static str,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub fn encode(req: &Request, ids: &Ids<'_>) -> Target {
    let path = req
        .raw_path
        .clone()
        .unwrap_or_else(|| encode_route(&req.route, ids));
    let query = req
        .raw_query
        .clone()
        .unwrap_or_else(|| encode_query(&req.query));
    let path_and_query = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    let (content_type, body) = encode_body(&req.body);
    let mut headers = Vec::new();
    if let Some(value) = auth_header(req.auth, ids) {
        push_header(&mut headers, "Authorization", &value);
    }
    if let Some(content_type) = content_type {
        push_header(&mut headers, "Content-Type", &content_type);
    }
    for (name, value) in &req.extra_headers {
        push_header(&mut headers, name, value);
    }
    Target {
        method: req.method,
        path_and_query,
        headers,
        body,
    }
}

fn encode_route(route: &Route, ids: &Ids<'_>) -> String {
    match route {
        Route::Ping => "/api/v1/ping".into(),
        Route::OpenApi => "/api/v1/openapi.yaml".into(),
        Route::Documents => "/api/v1/documents".into(),
        Route::Document { action, id } => {
            let id = path_escape(&id.resolve(ids));
            match action {
                DocAction::Show => format!("/api/v1/documents/{id}"),
                DocAction::Download => format!("/api/v1/documents/{id}/download"),
            }
        }
        Route::Bulk => "/api/v1/documents/bulk_destroy".into(),
        Route::Tags { id } => format!("/api/v1/documents/{}/tags", path_escape(&id.resolve(ids))),
        Route::Departments => "/api/v1/departments".into(),
        Route::Upload { id } => {
            format!("/api/v1/upload_requests/{}", path_escape(&id.resolve(ids)))
        }
        Route::Literal(path) => path.clone(),
    }
}

fn encode_query(query: &[(String, String)]) -> String {
    let refs: Vec<(&str, &str)> = query
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    form_encode(&refs)
}

fn encode_body(body: &Body) -> (Option<String>, Vec<u8>) {
    match body {
        Body::None => (None, Vec::new()),
        Body::Raw {
            content_type,
            bytes,
        } => (Some(content_type.clone()), bytes.clone()),
        Body::Form(fields) => {
            let refs: Vec<(&str, &str)> = fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            (
                Some("application/x-www-form-urlencoded".into()),
                form_encode(&refs).into_bytes(),
            )
        }
        Body::Multipart {
            boundary,
            fields,
            file,
        } => {
            let bytes = encode_multipart(boundary, fields, file.as_ref());
            (
                Some(format!("multipart/form-data; boundary={boundary}")),
                bytes,
            )
        }
    }
}

fn encode_multipart(
    boundary: &str,
    fields: &[(String, String)],
    file: Option<&FilePart>,
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        push_str(
            &mut body,
            &format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            ),
        );
    }
    if let Some(file) = file {
        let quoted = quote_filename(&file.filename);
        push_str(
            &mut body,
            &format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename={quoted}\r\nContent-Type: {}\r\n\r\n",
                file.content_type
            ),
        );
        body.extend_from_slice(&file.bytes);
        push_str(&mut body, "\r\n");
    }
    push_str(&mut body, &format!("--{boundary}--\r\n"));
    body
}

fn quote_filename(name: &str) -> String {
    let mut out = String::from("\"");
    for c in name.chars() {
        if c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn push_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
}

fn auth_header(auth: Auth, ids: &Ids<'_>) -> Option<String> {
    match auth {
        Auth::None => None,
        Auth::Valid => Some(format!("Bearer {}", ids.token_valid)),
        Auth::Expired => Some(format!("Bearer {}", ids.token_expired)),
        Auth::BadAud => Some(format!("Bearer {}", ids.token_bad_aud)),
        Auth::EmptyBearer => Some("Bearer".into()),
        Auth::Garbage => Some("Bearer not-a-jwt".into()),
        Auth::Basic => Some("Basic bm90OmE=".into()),
        Auth::Unknown => Some(format!("Bearer {}", ids.token_unknown)),
        Auth::NoGrant => Some(format!("Bearer {}", ids.token_nogrant)),
        Auth::Other => Some(format!("Bearer {}", ids.token_other)),
    }
}

fn push_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    headers.push((name.to_owned(), value.to_owned()));
}

/// Percent-encode a path segment. `/` becomes `%2F` so it stays one segment.
pub fn path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids<'a>(docs: &'a [String], uploads: &'a [String]) -> Ids<'a> {
        Ids {
            seed_doc: "dddddddd-dddd-4ddd-8ddd-ddddddddddd1",
            created_docs: docs,
            created_uploads: uploads,
            token_valid: "valid-token",
            token_expired: "expired-token",
            token_bad_aud: "badaud-token",
            token_unknown: "unknown-token",
            token_nogrant: "nogrant-token",
            token_other: "other-token",
        }
    }

    #[test]
    fn encode_is_stable_and_percent_encodes_the_query() {
        let mut req = Request::get("list-documents", Route::Documents);
        req.query = vec![
            ("tag".into(), "type".into()),
            ("limit".into(), "1".into()),
            ("tag_value".into(), "a b".into()),
        ];
        let empty: [String; 0] = [];
        let once = encode(&req, &ids(&empty, &empty));
        let twice = encode(&req, &ids(&empty, &empty));
        assert_eq!(once, twice);
        assert_eq!(once.method, "GET");
        assert_eq!(
            once.path_and_query,
            "/api/v1/documents?tag=type&limit=1&tag_value=a%20b"
        );
        assert_eq!(
            once.headers,
            vec![("Authorization".into(), "Bearer valid-token".into())]
        );
        assert!(once.body.is_empty());
    }

    #[test]
    fn created_ids_are_per_side_and_fall_back_to_the_seed() {
        let req = Request {
            category: "show-document",
            method: "GET",
            route: Route::Document {
                action: DocAction::Show,
                id: IdRef::CreatedDoc(0),
            },
            raw_path: None,
            query: Vec::new(),
            raw_query: None,
            extra_headers: Vec::new(),
            auth: Auth::Expired,
            body: Body::None,
        };
        let docs = vec!["eeeeeeee-eeee-4eee-8eee-eeeeeeeeeee1".into()];
        let empty: [String; 0] = [];
        let with = encode(&req, &ids(&docs, &empty));
        assert!(
            with.path_and_query
                .contains("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeee1")
        );
        assert!(with.headers[0].1.contains("expired-token"));
        let without = encode(&req, &ids(&empty, &empty));
        assert!(
            without
                .path_and_query
                .contains("dddddddd-dddd-4ddd-8ddd-ddddddddddd1")
        );
    }

    #[test]
    fn a_raw_query_is_not_reencoded() {
        let mut req = Request::get("malformed-percent", Route::Ping);
        req.raw_query = Some("a=%zz".into());
        req.auth = Auth::None;
        let empty: [String; 0] = [];
        let target = encode(&req, &ids(&empty, &empty));
        assert_eq!(target.path_and_query, "/api/v1/ping?a=%zz");
        assert!(target.headers.is_empty());
    }

    #[test]
    fn multipart_boundary_and_filename_are_taken_from_the_request() {
        let req = Request {
            category: "multipart",
            method: "POST",
            route: Route::Documents,
            raw_path: None,
            query: Vec::new(),
            raw_query: None,
            extra_headers: vec![("Accept".into(), "application/json".into())],
            auth: Auth::Valid,
            body: Body::Multipart {
                boundary: "fuzz7".into(),
                fields: vec![("department_id".into(), "1".into())],
                file: Some(FilePart {
                    filename: "a\".pdf".into(),
                    content_type: "application/pdf".into(),
                    bytes: b"%PDF".to_vec(),
                }),
            },
        };
        let empty: [String; 0] = [];
        let target = encode(&req, &ids(&empty, &empty));
        let text = String::from_utf8(target.body.clone()).expect("utf-8 multipart");
        assert!(
            text.contains("boundary") || target.headers.iter().any(|(_, v)| v.contains("fuzz7"))
        );
        assert!(text.contains("name=\"department_id\""));
        assert!(text.contains("filename=\"a\\\".pdf\""));
        assert!(text.contains("%PDF"));
        assert!(
            target
                .headers
                .iter()
                .any(|(k, v)| k == "Accept" && v == "application/json")
        );
    }
}
