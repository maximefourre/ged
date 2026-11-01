#![allow(dead_code)]

use std::time::Duration;

use parity::admin::{AdminSession, SuiteLock};
use parity::forge::Template;
use parity::http::FilePart;
use parity::json::{assert_json_content_type, assert_status};
use parity::oauth::{self, Credentials};
use parity::{Api, Backend, Resp, backend, fixtures};
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

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

/// The API time format, fixed once by T4: a UTC time as `YYYY-MM-DDTHH:MM:SS.mmmZ`
/// (three fractional digits, truncated, always `Z`).
pub fn assert_api_time(value: &Value) {
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
    assert!(shape, "expected a UTC millisecond time, got {s:?}");
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

/// One admin HTTP journey. Holds the cross-process suite lock and deletes the
/// departments and clients it created, even when the test panics.
pub struct AdminWorld {
    pub backend: Box<dyn Backend>,
    pub admin: AdminSession,
    database_url: String,
    departments: Vec<i64>,
    clients: Vec<i64>,
    lock: Option<SuiteLock>,
}

impl AdminWorld {
    pub async fn new() -> Self {
        let lock = SuiteLock::acquire().await;
        let backend = backend().await;
        let admin = AdminSession::cached(backend.base_url())
            .await
            .expect("admin login");
        ensure_queue_active(&admin).await;
        let database_url = parity::stack::stack().database_url.clone();
        Self {
            backend,
            admin,
            database_url,
            departments: Vec::new(),
            clients: Vec::new(),
            lock: Some(lock),
        }
    }

    pub fn db(&self) -> &sqlx::PgPool {
        self.backend.db()
    }

    pub async fn department(&mut self, name: &str, parent_id: Option<i64>) -> i64 {
        let id = fixtures::create_department(self.db(), name, parent_id).await;
        self.departments.push(id);
        id
    }

    pub async fn client(&mut self, identifier: &str) -> i64 {
        let id = fixtures::create_app_client(self.db(), identifier).await;
        self.clients.push(id);
        id
    }

    pub async fn grant(&self, client_id: i64, department_id: i64, read: bool, write: bool) {
        fixtures::grant(self.db(), client_id, department_id, read, write).await;
    }

    pub async fn document(
        &self,
        department_id: i64,
        filename: &str,
        tags: Value,
        byte_size: Option<i64>,
    ) -> Uuid {
        fixtures::admin_insert_document(self.db(), department_id, filename, "seed", tags, byte_size)
            .await
    }

    pub fn track_department(&mut self, id: i64) {
        if !self.departments.contains(&id) {
            self.departments.push(id);
        }
    }

    pub fn track_client(&mut self, id: i64) {
        if !self.clients.contains(&id) {
            self.clients.push(id);
        }
    }

    pub async fn upload_request(
        &self,
        department_id: i64,
        filename: &str,
        status: &str,
        error: Option<&str>,
    ) -> Uuid {
        fixtures::admin_insert_upload_request(self.db(), department_id, filename, status, error)
            .await
    }
}

impl Drop for AdminWorld {
    fn drop(&mut self) {
        let url = self.database_url.clone();
        let departments = std::mem::take(&mut self.departments);
        let clients = std::mem::take(&mut self.clients);
        if !departments.is_empty() || !clients.is_empty() {
            let joined = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("cleanup runtime");
                rt.block_on(async move {
                    if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
                        .max_connections(1)
                        .connect(&url)
                        .await
                    {
                        fixtures::delete_fixtures(&pool, &departments, &clients).await;
                    }
                });
            })
            .join();
            if let Err(err) = joined {
                eprintln!("fixture cleanup panicked: {err:?}");
            }
        }
        // Release the suite lock only after rows are gone.
        self.lock.take();
    }
}

pub fn name(prefix: &str) -> String {
    fixtures::unique(prefix)
}

pub fn owner() -> String {
    format!("admin:{}", parity::stack::admin_credentials().0)
}

pub fn assert_see(resp: &Resp, label: &str) {
    assert!(
        resp.text().contains(label),
        "missing {label:?} in HTTP {}:\n{}",
        resp.status,
        excerpt(&resp.text())
    );
}

pub fn assert_absent(resp: &Resp, label: &str) {
    assert!(
        !resp.text().contains(label),
        "unexpected {label:?} in HTTP {}:\n{}",
        resp.status,
        excerpt(&resp.text())
    );
}

pub fn assert_login_redirect(resp: &Resp) {
    assert!(
        matches!(resp.status, 302 | 303),
        "expected a login redirect, got HTTP {}\n{}",
        resp.status,
        excerpt(&resp.text())
    );
    let loc = resp.header("location").unwrap_or("");
    assert!(
        loc.contains("sign_in"),
        "redirect location {loc:?} is not the login form"
    );
}

pub fn tr_count(html: &str) -> usize {
    html.matches("<tr").count()
}

pub fn tr_containing<'a>(html: &'a str, needle: &str) -> Option<&'a str> {
    let mut rest = html;
    while let Some(start) = rest.find("<tr") {
        rest = &rest[start..];
        let end = rest.find("</tr>").map_or(rest.len(), |i| i + 5);
        let row = &rest[..end];
        if row.contains(needle) {
            return Some(row);
        }
        rest = &rest[end.min(rest.len())..];
    }
    None
}

pub fn td_texts(row: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = row;
    while let Some(start) = rest.find("<td") {
        rest = &rest[start..];
        let content_from = rest.find('>').map_or(0, |i| i + 1);
        let end = rest.find("</td>").unwrap_or(rest.len());
        let inner = &rest[content_from..end];
        out.push(
            strip_tags(inner)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        );
        rest = &rest[end..];
    }
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Words inside the `.queue` banner, tags removed. The badge and the
/// Pause/Resume control are visible labels; the tags around them are not.
pub fn queue_words(html: &str) -> Vec<String> {
    let Some(start) = html.find("class=\"queue") else {
        return Vec::new();
    };
    let after = &html[start..];
    let content_at = after.find('>').map_or(0, |i| i + 1);
    let rest = &after[content_at..];
    let end = rest.find("</div>").unwrap_or(rest.len());
    strip_tags(&rest[..end])
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn queue_has_word(html: &str, word: &str) -> bool {
    queue_words(html).iter().any(|w| w == word)
}

/// Tag-stripped body with whitespace collapsed, so a count and its label
/// read as one phrase (`3 pending requests`).
pub fn visible_text(html: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in strip_tags(html).chars() {
        if c.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

fn excerpt(body: &str) -> String {
    let max = 800;
    if body.chars().count() <= max {
        body.to_owned()
    } else {
        let end = body.char_indices().nth(max).map_or(body.len(), |(i, _)| i);
        format!("{}…", &body[..end])
    }
}

pub async fn admin_upload(
    admin: &AdminSession,
    department_id: i64,
    filename: &str,
    bytes: &[u8],
    content_type: &str,
    tags_text: Option<&str>,
) -> Resp {
    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(filename.to_owned())
        .mime_str(content_type)
        .expect("mime");
    let mut form = reqwest::multipart::Form::new()
        .text("document[department_id]", department_id.to_string())
        .part("document[file]", part);
    if let Some(tags) = tags_text {
        form = form.text("document[tags_text]", tags.to_owned());
    }
    admin.submit_multipart("/admin/documents", form).await
}

pub async fn admin_document_id(db: &sqlx::PgPool, department_id: i64, filename: &str) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM documents WHERE department_id = $1 AND filename = $2 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(department_id)
    .bind(filename)
    .fetch_one(db)
    .await
    .unwrap_or_else(|e| panic!("document {filename} in {department_id}: {e}"))
}

pub async fn tags_of(db: &sqlx::PgPool, id: Uuid) -> Value {
    sqlx::query_scalar("SELECT tags FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("tags")
}

pub async fn deleted_at_document(db: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("deleted_at")
}

pub async fn department_id_by_name(db: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM departments WHERE name = $1")
        .bind(name)
        .fetch_one(db)
        .await
        .unwrap_or_else(|e| panic!("department {name}: {e}"))
}

pub async fn client_id_by_identifier(db: &sqlx::PgPool, identifier: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM app_clients WHERE client_identifier = $1")
        .bind(identifier)
        .fetch_one(db)
        .await
        .unwrap_or_else(|e| panic!("client {identifier}: {e}"))
}

pub async fn department_gone(db: &sqlx::PgPool, id: i64) -> bool {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM departments WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("count department");
    n == 0
}

pub async fn department_trashed(db: &sqlx::PgPool, id: i64) -> bool {
    sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM departments WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("department deleted_at")
}

pub async fn doc_department(db: &sqlx::PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT department_id FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("document department")
}

pub async fn request_department(db: &sqlx::PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT department_id FROM upload_requests WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("request department")
}

pub async fn access_flags(
    db: &sqlx::PgPool,
    client_id: i64,
    department_id: i64,
) -> Option<(bool, bool)> {
    sqlx::query_as(
        "SELECT can_read, can_write FROM client_department_accesses \
         WHERE app_client_id = $1 AND department_id = $2",
    )
    .bind(client_id)
    .bind(department_id)
    .fetch_optional(db)
    .await
    .expect("access flags")
}

pub async fn access_id(db: &sqlx::PgPool, client_id: i64, department_id: i64) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT id FROM client_department_accesses \
         WHERE app_client_id = $1 AND department_id = $2",
    )
    .bind(client_id)
    .bind(department_id)
    .fetch_optional(db)
    .await
    .expect("access id")
}

pub async fn ensure_queue_active(admin: &AdminSession) {
    let page = admin.get("/admin/upload_requests").await;
    if page.status == 200 && queue_has_word(&page.text(), "Resume") {
        let resp = admin
            .submit(Method::DELETE, "/admin/upload_queue_pause", &[])
            .await;
        assert!(
            matches!(resp.status, 302 | 303),
            "resume failed: HTTP {}",
            resp.status
        );
    }
}

pub fn zip_entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .unwrap_or_else(|e| panic!("zip: {e} ({} bytes)", bytes.len()));
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).expect("zip entry");
        let name = file.name().to_owned();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf).expect("zip bytes");
        out.push((name, buf));
    }
    out
}

pub fn pdf_bytes() -> &'static [u8] {
    b"%PDF-1.1\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n"
}

pub async fn pause_for_login_window() {
    parity::admin::wait_for_login_budget(LOGIN_BUDGET_FULL).await;
}

const LOGIN_BUDGET_FULL: usize = 10;

pub fn json_tags(pairs: &[(&str, Option<&str>)]) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert((*k).to_owned(), v.map(|s| json!(s)).unwrap_or(Value::Null));
    }
    Value::Object(map)
}

pub async fn settle_login_budget() {
    parity::admin::wait_for_login_budget(10).await;
    // The wait returns as soon as our log says the window is clear. Give Rails
    // the same moment to expire its own counter when the two clocks meet.
    tokio::time::sleep(Duration::from_millis(200)).await;
}
