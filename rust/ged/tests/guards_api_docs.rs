//! Port of `test/integration/api_docs_test.rb`.
//!
//! The spec file, the public Redoc page and the routed `/api/v1` paths stay
//! in lockstep. Parameter names (`{id}` vs `{document_id}`) compare equal.
//!
//! Routed paths come from [`ged::api::V1_PATHS`] and from a recursive scan of
//! `src/`. A `.route("…")` literal, the bare prefix, or a mention of a path
//! the router already registers are recognized. `concat!`, `format!`, `.nest`,
//! and any other spelling of `/api/v1` fail the guard.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use ged::config::Config;
use ged::http::AppState;
use ged::router;
use http_body_util::BodyExt;
use serde_yaml::Value;
use tower::ServiceExt;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn normalize(path: &str) -> String {
    // Same two substitutions as `ApiDocsTest#normalize`: Rails `:id`, then
    // OpenAPI `{id}`.
    let colon = regex::Regex::new(r":[A-Za-z_]+").expect("colon");
    let brace = regex::Regex::new(r"\{[^}]+\}").expect("brace");
    brace
        .replace_all(&colon.replace_all(path, "{}"), "{}")
        .into_owned()
}

fn spec_value() -> Value {
    serde_yaml::from_str(ged::api::docs::SPEC).expect("parse embedded openapi.yaml")
}

fn dig<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |cur, key| cur.get(*key))
}

fn app() -> axum::Router {
    let config = Config::from_lookup(&|key| (key == "RAILS_ENV").then(|| "test".to_owned()))
        .expect("test config");
    router(AppState::without_database(config))
}

async fn send(uri: &str) -> (StatusCode, String, String) {
    let response = app()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[test]
fn normalize_collapses_both_parameter_styles() {
    assert_eq!(normalize("/api/v1/documents/{id}"), "/api/v1/documents/{}");
    assert_eq!(
        normalize("/api/v1/documents/:id/tags"),
        "/api/v1/documents/{}/tags"
    );
    assert_eq!(normalize("/api/v1/ping"), "/api/v1/ping");
}

#[tokio::test]
async fn openapi_yaml_is_served_without_authentication() {
    let (status, content_type, body) = send("/api/v1/openapi.yaml").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("application/yaml"),
        "content type {content_type}"
    );
    let spec: Value = serde_yaml::from_str(&body).expect("response is yaml");
    let version = spec
        .get("openapi")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(version.starts_with("3.1."), "openapi version {version}");
}

#[tokio::test]
async fn api_docs_renders_redoc_pointing_at_the_spec() {
    let (status, content_type, body) = send("/api/docs").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("text/html"),
        "content type {content_type}"
    );
    assert!(body.contains("redoc"), "redoc missing");
    assert!(body.contains("/api/v1/openapi.yaml"));
}

#[test]
fn every_routed_api_v1_path_is_documented() {
    let spec = spec_value();
    let documented: BTreeSet<String> = spec
        .get("paths")
        .and_then(Value::as_mapping)
        .expect("paths")
        .keys()
        .filter_map(Value::as_str)
        .map(normalize)
        .collect();

    let scan = scan_src_tree();
    assert!(
        scan.unrecognized.is_empty(),
        "unrecognized /api/v1 forms (concat!, format!, nest, or a path the router does not register):\n{}",
        scan.unrecognized.join("\n")
    );
    let exported: BTreeSet<String> = ged::api::V1_PATHS
        .iter()
        .map(|path| (*path).to_owned())
        .collect();
    assert_eq!(
        scan.routes, exported,
        "ged::api::V1_PATHS must be exactly the .route(\"/api/v1…\") literals under src/"
    );
    let routed: BTreeSet<String> = scan
        .routes
        .iter()
        .filter(|path| path.as_str() != "/api/v1/openapi.yaml")
        .map(|path| normalize(path))
        .collect();
    assert!(!routed.is_empty(), "no /api/v1 routes found");
    let missing: Vec<_> = routed.difference(&documented).cloned().collect();
    assert!(
        missing.is_empty(),
        "undocumented API paths: {}",
        missing.join(", ")
    );
}

#[test]
fn source_scan_walks_src_recursively() {
    let mut files = Vec::new();
    rust_files(&root().join("rust/ged/src"), &mut files);
    let joined: Vec<String> = files
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        joined
            .iter()
            .any(|path| path.ends_with("rust/ged/src/http/mod.rs")),
        "http/ is outside src/api and must still be scanned"
    );
    assert!(
        joined
            .iter()
            .any(|path| path.ends_with("rust/ged/src/api/documents.rs")),
        "api routes must be scanned"
    );
}

#[test]
fn concat_format_nest_and_a_non_literal_route_are_rejected() {
    let samples = [
        r#"fn f() { let _ = concat!("/api/v1/", "zzz"); }"#,
        r#"fn f() { let _ = format!("/api/v1/{}", "zzz"); }"#,
        r#"fn f(r: u8) { let _ = r.nest("/api/v1/zzz", r); }"#,
        r#"fn f() { Router::new().route(concat!("/api/v1/", "zzz"), get(show)); }"#,
        r#"const SNEAK: &str = "/api/v1/not-a-route";"#,
    ];
    for src in samples {
        let scan = finish(scan_source(src));
        assert!(
            !scan.unrecognized.is_empty(),
            "accepted an unrecognized form:\n{src}"
        );
    }
}

#[test]
fn comments_prefix_and_literal_routes_are_recognized() {
    let commented = "//! hidden `/api/v1/not-a-route`\nconst API_PREFIX: &str = \"/api/v1\";\n";
    let scan = finish(scan_source(commented));
    assert!(scan.unrecognized.is_empty(), "{:?}", scan.unrecognized);
    assert!(scan.routes.is_empty());

    let routed = "fn routes() {\n    Router::new().route(\n        \"/api/v1/documents/{id}\",\n        get(show),\n    )\n}\n";
    let scan = finish(scan_source(routed));
    assert!(scan.unrecognized.is_empty(), "{:?}", scan.unrecognized);
    assert!(scan.routes.contains("/api/v1/documents/{id}"));
}

#[test]
fn a_mention_of_an_already_routed_path_is_allowed() {
    let src = r##"
        fn routes() { Router::new().route("/api/v1/openapi.yaml", get(spec)); }
        const PAGE: &str = r#"<redoc spec-url="/api/v1/openapi.yaml"></redoc>"#;
    "##;
    let scan = finish(scan_source(src));
    assert!(scan.unrecognized.is_empty(), "{:?}", scan.unrecognized);
    assert!(scan.routes.contains("/api/v1/openapi.yaml"));
}

#[test]
fn documents_list_response_conforms_to_the_envelope_schema() {
    let spec = spec_value();
    let schema_ref = dig(
        &spec,
        &[
            "paths",
            "/api/v1/documents",
            "get",
            "responses",
            "200",
            "content",
            "application/json",
            "schema",
            "$ref",
        ],
    )
    .and_then(Value::as_str);
    assert_eq!(
        schema_ref,
        Some("#/components/schemas/DocumentList"),
        "documents list 200 response should $ref DocumentList"
    );

    let properties = dig(
        &spec,
        &["components", "schemas", "DocumentList", "properties"],
    )
    .and_then(Value::as_mapping)
    .expect("DocumentList schema");
    let mut keys: Vec<_> = properties.keys().filter_map(Value::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["documents", "pagination"]);

    let pagination = dig(
        &spec,
        &["components", "schemas", "Pagination", "properties"],
    )
    .and_then(Value::as_mapping)
    .expect("Pagination schema");
    let mut page_keys: Vec<_> = pagination.keys().filter_map(Value::as_str).collect();
    page_keys.sort_unstable();
    assert_eq!(page_keys, ["limit", "offset", "total"]);
}

struct Found {
    routes: BTreeSet<String>,
    unrecognized: Vec<String>,
}

struct Partial {
    routes: BTreeSet<String>,
    unrecognized: Vec<String>,
    others: Vec<String>,
}

enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

enum Call {
    Route,
    Nest,
    Concat,
    Format,
}

fn scan_src_tree() -> Found {
    let mut files = Vec::new();
    rust_files(&root().join("rust/ged/src"), &mut files);
    files.sort();
    let mut routes = BTreeSet::new();
    let mut unrecognized = Vec::new();
    let mut others = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let part = scan_source(&source);
        let rel = path
            .strip_prefix(root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        routes.extend(part.routes);
        for item in part.unrecognized {
            unrecognized.push(format!("{rel}: {item}"));
        }
        for item in part.others {
            others.push((rel.clone(), item));
        }
    }
    for (rel, value) in others {
        if mentions_are_known(&routes, &value) {
            continue;
        }
        unrecognized.push(format!(
            "{rel}: unrecognized /api/v1 string: {}",
            preview(&value)
        ));
    }
    Found {
        routes,
        unrecognized,
    }
}

fn finish(part: Partial) -> Found {
    let mut unrecognized = part.unrecognized;
    for value in part.others {
        if mentions_are_known(&part.routes, &value) {
            continue;
        }
        unrecognized.push(format!("unrecognized /api/v1 string: {}", preview(&value)));
    }
    Found {
        routes: part.routes,
        unrecognized,
    }
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn preview(value: &str) -> String {
    let flat = value.replace('\n', " ");
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn mentions_are_known(routes: &BTreeSet<String>, value: &str) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find("/api/v1") {
        let tail = &rest[at..];
        if let Some(len) = longest_routed_prefix(routes, tail) {
            rest = &tail[len..];
            continue;
        }
        if tail_is_bare_prefix(tail) {
            rest = &tail["/api/v1".len()..];
            continue;
        }
        return false;
    }
    true
}

fn longest_routed_prefix(routes: &BTreeSet<String>, tail: &str) -> Option<usize> {
    routes
        .iter()
        .filter(|route| tail.starts_with(route.as_str()))
        .map(|route| route.len())
        .filter(|len| boundary_after(tail, *len))
        .max()
}

fn tail_is_bare_prefix(tail: &str) -> bool {
    tail.starts_with("/api/v1") && boundary_after(tail, "/api/v1".len())
}

fn boundary_after(value: &str, len: usize) -> bool {
    match value[len..].chars().next() {
        None => true,
        Some(c) => !is_path_char(c),
    }
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | '{' | '}')
}

fn scan_source(src: &str) -> Partial {
    let tokens = tokenize(src);
    let mut part = Partial {
        routes: BTreeSet::new(),
        unrecognized: Vec::new(),
        others: Vec::new(),
    };
    let mut i = 0;
    while i < tokens.len() {
        if let Some((call, paren)) = call_at(&tokens, i) {
            let end = call_end(&tokens, paren);
            match call {
                Call::Route => classify_route(&tokens[paren + 1..end], &mut part),
                Call::Nest => flag_forbidden("nest", &tokens[paren + 1..end], &mut part),
                Call::Concat => flag_forbidden("concat!", &tokens[paren + 1..end], &mut part),
                Call::Format => flag_forbidden("format!", &tokens[paren + 1..end], &mut part),
            }
            i = end;
            continue;
        }
        if let Tok::Str(value) = &tokens[i]
            && value.contains("/api/v1")
        {
            part.others.push(value.clone());
        }
        i += 1;
    }
    part
}

fn classify_route(body: &[Tok], part: &mut Partial) {
    match body.first() {
        Some(Tok::Str(path)) if path.contains("/api/v1") => {
            part.routes.insert(path.clone());
        }
        Some(Tok::Str(_)) => {}
        _ => flag_forbidden("non-literal route", body, part),
    }
}

fn flag_forbidden(kind: &str, body: &[Tok], part: &mut Partial) {
    for tok in body {
        if let Tok::Str(value) = tok
            && value.contains("/api/v1")
        {
            part.unrecognized
                .push(format!("{kind}: {}", preview(value)));
        }
    }
}

fn call_at(tokens: &[Tok], i: usize) -> Option<(Call, usize)> {
    if let Tok::Punct('.') = tokens.get(i)?
        && let Tok::Ident(name) = tokens.get(i + 1)?
        && let Tok::Punct('(') = tokens.get(i + 2)?
    {
        return match name.as_str() {
            "route" => Some((Call::Route, i + 2)),
            "nest" => Some((Call::Nest, i + 2)),
            _ => None,
        };
    }
    if let Tok::Ident(name) = tokens.get(i)?
        && let Tok::Punct('!') = tokens.get(i + 1)?
        && let Tok::Punct('(') = tokens.get(i + 2)?
    {
        return match name.as_str() {
            "concat" => Some((Call::Concat, i + 2)),
            "format" => Some((Call::Format, i + 2)),
            _ => None,
        };
    }
    None
}

fn call_end(tokens: &[Tok], paren: usize) -> usize {
    let mut depth = 0;
    for (offset, tok) in tokens.iter().enumerate().skip(paren) {
        match tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    return offset + 1;
                }
            }
            _ => {}
        }
    }
    tokens.len()
}

fn tokenize(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == 'r'
            && let Some((end, content)) = raw_string(&chars, i)
        {
            out.push(Tok::Str(content));
            i = end;
            continue;
        }
        if c == '"' {
            let (end, content) = cooked_string(&chars, i);
            out.push(Tok::Str(content));
            i = end;
            continue;
        }
        if c == '\'' {
            i = skip_tick(&chars, i);
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        if "()!.,:".contains(c) {
            out.push(Tok::Punct(c));
        }
        i += 1;
    }
    out
}

fn raw_string(chars: &[char], i: usize) -> Option<(usize, String)> {
    if chars.get(i) != Some(&'r') {
        return None;
    }
    let mut j = i + 1;
    let mut hashes = 0;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    if chars.get(j) != Some(&'"') {
        return None;
    }
    j += 1;
    let start = j;
    while j < chars.len() {
        if chars[j] == '"' {
            let tail = &chars[j + 1..];
            if tail.len() >= hashes && tail[..hashes].iter().all(|c| *c == '#') {
                return Some((j + 1 + hashes, chars[start..j].iter().collect()));
            }
        }
        j += 1;
    }
    None
}

fn cooked_string(chars: &[char], start: usize) -> (usize, String) {
    let mut i = start + 1;
    let mut value = String::new();
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            value.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if chars[i] == '"' {
            return (i + 1, value);
        }
        value.push(chars[i]);
        i += 1;
    }
    (chars.len(), value)
}

fn skip_tick(chars: &[char], i: usize) -> usize {
    let mut j = i + 1;
    if j >= chars.len() {
        return j;
    }
    if chars[j] == '\\' {
        j += 1;
        if j < chars.len() {
            j += 1;
        }
        if chars.get(j) == Some(&'\'') {
            j += 1;
        }
        return j;
    }
    if chars.get(j + 1) == Some(&'\'') {
        return j + 2;
    }
    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    j.max(i + 1)
}
