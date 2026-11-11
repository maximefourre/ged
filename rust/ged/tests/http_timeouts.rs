//! Connection deadlines of `ged::http::serve` (final review M2). Real TCP
//! sockets and short deadlines; the production values are the defaults of
//! `Timeouts` (see `docs/migration/parity-gaps.md`).

mod support;

use std::path::Path;
use std::time::Duration;

use ged::http::serve::{Timeouts, serve};
use ged::storage::{Storage, StagingStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SHORT: Duration = Duration::from_millis(300);

struct Server {
    addr: std::net::SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start(app: axum::Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, rx) = tokio::sync::oneshot::channel::<()>();
        let timeouts = Timeouts {
            header_read: SHORT,
            body_idle: SHORT,
        };
        let task = tokio::spawn(serve(listener, app, timeouts, async move {
            let _ = rx.await;
        }));
        Self {
            addr,
            stop: Some(stop),
            task,
        }
    }

    async fn finish(mut self) {
        let _ = self.stop.take().map(|s| s.send(()));
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("the server stops once its connections are closed")
            .unwrap()
            .unwrap();
    }
}

/// Reads until the peer closes (or errors); everything received.
async fn read_to_close(stream: &mut TcpStream, within: Duration) -> Result<String, &'static str> {
    let mut out = Vec::new();
    match tokio::time::timeout(within, async {
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await
    {
        Ok(()) => Ok(String::from_utf8_lossy(&out).into_owned()),
        Err(_) => Err("the connection stayed open"),
    }
}

fn files_under(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() { files_under(&p) } else { 1 }
            })
            .sum()
    })
}

#[tokio::test]
async fn incomplete_headers_are_cut_after_the_header_deadline() {
    let server = Server::start(support::app()).await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream.write_all(b"GET /up HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    // The headers never finish: the server must hang up on its own.
    let received = read_to_close(&mut stream, Duration::from_secs(5)).await;
    assert!(
        received.is_ok(),
        "headers left incomplete kept the connection open"
    );
    assert!(
        !received.unwrap().contains("200 OK"),
        "an unfinished request was served"
    );
    server.finish().await;
}

#[tokio::test]
async fn an_idle_kept_alive_connection_is_closed_and_a_prompt_request_is_served() {
    let server = Server::start(support::app()).await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream
        .write_all(b"GET /up HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0_u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    // Nothing more is sent: the wait for the next request is bounded too.
    assert!(
        read_to_close(&mut stream, Duration::from_secs(5))
            .await
            .is_ok(),
        "an idle kept-alive connection stayed open"
    );
    server.finish().await;
}

#[tokio::test]
async fn a_slow_but_progressing_body_is_not_cut() {
    // Whole upload takes ~4x the idle deadline; every gap is well below it.
    let server = Server::start(support::app_with_probes()).await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    let body = format!("name={}", "a".repeat(8));
    stream
        .write_all(
            format!(
                "POST /api/v1/probe HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    for piece in body.as_bytes().chunks(2) {
        stream.write_all(piece).await.unwrap();
        tokio::time::sleep(Duration::from_millis(SHORT.as_millis() as u64 / 3)).await;
    }
    let received = read_to_close(&mut stream, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(received.starts_with("HTTP/1.1 200"), "{received}");
    assert!(received.contains("aaaaaaaa"), "{received}");
    server.finish().await;
}

#[tokio::test]
async fn a_stalled_body_is_cut_after_the_idle_deadline() {
    let server = Server::start(support::app_with_probes()).await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream
        .write_all(
            b"POST /api/v1/probe HTTP/1.1\r\nHost: x\r\n\
              Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 100000\r\n\r\nname=a",
        )
        .await
        .unwrap();
    // The remaining 99,993 bytes never come.
    let received = read_to_close(&mut stream, Duration::from_secs(5)).await;
    assert!(received.is_ok(), "a stalled body kept the connection open");
    assert!(
        !received.unwrap().contains("200 OK"),
        "a truncated body was served as complete"
    );
    server.finish().await;
}

#[tokio::test]
async fn an_abandoned_upload_leaves_no_part_file() {
    let dir = std::env::temp_dir().join(format!("ged-timeouts-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = support::state().with_storage(
        Storage::new(dir.join("files")),
        StagingStore::new(dir.join("staging")),
    );
    let server = Server::start(ged::router(state)).await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    let head = "--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\
                Content-Type: text/plain\r\n\r\n";
    stream
        .write_all(
            format!(
                "POST /api/v1/documents HTTP/1.1\r\nHost: x\r\n\
                 Content-Type: multipart/form-data; boundary=XX\r\nContent-Length: 5000000\r\n\r\n{head}"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(&vec![b'z'; 200_000]).await.unwrap();
    // ... then silence: no closing boundary, no more bytes.
    let received = read_to_close(&mut stream, Duration::from_secs(5)).await;
    assert!(received.is_ok(), "the stalled upload kept the connection open");
    // The handler is dropped on the deadline; let it clean up.
    for _ in 0..50 {
        if files_under(&dir) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(files_under(&dir), 0, "an abandoned upload left a file behind");
    server.finish().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn shutdown_lets_an_in_flight_request_finish() {
    let server = Server::start(support::app()).await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream.write_all(b"GET /up HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let mut buf = [0_u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    drop(stream);
    server.finish().await;
}
