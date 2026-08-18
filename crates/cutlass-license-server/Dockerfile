# Build context is the REPO ROOT (needs the workspace manifests), e.g.:
#   docker build -f crates/cutlass-license-server/Dockerfile -t cutlass-license .
# Only the license server + cutlass-license are compiled — not the media crates.

FROM rust:1-slim AS build
WORKDIR /src
# build-essential gives cc for rusqlite's bundled SQLite; ca-certificates for crates.io
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release -p cutlass-license-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/cutlass-license-server /app/cutlass-license-server
# SQLite lives here; mount a persistent volume at /data (Railway: mount path
# /data) so licenses survive redeploys. DON'T set CUTLASS_PORT — the platform
# injects $PORT and the server binds it automatically.
ENV CUTLASS_DB_PATH=/data/cutlass-license.db
EXPOSE 8787
CMD ["/app/cutlass-license-server"]
