//! HTTP handlers: static pages/assets, write endpoints (auth-gated), and read APIs.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{db, AppState};

// Frontend assets are baked into the binary so the container ships one file and
// never reaches out to a CDN.
const INDEX_HTML: &str = include_str!("../static/index.html");
const DEVICE_HTML: &str = include_str!("../static/device.html");
const APP_JS: &str = include_str!("../static/app.js");
const STYLE_CSS: &str = include_str!("../static/style.css");
const PICO_CSS: &str = include_str!("../static/pico.min.css");
const UPLOT_CSS: &str = include_str!("../static/uplot.min.css");
const UPLOT_JS: &str = include_str!("../static/uplot.min.js");
const FAVICON_SVG: &str = include_str!("../static/favicon.svg");

/// Current unix time in seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Device and sensor names must be short and URL-safe (spaces allowed; the frontend
/// percent-encodes names in every URL it builds).
fn valid_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b' ')
}

/// True if the request carries a valid write key in `X-API-Key`. Compared in
/// constant time against every configured secret (no early return on match).
fn authorized(headers: &HeaderMap, state: &AppState) -> bool {
    let Some(presented) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let pb = presented.as_bytes();
    let mut ok = false;
    for k in state.keys.iter() {
        if constant_time_eq::constant_time_eq(pb, k.secret.as_bytes()) {
            ok = true;
        }
    }
    ok
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key").into_response()
}

// ---- Pages & static assets ----

pub async fn index_page() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Same shell for every device; the page's JS reads the device name from the URL.
pub async fn device_page() -> Html<&'static str> {
    Html(DEVICE_HTML)
}

pub async fn static_asset(Path(file): Path<String>) -> Response {
    let (ct, body): (&str, &str) = match file.as_str() {
        "app.js" => ("text/javascript; charset=utf-8", APP_JS),
        "style.css" => ("text/css; charset=utf-8", STYLE_CSS),
        "pico.min.css" => ("text/css; charset=utf-8", PICO_CSS),
        "uplot.min.css" => ("text/css; charset=utf-8", UPLOT_CSS),
        "uplot.min.js" => ("text/javascript; charset=utf-8", UPLOT_JS),
        "favicon.svg" => ("image/svg+xml", FAVICON_SVG),
        _ => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    ([(header::CONTENT_TYPE, ct)], body).into_response()
}

// ---- Write endpoints (require a valid write key) ----

#[derive(Deserialize)]
pub struct CreateDevice {
    pub name: String,
}

pub async fn create_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateDevice>,
) -> Response {
    if !authorized(&headers, &state) {
        return unauthorized();
    }
    let name = body.name.trim();
    if !valid_name(name) {
        return (
            StatusCode::BAD_REQUEST,
            "invalid device name (allowed: letters, digits, space, _ and - ; 1-64 chars)",
        )
            .into_response();
    }
    let conn = state.db.lock().await;
    match db::create_device(&conn, name, now()) {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true, "name": name}))).into_response(),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            (StatusCode::CONFLICT, "device already exists").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    }
}

#[derive(Deserialize)]
pub struct ValueQuery {
    pub value: Option<String>,
}

pub async fn update_sensor(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((device, sensor)): Path<(String, String)>,
    Query(q): Query<ValueQuery>,
    body: String,
) -> Response {
    if !authorized(&headers, &state) {
        return unauthorized();
    }
    if !valid_name(&device) || !valid_name(&sensor) {
        return (StatusCode::BAD_REQUEST, "invalid device or sensor name").into_response();
    }
    // Value from the request body first (curl -d "23.5"), else the ?value= fallback.
    let raw = if !body.trim().is_empty() {
        body.trim().to_string()
    } else {
        q.value.unwrap_or_default()
    };
    let value: f64 = match raw.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => v,
        _ => return (StatusCode::BAD_REQUEST, "value must be a finite number").into_response(),
    };

    let conn = state.db.lock().await;
    let Some(did) = db::device_id(&conn, &device).unwrap_or(None) else {
        return (
            StatusCode::NOT_FOUND,
            "unknown device — create it in the UI first",
        )
            .into_response();
    };
    match db::insert_reading(&conn, did, &sensor, now(), value) {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    }
}

pub async fn delete_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if !authorized(&headers, &state) {
        return unauthorized();
    }
    let mut conn = state.db.lock().await;
    // &mut *conn — hand the underlying Connection to the transaction.
    match db::delete_device(&mut conn, &name) {
        Ok(true) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "unknown device").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    }
}

// ---- Read APIs (public) ----

pub async fn list_devices(State(state): State<AppState>) -> Response {
    let conn = state.db.lock().await;
    match db::list_devices(&conn) {
        Ok(devs) => Json(json!({ "devices": devs })).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    }
}

#[derive(Deserialize)]
pub struct SinceQuery {
    pub since: Option<i64>,
}

pub async fn device_data(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<SinceQuery>,
) -> Response {
    let conn = state.db.lock().await;
    let Some(did) = db::device_id(&conn, &name).unwrap_or(None) else {
        return (StatusCode::NOT_FOUND, "unknown device").into_response();
    };
    // Default window: the last 24h.
    let since = q.since.unwrap_or_else(|| now() - 24 * 3600);
    match db::device_data(&conn, did, since) {
        Ok(series) => Json(json!({ "device": name, "sensors": series })).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    }
}
