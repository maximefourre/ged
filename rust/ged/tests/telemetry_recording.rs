//! Records the three `ged.*` metrics and the HTTP/SQL spans against an
//! in-memory SDK. Separate binary from `telemetry.rs` so installing a
//! provider here cannot make the dormant-SDK test observe one.

mod support;

use axum::body::Body;
use axum::http::Request;
use ged::telemetry::{CallbackOutcome, JobClass, JobQueue, UploadMode};
use ged::{router, telemetry};
use opentelemetry::global;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tower::ServiceExt;

#[tokio::test]
async fn metrics_and_traces_use_the_rails_names_when_a_provider_is_installed() {
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
