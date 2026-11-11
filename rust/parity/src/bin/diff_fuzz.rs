//! Differential fuzzer: the same seeded `/api/v1` traffic against a Rails stack
//! and a Rust stack, with shared SQL fixtures and the same signing key.
//!
//! ```text
//! cargo run -p parity --bin diff_fuzz -- \
//!   --rails-url http://localhost:3440 --rust-url http://localhost:3040 \
//!   --rails-db postgres://postgres:postgres@localhost:5872/ged_production \
//!   --rust-db postgres://postgres:postgres@localhost:5472/ged_production \
//!   --oauth-url http://localhost:9332 --signing-key /tmp/signing_key.pem \
//!   --seed 13 --count 100000
//! ```
//!
//! Exit 0 when every divergence is a documented gap. Exit 1 when an
//! undocumented one remains. Exit 2 when the harness itself fails.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode as sign_jwt};
use parity::diff_fuzz::{
    CLIENT_ID, Diff, Generator, Ids, Normalized, Request, Target, compare, documented_gap,
    encode as encode_request, normalize, seed_upload, smaller_candidates,
};
use serde_json::{Value, json};
use sqlx::PgPool;

const WINDOW: usize = 24;

struct Args {
    rails_url: String,
    rust_url: String,
    rails_db: String,
    rust_db: String,
    oauth_url: String,
    signing_key: String,
    issuer: String,
    audience: String,
    seed: u64,
    count: u64,
    keep_going: usize,
    reset_every: u64,
    report: Option<String>,
}

struct Tokens {
    valid: String,
    expired: String,
    bad_aud: String,
}

#[derive(Default)]
struct Side {
    seed_doc: String,
    created_docs: Vec<String>,
    created_uploads: Vec<String>,
}

struct RawResp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Campaign {
    client: reqwest::Client,
    rails_url: String,
    rust_url: String,
    rails_db: PgPool,
    rust_db: PgPool,
    key: EncodingKey,
    kid: String,
    issuer: String,
    audience: String,
    tokens: Tokens,
}

enum Outcome {
    Clean {
        requests: u64,
        documented: u64,
    },
    Diverged {
        requests: u64,
        documented: u64,
        undocumented: usize,
    },
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(Outcome::Clean {
            requests,
            documented,
        }) => {
            println!("result=ok requests={requests} documented={documented} undocumented=0");
            0
        }
        Ok(Outcome::Diverged {
            requests,
            documented,
            undocumented,
        }) => {
            println!(
                "result=diverged requests={requests} documented={documented} undocumented={undocumented}"
            );
            1
        }
        Err(err) => {
            eprintln!("diff_fuzz: {err}");
            2
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<Outcome, String> {
    let args = parse_args()?;
    let pem = std::fs::read_to_string(&args.signing_key)
        .map_err(|e| format!("read signing key {}: {e}", args.signing_key))?;
    let key = EncodingKey::from_rsa_pem(pem.as_bytes()).map_err(|e| format!("signing key: {e}"))?;
    let kid = jwks_kid(&args.oauth_url).await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let rails_db = connect(&args.rails_db).await?;
    let rust_db = connect(&args.rust_db).await?;
    let mut campaign = Campaign {
        client,
        rails_url: args.rails_url.clone(),
        rust_url: args.rust_url.clone(),
        rails_db,
        rust_db,
        key,
        kid,
        issuer: args.issuer.clone(),
        audience: args.audience.clone(),
        tokens: Tokens {
            valid: String::new(),
            expired: String::new(),
            bad_aud: String::new(),
        },
    };
    campaign.refresh_tokens()?;
    let started = Instant::now();
    println!(
        "diff_fuzz seed={} count={} rails={} rust={} issuer={}",
        args.seed, args.count, args.rails_url, args.rust_url, args.issuer
    );

    let mut rails = Side::default();
    let mut rust = Side::default();
    campaign.reset_world(&mut rails, &mut rust).await?;

    let mut generated = Generator::new(args.seed);
    let mut since_reset: Vec<Request> = Vec::new();
    let mut documented = 0u64;
    let mut signatures = BTreeSet::new();
    let mut report = String::new();

    for index in 0..args.count {
        if index > 0 && args.reset_every > 0 && index % args.reset_every == 0 {
            campaign.reset_world(&mut rails, &mut rust).await?;
            since_reset.clear();
        }
        if index > 0 && index % 2000 == 0 {
            campaign.refresh_tokens()?;
        }
        let Some(req) = generated.next() else {
            return Err("generator ended".into());
        };
        since_reset.push(req.clone());
        let (raw_rails, raw_rust) = campaign.send_both(&req, &rails, &rust).await?;
        let norm_rails = normalize(raw_rails.status, &raw_rails.headers, &raw_rails.body);
        let norm_rust = normalize(raw_rust.status, &raw_rust.headers, &raw_rust.body);
        if let Some(diff) = compare(&norm_rails, &norm_rust) {
            let target = encode_request(&req, &ids_for(&campaign.tokens, &rails));
            if let Some(reason) = documented_gap(&target, &norm_rails, &norm_rust) {
                documented += 1;
                if signatures.insert(format!("doc:{reason}:{}", req.category)) {
                    let line = format!("documented index={index} {reason} {}", req.summary());
                    eprintln!("{line}");
                    report.push_str(&line);
                    report.push('\n');
                }
            } else {
                let sig = signature(&req, &diff);
                let fresh = signatures.insert(sig.clone());
                if fresh {
                    let minimal = campaign
                        .minimize_case(&since_reset, &req, &norm_rails, &norm_rust)
                        .await?;
                    let block = format_divergence(args.seed, index, &req, &diff, &minimal);
                    eprintln!("{block}");
                    report.push_str(&block);
                    report.push('\n');
                }
                // Minimizing resets both databases. Replay what this window
                // already did so the two sides stay aligned for the next call.
                campaign
                    .restore(&since_reset, &mut rails, &mut rust)
                    .await?;
                let undocumented = signatures.iter().filter(|s| !s.starts_with("doc:")).count();
                if undocumented > args.keep_going {
                    write_report(args.report.as_deref(), &report)?;
                    let elapsed = started.elapsed().as_secs();
                    println!(
                        "seed={} elapsed={elapsed}s stopped_at={} documented={documented}",
                        args.seed,
                        index + 1
                    );
                    return Ok(Outcome::Diverged {
                        requests: index + 1,
                        documented,
                        undocumented,
                    });
                }
                continue;
            }
        }
        note_created(&raw_rails.body, &raw_rust.body, &mut rails, &mut rust);
        if index > 0 && index % 1000 == 0 {
            eprintln!(
                "progress {index}/{} documented={documented} undocumented={}",
                args.count,
                signatures.iter().filter(|s| !s.starts_with("doc:")).count()
            );
        }
    }

    write_report(args.report.as_deref(), &report)?;
    let undocumented = signatures.iter().filter(|s| !s.starts_with("doc:")).count();
    println!(
        "seed={} elapsed={}s requests={} documented={documented} undocumented={undocumented}",
        args.seed,
        started.elapsed().as_secs(),
        args.count
    );
    if undocumented == 0 {
        Ok(Outcome::Clean {
            requests: args.count,
            documented,
        })
    } else {
        Ok(Outcome::Diverged {
            requests: args.count,
            documented,
            undocumented,
        })
    }
}

impl Campaign {
    fn refresh_tokens(&mut self) -> Result<(), String> {
        let now = unix_now()?;
        self.tokens.valid = sign(
            &self.key,
            &self.kid,
            &self.issuer,
            &self.audience,
            now,
            now + 7200,
        )?;
        self.tokens.expired = sign(
            &self.key,
            &self.kid,
            &self.issuer,
            &self.audience,
            now - 120,
            now - 60,
        )?;
        self.tokens.bad_aud = sign(
            &self.key,
            &self.kid,
            &self.issuer,
            "http://wrong.example/aud",
            now,
            now + 7200,
        )?;
        Ok(())
    }

    async fn reset_world(&self, rails: &mut Side, rust: &mut Side) -> Result<(), String> {
        install(&self.rails_db)
            .await
            .map_err(|e| format!("rails fixtures: {e}"))?;
        install(&self.rust_db)
            .await
            .map_err(|e| format!("rust fixtures: {e}"))?;
        *rails = Side::default();
        *rust = Side::default();
        let req = seed_upload();
        let (raw_rails, raw_rust) = self.send_both(&req, rails, rust).await?;
        let norm_rails = normalize(raw_rails.status, &raw_rails.headers, &raw_rails.body);
        let norm_rust = normalize(raw_rust.status, &raw_rust.headers, &raw_rust.body);
        if let Some(diff) = compare(&norm_rails, &norm_rust) {
            return Err(format!(
                "seed upload diverged\n{}",
                format_divergence(0, 0, &req, &diff, &[])
            ));
        }
        if raw_rails.status != 201 || raw_rust.status != 201 {
            return Err(format!(
                "seed upload status rails={} rust={} rails_body={} rust_body={}",
                raw_rails.status,
                raw_rust.status,
                excerpt(&norm_rails.body, 300),
                excerpt(&norm_rust.body, 300)
            ));
        }
        rails.seed_doc = json_str(&raw_rails.body, "id").ok_or("rails seed response has no id")?;
        rust.seed_doc = json_str(&raw_rust.body, "id").ok_or("rust seed response has no id")?;
        Ok(())
    }

    async fn restore(
        &self,
        since_reset: &[Request],
        rails: &mut Side,
        rust: &mut Side,
    ) -> Result<(), String> {
        self.reset_world(rails, rust).await?;
        for req in since_reset {
            let (raw_rails, raw_rust) = self.send_both(req, rails, rust).await?;
            note_created(&raw_rails.body, &raw_rust.body, rails, rust);
        }
        Ok(())
    }

    async fn minimize_case(
        &self,
        since_reset: &[Request],
        req: &Request,
        rails_norm: &Normalized,
        rust_norm: &Normalized,
    ) -> Result<Vec<String>, String> {
        let window: Vec<Request> = since_reset
            .iter()
            .rev()
            .take(WINDOW)
            .rev()
            .cloned()
            .collect();
        if self.isolated_diverges(std::slice::from_ref(req)).await? {
            let shrunk = self.shrink_request(req.clone(), &[]).await?;
            return Ok(vec![shrunk.summary()]);
        }
        let (shrunk, reproduced) = self.shrink_window(window).await?;
        if !reproduced {
            return Ok(vec![format!(
                "not reproduced after reset; original rails={} rust={} body_rails={} body_rust={}",
                rails_norm.status,
                rust_norm.status,
                excerpt(&rails_norm.body, 200),
                excerpt(&rust_norm.body, 200)
            )]);
        }
        let Some(last) = shrunk.last().cloned() else {
            return Ok(vec!["empty window".into()]);
        };
        let prefix = &shrunk[..shrunk.len() - 1];
        let last = self.shrink_request(last, prefix).await?;
        let mut lines: Vec<String> = prefix.iter().map(Request::summary).collect();
        lines.push(last.summary());
        Ok(lines)
    }

    async fn shrink_request(
        &self,
        mut req: Request,
        prefix: &[Request],
    ) -> Result<Request, String> {
        if !self.diverges_with_prefix(prefix, &req).await? {
            return Ok(req);
        }
        loop {
            let mut next = None;
            for trial in smaller_candidates(&req) {
                if self.diverges_with_prefix(prefix, &trial).await? {
                    next = Some(trial);
                    break;
                }
            }
            match next {
                Some(trial) => req = trial,
                None => break,
            }
        }
        Ok(req)
    }

    async fn shrink_window(&self, mut kept: Vec<Request>) -> Result<(Vec<Request>, bool), String> {
        if kept.is_empty() || !self.isolated_diverges(&kept).await? {
            return Ok((kept, false));
        }
        let mut index = 0;
        while index + 1 < kept.len() {
            let mut trial = Vec::with_capacity(kept.len() - 1);
            trial.extend(kept[..index].iter().cloned());
            trial.extend(kept[index + 1..].iter().cloned());
            if self.isolated_diverges(&trial).await? {
                kept = trial;
            } else {
                index += 1;
            }
        }
        Ok((kept, true))
    }

    async fn diverges_with_prefix(
        &self,
        prefix: &[Request],
        last: &Request,
    ) -> Result<bool, String> {
        let mut seq = Vec::with_capacity(prefix.len() + 1);
        seq.extend(prefix.iter().cloned());
        seq.push(last.clone());
        self.isolated_diverges(&seq).await
    }

    async fn isolated_diverges(&self, seq: &[Request]) -> Result<bool, String> {
        install(&self.rails_db).await?;
        install(&self.rust_db).await?;
        let mut rails = Side::default();
        let mut rust = Side::default();
        let (seed_rails, seed_rust) = self.send_both(&seed_upload(), &rails, &rust).await?;
        rails.seed_doc = json_str(&seed_rails.body, "id").unwrap_or_default();
        rust.seed_doc = json_str(&seed_rust.body, "id").unwrap_or_default();
        if seq.is_empty() {
            return Ok(false);
        }
        for req in &seq[..seq.len() - 1] {
            let (raw_rails, raw_rust) = self.send_both(req, &rails, &rust).await?;
            note_created(&raw_rails.body, &raw_rust.body, &mut rails, &mut rust);
        }
        let last = &seq[seq.len() - 1];
        let (raw_rails, raw_rust) = self.send_both(last, &rails, &rust).await?;
        let norm_rails = normalize(raw_rails.status, &raw_rails.headers, &raw_rails.body);
        let norm_rust = normalize(raw_rust.status, &raw_rust.headers, &raw_rust.body);
        if compare(&norm_rails, &norm_rust).is_none() {
            return Ok(false);
        }
        let target = encode_request(last, &ids_for(&self.tokens, &rails));
        Ok(documented_gap(&target, &norm_rails, &norm_rust).is_none())
    }

    async fn send_both(
        &self,
        req: &Request,
        rails: &Side,
        rust: &Side,
    ) -> Result<(RawResp, RawResp), String> {
        let left = encode_request(req, &ids_for(&self.tokens, rails));
        let right = encode_request(req, &ids_for(&self.tokens, rust));
        let (a, b) = tokio::join!(
            send_retry(&self.client, &self.rails_url, &left),
            send_retry(&self.client, &self.rust_url, &right),
        );
        Ok((a?, b?))
    }
}

fn ids_for<'a>(tokens: &'a Tokens, side: &'a Side) -> Ids<'a> {
    Ids {
        seed_doc: &side.seed_doc,
        created_docs: &side.created_docs,
        created_uploads: &side.created_uploads,
        token_valid: &tokens.valid,
        token_expired: &tokens.expired,
        token_bad_aud: &tokens.bad_aud,
    }
}

fn note_created(rails_body: &[u8], rust_body: &[u8], rails: &mut Side, rust: &mut Side) {
    if let (Some(left), Some(right)) = (json_str(rails_body, "id"), json_str(rust_body, "id"))
        && is_uuid(&left)
        && is_uuid(&right)
    {
        rails.created_docs.push(left);
        rust.created_docs.push(right);
    }
    if let (Some(left), Some(right)) = (
        json_str(rails_body, "request_id"),
        json_str(rust_body, "request_id"),
    ) {
        rails.created_uploads.push(left);
        rust.created_uploads.push(right);
    }
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, byte) in b.iter().enumerate() {
        let dash = i == 8 || i == 13 || i == 18 || i == 23;
        if dash {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn json_str(body: &[u8], field: &str) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value.get(field)?.as_str().map(str::to_owned)
}

async fn send_retry(
    client: &reqwest::Client,
    base: &str,
    target: &Target,
) -> Result<RawResp, String> {
    let mut last = String::new();
    for attempt in 0..2 {
        match send_once(client, base, target).await {
            Ok(resp) => return Ok(resp),
            Err(err) => {
                last = err;
                if attempt == 0 {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            }
        }
    }
    Err(last)
}

async fn send_once(
    client: &reqwest::Client,
    base: &str,
    target: &Target,
) -> Result<RawResp, String> {
    let method = reqwest::Method::from_bytes(target.method.as_bytes())
        .map_err(|e| format!("method {}: {e}", target.method))?;
    let url = format!("{}{}", base.trim_end_matches('/'), target.path_and_query);
    let mut builder = client.request(method, &url);
    for (name, value) in &target.headers {
        builder = builder.header(name, value);
    }
    if !target.body.is_empty()
        || target
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        builder = builder.body(target.body.clone());
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| format!("send {url}: {e}"))?;
    let status = resp.status().as_u16();
    let mut headers = Vec::new();
    for (name, value) in resp.headers() {
        let text = value
            .to_str()
            .map(str::to_owned)
            .unwrap_or_else(|_| "<non-utf8>".into());
        headers.push((name.as_str().to_owned(), text));
    }
    let body = resp.bytes().await.map_err(|e| format!("read {url}: {e}"))?;
    if body.len() > 5_000_000 {
        return Err(format!("{url} body is {} bytes", body.len()));
    }
    Ok(RawResp {
        status,
        headers,
        body: body.to_vec(),
    })
}

fn sign(
    key: &EncodingKey,
    kid: &str,
    issuer: &str,
    audience: &str,
    iat: i64,
    exp: i64,
) -> Result<String, String> {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_owned());
    let claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": CLIENT_ID,
        "iat": iat,
        "exp": exp,
    });
    sign_jwt(&header, &claims, key).map_err(|e| format!("sign token: {e}"))
}

async fn jwks_kid(oauth_url: &str) -> Result<String, String> {
    let url = format!("{}/.well-known/jwks.json", oauth_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("jwks client: {e}"))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    let body = resp.bytes().await.map_err(|e| format!("read {url}: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "GET {url} -> {status} {}",
            excerpt(&String::from_utf8_lossy(&body), 180)
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|e| format!("jwks json: {e}"))?;
    value
        .get("keys")
        .and_then(|k| k.get(0))
        .and_then(|k| k.get("kid"))
        .and_then(|k| k.as_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "jwks has no keys[0].kid: {}",
                excerpt(&String::from_utf8_lossy(&body), 180)
            )
        })
}

fn unix_now() -> Result<i64, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("clock: {e}"))?
        .as_secs();
    i64::try_from(secs).map_err(|e| format!("clock: {e}"))
}

async fn connect(url: &str) -> Result<PgPool, String> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .map_err(|e| format!("postgres {url}: {e}"))?;
    sqlx::query("SET lock_timeout = '8s'")
        .execute(&pool)
        .await
        .map_err(|e| format!("lock_timeout: {e}"))?;
    Ok(pool)
}

async fn install(pool: &PgPool) -> Result<(), String> {
    for sql in FIXTURES {
        if sql.starts_with("SELECT") {
            let _: i64 = sqlx::query_scalar(*sql)
                .fetch_one(pool)
                .await
                .map_err(|e| format!("{e}\n{sql}"))?;
        } else {
            sqlx::query(*sql)
                .execute(pool)
                .await
                .map_err(|e| format!("{e}\n{sql}"))?;
        }
    }
    Ok(())
}

const FIXTURES: &[&str] = &[
    "TRUNCATE TABLE upload_requests, documents, client_department_accesses, departments, app_clients RESTART IDENTITY CASCADE",
    "INSERT INTO departments (id, name, parent_id, created_at, updated_at) VALUES \
     (1, 'Fuzz Finance', NULL, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00'), \
     (2, 'Fuzz Secret', NULL, '2026-01-01 00:00:01+00', '2026-01-01 00:00:01+00'), \
     (3, 'Fuzz Payroll', 1, '2026-01-01 00:00:02+00', '2026-01-01 00:00:02+00'), \
     (4, 'Fuzz Archive', 1, '2026-01-01 00:00:03+00', '2026-01-01 00:00:03+00')",
    "SELECT setval(pg_get_serial_sequence('departments','id'), 4, true)",
    "INSERT INTO app_clients (client_identifier, description, created_at, updated_at) VALUES \
     ('fuzz-client', 'diff fuzz', '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00')",
    "INSERT INTO client_department_accesses (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
     SELECT id, 1, true, true, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00' FROM app_clients WHERE client_identifier = 'fuzz-client'",
    "INSERT INTO client_department_accesses (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
     SELECT id, 3, true, false, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00' FROM app_clients WHERE client_identifier = 'fuzz-client'",
    "INSERT INTO client_department_accesses (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
     SELECT id, 4, false, true, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00' FROM app_clients WHERE client_identifier = 'fuzz-client'",
    "INSERT INTO documents (id, department_id, filename, content_type, byte_size, uploaded_by, tags, uploaded_at, stored_async, created_at, updated_at) VALUES \
     ('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1', 1, 'invoice.pdf', 'application/pdf', 4, 'fuzz-client', '{\"type\":\"facture\",\"urgent\":null}'::jsonb, '2026-01-02 00:00:00+00', false, '2026-01-02 00:00:00+00', '2026-01-02 00:00:00+00'), \
     ('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2', 1, 'async.pdf', 'application/pdf', 4, 'fuzz-client', '{}'::jsonb, '2026-01-03 00:00:00+00', true, '2026-01-03 00:00:00+00', '2026-01-03 00:00:00+00'), \
     ('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3', 3, 'payroll.pdf', 'application/pdf', 4, 'fuzz-client', '{\"type\":\"paie\"}'::jsonb, '2026-01-04 00:00:00+00', false, '2026-01-04 00:00:00+00', '2026-01-04 00:00:00+00')",
    "INSERT INTO upload_requests (id, department_id, callback_url, filename, content_type, uploaded_by, status, error_message, tags, created_at, updated_at) VALUES \
     ('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1', 1, 'https://example.com/fuzz-cb', 'pending.pdf', 'application/pdf', 'fuzz-client', 'pending', NULL, '{}'::jsonb, '2026-01-05 00:00:00+00', '2026-01-05 00:00:00+00'), \
     ('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2', 1, 'https://example.com/fuzz-cb', 'failed.pdf', 'application/pdf', 'fuzz-client', 'failed', 'boom', '{}'::jsonb, '2026-01-06 00:00:00+00', '2026-01-06 00:00:00+00')",
];

fn signature(req: &Request, diff: &Diff) -> String {
    let headers = diff
        .headers
        .iter()
        .map(|h| h.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{} {} {}->{} [{headers}] {} || {}",
        req.category,
        req.method,
        diff.status_rails,
        diff.status_rust,
        excerpt(&diff.body_rails, 160),
        excerpt(&diff.body_rust, 160)
    )
}

fn format_divergence(
    seed: u64,
    index: u64,
    req: &Request,
    diff: &Diff,
    minimal: &[String],
) -> String {
    let mut out = format!(
        "DIVERGENCE seed={seed} index={index} category={}\n  request: {}\n",
        req.category,
        req.summary()
    );
    if minimal.is_empty() {
        out.push_str("  minimal: (none)\n");
    } else {
        out.push_str("  minimal:\n");
        for line in minimal {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "  rails {} {}\n  rust  {} {}\n",
        diff.status_rails,
        excerpt(&diff.body_rails, 700),
        diff.status_rust,
        excerpt(&diff.body_rust, 700)
    ));
    for header in &diff.headers {
        out.push_str(&format!(
            "  header {} rails={:?} rust={:?}\n",
            header.name, header.rails, header.rust
        ));
    }
    out
}

fn excerpt(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

fn write_report(path: Option<&str>, report: &str) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    std::fs::write(path, report).map_err(|e| format!("write report {path}: {e}"))
}

fn parse_args() -> Result<Args, String> {
    let mut rails_url = String::new();
    let mut rust_url = String::new();
    let mut rails_db = String::new();
    let mut rust_db = String::new();
    let mut oauth_url = String::new();
    let mut signing_key = String::new();
    let mut issuer = String::new();
    let mut audience = String::new();
    let mut seed = 1u64;
    let mut count: Option<u64> = None;
    let mut keep_going = 0usize;
    let mut reset_every = 200u64;
    let mut report = None;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--rails-url" => rails_url = need(&mut it, &flag)?,
            "--rust-url" => rust_url = need(&mut it, &flag)?,
            "--rails-db" => rails_db = need(&mut it, &flag)?,
            "--rust-db" => rust_db = need(&mut it, &flag)?,
            "--oauth-url" => oauth_url = need(&mut it, &flag)?,
            "--signing-key" => signing_key = need(&mut it, &flag)?,
            "--issuer" => issuer = need(&mut it, &flag)?,
            "--audience" => audience = need(&mut it, &flag)?,
            "--seed" => seed = parse_u64(&need(&mut it, &flag)?)?,
            "--count" => count = Some(parse_u64(&need(&mut it, &flag)?)?),
            "--keep-going" => keep_going = parse_usize(&need(&mut it, &flag)?)?,
            "--reset-every" => reset_every = parse_u64(&need(&mut it, &flag)?)?,
            "--report" => report = Some(need(&mut it, &flag)?),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if rails_url.is_empty() {
        rails_url = env_or("RAILS_BASE_URL")?;
    }
    if rust_url.is_empty() {
        rust_url = env_or("RUST_BASE_URL")?;
    }
    if rails_db.is_empty() {
        rails_db = env_or("RAILS_DATABASE_URL")?;
    }
    if rust_db.is_empty() {
        rust_db = env_or("RUST_DATABASE_URL")?;
    }
    if oauth_url.is_empty() {
        oauth_url = env_or("OAUTH_URL")?;
    }
    if signing_key.is_empty() {
        signing_key = env_or("SIGNING_KEY")?;
    }
    if issuer.is_empty() {
        issuer = "http://oauth-provider:9292".into();
    }
    if audience.is_empty() {
        audience = "http://web:80".into();
    }
    let Some(count) = count else {
        return Err(format!("--count is required\n{USAGE}"));
    };
    Ok(Args {
        rails_url,
        rust_url,
        rails_db,
        rust_db,
        oauth_url,
        signing_key,
        issuer,
        audience,
        seed,
        count,
        keep_going,
        reset_every,
        report,
    })
}

fn need(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_u64(s: &str) -> Result<u64, String> {
    s.parse::<u64>().map_err(|_| format!("not an integer: {s}"))
}

fn parse_usize(s: &str) -> Result<usize, String> {
    s.parse::<usize>()
        .map_err(|_| format!("not an integer: {s}"))
}

fn env_or(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("missing {key} (or the matching flag)\n{USAGE}"))
}

const USAGE: &str = "\
diff_fuzz --rails-url URL --rust-url URL --rails-db URL --rust-db URL \
--oauth-url URL --signing-key PATH --count N [--seed N] [--keep-going N] \
[--reset-every N] [--report PATH] [--issuer URL] [--audience URL]";
