//! Live export. Ignored by `cargo test`: it talks to a collector.
//!
//!     OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
//!     OTEL_SERVICE_NAME=ged \
//!     cargo test -p ged --test telemetry_otlp -- --ignored

use ged::telemetry::{CallbackOutcome, JobClass, JobQueue, UploadMode};

#[tokio::test]
#[ignore = "needs a collector at OTEL_EXPORTER_OTLP_ENDPOINT"]
async fn exports_metrics_and_a_span_over_otlp() {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .expect("OTEL_EXPORTER_OTLP_ENDPOINT must be set");
    assert!(
        !endpoint.trim().is_empty(),
        "blank endpoint must not start the SDK"
    );
    let guard = ged::telemetry::install_from_env();
    assert!(
        ged::telemetry::sdk_active(),
        "SDK stayed inactive with endpoint {endpoint}"
    );
    ged::telemetry::documents_uploaded(UploadMode::Sync);
    ged::telemetry::callback_deliveries(CallbackOutcome::Success);
    ged::telemetry::job_duration(JobClass::StoreDocumentJob, JobQueue::Uploads, 0.01);
    let sent = ged::telemetry::trace_outgoing_http("GET", "http://idp.example/jwks", async {
        (200_u16, ())
    })
    .await;
    assert_eq!(sent, ());
    drop(guard);
}
