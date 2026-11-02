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
        // Ruby `\'` then `.*` (stops at `\n`). Same outputs as the helper.
        ("SELECT 'a\\' secret'", "SELECT ?"),
        ("SELECT 'a\\' AND name = 'visible'", "SELECT ?"),
        ("SELECT 'a\\' , secret.pdf", "SELECT ?"),
        ("SELECT 'a\\secret' FROM t", "SELECT ? FROM t"),
        (
            "SELECT 'a\\'\nWHERE name = 'secret'",
            "SELECT ?\nWHERE name = ?",
        ),
        (
            "SELECT 'a\\'\nsecret'",
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

#[test]
fn sql_obfuscation_outside_the_ruby_goldens_does_not_leave_a_literal() {
    // Rust results, not Ruby goldens. The Ruby helper disagrees on these
    // inputs; the differences are recorded in docs/migration/parity-gaps.md.
    let cases = [
        ("SELECT $$a\nsecret$$ FROM t", "SELECT ? FROM t"),
        ("SELECT 1 -- secret\r\nFROM t", "SELECT ? ?\nFROM t"),
        ("SELECT 1 -- x\rFROM t", "SELECT ? ?"),
    ];
    for (sql, expected) in cases {
        let obfuscated = obfuscate_sql(sql);
        assert_eq!(obfuscated, expected, "sql: {sql:?}");
        assert!(!obfuscated.contains("secret"), "{obfuscated}");
    }
}

#[tokio::test]
async fn inactive_sdk_does_not_obfuscate_or_allocate_span_fields() {
    assert!(!telemetry::sdk_active());
    let before = telemetry::span_preparations();
    let sql = format!(
        "SELECT filename = '{}' AND note = 'secret.pdf'",
        "x".repeat(1800)
    );
    let value = telemetry::trace_sql(&sql, async { 7 }).await;
    assert_eq!(value, 7);
    let value = telemetry::trace_outgoing_http(
        "POST",
        "https://user:pw@hooks.example/callback?token=secret",
        async { (500_u16, "ok") },
    )
    .await;
    assert_eq!(value, "ok");

    let app = telemetry::wrap(router(support::state()));
    let response = app
        .oneshot(Request::get("/up").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        telemetry::span_preparations(),
        before,
        "inactive SDK prepared a span (obfuscation or HTTP field allocation)"
    );
}

#[tokio::test]
async fn sigterm_lets_the_server_return_so_the_guard_can_flush() {
    const CHILD: &str = "GED_T12_SHUTDOWN_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let waiting = tokio::spawn(telemetry::wait_for_shutdown_signal());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let status = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("send SIGTERM");
        assert!(status.success(), "kill: {status}");
        tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
            .await
            .expect("SIGTERM did not unblock shutdown")
            .expect("shutdown task");
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().expect("test exe"))
        .args([
            "--exact",
            "sigterm_lets_the_server_return_so_the_guard_can_flush",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .output()
        .expect("spawn shutdown child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("sigterm_lets_the_server_return_so_the_guard_can_flush ... ok"),
        "child did not run the signal test\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn serve_exits_zero_on_sigterm_so_the_guard_is_dropped() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let mut child = Command::new(env!("CARGO_BIN_EXE_ged"))
        .args(["serve"])
        .env_clear()
        .env("RAILS_ENV", "test")
        .env("DATABASE_URL", "postgres://localhost/unused")
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ged serve");

    let read_err = |child: &mut std::process::Child| {
        let mut err = String::new();
        if let Some(stderr) = child.stderr.as_mut() {
            let _ = stderr.read_to_string(&mut err);
        }
        err
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut up = false;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("wait") {
            panic!("serve exited before /up: {status} {}", read_err(&mut child));
        }
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("timeout");
            let _ = write!(
                stream,
                "GET /up HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            );
            let mut body = String::new();
            let _ = stream.read_to_string(&mut body);
            if body.starts_with("HTTP/1.1 200") {
                up = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if !up {
        let _ = child.kill();
        let _ = child.wait();
        panic!("serve never answered /up: {}", read_err(&mut child));
    }

    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(status.success(), "kill: {status}");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            assert_eq!(
                status.code(),
                Some(0),
                "SIGTERM should return from serve so the guard drops; stderr: {}",
                read_err(&mut child)
            );
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("SIGTERM did not stop ged serve");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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
