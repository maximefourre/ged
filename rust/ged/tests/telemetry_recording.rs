//! Records the three `ged.*` metrics and the HTTP/SQL spans against an
//! in-memory SDK. Separate binary from `telemetry.rs` so installing a
//! provider here cannot make the dormant-SDK test observe one.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use ged::telemetry::{CallbackOutcome, JobClass, JobQueue, UploadMode};
use ged::{router, telemetry};
use opentelemetry::global;
use opentelemetry::trace::{SpanKind, Status};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tower::ServiceExt;

async fn lock_sdk() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn path_of(span: &SpanData) -> Option<String> {
    span.attributes
        .iter()
        .find_map(|kv| (kv.key.as_str() == "url.path").then(|| kv.value.as_str().into_owned()))
}

#[tokio::test]
async fn metrics_and_traces_use_the_rails_names_when_a_provider_is_installed() {
    let _lock = lock_sdk().await;
    let metrics = InMemoryMetricExporter::default();
    let meter_provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(metrics.clone()).build())
        .build();
    global::set_meter_provider(meter_provider.clone());

    let spans = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(spans.clone())
        .build();
    global::set_tracer_provider(tracer_provider.clone());
    // install() is what normally flips this. The suite builds providers by hand.
    telemetry::mark_sdk_active_for_test(true);

    telemetry::documents_uploaded(UploadMode::Async);
    telemetry::callback_deliveries(CallbackOutcome::Refused);
    telemetry::job_duration(JobClass::DeliverCallbackJob, JobQueue::Default, 0.25);

    let response = telemetry::wrap(router(support::state()))
        .oneshot(Request::get("/up").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let saw = telemetry::trace_sql(
        "SELECT filename FROM documents WHERE filename = 'secret.pdf'",
        async { true },
    )
    .await;
    assert!(saw);

    let host = telemetry::trace_outgoing_http(
        "POST",
        "https://user:pw@hooks.example/callback?token=secret",
        async { (204, "sent") },
    )
    .await;
    assert_eq!(host, "sent");

    meter_provider.force_flush().expect("flush metrics");
    tracer_provider.force_flush().expect("flush spans");

    let exported = metrics.get_finished_metrics().expect("read metrics");
    let text = format!("{exported:?}");
    assert!(text.contains("ged.documents.uploaded"), "{text}");
    assert!(text.contains("ged.callbacks.deliveries"), "{text}");
    assert!(text.contains("ged.job.duration"), "{text}");
    assert!(text.contains("upload_mode"), "{text}");
    assert!(text.contains("async"), "{text}");
    assert!(text.contains("outcome"), "{text}");
    assert!(text.contains("refused"), "{text}");
    assert!(text.contains("job.class"), "{text}");
    assert!(text.contains("DeliverCallbackJob"), "{text}");
    assert!(text.contains("queue"), "{text}");
    assert!(text.contains("default"), "{text}");
    assert!(text.contains("{document}"), "{text}");
    assert!(text.contains("{attempt}"), "{text}");

    let finished: Vec<SpanData> = spans.get_finished_spans().expect("read spans");
    let rendered = format!("{finished:?}");
    assert!(rendered.contains("HTTP GET"), "{rendered}");
    assert!(rendered.contains("/up"), "{rendered}");
    assert!(rendered.contains("postgresql"), "{rendered}");
    assert!(
        rendered.contains("SELECT filename FROM documents WHERE filename = ?"),
        "{rendered}"
    );
    assert!(!rendered.contains("secret.pdf"), "{rendered}");
    assert!(rendered.contains("hooks.example"), "{rendered}");
    assert!(!rendered.contains("callback?token"), "{rendered}");
    assert!(!rendered.contains("user:pw"), "{rendered}");
    assert!(!rendered.contains("secret"), "{rendered}");
}

#[tokio::test]
async fn a_server_span_is_an_error_only_when_the_status_is_5xx() {
    let _lock = lock_sdk().await;
    let spans = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(spans.clone())
        .build();
    global::set_tracer_provider(tracer_provider.clone());
    telemetry::mark_sdk_active_for_test(true);

    let app = telemetry::wrap(
        Router::new()
            .route("/ok", get(|| async { StatusCode::OK }))
            .route("/denied", get(|| async { StatusCode::BAD_REQUEST }))
            .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
            .route("/err", get(|| async { StatusCode::INTERNAL_SERVER_ERROR })),
    );
    for path in ["/ok", "/denied", "/missing", "/err"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let expected = match path {
            "/ok" => 200,
            "/denied" => 400,
            "/missing" => 404,
            "/err" => 500,
            _ => unreachable!(),
        };
        assert_eq!(response.status().as_u16(), expected);
    }

    tracer_provider.force_flush().expect("flush spans");
    let finished = spans.get_finished_spans().expect("read spans");
    let span_for = |path: &str| {
        let matched: Vec<_> = finished
            .iter()
            .filter(|span| path_of(span).as_deref() == Some(path))
            .collect();
        assert_eq!(matched.len(), 1, "spans for {path}: {finished:?}");
        matched[0]
    };

    let ok = span_for("/ok");
    assert_eq!(ok.span_kind, SpanKind::Server);
    assert_eq!(ok.status, Status::Unset);
    assert_eq!(span_for("/denied").status, Status::Unset);
    assert_eq!(span_for("/missing").status, Status::Unset);

    let err = span_for("/err");
    assert_eq!(err.span_kind, SpanKind::Server);
    assert_eq!(err.name.as_ref(), "HTTP GET");
    // Rack sets Status.error with the default empty description on 500..=599.
    assert_eq!(err.status, Status::error(""));
}
