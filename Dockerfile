# syntax=docker/dockerfile:1
#
# Multi-stage build for the aaemu-custom sidecar.
#
#   build:  docker buildx build -t aaemu-custom:latest --load .
#   init:   docker run --rm --add-host=host.docker.internal:host-gateway \
#             -v "$PWD/config.docker.toml:/app/config.toml:ro" \
#             aaemu-custom:latest --init-db
#   run:    docker run -d --name aaemu-custom -p 1281:1281 \
#             --add-host=host.docker.internal:host-gateway \
#             -v "$PWD/config.docker.toml:/app/config.toml:ro" \
#             aaemu-custom:latest
#
# config.toml is NOT baked in (it holds DB credentials). Two ways to supply it:
#   - Mount one at /app/config.toml (read-only) — used verbatim (Start-AAEmu.ps1
#     does this with config.docker.toml). No env needed.
#   - Or pass SIDECAR_DB_URL + SIDECAR_LISTEN env and mount nothing — the
#     docker-entrypoint.sh generates /tmp/config.toml from config.example.toml
#     with those two lines overridden (the docker-compose.yaml deploy path).
# In both cases the entrypoint supplies --config; don't pass --config yourself.

# --- builder ----------------------------------------------------------------
FROM rust:1.88 AS builder
WORKDIR /app

# Cache the dependency tree: build a dummy crate from the manifests first so
# source-only changes don't re-download and recompile every dependency.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
 && printf 'fn main() {}\n' > src/main.rs \
 && printf '' > src/lib.rs \
 && cargo build --release || true \
 && rm -rf src

# Real source + schema, then build. `touch` invalidates the dummy artifacts.
COPY src ./src
COPY schema.sql ./
RUN touch src/main.rs src/lib.rs && cargo build --release

# --- runtime ----------------------------------------------------------------
# rustls (no native OpenSSL) + dynamic glibc only -> debian-slim is enough.
# rust:1.88 is bookworm-based, so the binary's glibc matches bookworm-slim.
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /app/target/release/aaemu-custom /usr/local/bin/aaemu-custom
COPY schema.sql config.example.toml ./
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh \
 && sed -i 's/\r$//' /usr/local/bin/docker-entrypoint.sh
EXPOSE 1281
# The entrypoint generates /tmp/config.toml from config.example.toml + the
# SIDECAR_DB_URL / SIDECAR_LISTEN env vars, then execs the binary. Extra args
# (e.g. --init-db) pass through as "$@".
ENTRYPOINT ["docker-entrypoint.sh"]