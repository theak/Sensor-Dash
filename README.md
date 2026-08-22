# SensorDash

A dirt-simple, self-hostable app to **push numeric sensor readings over HTTP** and
visualize the data as **timeseries charts**. One small Rust binary + a SQLite file, shipped as
a tiny (~10–15 MB) `scratch` Docker image.

- Push a reading: `POST /update_sensor/{device}/{sensor}` with the value in the body.
- Sensors **auto-create** on first post. All values are numeric.
- **Writes** need a key; **viewing is public**.

## Quickstart (local)

```sh
# One or more named write keys: "name:secretkey,name2:secretkey2"
WRITE_KEYS="esp-garage:supersecret,ci:cikey" cargo run
# → http://localhost:8000
```

## Configuration (env vars)

| Var | Required | Default | Purpose |
|---|---|---|---|
| `WRITE_KEYS` | **yes** | — | `name:secret` pairs, comma-separated. App refuses to start if empty. |
| `DB_PATH` | no | `sensors.db` (`/data/sensors.db` in Docker) | SQLite file location. |
| `PORT` | no | `8000` | Listen port. |
| `RETENTION_DAYS` | no | off | Prune readings older than N days (runs on start + daily). |

The write-key **name** is only for your own bookkeeping (which key is which / who to
blame in logs). Requests authenticate with the **secret** via the `X-API-Key` header.

## Docker

```sh
docker build -t sensordash .
docker run -e WRITE_KEYS="esp-garage:supersecret" \
  -p 8000:8000 -v "$PWD/data:/data" sensordash
```

Or with Compose (pulls `akshaykannan/sensordash` from Docker Hub):

```sh
echo 'WRITE_KEYS=esp-garage:supersecret' > .env
docker compose up -d
```

The image is built `FROM scratch` with a static musl binary — no OS, no shell, no CA
certs (none are needed). Data lives on the `/data` volume.

## Deploying to the public internet

Put an HTTPS reverse proxy in front and point it at the container's port:

- **Cloudflare Tunnel** (nice for a home server): no open ports, TLS handled by
  Cloudflare, origin IP hidden. Add Cloudflare Access later if you ever want to gate
  reads too.
- **Caddy / Traefik / nginx** on a VPS: terminate TLS and proxy to `:8000`.
- Any PaaS that runs a container with a persistent volume for `sensors.db`.

### Is it risky to expose publicly?

Reasonably safe, with these things baked in:

- `WRITE_KEYS` required at boot; writes need `X-API-Key` (constant-time compared).
  The key travels in a **header, not the URL**, so it won't appear in access logs.
- Device/sensor names are validated; values must be finite numbers; request bodies are
  size-capped (1 KB); writes are rate-limited per IP.
- Optional `RETENTION_DAYS` caps storage growth if a key leaks or a device misbehaves.

Accept these knowingly:

- **Reads are public** — anyone with a device URL can see its data (e.g. temperature /
  occupancy patterns). Put the whole thing behind Cloudflare Access / basic-auth if
  that matters.
- The per-IP rate limit reads `X-Forwarded-For`, which is only trustworthy **behind a
  proxy you control**. Don't rely on it if the container is exposed directly.
- Rotate/revoke a key by editing `WRITE_KEYS` and redeploying.

## Development

```sh
cargo test        # unit/integration tests (in-memory SQLite)
cargo run         # needs WRITE_KEYS set
```

Frontend assets in `static/` (Pico + uPlot are vendored) are baked into the binary via
`include_str!`, so there's nothing to build and nothing to serve from a CDN.

### Multi-arch CI

`.github/workflows/ci.yml` runs tests, then on pushes to `main` builds and pushes a
multi-arch image (`linux/amd64,linux/arm64`) to `akshaykannan/sensordash:latest`.
Set repo secrets `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN`. Cross-compiling for arm64
runs under QEMU and is slow; if it drags, switch the builder stage to
`cargo-zigbuild` or `--platform=$BUILDPLATFORM` cross-compilation.
