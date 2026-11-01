#![allow(dead_code)]

use std::time::Duration;

use parity::forge::Template;
use parity::http::FilePart;
use parity::json::{assert_json_content_type, assert_status};
use parity::oauth::{self, Credentials};
use parity::{Api, Backend, Resp, backend};
use serde_json::Value;

pub const PDF: &[u8] = b"PDF!";

/// Finance (granted) → Invoices, plus a sibling HR the client cannot see.
pub struct Tree {
    pub finance_name: String,
    pub invoices_name: String,
    pub hr_name: String,
    pub finance: i64,
    pub invoices: i64,
    pub hr: i64,
}

pub struct ExtraClient {
    pub client_id: String,
    pub token: String,
    pub id: i64,
}

/// A registered client (provider + GED), a genuine token and its claims.
pub struct World {
    pub backend: Box<dyn Backend>,
    pub api: Api,
    pub client: Credentials,
    pub token: String,
    pub template: Template,
}

impl World {
    pub async fn new() -> Self {
        let backend = backend().await;
        let api = Api::new(backend.base_url());
        let client = oauth::register_everywhere(backend.db(), "erp-app").await;
        let token = oauth::fetch_token(&client).await;
        let template = Template::from_token(&token);
        Self {
            backend,
            api,
            client,
            token,
            template,
        }
    }

    pub async fn db_id(&self) -> i64 {
        parity::fixtures::app_client_id(self.backend.db(), &self.client.client_id).await
    }

    /// Register another provider+GED client and return a genuine token.
    pub async fn extra_client(&self, prefix: &str) -> ExtraClient {
        let creds = oauth::register_everywhere(self.backend.db(), prefix).await;
        let token = oauth::fetch_token(&creds).await;
        let id = parity::fixtures::app_client_id(self.backend.db(), &creds.client_id).await;
        ExtraClient {
            client_id: creds.client_id,
            token,
            id,
        }
    }

    /// Three departments; this world's client can read and write `finance`
    /// (and therefore its subtree).
    pub async fn read_write_tree(&self) -> Tree {
        let db = self.backend.db();
        let finance_name = parity::fixtures::unique("finance");
        let invoices_name = parity::fixtures::unique("invoices");
        let hr_name = parity::fixtures::unique("hr");
        let finance = parity::fixtures::create_department(db, &finance_name, None).await;
        let invoices = parity::fixtures::create_department(db, &invoices_name, Some(finance)).await;
        let hr = parity::fixtures::create_department(db, &hr_name, None).await;
        parity::fixtures::grant(db, self.db_id().await, finance, true, true).await;
        Tree {
            finance_name,
            invoices_name,
            hr_name,
            finance,
            invoices,
            hr,
        }
    }
}

pub async fn upload(
    api: &Api,
    token: &str,
    department_id: i64,
    filename: &str,
    extra: &[(&str, &str)],
) -> Resp {
    let dept = department_id.to_string();
    let mut fields = vec![("department_id", dept.as_str())];
    fields.extend_from_slice(extra);
    api.post_multipart(
        "/api/v1/documents",
        token,
        &fields,
        Some(FilePart {
            field: "file",
            filename,
            content_type: "application/pdf",
            bytes: PDF,
        }),
    )
    .await
}

pub fn query(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn documents_of(resp: &Resp) -> Vec<Value> {
    resp.json()["documents"]
        .as_array()
        .unwrap_or_else(|| panic!("no documents array: {}", resp.text()))
        .clone()
}

pub fn filenames(docs: &[Value]) -> Vec<String> {
    docs.iter()
        .map(|d| {
            d["filename"]
                .as_str()
                .unwrap_or_else(|| panic!("filename: {d}"))
                .to_owned()
        })
        .collect()
}

pub fn pagination_of(resp: &Resp) -> Value {
    resp.json()["pagination"].clone()
}

pub fn document_id(resp: &Resp) -> String {
    resp.json()["id"]
        .as_str()
        .unwrap_or_else(|| panic!("document id: {}", resp.text()))
        .to_owned()
}

/// Rails `as_json` for a UTC time: `YYYY-MM-DDTHH:MM:SS.mmmZ` (precision 3).
pub fn assert_rails_time(value: &Value) {
    let s = value
        .as_str()
        .unwrap_or_else(|| panic!("expected a time string, got {value}"));
    let b = s.as_bytes();
    let shape = b.len() == 24
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'.'
        && b[23] == b'Z'
        && b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19 | 23) || c.is_ascii_digit());
    assert!(shape, "expected Rails UTC millisecond time, got {s:?}");
}

pub fn assert_document_keys(doc: &Value) {
    let mut keys: Vec<&str> = doc
        .as_object()
        .unwrap_or_else(|| panic!("document is not an object: {doc}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "byte_size",
            "content_type",
            "created_at",
            "department_id",
            "filename",
            "id",
            "metadata",
            "tags",
            "uploaded_by",
        ]
    );
}

pub async fn poll_upload(api: &Api, token: &str, request_id: &str, want: &str) -> Value {
    let path = format!("/api/v1/upload_requests/{request_id}");
    let mut last = String::new();
    for _ in 0..40 {
        let resp = api.get_bearer(&path, token).await;
        assert_status(&resp, 200);
        assert_json_content_type(&resp);
        let body = resp.json();
        if body["status"] == want {
            return body;
        }
        last = resp.text();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("upload request {request_id} did not reach {want}; last body {last}");
}
