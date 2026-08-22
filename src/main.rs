//! SensorDash — a minimal push-based sensor timeseries app.
//!
//! Ingest:  POST /update_sensor/{device}/{sensor}  (body = numeric value)  [write key]
//! View:    GET  /  and  /device/{name}                                    [public]

mod db;
mod handlers;
#[cfg(test)]
mod tests;

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
    Router,
};
use rusqlite::Connection;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};

/// One configured write key. The name is bookkeeping only (which key is which / who
/// to blame in logs); requests authenticate with the secret alone.
pub struct KeyEntry {
    pub name: String,
    pub secret: String,
}

/// Shared, cheaply-cloneable application state.
#[derive(Clone)]
pub struct AppState {
    /// A single SQLite connection behind a mutex. Home-telemetry volume is tiny, so
    /// one serialized connection (with WAL) is correct and far simpler than a pool.
    pub db: Arc<Mutex<Connection>>,
    /// Configured write keys, parsed once from WRITE_KEYS at boot.
    pub keys: Arc<Vec<KeyEntry>>,
}

/// Parse WRITE_KEYS="name:secret,name2:secret2" into entries, skipping malformed parts.
pub fn parse_keys(raw: &str) -> Vec<KeyEntry> {
    raw.split(',')
        .filter_map(|part| {
            let (name, secret) = part.trim().split_once(':')?;
            let (name, secret) = (name.trim(), secret.trim());
            if name.is_empty() || secret.is_empty() {
                return None;
            }
            Some(KeyEntry {
                name: name.to_string(),
                secret: secret.to_string(),
            })
        })
        .collect()
}

/// Build the app router (routes + body limit). The rate-limit layer is added
/// separately in `main` because it needs per-connection info; tests use this directly.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::index_page))
        .route("/device/{name}", get(handlers::device_page))
        .route("/static/{file}", get(handlers::static_asset))
        .route(
            "/api/devices",
            get(handlers::list_devices).post(handlers::create_device),
        )
        .route("/api/devices/{name}", delete(handlers::delete_device))
        .route("/api/devices/{name}/data", get(handlers::device_data))
        .route(
            "/update_sensor/{device}/{sensor}",
            post(handlers::update_sensor),
        )
        // Values and device names are tiny; cap the body so writes can't be abused.
        .layer(DefaultBodyLimit::max(1024))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    let keys = parse_keys(&std::env::var("WRITE_KEYS").unwrap_or_default());
    if keys.is_empty() {
        eprintln!(
            "FATAL: WRITE_KEYS is required. Example:\n  \
             WRITE_KEYS=\"esp-garage:s3cret1,ci:s3cret2\""
        );
        std::process::exit(1);
    }
    let key_names: Vec<&str> = keys.iter().map(|k| k.name.as_str()).collect();
    eprintln!(
        "sensordash: loaded {} write key(s): {}",
        keys.len(),
        key_names.join(", ")
    );

    let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "sensors.db".to_string());
    let conn = db::init(&db_path).expect("failed to open/initialize database");
    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        keys: Arc::new(keys),
    };

    // Optional retention: prune old readings on startup and once a day thereafter.
    if let Some(days) = std::env::var("RETENTION_DAYS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|d| *d > 0)
    {
        eprintln!("sensordash: retention enabled — keeping {days} day(s) of readings");
        let db = state.db.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(24 * 3600));
            loop {
                ticker.tick().await;
                let cutoff = handlers::now() - days * 86_400;
                let conn = db.lock().await;
                match db::prune(&conn, cutoff) {
                    Ok(n) if n > 0 => eprintln!("[retention] pruned {n} reading(s) older than {days}d"),
                    Ok(_) => {}
                    Err(e) => eprintln!("[retention] prune error: {e}"),
                }
            }
        });
    }

    // Per-IP rate limit on all routes: ~5 req/s sustained, bursts up to 60.
    // SmartIpKeyExtractor reads X-Forwarded-For / X-Real-IP (set by your proxy) and
    // falls back to the peer address — so trust it only behind a proxy you control.
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_millisecond(200)
            .burst_size(60)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("valid governor config"),
    );

    let app = build_router(state).layer(GovernorLayer::new(governor_conf));

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
    eprintln!("sensordash: listening on http://{addr}  (db: {db_path})");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
