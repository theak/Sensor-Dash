# ---- builder: static musl binary (rust:alpine targets musl by default) ----
FROM rust:1-alpine AS builder
# musl-dev provides the C toolchain rusqlite's bundled SQLite compiles against.
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY static ./static
RUN cargo build --release --locked

# ---- runtime: nothing but the binary ----
FROM scratch
COPY --from=builder /app/target/release/sensordash /sensordash
ENV DB_PATH=/data/sensors.db
ENV PORT=8000
EXPOSE 8000
VOLUME ["/data"]
ENTRYPOINT ["/sensordash"]
