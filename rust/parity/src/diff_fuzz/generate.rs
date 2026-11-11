//! Seeded `/api/v1` requests: documents, departments, upload requests, tags,
//! filters, pagination, invalid ids, malformed JSON, multipart, headers, and
//! missing, expired or wrong-audience tokens.

use super::model::{Auth, Body, DocAction, FilePart, IdRef, Request, Route};
use super::rng::Rng;

pub const CATEGORIES: &[&str] = &[
    "list-documents",
    "show-document",
    "download",
    "multipart",
    "malformed-json",
    "delete",
    "bulk-destroy",
    "tags",
    "list-departments",
    "create-department",
    "upload-request",
    "ping",
    "auth-missing",
    "auth-expired",
    "auth-bad-aud",
    "auth-garbage",
    "headers",
    "malformed-percent",
    "invalid-id",
    "odd-verb",
];

pub struct Generator {
    rng: Rng,
    seq: u64,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            seq: 0,
        }
    }
}

impl Iterator for Generator {
    type Item = Request;

    fn next(&mut self) -> Option<Self::Item> {
        let req = build(&mut self.rng, self.seq);
        self.seq = self.seq.saturating_add(1);
        Some(req)
    }
}

/// The one upload both databases can download. Not part of the random stream:
/// the driver sends it again after every fixture reset.
pub fn seed_upload() -> Request {
    let mut req = Request::get("seed", Route::Documents);
    req.method = "POST";
    req.body = Body::Multipart {
        boundary: "fuzzseed".into(),
        fields: vec![
            ("department_id".into(), "1".into()),
            ("tags[type]".into(), "seed".into()),
        ],
        file: Some(FilePart {
            filename: "seed.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\n%%EOF\n".to_vec(),
        }),
    };
    req
}

fn build(rng: &mut Rng, seq: u64) -> Request {
    let category = CATEGORIES[rng.gen_range(CATEGORIES.len())];
    let mut req = match category {
        "list-documents" => list_documents(rng),
        "show-document" => show_document(rng),
        "download" => download(rng),
        "multipart" => multipart(rng, seq),
        "malformed-json" => malformed_json(rng),
        "delete" => delete_document(rng),
        "bulk-destroy" => bulk_destroy(rng),
        "tags" => replace_tags(rng, seq),
        "list-departments" => list_departments(rng),
        "create-department" => create_department(rng, seq),
        "upload-request" => show_upload(rng),
        "ping" => ping(rng),
        "auth-missing" => forced_auth(rng, seq, Auth::None),
        "auth-expired" => forced_auth(rng, seq, Auth::Expired),
        "auth-bad-aud" => forced_auth(rng, seq, Auth::BadAud),
        "auth-garbage" => {
            let auth = if rng.chance(1, 2) {
                Auth::Garbage
            } else {
                Auth::EmptyBearer
            };
            forced_auth(rng, seq, auth)
        }
        "headers" => with_headers(rng, seq),
        "malformed-percent" => malformed_percent(rng),
        "invalid-id" => invalid_id(rng),
        "odd-verb" => odd_verb(rng),
        _ => ping(rng),
    };
    req.category = category;
    if !category.starts_with("auth-")
        && !matches!(category, "malformed-percent" | "seed")
        && rng.chance(1, 8)
    {
        req.auth = random_auth(rng);
    }
    req
}

fn list_documents(rng: &mut Rng) -> Request {
    let mut req = Request::get("list-documents", Route::Documents);
    maybe(
        rng,
        &mut req.query,
        "department_id",
        &["1", "2", "3", "4", "0", "-1", "abc", ""],
    );
    maybe(
        rng,
        &mut req.query,
        "tag",
        &["type", "urgent", "missing", "Type", ""],
    );
    if req.query.iter().any(|(k, _)| k == "tag") && rng.chance(1, 2) {
        push(rng, &mut req.query, "tag_value", &["facture", "nope", ""]);
    } else if rng.chance(1, 5) {
        req.query.push(("tag_value".into(), "facture".into()));
    }
    maybe(
        rng,
        &mut req.query,
        "uploaded_by",
        &["fuzz-client", "other", ""],
    );
    maybe(
        rng,
        &mut req.query,
        "upload_mode",
        &["sync", "async", "nope", ""],
    );
    maybe(
        rng,
        &mut req.query,
        "uploaded_after",
        &["2026-01-01T00:00:00.000Z", "2026-01-02", "not-a-date", ""],
    );
    maybe(
        rng,
        &mut req.query,
        "uploaded_before",
        &["2026-02-01T00:00:00.000Z", "yesterday", ""],
    );
    maybe(
        rng,
        &mut req.query,
        "indexed_after",
        &["2026-01-01T00:00:00.000Z", "nope"],
    );
    maybe(
        rng,
        &mut req.query,
        "indexed_before",
        &["2026-06-01T00:00:00.000Z", ""],
    );
    maybe(
        rng,
        &mut req.query,
        "updated_after",
        &["2026-01-01T00:00:00.000Z"],
    );
    maybe(
        rng,
        &mut req.query,
        "updated_before",
        &["2026-12-01T00:00:00.000Z", "nope"],
    );
    if rng.chance(2, 3) {
        push(
            rng,
            &mut req.query,
            "limit",
            &["0", "1", "2", "100", "500", "501", "-1", "abc", "1.5", ""],
        );
    }
    if rng.chance(2, 3) {
        push(
            rng,
            &mut req.query,
            "offset",
            &["0", "1", "2", "1000000", "1000001", "-1", "abc", ""],
        );
    }
    if rng.chance(1, 6) {
        req.query.push(("limit".into(), "1".into()));
        req.query.push(("limit".into(), "2".into()));
    }
    if rng.chance(1, 8) {
        req.query.push(("unknown".into(), "x".into()));
    }
    req
}

fn show_document(rng: &mut Rng) -> Request {
    Request::get(
        "show-document",
        Route::Document {
            action: DocAction::Show,
            id: doc_ref(rng, true),
        },
    )
}

fn download(rng: &mut Rng) -> Request {
    let id = if rng.chance(1, 3) {
        IdRef::CreatedDoc(rng.gen_range(4) as u8)
    } else {
        IdRef::SeedDoc
    };
    Request::get(
        "download",
        Route::Document {
            action: DocAction::Download,
            id,
        },
    )
}

fn multipart(rng: &mut Rng, seq: u64) -> Request {
    let mut req = Request::get("multipart", Route::Documents);
    req.method = "POST";
    let mut fields = vec![("department_id".into(), dept_choice(rng))];
    if rng.chance(2, 3) {
        fields.push((
            "tags[type]".into(),
            rng.pick(&["facture", "note", "a<b>&c", ""]).to_string(),
        ));
    }
    if rng.chance(1, 3) {
        fields.push(("tags[urgent]".into(), String::new()));
    }
    if rng.chance(1, 4) {
        fields.push((
            "callback_url".into(),
            rng.pick(&[
                "https://example.com/cb",
                "http://127.0.0.1/cb",
                "http://10.1.2.3/cb",
                "not a url",
                "",
            ])
            .to_string(),
        ));
    }
    if rng.chance(1, 10) {
        fields.push(("tags[a][b]".into(), "nested".into()));
    }
    let file = if rng.chance(1, 8) {
        None
    } else {
        Some(FilePart {
            filename: filename(rng, seq),
            content_type: rng
                .pick(&[
                    "application/pdf",
                    "text/plain",
                    "application/octet-stream",
                    "image/png",
                    "",
                ])
                .to_string(),
            bytes: file_bytes(rng),
        })
    };
    // A wrong part name: the file is ignored, both sides should reject it.
    if rng.chance(1, 12) {
        fields.push(("document".into(), "not-the-file-field".into()));
    }
    req.body = Body::Multipart {
        boundary: format!("fuzz{seq}"),
        fields,
        file,
    };
    req
}

fn malformed_json(rng: &mut Rng) -> Request {
    let mut req = Request::get("malformed-json", Route::Documents);
    req.method = *rng.pick(&["POST", "PUT"]);
    if req.method == "PUT" {
        req.route = Route::Tags {
            id: doc_ref(rng, true),
        };
    }
    let samples: &[&[u8]] = &[
        b"{",
        b"{bad",
        b"",
        b"null",
        b"[]",
        b"\"x\"",
        b"{ \"department_id\": ",
        b"{ \"department_id\": 1, }",
        b"/* c */ {\"department_id\":1}",
        b"{ \"department_id\": 1 }\n",
        b"{ \"a\": 1, \"a\": 2 }",
        b"{\"parent_id\":1,\"name\":\"x\"}",
    ];
    let bytes = rng.pick(samples).to_vec();
    let content_type = rng
        .pick(&[
            "application/json",
            "application/json; charset=utf-8",
            "text/plain",
        ])
        .to_string();
    req.body = Body::Raw {
        content_type,
        bytes,
    };
    req
}

fn delete_document(rng: &mut Rng) -> Request {
    let mut req = Request::get(
        "delete",
        Route::Document {
            action: DocAction::Show,
            id: doc_ref(rng, true),
        },
    );
    req.method = "DELETE";
    req
}

fn bulk_destroy(rng: &mut Rng) -> Request {
    let mut req = Request::get("bulk-destroy", Route::Bulk);
    req.method = "DELETE";
    let mut fields = Vec::new();
    let n = rng.gen_range(4);
    if n == 0 {
        // Missing document_ids.
    } else {
        for _ in 0..n {
            let id = match rng.gen_range(5) {
                0 => super::model::DOC_FIXTURE.to_owned(),
                1 => super::model::UNKNOWN_UUID.to_owned(),
                2 => "not-a-uuid".into(),
                3 => String::new(),
                _ => super::model::DOC_READONLY.to_owned(),
            };
            fields.push(("document_ids[]".into(), id));
        }
    }
    if rng.chance(1, 5) {
        req.body = Body::Raw {
            content_type: "application/json".into(),
            bytes: br#"{"document_ids":["not-a-uuid"]}"#.to_vec(),
        };
    } else {
        req.body = Body::Form(fields);
    }
    req
}

fn replace_tags(rng: &mut Rng, seq: u64) -> Request {
    let mut req = Request::get(
        "tags",
        Route::Tags {
            id: doc_ref(rng, true),
        },
    );
    req.method = "PUT";
    if rng.chance(1, 5) {
        req.body = Body::Raw {
            content_type: "application/json".into(),
            bytes: format!("{{\"tags\":{{\"n\":\"{seq}\"}}}}").into_bytes(),
        };
    } else {
        let mut fields = Vec::new();
        match rng.gen_range(5) {
            0 => {}
            1 => fields.push(("tags".into(), "scalar".into())),
            2 => fields.push(("tags[type]".into(), format!("t{seq}"))),
            3 => {
                fields.push(("tags[type]".into(), "facture".into()));
                fields.push(("tags[urgent]".into(), String::new()));
            }
            _ => fields.push(("tags[clé]".into(), "x".into())),
        }
        req.body = Body::Form(fields);
    }
    req
}

fn list_departments(rng: &mut Rng) -> Request {
    let mut req = Request::get("list-departments", Route::Departments);
    if rng.chance(1, 2) {
        push(
            rng,
            &mut req.query,
            "limit",
            &["1", "100", "501", "0", "-1", "abc"],
        );
        push(rng, &mut req.query, "offset", &["0", "1", "1000001"]);
    }
    req
}

fn create_department(rng: &mut Rng, seq: u64) -> Request {
    let mut req = Request::get("create-department", Route::Departments);
    req.method = "POST";
    let name = match rng.gen_range(6) {
        0 => format!("fuzz-dept-{seq}"),
        1 => "Fuzz Payroll".into(),
        2 => String::new(),
        3 => format!("café-{seq}"),
        4 => format!("a<b>&c-{seq}"),
        _ => format!("fuzz-dept-{seq}"),
    };
    let parent = dept_choice(rng);
    if rng.chance(1, 4) {
        req.body = Body::Raw {
            content_type: "application/json".into(),
            bytes: format!(
                "{{\"parent_id\":{},\"name\":{}}}",
                parent_json(&parent),
                serde_json_string(&name)
            )
            .into_bytes(),
        };
    } else {
        let mut fields = vec![("name".into(), name)];
        if rng.chance(5, 6) {
            fields.push(("parent_id".into(), parent));
        }
        req.body = Body::Form(fields);
    }
    req
}

fn show_upload(rng: &mut Rng) -> Request {
    let id = match rng.gen_range(5) {
        0 => IdRef::UploadPending,
        1 => IdRef::UploadFailed,
        2 => IdRef::Unknown,
        3 => IdRef::CreatedUpload(rng.gen_range(4) as u8),
        _ => IdRef::Raw(rng.pick(&["not-a-uuid", "1", ".."]).to_string()),
    };
    Request::get("upload-request", Route::Upload { id })
}

fn ping(rng: &mut Rng) -> Request {
    let mut req = Request::get("ping", Route::Ping);
    if rng.chance(1, 2) {
        req.auth = Auth::None;
    }
    if rng.chance(1, 3) {
        req.route = Route::OpenApi;
        req.auth = Auth::None;
    }
    req
}

fn forced_auth(rng: &mut Rng, seq: u64, auth: Auth) -> Request {
    let mut req = match rng.gen_range(4) {
        0 => list_documents(rng),
        1 => show_document(rng),
        2 => multipart(rng, seq),
        _ => create_department(rng, seq),
    };
    req.auth = auth;
    req
}

fn with_headers(rng: &mut Rng, seq: u64) -> Request {
    let mut req = match rng.gen_range(3) {
        0 => list_documents(rng),
        1 => show_document(rng),
        _ => multipart(rng, seq),
    };
    let (name, value) = *rng.pick(&[
        ("Accept", "application/json"),
        ("Accept", "text/html"),
        ("Accept", "*/*"),
        ("Accept", "application/xml"),
        ("X-Request-Id", "fuzz"),
        ("If-None-Match", "W/\"00000000000000000000000000000000\""),
        ("If-None-Match", "*"),
        ("Origin", "http://localhost"),
        ("X-Forwarded-For", "203.0.113.5"),
        ("Range", "bytes=0-1"),
        ("Accept-Language", "fr"),
    ]);
    req.extra_headers.push((name.into(), value.into()));
    req
}

fn malformed_percent(rng: &mut Rng) -> Request {
    let mut req = Request::get("malformed-percent", Route::Documents);
    if rng.chance(1, 2) {
        req.method = "POST";
        req.body = Body::Raw {
            content_type: "application/x-www-form-urlencoded".into(),
            bytes: rng
                .pick(&[
                    &b"a=%zz"[..],
                    &b"a=%zz&b=1"[..],
                    &b"department_id=%2"[..],
                    &b"a=%"[..],
                    &b"a=%ff"[..],
                ])
                .to_vec(),
        };
    } else {
        req.raw_query = Some(
            rng.pick(&[
                "a=%zz",
                "a=%zz&limit=1",
                "a=%2",
                "a=%",
                "a=%2G",
                "a=1&b=%zz&c=3",
                "limit=%zz",
                "a=%ff",
                "tag=%zz",
            ])
            .to_string(),
        );
        if rng.chance(1, 4) {
            req.raw_path = Some("/api/v1/documents/%zz".into());
        }
    }
    req
}

fn invalid_id(rng: &mut Rng) -> Request {
    let raw = match rng.gen_range(8) {
        0 => "not-a-uuid".into(),
        1 => "1".into(),
        2 => "..".into(),
        3 => " ".into(),
        4 => format!("{} ", super::model::DOC_FIXTURE),
        5 => super::model::DOC_FIXTURE.to_ascii_uppercase(),
        6 => "x".repeat(80),
        _ => "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa1".into(),
    };
    let id = IdRef::Raw(raw);
    let mut req = if rng.chance(1, 3) {
        Request::get("invalid-id", Route::Upload { id })
    } else if rng.chance(1, 2) {
        let mut req = Request::get(
            "invalid-id",
            Route::Document {
                action: DocAction::Show,
                id,
            },
        );
        req.method = *rng.pick(&["GET", "DELETE"]);
        req
    } else {
        Request::get("invalid-id", Route::Tags { id })
    };
    if req.route_is_tags() {
        req.method = "PUT";
        req.body = Body::Form(vec![("tags[type]".into(), "x".into())]);
    }
    req
}

fn odd_verb(rng: &mut Rng) -> Request {
    match rng.gen_range(8) {
        0 => verb("PATCH", Route::Documents),
        1 => verb("POST", Route::Ping),
        2 => verb("GET", Route::Bulk),
        3 => verb("PUT", Route::Departments),
        4 => verb(
            "DELETE",
            Route::Upload {
                id: IdRef::UploadPending,
            },
        ),
        5 => verb("HEAD", Route::Ping),
        6 => verb("OPTIONS", Route::Documents),
        _ => verb("POST", Route::Literal("/api/v1/nope".into())),
    }
}

fn verb(method: &'static str, route: Route) -> Request {
    let mut req = Request::get("odd-verb", route);
    req.method = method;
    req
}

fn random_auth(rng: &mut Rng) -> Auth {
    *rng.pick(&[
        Auth::None,
        Auth::Valid,
        Auth::Expired,
        Auth::BadAud,
        Auth::EmptyBearer,
        Auth::Garbage,
        Auth::Basic,
    ])
}

fn doc_ref(rng: &mut Rng, allow_invalid: bool) -> IdRef {
    match rng.gen_range(if allow_invalid { 8 } else { 6 }) {
        0 => IdRef::Doc,
        1 => IdRef::DocAsync,
        2 => IdRef::DocReadonly,
        3 => IdRef::SeedDoc,
        4 => IdRef::CreatedDoc(rng.gen_range(4) as u8),
        5 => IdRef::Unknown,
        6 => IdRef::Raw("not-a-uuid".into()),
        _ => IdRef::Raw("..".into()),
    }
}

fn dept_choice(rng: &mut Rng) -> String {
    rng.pick(&["1", "2", "3", "4", "0", "-1", "abc", "999999", ""])
        .to_string()
}

fn filename(rng: &mut Rng, seq: u64) -> String {
    match rng.gen_range(8) {
        0 => format!("file-{seq}.pdf"),
        1 => format!("../x-{seq}.txt"),
        2 => format!("quote\"-{seq}.pdf"),
        3 => format!("café-{seq}.pdf"),
        4 => format!("a<b>&c-{seq}.txt"),
        5 => format!("line\n{seq}.txt"),
        6 => format!("{seq}"),
        _ => format!("spaced name {seq}.pdf"),
    }
}

fn file_bytes(rng: &mut Rng) -> Vec<u8> {
    match rng.gen_range(5) {
        0 => Vec::new(),
        1 => b"hello".to_vec(),
        2 => b"%PDF-1.4\n".to_vec(),
        3 => b"\x89PNG\r\n".to_vec(),
        _ => vec![b'a'; 64 + rng.gen_range(64)],
    }
}

fn maybe(rng: &mut Rng, query: &mut Vec<(String, String)>, key: &str, values: &[&str]) {
    if rng.chance(1, 2) {
        push(rng, query, key, values);
    }
}

fn push(rng: &mut Rng, query: &mut Vec<(String, String)>, key: &str, values: &[&str]) {
    query.push((key.into(), rng.pick(values).to_string()));
}

fn parent_json(parent: &str) -> String {
    if parent.is_empty() {
        "null".into()
    } else if let Ok(n) = parent.parse::<i64>() {
        n.to_string()
    } else {
        serde_json_string(parent)
    }
}

fn serde_json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

trait RouteKind {
    fn route_is_tags(&self) -> bool;
}

impl RouteKind for Request {
    fn route_is_tags(&self) -> bool {
        matches!(self.route, Route::Tags { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_yields_the_same_requests() {
        let a: Vec<Request> = Generator::new(13).take(40).collect();
        let b: Vec<Request> = Generator::new(13).take(40).collect();
        let c: Vec<Request> = Generator::new(14).take(40).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn a_few_hundred_requests_cover_every_category() {
        let seen: std::collections::BTreeSet<&str> =
            Generator::new(7).take(500).map(|r| r.category).collect();
        for category in CATEGORIES {
            assert!(seen.contains(category), "missing {category}");
        }
    }

    #[test]
    fn the_seed_upload_is_stable() {
        assert_eq!(seed_upload(), seed_upload());
        assert_eq!(seed_upload().method, "POST");
    }
}
