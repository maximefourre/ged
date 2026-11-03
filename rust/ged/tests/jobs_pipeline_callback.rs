//! T9: `DeliverCallbackJob`: payload, SSRF decision, IP pinning, retries.
//! `policy::callback_url` belongs to T7 and is not in this base, so the
//! decision is a stub honouring the wave-6 contract:
//! `decide(url, resolve: true) -> Result<pinned address, refusal reason>`.

mod pipeline_support;
mod support;

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use ged::jobs::deliver_callback::{CallbackPolicy, DeliverCallbackJob};
use ged::jobs::heartbeat::HeartbeatConfig;
use ged::jobs::pool::{Runner, RunnerConfig};
use ged::jobs::queue::{self, JobPayload, QueueName};
use ged::jobs::{Job, JobError, Registry, polynomial_backoff};
use pipeline_support::{Fixture, Hook, hello_sniffer, parse_request};
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;

/// Pins `ip`, recording every URL it was asked about.
struct Pin {
    ip: IpAddr,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Pin {
    fn loopback() -> (Arc<Self>, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                ip: "127.0.0.1".parse().unwrap(),
                asked: asked.clone(),
            }),
            asked,
        )
    }
}

impl CallbackPolicy for Pin {
    fn decide(&self, url: &str) -> BoxFuture<'static, Result<IpAddr, String>> {
        self.asked.lock().unwrap().push(url.to_owned());
        let ip = self.ip;
        Box::pin(async move { Ok(ip) })
    }
}

struct Refuse(&'static str);

impl CallbackPolicy for Refuse {
    fn decide(&self, _url: &str) -> BoxFuture<'static, Result<IpAddr, String>> {
        let reason = self.0.to_owned();
        Box::pin(async move { Err(reason) })
    }
}

fn args(id: Uuid) -> Value {
    json!({ "upload_request_id": id.to_string() })
}

async fn completed_request(f: &Fixture, url: &str) -> (Uuid, Uuid) {
    let dept = f.department("Compta", None).await;
    let id = f.upload_request(dept, "pending", url, None).await;
    let doc: Uuid = sqlx::query_scalar(
        "INSERT INTO documents (department_id, filename, content_type, byte_size, uploaded_by, tags, \
         uploaded_at, stored_async, created_at, updated_at) \
         VALUES ($1, 'report.txt', 'text/plain', 3, 'client-a', '{\"type\": \"facture\"}', \
         '2026-11-01 10:00:00.5', true, '2026-11-01 10:00:01', '2026-11-01 10:00:02') RETURNING id",
    )
    .bind(dept)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE upload_requests SET status = 'completed', document_id = $2 WHERE id = $1")
        .bind(id)
        .bind(doc)
        .execute(&f.pool)
        .await
        .unwrap();
    (id, doc)
}

#[tokio::test]
async fn it_posts_the_documented_payload_for_a_completed_request() {
    let f = Fixture::new().await;
    let hook = Hook::start(200).await;
    let (id, doc) = completed_request(&f, &hook.url()).await;
    let (policy, _) = Pin::loopback();

    DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await
        .unwrap();

    let requests = hook.requests();
    assert_eq!(requests.len(), 1);
    let (line, headers, body) = parse_request(&requests[0]);
    assert_eq!(line, "POST /hook HTTP/1.1");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{headers}"
    );
    // key order is the Rails hash order
    let keys: Vec<_> = serde_json::from_str::<serde_json::Map<String, Value>>(&body)
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        [
            "request_id",
            "status",
            "document_id",
            "error_message",
            "tags",
            "metadata"
        ]
    );
    let payload: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        payload,
        json!({
            "request_id": id.to_string(),
            "status": "completed",
            "document_id": doc.to_string(),
            "error_message": null,
            "tags": {"type": "facture"},
            "metadata": {
                "uploaded_at": "2026-11-01T10:00:00.500Z",
                "updated_at": "2026-11-01T10:00:02.000Z",
                "upload_mode": "async"
            }
        })
    );
    f.finish().await;
}

#[tokio::test]
async fn a_failed_request_carries_its_error_and_null_tags_and_metadata() {
    let f = Fixture::new().await;
    let hook = Hook::start(204).await;
    let dept = f.department("Compta", None).await;
    let id = f.upload_request(dept, "failed", &hook.url(), None).await;
    sqlx::query("UPDATE upload_requests SET error_message = 'staged file lost before processing' WHERE id = $1")
        .bind(id)
        .execute(&f.pool)
        .await
        .unwrap();
    let (policy, _) = Pin::loopback();

    DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await
        .unwrap();

    let (_, _, body) = parse_request(&hook.requests()[0]);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({
            "request_id": id.to_string(),
            "status": "failed",
            "document_id": null,
            "error_message": "staged file lost before processing",
            "tags": null,
            "metadata": null
        })
    );
    f.finish().await;
}

/// Plan T9: one resolution, the vetted address pinned, Host kept. The name
/// below resolves nowhere: reaching the hook proves the pin was used.
#[tokio::test]
async fn it_connects_to_the_pinned_address_and_keeps_the_host() {
    let f = Fixture::new().await;
    let hook = Hook::start(200).await;
    let url = format!("http://callback.invalid:{}/hook", hook.addr.port());
    let (id, _) = completed_request(&f, &url).await;
    let (policy, asked) = Pin::loopback();

    DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await
        .unwrap();

    assert_eq!(
        asked.lock().unwrap().as_slice(),
        std::slice::from_ref(&url),
        "decided once, on the stored URL"
    );
    let (_, headers, _) = parse_request(&hook.requests()[0]);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains(&format!("host: callback.invalid:{}", hook.addr.port())),
        "{headers}"
    );
    f.finish().await;
}

#[tokio::test]
async fn tls_still_names_the_original_host_in_the_sni() {
    let f = Fixture::new().await;
    let (addr, hello) = hello_sniffer().await;
    let url = format!("https://callback.invalid:{}/hook", addr.port());
    let (id, _) = completed_request(&f, &url).await;
    let (policy, _) = Pin::loopback();

    let result = DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await;

    assert!(matches!(result, Err(JobError::Retry(_))), "{result:?}");
    let bytes = hello.await.unwrap();
    assert_eq!(bytes.first(), Some(&0x16), "a TLS handshake record");
    assert!(
        bytes
            .windows(b"callback.invalid".len())
            .any(|w| w == b"callback.invalid"),
        "SNI carries the hostname, not the pinned IP"
    );
    f.finish().await;
}

#[tokio::test]
async fn a_refused_target_is_neither_contacted_nor_retried() {
    let f = Fixture::new().await;
    let hook = Hook::start(200).await;
    let (id, _) = completed_request(&f, &hook.url()).await;
    let job = DeliverCallbackJob::new(
        f.env.clone(),
        Arc::new(Refuse("must not target a private or internal address")),
    );

    assert_eq!(
        job.perform(args(id)).await,
        Ok(()),
        "refusal is final, not Retry"
    );
    assert!(hook.requests().is_empty());
    f.finish().await;
}

#[tokio::test]
async fn a_non_success_status_is_a_retryable_failure() {
    let f = Fixture::new().await;
    let hook = Hook::start(500).await;
    let (id, _) = completed_request(&f, &hook.url()).await;
    let (policy, _) = Pin::loopback();

    let result = DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await;

    assert_eq!(
        result,
        Err(JobError::Retry("callback returned HTTP 500".into()))
    );
    f.finish().await;
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let f = Fixture::new().await;
    let hook = Hook::start(302).await;
    let (id, _) = completed_request(&f, &hook.url()).await;
    let (policy, asked) = Pin::loopback();

    let result = DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await;

    assert_eq!(
        result,
        Err(JobError::Retry("callback returned HTTP 302".into()))
    );
    assert_eq!(hook.requests().len(), 1);
    assert_eq!(asked.lock().unwrap().len(), 1, "no unvetted second hop");
    f.finish().await;
}

#[tokio::test]
async fn a_connection_error_is_a_retryable_failure() {
    let f = Fixture::new().await;
    // nothing listens on this port any more
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (id, _) = completed_request(&f, &format!("http://{dead}/hook")).await;
    let (policy, _) = Pin::loopback();

    let result = DeliverCallbackJob::new(f.env.clone(), policy)
        .perform(args(id))
        .await;

    assert!(matches!(result, Err(JobError::Retry(_))), "{result:?}");
    f.finish().await;
}

#[tokio::test]
async fn an_unknown_request_or_bad_args_is_discarded() {
    let f = Fixture::new().await;
    let (policy, _) = Pin::loopback();
    let job = DeliverCallbackJob::new(f.env.clone(), policy);
    assert!(matches!(
        job.perform(args(Uuid::new_v4())).await,
        Err(JobError::Discard(_))
    ));
    assert!(matches!(
        job.perform(json!({"upload_request_id": 3})).await,
        Err(JobError::Discard(_))
    ));
    f.finish().await;
}

#[tokio::test]
async fn it_retries_five_times_with_polynomial_backoff() {
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@localhost/unused")
        .unwrap();
    let env = ged::jobs::env::PipelineEnv::from_config(db, None);
    let job = DeliverCallbackJob::new(env, Arc::new(Refuse("x")));
    assert_eq!(job.kind(), "DeliverCallbackJob");
    assert_eq!(job.max_attempts(), 5);
    let waits: Vec<_> = (1..5).map(|a| job.backoff(a)).collect();
    assert_eq!(waits, (1..5).map(polynomial_backoff).collect::<Vec<_>>());
    assert_eq!(waits[0], Duration::from_secs(3));
}

/// Delegates to the real job but without waiting between attempts.
struct Instant(DeliverCallbackJob);

impl Job for Instant {
    fn kind(&self) -> &'static str {
        self.0.kind()
    }
    fn max_attempts(&self) -> i32 {
        self.0.max_attempts()
    }
    fn backoff(&self, _attempt: i32) -> Duration {
        Duration::ZERO
    }
    fn perform(&self, args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        self.0.perform(args)
    }
    fn exhausted(&self, args: &Value, error: &str) {
        self.0.exhausted(args, error);
    }
}

#[tokio::test]
async fn the_queue_gives_up_after_exactly_five_attempts() {
    let f = Fixture::new().await;
    let hook = Hook::start(503).await;
    let (id, _) = completed_request(&f, &hook.url()).await;
    let (policy, _) = Pin::loopback();
    let mut registry = Registry::new();
    registry.register(Arc::new(Instant(DeliverCallbackJob::new(
        f.env.clone(),
        policy,
    ))));
    let mut tx = f.pool.begin().await.unwrap();
    queue::enqueue(
        &mut tx,
        JobPayload::new("DeliverCallbackJob", args(id)),
        QueueName::Default,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let (stop, rx) = watch::channel(false);
    let runner = Runner::new(
        f.pool.clone(),
        Arc::new(registry),
        RunnerConfig {
            queue: QueueName::Default,
            threads: 1,
            polling_interval: Duration::from_millis(20),
            heartbeat: HeartbeatConfig::default(),
            shutdown_timeout: Duration::from_secs(5),
        },
    );
    let handle = tokio::spawn(async move { runner.run(rx).await });
    for _ in 0..200 {
        if f.count("SELECT count(*) FROM jobs WHERE failed_at IS NOT NULL")
            .await
            == 1
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop.send(true).unwrap();
    handle.await.unwrap().unwrap();

    assert_eq!(hook.requests().len(), 5);
    assert_eq!(f.count("SELECT attempts::bigint FROM jobs").await, 5);
    f.finish().await;
}
