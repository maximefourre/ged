//! Custom metrics and basic HTTP/SQL traces.
//!
//! The SDK starts only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set and
//! `OTEL_SDK_DISABLED` is not `"true"` — the same gate as
//! `config/initializers/opentelemetry.rb`. Until then every call is a no-op
//! on the process-global API, so other tasks can record unconditionally.
//! Call [`install_from_env`] once at boot, before the first record: instruments
//! bind to whatever meter is global at first use.
//!
//! Names match `lib/telemetry.rb`. Attribute values are closed enums, so a
//! filename, a tag, a department or a URL cannot be attached to a metric.
//! SQL sent on a span is obfuscated in the shape of `db_statement: :obfuscate`
//! (literals become `?`). The golden strings match the Ruby helper; inputs
//! that diverge are listed in `docs/migration/parity-gaps.md`.

use std::future::Future;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::trace::{FutureExt, SpanKind, Status as SpanStatus, TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_otlp::MetricExporter;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use regex::Regex;

/// Meter name. Rails uses `OpenTelemetry.meter_provider.meter("ged")`.
pub const METER_NAME: &str = "ged";

pub const DOCUMENTS_UPLOADED: &str = "ged.documents.uploaded";
pub const DOCUMENTS_UPLOADED_UNIT: &str = "{document}";
pub const DOCUMENTS_UPLOADED_DESCRIPTION: &str =
    "Documents created, by storage mode (attribute upload_mode)";

pub const CALLBACK_DELIVERIES: &str = "ged.callbacks.deliveries";
pub const CALLBACK_DELIVERIES_UNIT: &str = "{attempt}";
pub const CALLBACK_DELIVERIES_DESCRIPTION: &str = "Callback delivery attempts by outcome (success/failure/refused/exhausted; failure counts per attempt)";

pub const JOB_DURATION: &str = "ged.job.duration";
pub const JOB_DURATION_UNIT: &str = "s";
pub const JOB_DURATION_DESCRIPTION: &str = "Active Job perform duration";

/// `stored_async ? "async" : "sync"` in `DocumentCreator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadMode {
    Sync,
    Async,
}

impl UploadMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Async => "async",
        }
    }
}

/// `DeliverCallbackJob` outcome attribute. One increment per attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackOutcome {
    Success,
    Failure,
    Refused,
    Exhausted,
}

impl CallbackOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Refused => "refused",
            Self::Exhausted => "exhausted",
        }
    }
}

/// Job class names, identical to the Rails class names recorded on
/// `perform.active_job`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobClass {
    StoreDocumentJob,
    DeliverCallbackJob,
    ReconcileUploadRequestsJob,
    PurgeDeletedDocumentsJob,
}

impl JobClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StoreDocumentJob => "StoreDocumentJob",
            Self::DeliverCallbackJob => "DeliverCallbackJob",
            Self::ReconcileUploadRequestsJob => "ReconcileUploadRequestsJob",
            Self::PurgeDeletedDocumentsJob => "PurgeDeletedDocumentsJob",
        }
    }
}

/// Queue name recorded next to the job class (`uploads` or `default`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobQueue {
    Uploads,
    Default,
}

impl JobQueue {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uploads => "uploads",
            Self::Default => "default",
        }
    }
}

/// Rails `at_exit` passes `timeout: 3` to both provider shutdowns.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Holds the providers so shutdown runs when the process command returns.
/// Drop flushes the span batch and the metric reader, then stops them.
/// A guard from a disabled SDK does nothing.
#[must_use = "dropping the guard shuts the OpenTelemetry exporters down"]
pub struct TelemetryGuard {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
}

impl TelemetryGuard {
    fn inactive() -> Self {
        Self {
            tracer: None,
            meter: None,
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        // Tracer first, then meter: a meter shutdown can still emit spans.
        if let Some(tracer) = self.tracer.take()
            && let Err(err) = tracer.shutdown_with_timeout(SHUTDOWN_TIMEOUT)
        {
            eprintln!("ged: telemetry tracer shutdown: {err}");
        }
        if let Some(meter) = self.meter.take()
            && let Err(err) = meter.shutdown_with_timeout(SHUTDOWN_TIMEOUT)
        {
            eprintln!("ged: telemetry meter shutdown: {err}");
        }
    }
}

static SDK_ACTIVE: AtomicBool = AtomicBool::new(false);
static INSTRUMENTS: OnceLock<Instruments> = OnceLock::new();
/// Entries into a recording path, where span names, HTTP fields and SQL
/// obfuscation allocate. Stays put while the SDK is inactive.
static SPAN_PREPARATIONS: AtomicU64 = AtomicU64::new(0);

struct Instruments {
    documents_uploaded: Counter<u64>,
    callback_deliveries: Counter<u64>,
    job_duration: Histogram<f64>,
}

/// `OTEL_EXPORTER_OTLP_ENDPOINT` present and `OTEL_SDK_DISABLED != "true"`.
/// Blank (nil, empty, whitespace) counts as absent, matching Ruby `present?`.
pub fn enabled(endpoint: Option<&str>, sdk_disabled: Option<&str>) -> bool {
    endpoint.is_some_and(|value| !value.trim().is_empty()) && sdk_disabled != Some("true")
}

/// Whether this process started the OTLP SDK. False in tests and whenever
/// [`enabled`] is false — no exporter threads, no export.
pub fn sdk_active() -> bool {
    SDK_ACTIVE.load(Ordering::SeqCst)
}

/// How many times a recording path prepared a span. Tests use this to prove
/// the inactive SDK neither obfuscates SQL nor builds HTTP span strings.
pub fn span_preparations() -> u64 {
    SPAN_PREPARATIONS.load(Ordering::Relaxed)
}

fn note_span_preparation() {
    SPAN_PREPARATIONS.fetch_add(1, Ordering::Relaxed);
}

/// The recording suite installs in-memory providers without [`install`],
/// which is what sets [`sdk_active`] in a real process.
#[doc(hidden)]
pub fn mark_sdk_active_for_test(active: bool) {
    SDK_ACTIVE.store(active, Ordering::SeqCst);
}

/// Completes on SIGTERM or SIGINT. `serve` waits on this, returns, and the
/// process then drops [`TelemetryGuard`], whose [`Drop`] flushes exporters.
/// A handler that cannot be installed waits forever, so that failure does
/// not stop the server at boot.
pub async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(mut sigterm) = signal(SignalKind::terminate()) else {
        eprintln!("ged: SIGTERM handler unavailable");
        std::future::pending::<()>().await;
        return;
    };
    let Ok(mut sigint) = signal(SignalKind::interrupt()) else {
        eprintln!("ged: SIGINT handler unavailable");
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = sigterm.recv() => {}
        _ = sigint.recv() => {}
    }
}

/// Boot hook. Reads the process environment, like the Rails initializer.
pub fn install_from_env() -> TelemetryGuard {
    install(&|key| std::env::var(key).ok())
}

/// Starts the SDK when `lookup` reports an endpoint. The OTLP exporters read
/// `OTEL_EXPORTER_OTLP_*` from the process environment themselves (base URL,
/// signal path appended); `lookup` decides whether to start and which
/// service name to set.
pub fn install(lookup: &dyn Fn(&str) -> Option<String>) -> TelemetryGuard {
    let endpoint = lookup("OTEL_EXPORTER_OTLP_ENDPOINT");
    let disabled = lookup("OTEL_SDK_DISABLED");
    if !enabled(endpoint.as_deref(), disabled.as_deref()) {
        return TelemetryGuard::inactive();
    }
    // A second caller must not own shutdown; the first guard does.
    if SDK_ACTIVE.swap(true, Ordering::SeqCst) {
        return TelemetryGuard::inactive();
    }
    match start_sdk(lookup) {
        Ok(guard) => guard,
        Err(err) => {
            SDK_ACTIVE.store(false, Ordering::SeqCst);
            eprintln!("ged: OpenTelemetry disabled: {err}");
            TelemetryGuard::inactive()
        }
    }
}

fn start_sdk(lookup: &dyn Fn(&str) -> Option<String>) -> Result<TelemetryGuard, String> {
    let service = lookup("OTEL_SERVICE_NAME")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ged".to_owned());
    let resource = Resource::builder().with_service_name(service).build();

    let metric_exporter = MetricExporter::builder()
        .with_http()
        .build()
        .map_err(|err| err.to_string())?;
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_resource(resource.clone())
        .build();
    global::set_meter_provider(meter_provider.clone());

    let span_exporter = match SpanExporter::builder().with_http().build() {
        Ok(exporter) => exporter,
        Err(err) => {
            let _ = meter_provider.shutdown();
            return Err(err.to_string());
        }
    };
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource)
        .build();
    global::set_tracer_provider(tracer_provider.clone());

    Ok(TelemetryGuard {
        tracer: Some(tracer_provider),
        meter: Some(meter_provider),
    })
}

fn instruments() -> &'static Instruments {
    INSTRUMENTS.get_or_init(|| {
        let meter = global::meter(METER_NAME);
        Instruments {
            documents_uploaded: meter
                .u64_counter(DOCUMENTS_UPLOADED)
                .with_description(DOCUMENTS_UPLOADED_DESCRIPTION)
                .with_unit(DOCUMENTS_UPLOADED_UNIT)
                .build(),
            callback_deliveries: meter
                .u64_counter(CALLBACK_DELIVERIES)
                .with_description(CALLBACK_DELIVERIES_DESCRIPTION)
                .with_unit(CALLBACK_DELIVERIES_UNIT)
                .build(),
            job_duration: meter
                .f64_histogram(JOB_DURATION)
                .with_description(JOB_DURATION_DESCRIPTION)
                .with_unit(JOB_DURATION_UNIT)
                .build(),
        }
    })
}

/// `Telemetry.documents_uploaded.add(1, upload_mode:)`.
pub fn documents_uploaded(mode: UploadMode) {
    instruments()
        .documents_uploaded
        .add(1, &[KeyValue::new("upload_mode", mode.as_str())]);
}

/// `Telemetry.callback_deliveries.add(1, outcome:)`.
pub fn callback_deliveries(outcome: CallbackOutcome) {
    instruments()
        .callback_deliveries
        .add(1, &[KeyValue::new("outcome", outcome.as_str())]);
}

/// `Telemetry.job_duration.record(seconds, job.class:, queue:)`.
/// A non-finite or negative duration is ignored.
pub fn job_duration(class: JobClass, queue: JobQueue, seconds: f64) {
    if !seconds.is_finite() || seconds < 0.0 {
        return;
    }
    instruments().job_duration.record(
        seconds,
        &[
            KeyValue::new("job.class", class.as_str()),
            KeyValue::new("queue", queue.as_str()),
        ],
    );
}

/// Outermost HTTP span for one request. Installed by [`wrap`].
/// While the SDK is inactive this forwards the request and allocates nothing.
pub async fn trace_http(req: Request, next: Next) -> Response {
    if !sdk_active() {
        return next.run(req).await;
    }
    trace_http_recording(req, next).await
}

async fn trace_http_recording(req: Request, next: Next) -> Response {
    note_span_preparation();
    let method = bounded_method(req.method().as_str());
    let path = req.uri().path().to_owned();
    let tracer = global::tracer(METER_NAME);
    let span = tracer
        .span_builder(format!("HTTP {method}"))
        .with_kind(SpanKind::Server)
        .start(&tracer);
    let cx = Context::current_with_span(span);
    let response = next.run(req).with_context(cx.clone()).await;
    let span = cx.span();
    span.set_attribute(KeyValue::new("http.request.method", method));
    span.set_attribute(KeyValue::new("url.path", path));
    span.set_attribute(KeyValue::new(
        "http.response.status_code",
        i64::from(response.status().as_u16()),
    ));
    // Rack: `Status.error` (empty description) when the status is 500..=599.
    // A 4xx stays unset, so the dashboard's STATUS_CODE_ERROR rate counts
    // server failures only.
    if response.status().is_server_error() {
        span.set_status(SpanStatus::error(""));
    }
    span.end();
    response
}

/// Runs `fut` under a client span. `target` may be a URL; only the host is
/// recorded (no userinfo, path or query). The status is the HTTP status the
/// attempt observed.
pub async fn trace_outgoing_http<T>(
    method: &str,
    target: &str,
    fut: impl Future<Output = (u16, T)>,
) -> T {
    if !sdk_active() {
        return fut.await.1;
    }
    trace_outgoing_http_recording(method, target, fut).await
}

async fn trace_outgoing_http_recording<T>(
    method: &str,
    target: &str,
    fut: impl Future<Output = (u16, T)>,
) -> T {
    note_span_preparation();
    let method = bounded_method(method);
    let host = host_only(target);
    let tracer = global::tracer(METER_NAME);
    let span = tracer
        .span_builder(format!("HTTP {method}"))
        .with_kind(SpanKind::Client)
        .start(&tracer);
    let cx = Context::current_with_span(span);
    let (status, value) = fut.with_context(cx.clone()).await;
    let span = cx.span();
    span.set_attribute(KeyValue::new("http.request.method", method));
    if !host.is_empty() {
        span.set_attribute(KeyValue::new("server.address", host.to_owned()));
    }
    span.set_attribute(KeyValue::new(
        "http.response.status_code",
        i64::from(status),
    ));
    span.end();
    value
}

/// Runs `fut` under a Postgres span whose statement has had literals removed.
/// While the SDK is inactive this awaits `fut` and does not obfuscate.
pub async fn trace_sql<T>(statement: &str, fut: impl Future<Output = T>) -> T {
    if !sdk_active() {
        return fut.await;
    }
    trace_sql_recording(statement, fut).await
}

async fn trace_sql_recording<T>(statement: &str, fut: impl Future<Output = T>) -> T {
    note_span_preparation();
    let statement = obfuscate_sql(statement);
    let tracer = global::tracer(METER_NAME);
    let span = tracer.start("postgresql");
    let cx = Context::current_with_span(span);
    cx.span()
        .set_attribute(KeyValue::new("db.system", "postgresql"));
    cx.span()
        .set_attribute(KeyValue::new("db.statement", statement));
    let result = fut.with_context(cx.clone()).await;
    cx.span().end();
    result
}

/// Router layer that records one server span per request. While the SDK is
/// inactive the layer forwards the request and builds no span.
pub fn wrap(router: Router) -> Router {
    router.layer(middleware::from_fn(trace_http))
}

fn bounded_method(method: &str) -> String {
    if method.len() <= 16 && method.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        method.to_owned()
    } else {
        "UNKNOWN".to_owned()
    }
}

/// Host from a URL or `host:port`, without userinfo, port, path or query.
fn host_only(target: &str) -> &str {
    let rest = target.split_once("://").map_or(target, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(host) = rest.strip_prefix('[') {
        return host.split_once(']').map_or(host, |(host, _)| host);
    }
    rest.split_once(':').map_or(rest, |(host, _)| host)
}

/// Postgres `db.statement` obfuscation, in the shape of
/// `OpenTelemetry::Helpers::SqlProcessor.obfuscate_sql(adapter: :postgres)`
/// at its default 2000-character limit. Literals, comments and UUIDs become
/// `?`. A statement that still contains a quote after that, or that is
/// longer than the limit, is replaced by a fixed message that does not
/// contain the SQL. The golden strings match that helper; the inputs that
/// do not are listed in `docs/migration/parity-gaps.md`.
pub fn obfuscate_sql(sql: &str) -> String {
    const LIMIT: usize = 2000;
    const TOO_LONG: &str = "SQL not obfuscated, query exceeds 2000 characters";
    const FAILED: &str =
        "Failed to obfuscate SQL query - quote characters remained after obfuscation";

    if sql.len() > LIMIT && sql.chars().count() > LIMIT {
        return TOO_LONG.to_owned();
    }

    let mut out = String::with_capacity(sql.len());
    let mut index = 0;
    // Next regex literal at or after `index`. One `find_at` per pattern per
    // gap, not per byte: a later match is reused until a quote jumps past it.
    let mut pending: Option<(usize, usize)> = None;
    while index < sql.len() {
        if !sql.is_char_boundary(index) {
            return FAILED.to_owned();
        }
        if let Some(end) = single_quote(sql, index).or_else(|| dollar_quote(sql, index)) {
            if end <= index {
                return FAILED.to_owned();
            }
            out.push('?');
            index = end;
            if pending.is_some_and(|(start, _)| start < index) {
                pending = None;
            }
            continue;
        }
        if pending.is_none_or(|(start, _)| start < index) {
            pending = next_regex_literal(sql, index);
        }
        if let Some((start, end)) = pending
            && start == index
        {
            if end <= index {
                return FAILED.to_owned();
            }
            out.push('?');
            index = end;
            pending = None;
            continue;
        }
        let Some(ch) = sql[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    if quotes_remained(&out) {
        FAILED.to_owned()
    } else {
        out
    }
}

/// Leftmost regex literal at or after `index`. A match that starts at
/// `index` keeps pattern priority (uuid, then number, boolean, comments).
/// A later match is the earliest start; ties keep the earlier pattern.
fn next_regex_literal(sql: &str, index: usize) -> Option<(usize, usize)> {
    const PATTERNS: &[fn() -> &'static Regex] = &[
        uuid_re,
        numeric_re,
        boolean_re,
        line_comment_re,
        block_comment_re,
    ];
    let mut best: Option<(usize, usize)> = None;
    for pattern in PATTERNS {
        let Some(found) = pattern().find_at(sql, index) else {
            continue;
        };
        if found.start() == index {
            return Some((found.start(), found.end()));
        }
        if best.is_none_or(|(start, _)| found.start() < start) {
            best = Some((found.start(), found.end()));
        }
    }
    best
}

fn single_quote(sql: &str, index: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    if bytes.get(index) != Some(&b'\'') {
        return None;
    }
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        // Ruby `single_quotes`: `\'` then `.*` masks the rest of the line
        // (`.` stops at `\n`). Without this, `'a\' , secret.pdf` would leave
        // the filename on the span.
        if bytes[cursor] == b'\\' && bytes.get(cursor + 1) == Some(&b'\'') {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            return Some(cursor);
        }
        if bytes[cursor] == b'\'' {
            if bytes.get(cursor + 1) == Some(&b'\'') {
                cursor += 2;
                continue;
            }
            return Some(cursor + 1);
        }
        cursor += 1;
    }
    None
}

/// `$tag$ ... $tag$`, or through the end of the statement when the closer
/// is missing (so an opened dollar quote cannot leak the rest of the SQL).
/// `$1` is a bind parameter, not a dollar quote.
fn dollar_quote(sql: &str, index: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    if bytes.get(index) != Some(&b'$') {
        return None;
    }
    if bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut cursor = index + 1;
    while cursor < bytes.len() && bytes[cursor] != b'$' {
        cursor += 1;
    }
    if cursor >= bytes.len() {
        return None;
    }
    let tag_end = cursor + 1;
    let tag = &bytes[index..tag_end];
    let rest = &bytes[tag_end..];
    match rest.windows(tag.len()).position(|window| window == tag) {
        Some(pos) => Some(tag_end + pos + tag.len()),
        None => Some(bytes.len()),
    }
}

/// Ruby's postgres cleanup: a leftover quote, comment marker, or `$/` means
/// the statement was malformed and the whole text is discarded.
fn quotes_remained(sql: &str) -> bool {
    sql.contains('\'')
        || sql.contains("/*")
        || sql.contains("*/")
        || sql.as_bytes().windows(2).any(|window| window == b"$/")
}

fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{?(?:[0-9a-fA-F]-*){32}\}?").expect("uuid pattern"))
}

fn numeric_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"-?\b(?:[0-9]+\.)?[0-9]+(?:[eE][+-]?[0-9]+)?\b").expect("numeric pattern")
    })
}

fn boolean_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(?:true|false|null)\b").expect("boolean pattern"))
}

fn line_comment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:#|--).*").expect("line comment pattern"))
}

fn block_comment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)/\*.*?\*/").expect("block comment pattern"))
}
