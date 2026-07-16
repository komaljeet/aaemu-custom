# syntax=docker/dockerfile:1
#
# Multi-stage build for the aaemu-custom sidecar.
#
#   build:  docker buildx build -t aaemu-custom:latest --load .
#   init:   docker run --rm --add-host=host.docker.internal:host-gateway \
#             -v "$PWD/config.docker.toml:/app/config.toml:ro" \
#             aaemu-custom:latest --init-db --config config.toml
#   run:    docker run -d --name aaemu-custom -p 1281:1281 \
#             --add-host=host.docker.internal:host-gateway \
#             -v "$PWD/config.docker.toml:/app/config.toml:ro" \
#             aaemu-custom:latest
#
# config.toml is NOT baked in (it holds DB credentials). Mount it at run time,
# or copy config.example.toml -> config.toml and edit the [database] connection.

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
EXPOSE 1281
ENTRYPOINT ["aaemu-custom"]
CMD ["--config", "config.toml"]