# Sensor Dash

A dirt-simple, self-hostable app to **push numeric sensor readings over HTTP** and
visualize the data as **timeseries charts**. One small Rust binary + a SQLite file, shipped as
a tiny Docker image:
<p align="center">
<img width="584" height="525" alt="image" src="https://github.com/user-attachments/assets/5ee6ed70-5c69-4c4a-a307-f2b2bc5b867e" /> <img width="570" height="593" alt="image" src="https://github.com/user-attachments/assets/7679c449-089c-492b-98a9-d3b607cd9d03" />
</p>


- Push a reading: `POST /update_sensor/{device}/{sensor}` with the value in the body.
- Sensors **auto-create** on first post. All values are numeric.
- **Writes** need a key; **viewing is public**.

## Docker Quickstart (recommended)

```sh
# WRITE_KEYS = One or more named write keys: "name:secretkey,name2:secretkey2"
docker run -e WRITE_KEYS="esp-garage:supersecret" \
  -p 8000:8000 -v "$PWD/data:/data" akshaykannan/sensordash
```

## Local Development Quickstart

Run the tests: `cargo test`

Run the server:
```sh
# One or more named write keys: "name:secretkey,name2:secretkey2"
WRITE_KEYS="esp-garage:supersecret,ci:cikey" cargo run
```
Then navigate to: [http://localhost:8000](http://localhost:8000)

## Configuration (env vars)

| Var | Required | Default | Purpose |
|---|---|---|---|
| `WRITE_KEYS` | **yes** | — | `name:secret` pairs, comma-separated. App refuses to start if empty. |
| `DB_PATH` | no | `sensors.db` (`/data/sensors.db` in Docker) | SQLite file location. |
| `PORT` | no | `8000` | Listen port. |
| `RETENTION_DAYS` | no | off | Prune readings older than N days (runs on start + daily). |

The write-key **name** is only for your own bookkeeping (which key is which / who to
blame in logs). Requests authenticate with the **secret** via the `X-API-Key` header.

- **Reads are public** — anyone with a device URL can see its data (e.g. temperature /
  occupancy patterns).
- The per-IP rate limit reads `X-Forwarded-For`, which is only trustworthy **behind a
  proxy you control**. Don't rely on it if the container is exposed directly.
- Rotate/revoke a key by editing `WRITE_KEYS` and redeploying.
