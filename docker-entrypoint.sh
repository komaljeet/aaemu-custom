#!/bin/sh
# Generate /tmp/config.toml from the in-image config.example.toml (which carries
# the full economy tuning) by overriding the [database] connection and [api]
# listen lines from environment variables, then exec the sidecar binary.
#
# This lets `docker compose` pass the DB URL and listen address via env
# (SIDECAR_DB_URL / SIDECAR_LISTEN) with no secrets-laden config file on the host.
# The rest of the economy config is taken verbatim from config.example.toml.
#
# If the env vars are unset, the example file's defaults stand — so a plain
# `docker run` with a mounted config.toml still works (the binary is invoked with
# --config /tmp/config.toml, which falls back to the example's 127.0.0.1 values).
set -eu

# If a config.toml was mounted at /app/config.toml (e.g. Start-AAEmu.ps1 mounts
# config.docker.toml here), use it verbatim — don't override with env. This keeps
# the Windows dev path working unchanged. The generate-from-env path below is for
# the compose deploy, which mounts no config.toml.
if [ -f /app/config.toml ]; then
    exec aaemu-custom --config /app/config.toml "$@"
fi

TEMPLATE="/app/config.example.toml"
OUT="/tmp/config.toml"

cp "$TEMPLATE" "$OUT"

if [ -n "${SIDECAR_DB_URL:-}" ]; then
  # Replace the [database] connection line. The example uses:
  #   connection = "mysql://root:password@127.0.0.1:3306/aaemu_game"
  sed -i "s|^connection = .*|connection = \"$SIDECAR_DB_URL\"|" "$OUT"
fi

if [ -n "${SIDECAR_LISTEN:-}" ]; then
  # Replace the [api] listen line. The example uses:
  #   listen = "127.0.0.1:1281"
  sed -i "s|^listen = .*|listen = \"$SIDECAR_LISTEN\"|" "$OUT"
fi

exec aaemu-custom --config "$OUT" "$@"