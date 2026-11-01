use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, Method, redirect};
use serde_json::Value;

/// A fully-read HTTP response.
#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Body parsed as JSON; panics with the raw body when it is not JSON.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("body is not JSON ({e}): {:?}", self.text()))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// Minimal HTTP client for the API: no redirects followed, no cookies.
#[derive(Clone)]
pub struct Api {
    base: String,
    http: Client,
}

impl Api {
    pub fn new(base_url: &str) -> Self {
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .build()
            .expect("build http client");
        Self {
            base: base_url.trim_end_matches('/').to_owned(),
            http,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Send a request. `headers` are raw name/value pairs, so malformed
    /// `Authorization` values can be sent as-is.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Resp {
        let mut req = self.http.request(method, self.url(path));
        for (k, v) in headers {
            req = req.header(*k, HeaderValue::from_str(v).expect("header value"));
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        read(req.send().await.expect("send request")).await
    }

    pub async fn get(&self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.send(Method::GET, path, headers, None).await
    }

    /// GET with `Authorization: Bearer <token>`.
    pub async fn get_bearer(&self, path: &str, token: &str) -> Resp {
        let auth = format!("Bearer {token}");
        self.get(path, &[("Authorization", &auth)]).await
    }

    /// Send a JSON body with a bearer token.
    pub async fn json_bearer(&self, method: Method, path: &str, token: &str, body: &Value) -> Resp {
        let auth = format!("Bearer {token}");
        self.send(
            method,
            path,
            &[
                ("Authorization", &auth),
                ("Content-Type", "application/json"),
            ],
            Some(body.to_string().into_bytes()),
        )
        .await
    }
}

pub(crate) async fn read(resp: reqwest::Response) -> Resp {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.expect("read body").to_vec();
    Resp {
        status,
        headers,
        body,
    }
}
