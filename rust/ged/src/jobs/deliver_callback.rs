//! `DeliverCallbackJob` (queue `default`): POSTs the outcome of an async
//! upload to the caller's `callback_url`. Port of
//! `app/jobs/deliver_callback_job.rb`.
//!
//! SSRF barrier: the destination is vetted **once** by a [`CallbackPolicy`]
//! (`policy::callback_url::decide(url, resolve: true)`), which returns the
//! address it approved. The connection is then pinned to exactly that address
//! (no second resolution, so no rebinding window) while the URL keeps its
//! hostname for `Host`, SNI and certificate checks. A refused or unresolvable
//! target is final: logged, counted as `refused`, never retried.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use uuid::Uuid;

use super::env::PipelineEnv;
use super::store_document::request_id;
use super::{Job, JobError, timed};
use crate::domain::document::Document;
use crate::telemetry::{self, CallbackOutcome, JobClass, JobQueue};

pub const KIND: &str = "DeliverCallbackJob";
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ATTEMPTS: i32 = 5;

/// The SSRF decision for a delivery-time URL (resolution included).
///
/// `Ok(ip)`: the address to pin. `Err(reason)`: the refusal message
/// (`must not target a private or internal address`, ...). Production wiring
/// adapts `policy::callback_url::decide(url, true)` to this.
pub trait CallbackPolicy: Send + Sync + 'static {
    fn decide(&self, url: &str) -> BoxFuture<'static, Result<IpAddr, String>>;
}

/// Fail-closed stand-in until `policy::callback_url` (T7) is wired: every
/// delivery is refused (logged, counted `refused`, not retried), never sent.
pub struct PolicyNotWired;

impl CallbackPolicy for PolicyNotWired {
    fn decide(&self, _url: &str) -> BoxFuture<'static, Result<IpAddr, String>> {
        Box::pin(async { Err("is not vetted: the callback policy is not wired".to_owned()) })
    }
}

pub struct DeliverCallbackJob {
    env: PipelineEnv,
    policy: std::sync::Arc<dyn CallbackPolicy>,
}

impl DeliverCallbackJob {
    pub fn new(env: PipelineEnv, policy: std::sync::Arc<dyn CallbackPolicy>) -> Self {
        Self { env, policy }
    }
}

impl Job for DeliverCallbackJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    /// `retry_on DeliveryFailed, attempts: 5`.
    fn max_attempts(&self) -> i32 {
        MAX_ATTEMPTS
    }

    fn perform(&self, args: Value) -> BoxFuture<'static, Result<(), JobError>> {
        let env = self.env.clone();
        let policy = self.policy.clone();
        Box::pin(timed(
            JobClass::DeliverCallbackJob,
            JobQueue::Default,
            async move { deliver(&env, policy.as_ref(), request_id(&args)?).await },
        ))
    }

    fn exhausted(&self, args: &Value, error: &str) {
        telemetry::callback_deliveries(CallbackOutcome::Exhausted);
        let id = args
            .get("upload_request_id")
            .and_then(Value::as_str)
            .unwrap_or("?");
        eprintln!("Callback delivery permanently failed for upload_request {id}: {error}");
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    callback_url: String,
    status: String,
    document_id: Option<Uuid>,
    error_message: Option<String>,
}

async fn deliver(env: &PipelineEnv, policy: &dyn CallbackPolicy, id: Uuid) -> Result<(), JobError> {
    let retry = |e: sqlx::Error| JobError::Retry(e.to_string());
    let row: Option<Row> = sqlx::query_as(
        "SELECT callback_url, status, document_id, error_message FROM upload_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&env.db)
    .await
    .map_err(retry)?;
    let row = row.ok_or_else(|| JobError::Discard(format!("upload request {id} not found")))?;

    let ip = match policy.decide(&row.callback_url).await {
        Ok(ip) => ip,
        Err(reason) => {
            telemetry::callback_deliveries(CallbackOutcome::Refused);
            eprintln!("Callback refused for upload_request {id}: callback_url {reason}");
            return Ok(());
        }
    };

    // `upload_request.document&.tags`: a trashed document reads as nil.
    let document = match row.document_id {
        Some(doc) => {
            let mut conn = env.db.acquire().await.map_err(retry)?;
            Document::find(&mut conn, doc).await.map_err(retry)?
        }
        None => None,
    };
    let payload = json!({
        "request_id": id.to_string(),
        "status": row.status,
        "document_id": row.document_id.map(|d| d.to_string()),
        "error_message": row.error_message,
        "tags": document.as_ref().map(|d| d.tags.clone()),
        "metadata": document.as_ref().map(Document::metadata_hash),
    });

    let url = reqwest::Url::parse(&row.callback_url)
        .map_err(|e| JobError::Discard(format!("invalid callback_url: {e}")))?;
    let outcome = telemetry::trace_outgoing_http("POST", &row.callback_url, async {
        let result = post(&url, ip, &payload).await;
        (result.as_ref().copied().unwrap_or(0), result)
    })
    .await;
    match outcome {
        Ok(status) if (200..300).contains(&status) => {
            telemetry::callback_deliveries(CallbackOutcome::Success);
            Ok(())
        }
        Ok(status) => Err(fail_delivery(format!("callback returned HTTP {status}"))),
        Err(message) => Err(fail_delivery(message)),
    }
}

/// Failures are counted per attempt; the last one also emits `exhausted`.
fn fail_delivery(message: String) -> JobError {
    telemetry::callback_deliveries(CallbackOutcome::Failure);
    JobError::Retry(message)
}

/// One POST to `ip`, hostname kept for `Host`/SNI/certificate, no redirect
/// followed, no proxy (either would walk around the vetted address).
async fn post(url: &reqwest::Url, ip: IpAddr, payload: &Value) -> Result<u16, String> {
    let mut client = reqwest::Client::builder()
        .connect_timeout(OPEN_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let (Some(host), Some(port)) = (url.domain(), url.port_or_known_default()) {
        client = client.resolve(host, SocketAddr::new(ip, port));
    }
    let client = client.build().map_err(|e| e.to_string())?;
    let response = client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload.to_string())
        .send()
        .await
        .map_err(|e| describe(&e))?;
    Ok(response.status().as_u16())
}

/// `"#{e.class}: #{e.message}"`, with the cause chain reqwest hides.
fn describe(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
