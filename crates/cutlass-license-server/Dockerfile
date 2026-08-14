# Build context is the REPO ROOT (needs the workspace manifests), e.g.:
#   docker build -f crates/cutlass-license-server/Dockerfile -t cutlass-license .
# Only the license server + cutlass-license are compiled — not the media crates.

FROM rust:1-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release -p cutlass-license-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/cutlass-license-server /app/cutlass-license-server
# SQLite lives here; mount a volume at /data to persist across deploys.
ENV CUTLASS_DB_PATH=/data/cutlass-license.db
ENV CUTLASS_PORT=8787
EXPOSE 8787
CMD ["/app/cutlass-license-server"]
