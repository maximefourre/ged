//! The admin web UI: session login, CSRF, the management screens, and the
//! Rails behaviours that sit around them (method override, default headers,
//! static assets).
//!
//! Documents, preview, bulk actions and the trash are [`documents`] and
//! [`trash`]. Static files are [`assets`], outside the session guard.

pub mod app_clients;
pub mod assets;
pub mod csrf;
pub mod departments;
pub mod disk_space;
pub mod documents;
pub mod session;
pub mod status;
pub mod system_health;
pub mod trash;
pub mod upload_requests;
pub mod view;
mod zip_stream;

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::handler::Handler as _;
use axum::http::header::{CONTENT_TYPE, COOKIE, HeaderValue, ORIGIN, SET_COOKIE};
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use http_body_util::BodyExt as _;

use crate::http::AppState;
use crate::http::params::{BodyKind, FORM_BYTES_LIMIT, RackParams};
use session::{Codec, RateLimiter};
use view::Ctx;

/// Process-wide admin state, built once with the [`AppState`].
pub struct AdminRuntime {
    codec: Codec,
    login_limiter: RateLimiter,
    /// Hash verified for unknown usernames, built at boot.
    decoy: String,
}

impl AdminRuntime {
    /// `SECRET_KEY_BASE` signs the sessions; without one (development, test)
    /// a random key lasts for the life of the process.
    pub fn new(secret_key_base: Option<&str>) -> Self {
        let secret = secret_key_base.map_or_else(
            || {
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                )
            },
            str::to_owned,
        );
        Self {
            codec: Codec::new(&secret),
            login_limiter: session::login_limiter(),
            decoy: session::make_decoy(),
        }
    }
}

/// The admin routes. Everything but the assets runs behind [`guard`].
pub fn routes(state: AppState) -> Router<AppState> {
    // Lift axum's 2 MiB multipart default: the handler enforces the real cap.
    let upload_limit = usize::try_from(state.config.max_upload_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(1024 * 1024);
    let pages = Router::new()
        .route("/admins/sign_in", get(session::new).post(session::create))
        .route("/admins/sign_out", delete(session::destroy))
        .route("/admin", get(session::root))
        .route(
            "/admin/documents",
            get(documents::index)
                .post(documents::create.layer(DefaultBodyLimit::max(upload_limit))),
        )
        .route("/admin/documents/new", get(documents::new))
        .route(
            "/admin/documents/bulk_destroy",
            post(documents::bulk_destroy),
        )
        .route(
            "/admin/documents/bulk_download",
            get(documents::bulk_download),
        )
        .route("/admin/documents/{id}/edit", get(documents::edit))
        .route(
            "/admin/documents/{id}",
            patch(documents::update)
                .put(documents::update)
                .delete(documents::destroy),
        )
        .route("/admin/documents/{id}/download", get(documents::download))
        .route("/admin/documents/{id}/preview", get(documents::preview))
        .route("/admin/deleted_documents", get(trash::index))
        .route(
            "/admin/deleted_documents/{id}/restore",
            patch(trash::restore),
        )
        .route("/admin/status", get(status::index))
        .route(
            "/admin/departments",
            get(departments::index).post(departments::create),
        )
        .route("/admin/departments/new", get(departments::new))
        .route(
            "/admin/departments/{id}",
            patch(departments::update)
                .put(departments::update)
                .delete(departments::destroy),
        )
        .route("/admin/departments/{id}/edit", get(departments::edit))
        .route(
            "/admin/departments/{id}/confirm_deletion",
            get(departments::confirm_deletion),
        )
        .route(
            "/admin/departments/{id}/move_documents",
            post(departments::move_documents),
        )
        .route("/admin/departments/{id}/trash", post(departments::trash))
        .route(
            "/admin/departments/{id}/restore",
            patch(departments::restore),
        )
        .route(
            "/admin/app_clients",
            get(app_clients::index).post(app_clients::create),
        )
        .route("/admin/app_clients/new", get(app_clients::new))
        .route(
            "/admin/app_clients/{id}",
            patch(app_clients::update)
                .put(app_clients::update)
                .delete(app_clients::destroy),
        )
        .route("/admin/app_clients/{id}/edit", get(app_clients::edit))
        .route("/admin/upload_requests", get(upload_requests::index))
        .route(
            "/admin/upload_queue_pause",
            post(upload_requests::pause).delete(upload_requests::resume),
        )
        .route_layer(middleware::from_fn_with_state(state, guard));
    // Static files carry no session: they stay outside the guard.
    let files = Router::new()
        .route("/assets/{file}", get(assets::digested_asset))
        .route("/icon.png", get(assets::icon_png))
        .route("/icon.svg", get(assets::icon_svg))
        .route("/robots.txt", get(assets::robots));
    pages.merge(files)
}

fn is_admin_path(path: &str) -> bool {
    path == "/admin" || path.starts_with("/admin/") || path.starts_with("/admins/")
}

/// Whether a request changes state: Rails verifies the token on all but
/// GET and HEAD.
fn needs_csrf(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD)
}

fn cookie_value(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then(|| v.to_owned())
        })
}

fn session_cookie(value: &str) -> String {
    // Like Rails' cookie store: no Max-Age (a browser session), HttpOnly,
    // SameSite=Lax. No `Secure`: TLS ends at the proxy (`assume_ssl` is off).
    format!(
        "{}={value}; path=/; httponly; samesite=lax",
        session::COOKIE_NAME
    )
}

/// The token a request carries: the `authenticity_token` field of a form, or
/// the `X-CSRF-Token` header (Rails checks the form first).
fn submitted_token(form: Option<&RackParams>, req: &Request) -> Option<String> {
    form.and_then(|f| f.str("authenticity_token"))
        .map(str::to_owned)
        .or_else(|| {
            req.headers()
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
}

/// Buffers a urlencoded body so it can be read and still handed on.
async fn buffer_form(req: Request) -> Result<(Request, Option<RackParams>), Box<Response>> {
    if BodyKind::of(req.method(), req.headers().get(CONTENT_TYPE)) != BodyKind::Form {
        return Ok((req, None));
    }
    let (parts, body) = req.into_parts();
    let limited = http_body_util::Limited::new(body, FORM_BYTES_LIMIT);
    let bytes = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Err(Box::new(view::error_page(StatusCode::BAD_REQUEST))),
    };
    let form = RackParams::parse_form_body(&bytes).ok();
    Ok((Request::from_parts(parts, Body::from(bytes)), form))
}

/// Session, forgery protection and authentication, in Rails' order: the
/// token is checked before anyone is asked to sign in.
async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let runtime = &state.admin;
    let data = cookie_value(&req, session::COOKIE_NAME)
        .and_then(|v| runtime.codec.decode(&v))
        .unwrap_or_default();
    let original = data.clone();
    let ctx = Ctx::new(data, view::base_url(req.headers()));
    let mut response = run_guarded(&state, &ctx, req, next).await;
    let after = ctx.session();
    if after != original {
        let value = runtime.codec.encode(&after);
        if let Ok(v) = HeaderValue::from_str(&session_cookie(&value)) {
            response.headers_mut().append(SET_COOKIE, v);
        }
    }
    view::cache_headers(&mut response);
    response
}

/// The one multipart route that reads its own token: the upload, whose token
/// field may follow the file and so cannot be had without reading the file.
fn is_upload(method: &Method, path: &str) -> bool {
    *method == Method::POST && path == "/admin/documents"
}

fn is_multipart(req: &Request) -> bool {
    req.headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("multipart/form-data")
        })
}

/// Buffers a (small) multipart body to read its `authenticity_token` field,
/// then hands the body on untouched.
async fn buffer_multipart(req: Request) -> Result<(Request, Option<String>), Box<Response>> {
    use axum::extract::{FromRequest as _, Multipart};
    let (parts, body) = req.into_parts();
    let limited = http_body_util::Limited::new(body, FORM_BYTES_LIMIT);
    let bytes = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Err(Box::new(view::error_page(StatusCode::BAD_REQUEST))),
    };
    let copy = Request::from_parts(parts.clone(), Body::from(bytes.clone()));
    let mut token = None;
    if let Ok(mut form) = Multipart::from_request(copy, &()).await {
        while let Ok(Some(field)) = form.next_field().await {
            if field.name() == Some("authenticity_token") {
                token = field.text().await.ok();
                break;
            }
        }
    }
    Ok((Request::from_parts(parts, Body::from(bytes)), token))
}

async fn run_guarded(state: &AppState, ctx: &Ctx, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let defers_token = is_multipart(&req) && is_upload(&method, req.uri().path());
    let multipart = !defers_token && is_multipart(&req);

    let (mut req, form) = match buffer_form(req).await {
        Ok(pair) => pair,
        Err(response) => return *response,
    };
    let mut multipart_token = None;
    if needs_csrf(&method) && multipart {
        match buffer_multipart(req).await {
            Ok((r, token)) => (req, multipart_token) = (r, token),
            Err(response) => return *response,
        }
    }

    if needs_csrf(&method) {
        // Origin is always checked, the upload included. Its token is checked
        // by the handler once it has read the whole form (`Ctx::verify_csrf`);
        // any other request, multipart or not, is checked here.
        let origin_ok = req
            .headers()
            .get(ORIGIN)
            .is_none_or(|o| o.to_str().is_ok_and(|o| o == ctx.base()));
        let token = submitted_token(form.as_ref(), &req).or(multipart_token);
        if !origin_ok || !(defers_token || ctx.verify_csrf(token.as_deref())) {
            return view::error_page(StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    let path = req.uri().path().to_owned();
    let admin = match session::current_admin(&state.db, &ctx.session()).await {
        Ok(admin) => admin,
        Err(e) => return axum::response::IntoResponse::into_response(view::AdminError::from(e)),
    };
    ctx.set_admin(admin.clone());

    let public = path == "/admins/sign_in" || path == "/admins/sign_out";
    if admin.is_none() && !public {
        return session::refuse(ctx, &method, &req);
    }
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip());
    let client = session::client_key(peer, req.headers());
    req.extensions_mut().insert(session::ClientKey(client));
    req.extensions_mut().insert(ctx.clone());
    next.run(req).await
}

/// `Rack::MethodOverride`: a form POST may name its real verb in `_method`
/// (or the `X-HTTP-Method-Override` header). Runs before routing.
async fn method_override(req: Request, next: Next) -> Response {
    if req.method() != Method::POST || !is_admin_path(req.uri().path()) {
        return next.run(req).await;
    }
    let header = req
        .headers()
        .get("x-http-method-override")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let (mut req, form) = match buffer_form(req).await {
        Ok(pair) => pair,
        Err(response) => return *response,
    };
    let wanted = form
        .as_ref()
        .and_then(|f| f.str("_method").map(str::to_owned))
        .or(header);
    if let Some(wanted) = wanted
        && let Ok(method) = Method::from_bytes(wanted.to_ascii_uppercase().as_bytes())
        && matches!(
            method.as_str(),
            "GET" | "HEAD" | "PUT" | "POST" | "DELETE" | "OPTIONS" | "PATCH" | "LINK" | "UNLINK"
        )
    {
        *req.method_mut() = method;
    }
    next.run(req).await
}

/// Wraps the finished application with what must run before routing: the
/// method override. (Rails' default response headers are the app-wide
/// `http::rails_headers` layer, which covers the admin pages too.)
pub fn wrap(app: Router) -> Router {
    Router::new()
        .fallback_service(app)
        .layer(middleware::from_fn(method_override))
}

/// Where uploaded files live: `GED_STORAGE_ROOT`, else `storage` (the
/// directory the compose `storage` volume is mounted on).
pub fn storage_root() -> std::path::PathBuf {
    std::env::var("GED_STORAGE_ROOT")
        .ok()
        .filter(|v| !v.is_empty())
        .map_or_else(|| "storage".into(), Into::into)
}
