//! The OpenTelemetry SDK stays dormant unless an OTLP endpoint is configured,
//! and the façade matches `lib/telemetry.rb` name for name. Same regression
//! guard as `test/lib/telemetry_test.rb`: a test run must not start exporters.

mod support;

use axum::body::Body;
use axum::http::Request;
use ged::telemetry::{
    CallbackOutcome, JobClass, JobQueue, UploadMode, enabled, install, obfuscate_sql,
};
use ged::{router, telemetry};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn lookup_none(_: &str) -> Option<String> {
    None
}

#[test]
fn the_sdk_stays_inactive_without_an_otlp_endpoint() {
    assert!(!enabled(None, None));
    assert!(!enabled(Some(""), None));
    assert!(!enabled(Some("   "), None));
    assert!(!enabled(Some("http://localhost:4318"), Some("true")));
    // Only the exact string disables the SDK, as in the Rails initializer.
    assert!(enabled(Some("http://localhost:4318"), Some("false")));
    assert!(enabled(Some("http://localhost:4318"), None));
    assert!(enabled(Some("  http://localhost:4318  "), Some("")));

    let _guard = install(&lookup_none);
    assert!(!telemetry::sdk_active());
}

#[test]
fn instruments_are_noops_that_accept_the_rails_attributes() {
    assert!(!telemetry::sdk_active());
    for mode in [UploadMode::Sync, UploadMode::Async] {
        telemetry::documents_uploaded(mode);
    }
    for outcome in [
        CallbackOutcome::Success,
        CallbackOutcome::Failure,
        CallbackOutcome::Refused,
        CallbackOutcome::Exhausted,
    ] {
        telemetry::callback_deliveries(outcome);
    }
    for class in [
        JobClass::StoreDocumentJob,
        JobClass::DeliverCallbackJob,
        JobClass::ReconcileUploadRequestsJob,
        JobClass::PurgeDeletedDocumentsJob,
    ] {
        for queue in [JobQueue::Uploads, JobQueue::Default] {
            telemetry::job_duration(class, queue, 0.5);
        }
    }
    // A bad duration is dropped, not panicked on.
    telemetry::job_duration(JobClass::StoreDocumentJob, JobQueue::Uploads, f64::NAN);
    telemetry::job_duration(JobClass::StoreDocumentJob, JobQueue::Uploads, -1.0);
}

#[test]
fn metric_names_and_closed_attributes_match_lib_telemetry_rb() {
    assert_eq!(telemetry::DOCUMENTS_UPLOADED, "ged.documents.uploaded");
    assert_eq!(telemetry::DOCUMENTS_UPLOADED_UNIT, "{document}");
    assert_eq!(
        telemetry::DOCUMENTS_UPLOADED_DESCRIPTION,
        "Documents created, by storage mode (attribute upload_mode)"
    );
    assert_eq!(telemetry::CALLBACK_DELIVERIES, "ged.callbacks.deliveries");
    assert_eq!(telemetry::CALLBACK_DELIVERIES_UNIT, "{attempt}");
    assert_eq!(
        telemetry::CALLBACK_DELIVERIES_DESCRIPTION,
        "Callback delivery attempts by outcome (success/failure/refused/exhausted; failure counts per attempt)"
    );
    assert_eq!(telemetry::JOB_DURATION, "ged.job.duration");
    assert_eq!(telemetry::JOB_DURATION_UNIT, "s");
    assert_eq!(
        telemetry::JOB_DURATION_DESCRIPTION,
        "Active Job perform duration"
    );
    assert_eq!(telemetry::METER_NAME, "ged");

    assert_eq!(UploadMode::Sync.as_str(), "sync");
    assert_eq!(UploadMode::Async.as_str(), "async");
    assert_eq!(CallbackOutcome::Success.as_str(), "success");
    assert_eq!(CallbackOutcome::Failure.as_str(), "failure");
    assert_eq!(CallbackOutcome::Refused.as_str(), "refused");
    assert_eq!(CallbackOutcome::Exhausted.as_str(), "exhausted");
    assert_eq!(JobClass::StoreDocumentJob.as_str(), "StoreDocumentJob");
    assert_eq!(JobClass::DeliverCallbackJob.as_str(), "DeliverCallbackJob");
    assert_eq!(
        JobClass::ReconcileUploadRequestsJob.as_str(),
        "ReconcileUploadRequestsJob"
    );
    assert_eq!(
        JobClass::PurgeDeletedDocumentsJob.as_str(),
        "PurgeDeletedDocumentsJob"
    );
    assert_eq!(JobQueue::Uploads.as_str(), "uploads");
    assert_eq!(JobQueue::Default.as_str(), "default");
}

#[test]
fn sql_literals_are_obfuscated_like_the_rails_pg_instrumentation() {
    // Golden strings from OpenTelemetry::Helpers::SqlProcessor.obfuscate_sql
    // (adapter: :postgres), the helper Rails uses for db_statement: :obfuscate.
    let cases = [
        (
            "SELECT * FROM documents WHERE filename = 'secret.pdf' AND byte_size = 42",
            "SELECT * FROM documents WHERE filename = ? AND byte_size = ?",
        ),
        (
            r#"INSERT INTO documents (tags) VALUES ('{"type":"facture"}')"#,
            "INSERT INTO documents (tags) VALUES (?)",
        ),
        (
            "SELECT * FROM documents WHERE id = $1 AND name = 'a'",
            "SELECT * FROM documents WHERE id = $? AND name = ?",
        ),
        (
            "SELECT * FROM documents WHERE id = '550e8400-e29b-41d4-a716-446655440000'",
            "SELECT * FROM documents WHERE id = ?",
        ),
        (
            "SELECT true, FALSE, NULL, -12.5, 1e-3 FROM documents",
            "SELECT ?, ?, ?, ?, ? FROM documents",
        ),
        (
            "SELECT * FROM documents -- filename = secret.pdf\nWHERE id = 1",
            "SELECT * FROM documents ?\nWHERE id = ?",
        ),
        (
            "SELECT * FROM documents /* secret.pdf */ WHERE id = 1",
            "SELECT * FROM documents ? WHERE id = ?",
        ),
        ("SELECT $tag$secret.pdf$tag$", "SELECT ?"),
        (
            "SELECT * FROM documents WHERE filename = 'it''s'",
            "SELECT * FROM documents WHERE filename = ?",
        ),
        (
            r#"SELECT * FROM "documents" WHERE filename = 'x'"#,
            r#"SELECT * FROM "documents" WHERE filename = ?"#,
        ),
        ("SELECT $$secret$$", "SELECT ?"),
        ("SELECT $tag$secret", "SELECT ?"),
        ("SELECT col$name FROM t", "SELECT col$name FROM t"),
        (
            "SELECT * FROM t WHERE a = 1 AND b = $2",
            "SELECT * FROM t WHERE a = ? AND b = $?",
        ),
        ("SELECT E'secret'", "SELECT E?"),
        ("SELECT 'line\nbreak'", "SELECT ?"),
        ("SELECT /* secret\nstill */ 1", "SELECT ? ?"),
        ("SELECT # secret\n1", "SELECT ?\n?"),
        ("SELECT 0x1234", "SELECT 0x1234"),
        (
            "SELECT 'ok' || '",
            "Failed to obfuscate SQL query - quote characters remained after obfuscation",
        ),
    ];
    for (sql, expected) in cases {
        let obfuscated = obfuscate_sql(sql);
        assert_eq!(obfuscated, expected, "sql: {sql}");
        assert!(!obfuscated.contains("secret.pdf"), "{obfuscated}");
        assert!(!obfuscated.contains("facture"), "{obfuscated}");
    }
    let long = "a".repeat(2001);
    let obfuscated = obfuscate_sql(&long);
    assert_eq!(
        obfuscated,
        "SQL not obfuscated, query exceeds 2000 characters"
    );
    assert!(!obfuscated.contains(&long));
}

#[tokio::test]
async fn helpers_are_callable_and_the_response_is_unchanged_when_the_sdk_is_off() {
    assert!(!telemetry::sdk_active());
    let value = telemetry::trace_sql("SELECT filename = 'secret.pdf'", async { 7 }).await;
    assert_eq!(value, 7);
    let value = telemetry::trace_outgoing_http(
        "POST",
        "https://user:pw@hooks.example/callback?token=secret",
        async { (204, "ok") },
    )
    .await;
    assert_eq!(value, "ok");

    let app = telemetry::wrap(router(support::state()));
    let response = app
        .oneshot(Request::get("/up").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("green"), "{body}");
}
