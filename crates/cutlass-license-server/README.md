# Cutlass license server

Remembers, per hardware ID, when a trial started and whether it was purchased,
and hands the client a short **signed lease** it can trust offline. Because
trial-start lives here, uninstall + reinstall returns the *original* start date
— the trial can't be reset.

## Endpoints

| Method | Path | Body | Purpose |
|---|---|---|---|
| GET  | `/health` | — | liveness check |
| POST | `/activate` | `{ "hwid": "...", "app_version": "0.1.0" }` | create-or-fetch the trial; returns a signed lease |
| POST | `/redeem` | `{ "hwid": "...", "code": "CUTLASS-XXXX-XXXX-XXXX" }` | mark the machine paid; returns a paid lease |
| POST | `/admin/mint` | `{ "count": 5 }` + header `X-Admin-Token: <token>` | mint purchase codes |

`/activate` and `/redeem` return:
```json
{ "lease": { "payload": "...", "sig": "..." }, "status": "trial", "expires_at": 1699999999, "days_left": 7 }
```
The client verifies `lease` against the embedded public key and caches it.

## Configuration

All via environment variables — see [.env.example](.env.example). The only hard
requirement is `CUTLASS_LICENSE_PRIVATE_KEY` (the base64 Ed25519 private key from
`cutlass-keygen`). Set `CUTLASS_ADMIN_TOKEN` to mint codes, and
`CUTLASS_OWNER_HWIDS` to grant your own machine unlimited access.

## Run locally

```bash
export CUTLASS_LICENSE_PRIVATE_KEY=...        # from cutlass-keygen
export CUTLASS_ADMIN_TOKEN=$(openssl rand -hex 24)
cargo run --release -p cutlass-license-server
```

## Deploy (Docker)

```bash
docker build -f crates/cutlass-license-server/Dockerfile -t cutlass-license .
docker run -p 8787:8787 \
  -e CUTLASS_LICENSE_PRIVATE_KEY=... \
  -e CUTLASS_ADMIN_TOKEN=... \
  -e CUTLASS_OWNER_HWIDS=your-hwid \
  -v cutlass-data:/data \
  cutlass-license
```

The container writes SQLite to `/data` — mount a volume so licenses persist
across redeploys. Any Docker host works (Fly.io, Railway, Render, a VPS). Put it
behind HTTPS (the platform's TLS, or a reverse proxy) before pointing the client
at it — the client talks to it over `https://`.

## Mint purchase codes

```bash
curl -s https://your-host/admin/mint \
  -H "X-Admin-Token: $CUTLASS_ADMIN_TOKEN" \
  -H "Content-Type: application/json" -d '{"count":5}'
```

Each code is single-use; redeeming binds it to the first machine that claims it.
