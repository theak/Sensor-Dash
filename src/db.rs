//! SQLite access. One connection lives behind a Mutex in AppState; these are the
//! only queries the app runs. Sensors are implicit — a device's sensors are just the
//! distinct `sensor` values present in `readings`, so there is no sensors table.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS devices (
  id         INTEGER PRIMARY KEY,
  name       TEXT UNIQUE NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS readings (
  id        INTEGER PRIMARY KEY,
  device_id INTEGER NOT NULL REFERENCES devices(id),
  sensor    TEXT NOT NULL,
  ts        INTEGER NOT NULL,
  value     REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_readings_lookup ON readings(device_id, sensor, ts);
";

/// Open (or create) the database, enable WAL, and ensure the schema exists.
pub fn init(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // WAL lets reads proceed while a write is in flight; NORMAL is the safe pairing.
    // (Both are no-ops for an in-memory DB, which is what the tests use.)
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    // Keep temp b-trees in memory so the `scratch` container needs no temp directory.
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Insert a new device. Returns Err on a UNIQUE violation (caller maps to 409).
pub fn create_device(conn: &Connection, name: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO devices (name, created_at) VALUES (?1, ?2)",
        params![name, now],
    )?;
    Ok(())
}

/// Look up a device id by name, or None if it doesn't exist.
pub fn device_id(conn: &Connection, name: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT id FROM devices WHERE name = ?1", [name], |r| r.get(0))
        .optional()
}

/// Summary of one device for the homepage list. Serializes straight to the JSON the
/// homepage expects, so handlers can return it without a hand-built `json!` mapping.
#[derive(Serialize)]
pub struct DeviceSummary {
    pub name: String,
    pub created_at: i64,
    pub sensor_count: i64,
    pub last_seen: Option<i64>,
}

/// All devices, alphabetical, each with its sensor count and most-recent reading time.
pub fn list_devices(conn: &Connection) -> rusqlite::Result<Vec<DeviceSummary>> {
    let mut stmt = conn.prepare(
        "SELECT d.name, d.created_at,
                (SELECT COUNT(DISTINCT sensor) FROM readings r WHERE r.device_id = d.id),
                (SELECT MAX(ts) FROM readings r WHERE r.device_id = d.id)
         FROM devices d
         ORDER BY d.name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DeviceSummary {
            name: r.get(0)?,
            created_at: r.get(1)?,
            sensor_count: r.get(2)?,
            last_seen: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// Append one reading. The sensor name is free-form; a new name auto-creates the sensor.
pub fn insert_reading(
    conn: &Connection,
    device_id: i64,
    sensor: &str,
    ts: i64,
    value: f64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO readings (device_id, sensor, ts, value) VALUES (?1, ?2, ?3, ?4)",
        params![device_id, sensor, ts, value],
    )?;
    Ok(())
}

/// One sensor's timeseries: ordered (ts, value) points. Serializes to the shape the
/// charts consume (`{"name": ..., "points": [[ts, value], ...]}`); tuples become arrays.
#[derive(Serialize)]
pub struct Series {
    #[serde(rename = "name")]
    pub sensor: String,
    pub points: Vec<(i64, f64)>,
}

/// Every sensor's points for a device since `since` (unix seconds), grouped by sensor.
pub fn device_data(conn: &Connection, device_id: i64, since: i64) -> rusqlite::Result<Vec<Series>> {
    let mut stmt = conn.prepare(
        "SELECT sensor, ts, value FROM readings
         WHERE device_id = ?1 AND ts >= ?2
         ORDER BY sensor, ts",
    )?;
    let rows = stmt.query_map(params![device_id, since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?))
    })?;

    // Preserve grouping in a stable (alphabetical) order for the UI.
    let mut grouped: BTreeMap<String, Vec<(i64, f64)>> = BTreeMap::new();
    for row in rows {
        let (sensor, ts, value) = row?;
        grouped.entry(sensor).or_default().push((ts, value));
    }
    Ok(grouped
        .into_iter()
        .map(|(sensor, points)| Series { sensor, points })
        .collect())
}

/// Delete readings older than `cutoff` (unix seconds). Returns rows removed.
pub fn prune(conn: &Connection, cutoff: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM readings WHERE ts < ?1", params![cutoff])
}

/// Delete a device and all of its readings in one transaction.
/// Returns false if no device by that name exists.
pub fn delete_device(conn: &mut Connection, name: &str) -> rusqlite::Result<bool> {
    let Some(id) = device_id(conn, name)? else {
        return Ok(false);
    };
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM readings WHERE device_id = ?1", params![id])?;
    tx.execute("DELETE FROM devices WHERE id = ?1", params![id])?;
    tx.commit()?;
    Ok(true)
}
