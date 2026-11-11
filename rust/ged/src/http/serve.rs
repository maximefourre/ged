//! The HTTP server loop, with the connection deadlines `axum::serve` does not
//! set (review M2).
//!
//! The Rails image ran Thruster (Go `net/http`) in front of Puma. Their
//! defaults, read from the sources of the pinned versions (Thruster 0.1.23
//! `internal/config.go`, Puma 8.0.2 `lib/puma/configuration.rb`) and not
//! overridden in `rails-final`:
//!
//! | Layer | Setting | Default |
//! |---|---|---|
//! | Thruster | `HTTP_READ_TIMEOUT` (Go `ReadTimeout`, whole request) | 30 s |
//! | Thruster | `HTTP_WRITE_TIMEOUT` | 30 s |
//! | Thruster | `HTTP_IDLE_TIMEOUT` (keep-alive) | 60 s |
//! | Puma | `first_data_timeout` | 30 s |
//! | Puma | `persistent_timeout` | 65 s |
//!
//! This server keeps the 30 s header deadline ([`Timeouts::header_read`], also
//! the wait for the next request on a kept-alive connection), and a 30 s
//! inactivity deadline on the request body ([`Timeouts::body_idle`]). The body
//! deadline is per frame, not for the whole body: Thruster's 30 s covers the
//! whole request, which cuts a 50 MiB upload slower than ~14 Mbit/s; a slow but
//! progressing upload stays possible here. See `docs/migration/parity-gaps.md`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::time::Sleep;

/// Deadlines applied to every connection.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// From the first byte of a request (or the start of the wait on a
    /// kept-alive connection) to the end of its headers.
    pub header_read: Duration,
    /// Longest wait for the next piece of a request body.
    pub body_idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            header_read: Duration::from_secs(30),
            body_idle: Duration::from_secs(30),
        }
    }
}

/// Serves `app` until `shutdown` completes, then lets open connections finish.
/// The peer address is exposed as `ConnectInfo<SocketAddr>`.
pub async fn serve<F>(
    listener: TcpListener,
    app: Router,
    timeouts: Timeouts,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()>,
{
    let (stop, _) = tokio::sync::watch::channel(false);
    let mut connections = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            // EMFILE, ECONNABORTED...: back off instead of spinning or exiting.
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let app = app.clone().layer(axum::Extension(ConnectInfo(peer)));
        let mut stop = stop.subscribe();
        connections.spawn(async move {
            let service = TowerToHyperService::new(tower::util::MapRequest::new(
                app,
                move |request: hyper::Request<hyper::body::Incoming>| -> Request {
                    request.map(|body| Body::new(IdleBody::new(body, timeouts.body_idle)))
                },
            ));
            let mut builder = http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(timeouts.header_read);
            let connection = builder.serve_connection(TokioIo::new(stream), service);
            tokio::pin!(connection);
            tokio::select! {
                _ = connection.as_mut() => {}
                _ = stop.changed() => {
                    connection.as_mut().graceful_shutdown();
                    let _ = connection.as_mut().await;
                }
            }
        });
    }
    let _ = stop.send(true);
    while connections.join_next().await.is_some() {}
    Ok(())
}

/// Ends the body with an error when no frame arrives within `idle`.
pub struct IdleBody<B> {
    inner: B,
    idle: Duration,
    sleep: Pin<Box<Sleep>>,
}

impl<B> IdleBody<B> {
    pub fn new(inner: B, idle: Duration) -> Self {
        Self {
            inner,
            idle,
            sleep: Box::pin(tokio::time::sleep(idle)),
        }
    }
}

impl<B> http_body::Body for IdleBody<B>
where
    B: http_body::Body<Data = Bytes> + Unpin,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(frame) => {
                let deadline = tokio::time::Instant::now() + this.idle;
                this.sleep.as_mut().reset(deadline);
                Poll::Ready(frame.map(|f| f.map_err(Into::into)))
            }
            Poll::Pending => match this.sleep.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Some(Err("request body timed out".into()))),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
