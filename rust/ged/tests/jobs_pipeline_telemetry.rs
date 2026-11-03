//! T9: the pipeline jobs emit the `ged.*` metrics through the facade.
//! Own binary: it installs an in-memory SDK provider process-wide.

mod pipeline_support;
mod support;

use std::sync::Arc;

use futures_util::future::BoxFuture;
use ged::jobs::Job;
use ged::jobs::deliver_callback::{CallbackPolicy, DeliverCallbackJob};
use ged::jobs::store_document::StoreDocumentJob;
use ged::telemetry;
use opentelemetry::global;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use pipeline_support::{Fixture, Hook};
use serde_json::json;

struct Loopback;

impl CallbackPolicy for Loopback {
    fn decide(&self, _url: &str) -> BoxFuture<'static, Result<std::net::IpAddr, String>> {
        Box::pin(async { Ok("127.0.0.1".parse().unwrap()) })
    }
}

struct Refuse;

impl CallbackPolicy for Refuse {
    fn decide(&self, _url: &str) -> BoxFuture<'static, Result<std::net::IpAddr, String>> {
        Box::pin(async { Err("nope".to_owned()) })
    }
}

#[tokio::test]
async fn jobs_count_uploads_deliveries_and_durations() {
    let metrics = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(metrics.clone()).build())
        .build();
    global::set_meter_provider(provider.clone());
    telemetry::mark_sdk_active_for_test(true);

    let f = Fixture::new().await;
    let dept = f.department("Compta", None).await;
    let id = f
        .upload_request(dept, "pending", "https://x.example/h", Some(b"bytes"))
        .await;
    let args = json!({ "upload_request_id": id.to_string() });
    StoreDocumentJob::new(f.env.clone())
        .perform(args.clone())
        .await
        .unwrap();

    let hook = Hook::start(200).await;
    sqlx::query("UPDATE upload_requests SET callback_url = $2 WHERE id = $1")
        .bind(id)
        .bind(hook.url())
        .execute(&f.pool)
        .await
        .unwrap();
    let ok = DeliverCallbackJob::new(f.env.clone(), Arc::new(Loopback));
    ok.perform(args.clone()).await.unwrap();
    DeliverCallbackJob::new(f.env.clone(), Arc::new(Refuse))
        .perform(args.clone())
        .await
        .unwrap();
    ok.exhausted(&args, "callback returned HTTP 500");

    provider.force_flush().unwrap();
    let text = format!("{:?}", metrics.get_finished_metrics().unwrap());
    for needle in [
        "ged.documents.uploaded",
        "async",
        "ged.callbacks.deliveries",
        "success",
        "refused",
        "exhausted",
        "ged.job.duration",
        "StoreDocumentJob",
        "DeliverCallbackJob",
        "uploads",
        "default",
    ] {
        assert!(text.contains(needle), "{needle} missing in {text}");
    }
    telemetry::mark_sdk_active_for_test(false);
    f.finish().await;
}
