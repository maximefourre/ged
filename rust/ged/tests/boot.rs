//! Process-level checks of the `ged` binary: production refuses to boot on a
//! misconfiguration, and `serve` answers `/up` when it is sound.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn ged(args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ged"));
    cmd.args(args)
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

const PROD: &[(&str, &str)] = &[
    ("RAILS_ENV", "production"),
    ("SECRET_KEY_BASE", "s3cret"),
    ("DATABASE_URL", "postgres://localhost/unused"),
    ("GED_OIDC_ISSUER", "http://idp"),
    ("GED_OIDC_JWKS_URL", "http://idp/jwks"),
];

fn refuses(extra: &[(&str, &str)], needle: &str) {
    let mut env = PROD.to_vec();
    env.extend_from_slice(extra);
    let out = ged(&["serve"], &env).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "must refuse to boot; stderr: {stderr}"
    );
    assert!(stderr.contains(needle), "{needle:?} not in {stderr:?}");
}

#[test]
fn production_without_an_audience_does_not_boot() {
    refuses(&[], "Missing OIDC config: GED_OIDC_AUDIENCE");
}

#[test]
fn production_in_scope_mode_without_a_scope_does_not_boot() {
    refuses(
        &[
            ("GED_OIDC_TOKEN_BINDING", "scope"),
            ("GED_OIDC_AUDIENCE", "ged"),
        ],
        "Missing OIDC config: GED_OIDC_REQUIRED_SCOPE",
    );
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn http_status(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    text.lines().next().map(str::to_owned)
}

#[test]
fn a_sound_production_config_serves_up() {
    let port = free_port().to_string();
    let mut env = PROD.to_vec();
    env.extend_from_slice(&[("GED_OIDC_AUDIENCE", "ged"), ("PORT", &port)]);
    let _server = Server(ged(&["serve"], &env).spawn().unwrap());
    let port: u16 = port.parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(line) = http_status(port, "/up") {
            assert_eq!(line, "HTTP/1.1 200 OK");
            return;
        }
        assert!(Instant::now() < deadline, "server never answered /up");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn worker_is_not_implemented_yet() {
    let out = ged(&["worker"], &[("RAILS_ENV", "test")]).output().unwrap();
    assert!(!out.status.success());
}
