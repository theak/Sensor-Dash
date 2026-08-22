//! Integration tests exercising the router end-to-end against an in-memory SQLite,
//! via `oneshot`. The rate-limit layer is intentionally excluded here (it's added in
//! `main`), so these focus on auth, validation, and data behavior.

use crate::{build_router, parse_keys, AppState};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower::ServiceExt;

fn test_state() -> AppState {
    let conn = crate::db::init(":memory:").expect("in-memory db");
    AppState {
        db: Arc::new(Mutex::new(conn)),
        keys: Arc::new(parse_keys("test:secret")),
    }
}

async fn send(state: &AppState, req: Request<Body>) -> Response {
    build_router(state.clone()).oneshot(req).await.unwrap()
}

fn post_json(uri: &str, body: &str, key: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(k) = key {
        b = b.header("x-api-key", k);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn post_value(uri: &str, body: &str, key: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("POST").uri(uri);
    if let Some(k) = key {
        b = b.header("x-api-key", k);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn delete(uri: &str, key: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("DELETE").uri(uri);
    if let Some(k) = key {
        b = b.header("x-api-key", k);
    }
    b.body(Body::empty()).unwrap()
}

async fn json_body(resp: Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn parse_keys_skips_malformed_parts() {
    let ks = parse_keys("a:1, b:2 ,,bad, :nosecret, name:,c:3");
    let names: Vec<_> = ks.iter().map(|k| k.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
    assert_eq!(ks[0].secret, "1");
}

#[tokio::test]
async fn create_device_requires_valid_key() {
    let state = test_state();

    // No key.
    let r = send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, None)).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Wrong key.
    let r = send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, Some("nope"))).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Right key.
    let r = send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, Some("secret"))).await;
    assert_eq!(r.status(), StatusCode::OK);

    // Duplicate.
    let r = send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, Some("secret"))).await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn create_device_rejects_bad_names() {
    let state = test_state();
    for bad in [r#"{"name":"bad!name"}"#, r#"{"name":"has/slash"}"#, r#"{"name":""}"#] {
        let r = send(&state, post_json("/api/devices", bad, Some("secret"))).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "should reject {bad}");
    }
}

#[tokio::test]
async fn device_names_may_contain_spaces() {
    let state = test_state();
    assert_eq!(
        send(&state, post_json("/api/devices", r#"{"name":"my garage"}"#, Some("secret"))).await.status(),
        StatusCode::OK
    );
    // Reading round-trips under the spaced name.
    assert_eq!(
        send(&state, post_value("/update_sensor/my%20garage/temp", "21.5", Some("secret"))).await.status(),
        StatusCode::OK
    );
    let v = json_body(send(&state, get("/api/devices/my%20garage/data?since=0")).await).await;
    assert_eq!(v["device"], "my garage");
    assert_eq!(v["sensors"][0]["points"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn post_reading_flow() {
    let state = test_state();

    // Unknown device -> 404 (even with a valid key).
    let r = send(&state, post_value("/update_sensor/garage/temp", "21.5", Some("secret"))).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // Create the device.
    send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, Some("secret"))).await;

    // Missing key -> 401.
    let r = send(&state, post_value("/update_sensor/garage/temp", "21.5", None)).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Non-numeric body -> 400.
    let r = send(&state, post_value("/update_sensor/garage/temp", "hot", Some("secret"))).await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    // Two sensors auto-create on first post.
    assert_eq!(
        send(&state, post_value("/update_sensor/garage/temp", "21.5", Some("secret"))).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&state, post_value("/update_sensor/garage/humidity", "55", Some("secret"))).await.status(),
        StatusCode::OK
    );

    // Read back: two distinct sensors, temp has our one point.
    let r = send(&state, get("/api/devices/garage/data?since=0")).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_body(r).await;
    let sensors = v["sensors"].as_array().unwrap();
    assert_eq!(sensors.len(), 2);
    let names: Vec<&str> = sensors.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"temp") && names.contains(&"humidity"));

    let temp = sensors.iter().find(|s| s["name"] == "temp").unwrap();
    let pts = temp["points"].as_array().unwrap();
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0][1].as_f64().unwrap(), 21.5);
}

#[tokio::test]
async fn value_can_come_from_query_param() {
    let state = test_state();
    send(&state, post_json("/api/devices", r#"{"name":"g"}"#, Some("secret"))).await;

    // Empty body, value supplied via ?value=.
    let r = send(&state, post_value("/update_sensor/g/s?value=3.14", "", Some("secret"))).await;
    assert_eq!(r.status(), StatusCode::OK);

    let v = json_body(send(&state, get("/api/devices/g/data?since=0")).await).await;
    let pts = v["sensors"][0]["points"].as_array().unwrap();
    assert_eq!(pts[0][1].as_f64().unwrap(), 3.14);
}

#[tokio::test]
async fn data_for_unknown_device_is_404() {
    let state = test_state();
    let r = send(&state, get("/api/devices/ghost/data")).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_device_flow() {
    let state = test_state();
    send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, Some("secret"))).await;
    send(&state, post_value("/update_sensor/garage/temp", "21.5", Some("secret"))).await;

    // Delete needs a valid key.
    assert_eq!(
        send(&state, delete("/api/devices/garage", None)).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Delete removes the device...
    assert_eq!(
        send(&state, delete("/api/devices/garage", Some("secret"))).await.status(),
        StatusCode::OK
    );
    // ...and its readings (data now 404s since the device is gone).
    assert_eq!(
        send(&state, get("/api/devices/garage/data?since=0")).await.status(),
        StatusCode::NOT_FOUND
    );
    // Device list is empty again.
    let v = json_body(send(&state, get("/api/devices")).await).await;
    assert_eq!(v["devices"].as_array().unwrap().len(), 0);

    // Deleting a missing device is a 404.
    assert_eq!(
        send(&state, delete("/api/devices/garage", Some("secret"))).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn list_devices_is_public() {
    let state = test_state();
    send(&state, post_json("/api/devices", r#"{"name":"garage"}"#, Some("secret"))).await;
    let r = send(&state, get("/api/devices")).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_body(r).await;
    assert_eq!(v["devices"][0]["name"], "garage");
}
