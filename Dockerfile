# syntax=docker/dockerfile:1
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app
# sccache: a compiler-level cache. Layered on top of cargo-chef so that even when
# a layer is invalidated (e.g. a dependency bump busts the cook layer), only the
# crates that actually changed are recompiled; the rest are pulled from the
# cache. Install the prebuilt musl binary rather than `cargo install` (no build).
ARG SCCACHE_VERSION=0.10.0
RUN curl -fsSL "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
      | tar -xz -C /tmp \
  && mv "/tmp/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl/sccache" /usr/local/bin/sccache \
  && chmod +x /usr/local/bin/sccache \
  && rm -rf /tmp/sccache-*

# `with-sccache <cmd...>` enables sccache as RUSTC_WRAPPER *only if* its server
# can actually start with the configured backend. If GCS init fails (e.g. the
# metadata creds are unreachable), it logs why and runs the command WITHOUT a
# cache rather than failing — a build cache must never break the build. (When the
# server can't start, every rustc call fails fatally, which is what this guards.)
# Credentials are picked up automatically from the GCE metadata server (the build
# reaches Cloud Build's spoofed metadata via the `cloudbuild` network + the
# --add-host in cloudbuild.yaml), so no key file is needed.
RUN cat > /usr/local/bin/with-sccache <<'EOF' && chmod +x /usr/local/bin/with-sccache
#!/usr/bin/env bash
set -euo pipefail
if [ -n "${SCCACHE_GCS_BUCKET:-}" ]; then
  case "${SCCACHE_GCS_BUCKET}" in
    gs://*) echo "SCCACHE_GCS_BUCKET must be a bucket name without gs://" >&2; exit 1 ;;
  esac
  export SCCACHE_GCS_RW_MODE="${SCCACHE_GCS_RW_MODE:-READ_WRITE}"
  export SCCACHE_GCS_KEY_PREFIX="${SCCACHE_GCS_KEY_PREFIX:-${SCCACHE_GCS_PREFIX:-wf-cargo}}"
  if timeout 30 sccache --start-server >/tmp/sccache-start.log 2>&1; then
    export RUSTC_WRAPPER=sccache
    echo "sccache: enabled — gs://${SCCACHE_GCS_BUCKET} (prefix ${SCCACHE_GCS_KEY_PREFIX})"
  else
    echo "sccache: server failed to start; building WITHOUT cache" >&2
    sed 's/^/sccache: /' /tmp/sccache-start.log >&2 || true
  fi
else
  echo "sccache: no SCCACHE_GCS_BUCKET set; building without remote cache"
fi
status=0
"$@" || status=$?
[ -n "${RUSTC_WRAPPER:-}" ] && { sccache --show-stats || true; }
exit "$status"
EOF

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# When set, sccache stores its cache in this GCS bucket (shared across builds);
# SCCACHE_GCS_PREFIX scopes the cache keys. Passed via `--build-arg` from Cloud
# Build; left empty for local `docker build`, where `with-sccache` builds without
# a remote cache.
ARG SCCACHE_GCS_BUCKET=
ARG SCCACHE_GCS_PREFIX=wf-cargo
# Incremental compilation defeats sccache, so disable it for these builds.
ENV CARGO_INCREMENTAL=0

COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer! The cargo cache mounts
# speed up within-build crate downloads; cross-build persistence comes from the
# registry cache (cloudbuild.yaml) and sccache+GCS, which compose without overlap.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    with-sccache cargo chef cook --release --recipe-path recipe.json
# Build application (the `wf-api` binary from the workspace).
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    with-sccache cargo build --release --bin wf-api

# We do not need the Rust toolchain to run the binary!
FROM debian:trixie-slim AS runtime
WORKDIR /app
# wf-api makes outbound HTTPS calls (GitHub, Jira, Supabase JWKS, Postgres/TLS),
# so the runtime image needs CA root certificates — the slim base ships none.
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/wf-api /usr/local/bin
ENTRYPOINT ["/usr/local/bin/wf-api"]
