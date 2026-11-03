//! The admin's static files. The CSS and JS are Rails' own files, embedded
//! unchanged; like Propshaft they are served under a content-digested name
//! with a one-year cache. `public/` files (icons, robots.txt) keep their name.

use std::sync::OnceLock;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

const CSS: &[u8] = include_bytes!("../../assets/application.css");
const JS: &[u8] = include_bytes!("../../assets/application.js");
const ICON_PNG: &[u8] = include_bytes!("../../assets/icon.png");
const ICON_SVG: &[u8] = include_bytes!("../../assets/icon.svg");
const ROBOTS: &[u8] = include_bytes!("../../assets/robots.txt");

/// `Rails.application.config.public_file_server.headers` and Propshaft's header.
const CACHE: &str = "public, max-age=31556952";

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct Digested {
    css: String,
    js: String,
}

fn digested() -> &'static Digested {
    static NAMES: OnceLock<Digested> = OnceLock::new();
    NAMES.get_or_init(|| Digested {
        css: format!("application-{}.css", digest(CSS)),
        js: format!("application-{}.js", digest(JS)),
    })
}

/// `(stylesheet url, script url)` as the layout links them.
pub fn urls() -> (String, String) {
    let d = digested();
    (format!("/assets/{}", d.css), format!("/assets/{}", d.js))
}

fn file(content_type: &'static str, bytes: &'static [u8]) -> Response {
    let mut response = (StatusCode::OK, bytes).into_response();
    let h = response.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE));
    // Propshaft and the public file server are not controllers: no
    // X-Frame-Options and no ETag rewrite of this cache header.
    crate::http::bare(response)
}

/// `GET /assets/:file`: only the exact digested names exist.
pub async fn digested_asset(Path(name): Path<String>) -> Response {
    let d = digested();
    if name == d.css {
        file("text/css", CSS)
    } else if name == d.js {
        file("text/javascript", JS)
    } else {
        crate::http::bare(StatusCode::NOT_FOUND.into_response())
    }
}

pub async fn icon_png() -> Response {
    file("image/png", ICON_PNG)
}

pub async fn icon_svg() -> Response {
    file("image/svg+xml", ICON_SVG)
}

pub async fn robots() -> Response {
    file("text/plain", ROBOTS)
}
