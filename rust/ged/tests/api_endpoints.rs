//! `/api/v1` documents, departments and uploads against a real database,
//! through the router (no network): plan Review Focus 2 (cut or oversized
//! upload leaves no staging file and no phantom row) and 5 (concurrent
//! creation of one department: one 201, the rest 422, never a 500), plus the
//! conditional restore and the served OpenAPI contract.

mod auth_support;
mod support;

use auth_support::{Idp, claims, sign};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ged::AppState;
use ged::config::Config;
use ged::storage::{StagingStore, Storage};
use http_body_util::BodyExt;
use sqlx::PgPool;
use support::ScratchDb;
use tower::ServiceExt;

const LIMIT: u64 = 65_536;

struct World {
    scratch: ScratchDb,
    pool: PgPool,
    app: Router,
    token: String,
    dept: i64,
    dir: tempfile::TempDir,
    _idp: Idp,
}

async fn world() -> World {
    let idp = Idp::start().await;
    let scratch = ScratchDb::create().await;
    let pool = scratch.pool().await;
    ged::db::migrate(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::from_lookup(&|k| match k {
        "RAILS_ENV" => Some("test".to_owned()),
        "GED_OIDC_ISSUER" => Some(auth_support::ISSUER.to_owned()),
        "GED_OIDC_AUDIENCE" => Some(auth_support::AUDIENCE.to_owned()),
        "GED_OIDC_JWKS_URL" => Some(idp.url.clone()),
        "GED_MAX_UPLOAD_BYTES" => Some(LIMIT.to_string()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, pool.clone()).with_storage(
        Storage::new(dir.path().join("files")),
        StagingStore::new(dir.path().join("staging")),
    );
    let dept: i64 = sqlx::query_scalar(
        "INSERT INTO departments (name, created_at, updated_at) VALUES ('Finance', now(), now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let client: i64 = sqlx::query_scalar(
        "INSERT INTO app_clients (client_identifier, created_at, updated_at) VALUES ('c1', now(), now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO client_department_accesses (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
         VALUES ($1, $2, true, true, now(), now())",
    )
    .bind(client)
    .bind(dept)
    .execute(&pool)
    .await
    .unwrap();
    World {
        scratch,
        pool,
        app: ged::router(state),
        token: sign(&claims("c1")),
        dept,
        dir,
        _idp: idp,
    }
}

impl World {
    async fn finish(self) {
        self.pool.close().await;
        self.scratch.drop_db().await;
    }

    fn files(&self, sub: &str) -> usize {
        let mut count = 0;
        let mut stack = vec![self.dir.path().join(sub)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                if e.path().is_dir() {
                    stack.push(e.path())
                } else {
                    count += 1
                }
            }
        }
        count
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, String) {
        send(self.app.clone(), req).await
    }

    fn authed(&self, builder: axum::http::request::Builder) -> axum::http::request::Builder {
        builder.header("Authorization", format!("Bearer {}", self.token))
    }
}

async fn send(app: Router, req: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn multipart(fields: &[(&str, &str)], file: &[u8]) -> (String, Vec<u8>) {
    let mut out = Vec::new();
    for (k, v) in fields {
        out.extend_from_slice(
            format!("--XX\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    out.extend_from_slice(
        b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"r.pdf\"\r\nContent-Type: application/pdf\r\n\r\n",
    );
    out.extend_from_slice(file);
    out.extend_from_slice(b"\r\n--XX--\r\n");
    ("multipart/form-data; boundary=XX".into(), out)
}

fn upload_request(w: &World, fields: &[(&str, &str)], file: &[u8]) -> Request<Body> {
    let (ct, body) = multipart(fields, file);
    w.authed(Request::post("/api/v1/documents"))
        .header("Content-Type", ct)
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .unwrap()
}

/// Review Focus 5.
#[tokio::test]
async fn concurrent_creation_of_one_department_is_one_201_and_the_rest_422() {
    let w = world().await;
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let app = w.app.clone();
        let req = w
            .authed(Request::post("/api/v1/departments"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("name=Invoices&parent_id={}", w.dept)))
            .unwrap();
        tasks.push(tokio::spawn(send(app, req)));
    }
    let mut created = 0;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        match status.as_u16() {
            201 => created += 1,
            422 => assert_eq!(body, r#"{"error":"Name has already been taken"}"#),
            other => panic!("unexpected {other}: {body}"),
        }
    }
    assert_eq!(created, 1);
    assert_eq!(w.count("departments").await, 2);
    w.finish().await;
}

/// Review Focus 2: over the limit, announced or not.
#[tokio::test]
async fn an_oversized_upload_is_an_empty_413_with_no_file_and_no_row() {
    let w = world().await;
    let big = vec![b'a'; LIMIT as usize + 1];
    let dept = w.dept.to_string();

    // announced by Content-Length
    let (status, body) = w
        .send(upload_request(&w, &[("department_id", &dept)], &big))
        .await;
    assert_eq!((status.as_u16(), body.as_str()), (413, ""));

    // not announced (chunked): detected while streaming
    let (ct, raw) = multipart(&[("department_id", &dept)], &big);
    let stream = futures_util::stream::iter(
        raw.chunks(700)
            .map(|c| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(c)))
            .collect::<Vec<_>>(),
    );
    let req = w
        .authed(Request::post("/api/v1/documents"))
        .header("Content-Type", ct)
        .body(Body::from_stream(stream))
        .unwrap();
    let (status, body) = w.send(req).await;
    assert_eq!((status.as_u16(), body.as_str()), (413, ""));

    // and the async flavour
    let (status, body) = w
        .send(upload_request(
            &w,
            &[
                ("department_id", &dept),
                ("callback_url", "https://example.com/cb"),
            ],
            &big,
        ))
        .await;
    assert_eq!((status.as_u16(), body.as_str()), (413, ""));

    assert_eq!(w.count("documents").await, 0);
    assert_eq!(w.count("upload_requests").await, 0);
    assert_eq!(w.count("jobs").await, 0);
    assert_eq!(w.files("files"), 0);
    assert_eq!(w.files("staging"), 0);
    w.finish().await;
}

/// The limit is on the whole body: exactly at it is accepted.
#[tokio::test]
async fn a_body_of_exactly_the_limit_is_accepted() {
    let w = world().await;
    let dept = w.dept.to_string();
    let (_, empty) = multipart(&[("department_id", &dept)], b"");
    let file = vec![b'a'; LIMIT as usize - empty.len()];
    let req = upload_request(&w, &[("department_id", &dept)], &file);
    assert_eq!(req.headers()["content-length"], LIMIT.to_string());
    let (status, body) = w.send(req).await;
    assert_eq!(status.as_u16(), 201, "{body}");
    let req = upload_request(
        &w,
        &[("department_id", &dept)],
        &[file.as_slice(), b"a"].concat(),
    );
    assert_eq!(w.send(req).await.0.as_u16(), 413);
    assert_eq!(w.count("documents").await, 1);
    w.finish().await;
}

/// Review Focus 2: a body cut mid-file (sync and async).
#[tokio::test]
async fn an_upload_cut_mid_body_is_a_400_with_no_file_and_no_row() {
    let w = world().await;
    let dept = w.dept.to_string();
    for fields in [
        vec![("department_id", dept.as_str())],
        vec![
            ("department_id", dept.as_str()),
            ("callback_url", "https://example.com/cb"),
        ],
    ] {
        let (ct, raw) = multipart(&fields, &[b'x'; 2000]);
        let cut = bytes::Bytes::copy_from_slice(&raw[..raw.len() - 900]);
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(cut),
            Err(std::io::Error::other("connection reset")),
        ]);
        let req = w
            .authed(Request::post("/api/v1/documents"))
            .header("Content-Type", ct)
            .body(Body::from_stream(stream))
            .unwrap();
        let (status, _) = w.send(req).await;
        assert_eq!(status.as_u16(), 400);
    }
    assert_eq!(w.count("documents").await, 0);
    assert_eq!(w.count("upload_requests").await, 0);
    assert_eq!(w.files("files"), 0);
    assert_eq!(w.files("staging"), 0);
    w.finish().await;
}

/// Rack counts filenames toward 128 and every part toward 4096. 129 text
/// parts reach the controller. The 128th file part is an empty 500 before
/// authentication. Observed on the Rails stack, 2026-11-11.
#[tokio::test]
async fn text_parts_over_the_file_limit_reach_the_controller() {
    let w = world().await;

    let mut text = Vec::new();
    for i in 0..129 {
        text.extend(
            format!("--XX\r\nContent-Disposition: form-data; name=\"p{i}\"\r\n\r\n{i}\r\n")
                .into_bytes(),
        );
    }
    text.extend_from_slice(b"--XX--\r\n");
    let req = w
        .authed(Request::post("/api/v1/documents"))
        .header("Content-Type", "multipart/form-data; boundary=XX")
        .body(Body::from(text.clone()))
        .unwrap();
    let (status, body) = w.send(req).await;
    assert_eq!(
        (status.as_u16(), body.as_str()),
        (
            400,
            r#"{"error":"param is missing or the value is empty or invalid: department_id"}"#
        )
    );

    let req = Request::post("/api/v1/documents")
        .header("Content-Type", "multipart/form-data; boundary=XX")
        .body(Body::from(text))
        .unwrap();
    let (status, body) = w.send(req).await;
    assert_eq!(
        (status.as_u16(), body.as_str()),
        (401, r#"{"error":"missing bearer token"}"#)
    );

    let mut files = Vec::new();
    for i in 0..128 {
        files.extend(
            format!(
                "--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f{i}.txt\"\r\nContent-Type: text/plain\r\n\r\nx\r\n"
            )
            .into_bytes(),
        );
    }
    files.extend_from_slice(b"--XX--\r\n");
    let req = Request::post("/api/v1/documents")
        .header("Content-Type", "multipart/form-data; boundary=XX")
        .body(Body::from(files))
        .unwrap();
    let response = w.app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!((status.as_u16(), bytes.as_ref()), (500, b"".as_slice()));
    assert!(headers.get("content-type").is_none(), "{headers:?}");
    assert!(headers.get("cache-control").is_none(), "{headers:?}");
    assert!(headers.get("x-frame-options").is_none(), "{headers:?}");

    let dept = w.dept.to_string();
    let mut mixed = Vec::new();
    mixed.extend(
        format!("--XX\r\nContent-Disposition: form-data; name=\"department_id\"\r\n\r\n{dept}\r\n")
            .into_bytes(),
    );
    mixed.extend_from_slice(
        b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n",
    );
    for i in 2..200 {
        mixed.extend(
            format!("--XX\r\nContent-Disposition: form-data; name=\"p{i}\"\r\n\r\n{i}\r\n")
                .into_bytes(),
        );
    }
    mixed.extend_from_slice(b"--XX--\r\n");
    let req = w
        .authed(Request::post("/api/v1/documents"))
        .header("Content-Type", "multipart/form-data; boundary=XX")
        .body(Body::from(mixed))
        .unwrap();
    let (status, body) = w.send(req).await;
    assert_eq!(status.as_u16(), 201, "{body}");
    assert_eq!(w.count("documents").await, 1);
    assert_eq!(w.files("files"), 1);
    w.finish().await;
}

/// A refused upload (no write access, bad tags…) leaves nothing behind.
#[tokio::test]
async fn a_refused_upload_leaves_no_file() {
    let w = world().await;
    let dept = w.dept.to_string();
    let (status, _) = w
        .send(upload_request(
            &w,
            &[("department_id", &dept), ("tags", "nope")],
            b"PDF!",
        ))
        .await;
    assert_eq!(status.as_u16(), 422);
    let (status, _) = w
        .send(upload_request(
            &w,
            &[
                ("department_id", &dept),
                ("callback_url", "http://127.0.0.1/cb"),
            ],
            b"PDF!",
        ))
        .await;
    assert_eq!(status.as_u16(), 422);
    let (status, _) = w
        .send(upload_request(&w, &[("department_id", "999999")], b"PDF!"))
        .await;
    assert_eq!(status.as_u16(), 404);
    assert_eq!(w.files("files"), 0);
    assert_eq!(w.files("staging"), 0);
    assert_eq!(w.count("upload_requests").await, 0);
    w.finish().await;
}

/// Async: the row, the staged bytes and the job appear together.
#[tokio::test]
async fn an_async_upload_stages_the_bytes_and_enqueues_the_job() {
    let w = world().await;
    let dept = w.dept.to_string();
    let (status, body) = w
        .send(upload_request(
            &w,
            &[
                ("department_id", &dept),
                ("callback_url", "https://example.com/cb"),
                ("tags[type]", "facture"),
            ],
            b"PDF!",
        ))
        .await;
    assert_eq!(status.as_u16(), 202, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "pending");
    let id = json["request_id"].as_str().unwrap();
    let staged = std::fs::read(w.dir.path().join("staging").join(id)).unwrap();
    assert_eq!(staged, b"PDF!");
    assert_eq!(
        w.files("files"),
        0,
        "the spooled file moved, it was not copied"
    );
    let (queue, kind, args): (String, String, serde_json::Value) =
        sqlx::query_as("SELECT queue, kind, args FROM jobs")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(
        (queue.as_str(), kind.as_str()),
        ("uploads", "StoreDocumentJob")
    );
    assert_eq!(args, serde_json::json!({ "upload_request_id": id }));
    // polling
    let (status, body) = w
        .send(
            w.authed(Request::get(format!("/api/v1/upload_requests/{id}")))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(
        body,
        format!(
            r#"{{"request_id":"{id}","status":"pending","document_id":null,"error_message":null,"tags":null,"metadata":null}}"#
        )
    );
    w.finish().await;
}

#[tokio::test]
async fn sync_upload_then_streamed_download_with_hostile_name_headers() {
    let w = world().await;
    let dept = w.dept.to_string();
    let (_, raw) = multipart(&[("department_id", &dept)], b"%PDF-1.4 hello");
    let raw = String::from_utf8(raw)
        .unwrap()
        .replace("filename=\"r.pdf\"", "filename=\"na\\\"me é.pdf\"")
        .into_bytes();
    let req = w
        .authed(Request::post("/api/v1/documents"))
        .header("Content-Type", "multipart/form-data; boundary=XX")
        .body(Body::from(raw))
        .unwrap();
    let (status, body) = w.send(req).await;
    assert_eq!(status.as_u16(), 201, "{body}");
    let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = w
        .app
        .clone()
        .oneshot(
            w.authed(Request::get(format!("/api/v1/documents/{id}/download")))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let disposition = response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        !disposition.contains('\r') && !disposition.contains('\n'),
        "{disposition}"
    );
    assert!(!disposition.contains("\"b"), "{disposition}");
    assert_eq!(response.headers()["content-length"], "14");
    assert_eq!(response.headers()["content-type"], "application/pdf");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"%PDF-1.4 hello");
    w.finish().await;
}

#[tokio::test]
async fn the_list_total_is_counted_before_limit_and_offset() {
    let w = world().await;
    let dept = w.dept.to_string();
    for _ in 0..3 {
        assert_eq!(
            w.send(upload_request(&w, &[("department_id", &dept)], b"x"))
                .await
                .0
                .as_u16(),
            201
        );
    }
    let (status, body) = w
        .send(
            w.authed(Request::get("/api/v1/documents?limit=2&offset=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status.as_u16(), 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["documents"].as_array().unwrap().len(), 2);
    assert_eq!(
        json["pagination"],
        serde_json::json!({"limit": 2, "offset": 1, "total": 3})
    );
    let (_, body) = w
        .send(
            w.authed(Request::get("/api/v1/documents?offset=99999999999"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["pagination"]["offset"], 1_000_000);
    assert_eq!(json["pagination"]["total"], 3);
    w.finish().await;
}

#[tokio::test]
async fn bulk_destroy_is_fail_closed_and_capped_at_500() {
    let w = world().await;
    let dept = w.dept.to_string();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (_, body) = w
            .send(upload_request(&w, &[("department_id", &dept)], b"x"))
            .await;
        ids.push(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let del = |qs: String| {
        w.authed(Request::delete(format!(
            "/api/v1/documents/bulk_destroy?{qs}"
        )))
        .body(Body::empty())
        .unwrap()
    };
    // one unknown id refuses everything
    let qs = format!(
        "document_ids[]={}&document_ids[]={}",
        ids[0],
        uuid::Uuid::new_v4()
    );
    assert_eq!(w.send(del(qs)).await.0.as_u16(), 404);
    assert_eq!(w.count("documents").await, 2);
    let live = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM documents WHERE deleted_at IS NULL")
            .fetch_one(&w.pool)
            .await
            .unwrap()
    };
    assert_eq!(live().await, 2);
    // 501 ids (a JSON body: the query line is capped at 10 KiB)
    let json_del = |n: usize| {
        let ids: Vec<String> = (0..n).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        w.authed(Request::delete("/api/v1/documents/bulk_destroy"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({ "document_ids": ids }).to_string(),
            ))
            .unwrap()
    };
    let (status, body) = w.send(json_del(501)).await;
    assert_eq!(
        (status.as_u16(), body.as_str()),
        (400, r#"{"error":"too many document_ids (max 500)"}"#)
    );
    // exactly 500 unknown ids is a 404, not a size error
    assert_eq!(w.send(json_del(500)).await.0.as_u16(), 404);
    let (status, body) = w
        .send(del(format!(
            "document_ids[]={}&document_ids[]={}",
            ids[0], ids[1]
        )))
        .await;
    assert_eq!((status.as_u16(), body.as_str()), (200, r#"{"deleted":2}"#));
    assert_eq!(live().await, 0);
    w.finish().await;
}

/// The T6 follow-up: restoring only acts on a trashed document.
#[tokio::test]
async fn restore_only_acts_on_a_trashed_document() {
    use ged::domain::document::Document;
    let w = world().await;
    let dept = w.dept.to_string();
    let (_, body) = w
        .send(upload_request(&w, &[("department_id", &dept)], b"x"))
        .await;
    let id: uuid::Uuid = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    let now = chrono::Utc::now().naive_utc();
    assert!(
        Document::restore(&mut conn, id, now)
            .await
            .unwrap()
            .is_none(),
        "live: untouched"
    );
    let before = Document::find(&mut conn, id)
        .await
        .unwrap()
        .unwrap()
        .updated_at;
    assert_eq!(
        Document::find(&mut conn, id)
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        before
    );
    Document::soft_delete(&mut conn, id, now)
        .await
        .unwrap()
        .unwrap();
    assert!(
        Document::restore(&mut conn, id, now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        Document::restore(&mut conn, id, now)
            .await
            .unwrap()
            .is_none(),
        "second restore"
    );
    assert!(
        Document::restore(&mut conn, uuid::Uuid::new_v4(), now)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    w.finish().await;
}

#[tokio::test]
async fn openapi_and_redoc_are_public_and_the_spec_is_the_committed_file() {
    let w = world().await;
    let (status, body) = w
        .send(
            Request::get("/api/v1/openapi.yaml")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, include_str!("../../../docs/api/openapi.yaml"));
    let (status, body) = w
        .send(Request::get("/api/docs").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status.as_u16(), 200);
    assert!(body.contains("<redoc spec-url=\"/api/v1/openapi.yaml\">"));
    w.finish().await;
}

/// Every routed `/api/v1` path is documented (port of `api_docs_test.rb`).
#[test]
fn every_routed_path_is_in_the_openapi_spec() {
    let spec = include_str!("../../../docs/api/openapi.yaml");
    for path in [
        "/api/v1/ping",
        "/api/v1/documents",
        "/api/v1/documents/bulk_destroy",
        "/api/v1/documents/{id}",
        "/api/v1/documents/{id}/download",
        "/api/v1/documents/{id}/tags",
        "/api/v1/departments",
        "/api/v1/upload_requests/{id}",
    ] {
        assert!(
            spec.contains(&format!("  {path}:")),
            "{path} missing from openapi.yaml"
        );
    }
}

/// Observed on the Rails stack: `999.1.1.1` is a hostname to the request-time
/// check (accepted; delivery refuses it, unresolvable), `[::1]` is blocked.
#[tokio::test]
async fn request_time_ssrf_cases_match_rails() {
    let w = world().await;
    let dept = w.dept.to_string();
    for (host, want) in [
        ("999.1.1.1", 202),
        ("[::1]", 422),
        ("0x7f000001", 422),
        ("2130706433", 422),
        ("0177.0.0.1", 422),
        ("127.1", 422),
    ] {
        let cb = format!("http://{host}/cb");
        let (status, body) = w
            .send(upload_request(
                &w,
                &[("department_id", &dept), ("callback_url", &cb)],
                b"PDF!",
            ))
            .await;
        assert_eq!(status.as_u16(), want, "{host}: {body}");
        if want == 422 {
            assert_eq!(
                body,
                r#"{"error":"Callback url must not target a private or internal address"}"#
            );
        }
    }
    w.finish().await;
}
