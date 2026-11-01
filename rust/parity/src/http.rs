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

    /// POST a JSON body with a bearer token.
    pub async fn post_json(&self, path: &str, token: &str, body: &Value) -> Resp {
        self.json_bearer(Method::POST, path, token, body).await
    }

    /// `application/x-www-form-urlencoded` body with a bearer token.
    /// Keys are sent as given, so Rack bracket names (`tags[type]`,
    /// `document_ids[]`) round-trip unchanged.
    pub async fn form_bearer(
        &self,
        method: Method,
        path: &str,
        token: &str,
        fields: &[(&str, &str)],
    ) -> Resp {
        let auth = format!("Bearer {token}");
        let req = self
            .http
            .request(method, self.url(path))
            .header(
                "Authorization",
                HeaderValue::from_str(&auth).expect("authorization header"),
            )
            .form(&fields);
        read(req.send().await.expect("send form")).await
    }

    pub async fn post_form(&self, path: &str, token: &str, fields: &[(&str, &str)]) -> Resp {
        self.form_bearer(Method::POST, path, token, fields).await
    }

    pub async fn put_form(&self, path: &str, token: &str, fields: &[(&str, &str)]) -> Resp {
        self.form_bearer(Method::PUT, path, token, fields).await
    }

    pub async fn delete_form(&self, path: &str, token: &str, fields: &[(&str, &str)]) -> Resp {
        self.form_bearer(Method::DELETE, path, token, fields).await
    }

    /// DELETE with a bearer token and no body.
    pub async fn delete_bearer(&self, path: &str, token: &str) -> Resp {
        let auth = format!("Bearer {token}");
        self.send(Method::DELETE, path, &[("Authorization", &auth)], None)
            .await
    }

    /// GET with a bearer token and a query string. `fields` keeps duplicates
    /// and empty values (`limit=` is not the same encoding as an absent key);
    /// names and values are percent-encoded, which Rack decodes back.
    pub async fn get_bearer_query(&self, path: &str, token: &str, fields: &[(&str, &str)]) -> Resp {
        let path = append_query(path, fields);
        self.get_bearer(&path, token).await
    }

    /// PUT `application/x-www-form-urlencoded` with a bearer token.
    pub async fn put_form_bearer(&self, path: &str, token: &str, fields: &[(&str, &str)]) -> Resp {
        self.form_bearer(Method::PUT, path, token, fields).await
    }

    /// POST a raw body with a bearer token (malformed JSON, oversized bodies).
    pub async fn post_bytes_bearer(
        &self,
        path: &str,
        token: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Resp {
        self.bytes_bearer(Method::POST, path, token, content_type, body)
            .await
    }

    /// Raw body with a bearer token (malformed JSON, oversized bodies).
    pub async fn bytes_bearer(
        &self,
        method: Method,
        path: &str,
        token: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Resp {
        let auth = format!("Bearer {token}");
        self.send(
            method,
            path,
            &[("Authorization", &auth), ("Content-Type", content_type)],
            Some(body),
        )
        .await
    }

    /// Multipart document upload. `filename` is sent verbatim inside the
    /// Content-Disposition quotes (no client-side basename), so Rack sees the
    /// same string a hostile client would send. Field order is preserved,
    /// including duplicate names such as `tags[type]`.
    pub async fn upload_bearer(
        &self,
        path: &str,
        token: &str,
        fields: &[(&str, &str)],
        filename: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Resp {
        let (ct, body) = encode_multipart(fields, filename, content_type, bytes);
        let auth = format!("Bearer {token}");
        self.send(
            Method::POST,
            path,
            &[("Authorization", &auth), ("Content-Type", &ct)],
            Some(body),
        )
        .await
    }

    /// Multipart body with a bearer token. `file` is one uploaded part;
    /// `fields` are text parts (bracket keys allowed).
    pub async fn post_multipart(
        &self,
        path: &str,
        token: &str,
        fields: &[(&str, &str)],
        file: Option<FilePart<'_>>,
    ) -> Resp {
        let auth = format!("Bearer {token}");
        let mut form = reqwest::multipart::Form::new();
        for (key, value) in fields {
            form = form.text((*key).to_owned(), (*value).to_owned());
        }
        if let Some(file) = file {
            let part = reqwest::multipart::Part::bytes(file.bytes.to_vec())
                .file_name(file.filename.to_owned())
                .mime_str(file.content_type)
                .expect("content type");
            form = form.part(file.field.to_owned(), part);
        }
        let req = self
            .http
            .request(Method::POST, self.url(path))
            .header(
                "Authorization",
                HeaderValue::from_str(&auth).expect("authorization header"),
            )
            .multipart(form);
        read(req.send().await.expect("send multipart")).await
    }
}

/// One file part of a multipart request.
pub struct FilePart<'a> {
    pub field: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

fn append_query(path: &str, fields: &[(&str, &str)]) -> String {
    if fields.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", form_encode(fields))
    }
}

/// `application/x-www-form-urlencoded` encoding. Unreserved characters stay
/// literal; everything else (including `[]`) is percent-encoded.
pub fn form_encode(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn encode_multipart(
    fields: &[(&str, &str)],
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> (String, Vec<u8>) {
    let boundary = format!("parity{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::new();
    for (name, value) in fields {
        push_str(
            &mut body,
            &format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            ),
        );
    }
    let quoted = quote_filename(filename);
    push_str(
        &mut body,
        &format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename={quoted}\r\nContent-Type: {content_type}\r\n\r\n"
        ),
    );
    body.extend_from_slice(bytes);
    push_str(&mut body, &format!("\r\n--{boundary}--\r\n"));
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn quote_filename(name: &str) -> String {
    let mut out = String::from("\"");
    for c in name.chars() {
        if c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn push_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
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
