//! Cutlass licensing server.
//!
//! One job: remember, per hardware ID, when a trial started and whether it was
//! purchased — then hand back a short, signed **lease** the client can trust
//! offline. Because trial-start lives here (not on the user's disk), uninstall
//! + reinstall returns the ORIGINAL start date, so the trial can't be reset.
//!
//! Config (all via env):
//!   CUTLASS_LICENSE_PRIVATE_KEY  base64 Ed25519 private key   (required)
//!   CUTLASS_ADMIN_TOKEN          bearer token for /admin/*    (required for minting)
//!   CUTLASS_OWNER_HWIDS          comma-separated owner HWIDs  (optional)
//!   CUTLASS_DB_PATH              sqlite path (default cutlass-license.db)
//!   CUTLASS_TRIAL_DAYS           trial length (default 7)
//!   CUTLASS_TRIAL_GRACE_DAYS     offline grace for a trial lease (default 3)
//!   CUTLASS_PAID_GRACE_DAYS      offline grace for a paid lease (default 30)
//!   CUTLASS_PORT                 listen port (default 8787)

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use cutlass_license::{issue, signing_key_from_b64, Lease, PrivateKey, SignedLease, Status, NEVER};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    env,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn env_days(key: &str, default: i64) -> i64 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default) * 86_400
}

struct Config {
    trial_secs: i64,
    trial_grace_secs: i64,
    paid_grace_secs: i64,
    owner_hwids: HashSet<String>,
    admin_token: Option<String>,
}

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
    cfg: Arc<Config>,
    key: Arc<PrivateKey>,
}

// ── storage ──────────────────────────────────────────────────────────
fn init_db(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS licenses (
            hwid         TEXT PRIMARY KEY,
            status       TEXT NOT NULL,      -- trial | paid | owner
            trial_start  INTEGER NOT NULL,
            paid_at      INTEGER,
            redeemed     TEXT,               -- purchase code used
            created_at   INTEGER NOT NULL,
            last_seen    INTEGER NOT NULL,
            app_version  TEXT
         );
         CREATE TABLE IF NOT EXISTS codes (
            code         TEXT PRIMARY KEY,
            created_at   INTEGER NOT NULL,
            used_at      INTEGER,
            used_by      TEXT
         );",
    )
    .expect("init db");
}

struct Row {
    status: String,
    trial_start: i64,
}

/// Fetch the record for `hwid`, creating a fresh trial (or owner) row the first
/// time we ever see it. Owner HWIDs are always owner, even on first contact.
fn get_or_create(conn: &Connection, cfg: &Config, hwid: &str, ver: Option<&str>, t: i64) -> Row {
    let existing: Option<(String, i64)> = conn
        .query_row(
            "SELECT status, trial_start FROM licenses WHERE hwid = ?1",
            [hwid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    if let Some((status, trial_start)) = existing {
        // owner set can promote an existing row (e.g. you added your machine later)
        let status = if cfg.owner_hwids.contains(hwid) && status != "owner" {
            conn.execute("UPDATE licenses SET status='owner' WHERE hwid=?1", [hwid]).ok();
            "owner".to_string()
        } else {
            status
        };
        conn.execute(
            "UPDATE licenses SET last_seen=?2, app_version=?3 WHERE hwid=?1",
            rusqlite::params![hwid, t, ver],
        )
        .ok();
        return Row { status, trial_start };
    }

    let status = if cfg.owner_hwids.contains(hwid) { "owner" } else { "trial" };
    conn.execute(
        "INSERT INTO licenses (hwid, status, trial_start, created_at, last_seen, app_version)
         VALUES (?1, ?2, ?3, ?3, ?3, ?4)",
        rusqlite::params![hwid, status, t, ver],
    )
    .expect("insert license");
    Row { status: status.to_string(), trial_start: t }
}

/// Turn a stored row into a signed lease for `now`.
fn lease_for(row: &Row, cfg: &Config, t: i64) -> Lease {
    match row.status.as_str() {
        "owner" => Lease {
            hwid: String::new(),
            status: Status::Owner,
            trial_start: row.trial_start,
            expires_at: NEVER,
            lease_expires_at: NEVER,
            issued_at: t,
        },
        "paid" => Lease {
            hwid: String::new(),
            status: Status::Paid,
            trial_start: row.trial_start,
            expires_at: NEVER,
            lease_expires_at: t + cfg.paid_grace_secs,
            issued_at: t,
        },
        _ => Lease {
            hwid: String::new(),
            status: Status::Trial,
            trial_start: row.trial_start,
            expires_at: row.trial_start + cfg.trial_secs,
            lease_expires_at: t + cfg.trial_grace_secs,
            issued_at: t,
        },
    }
}

// ── request / response shapes ────────────────────────────────────────
#[derive(Deserialize)]
struct ActivateReq {
    hwid: String,
    app_version: Option<String>,
}

#[derive(Serialize)]
struct LeaseResp {
    lease: SignedLease,
    status: Status,
    expires_at: i64,
    days_left: Option<i64>,
}

#[derive(Deserialize)]
struct RedeemReq {
    hwid: String,
    code: String,
}

#[derive(Deserialize)]
struct MintReq {
    count: Option<u32>,
}

#[derive(Serialize)]
struct MintResp {
    codes: Vec<String>,
}

fn signed_response(state: &AppState, hwid: &str, mut lease: Lease) -> Json<LeaseResp> {
    lease.hwid = hwid.to_string();
    let signed = issue(&lease, &state.key);
    let days_left = lease.trial_days_left(lease.issued_at);
    Json(LeaseResp { lease: signed, status: lease.status, expires_at: lease.expires_at, days_left })
}

// ── handlers ─────────────────────────────────────────────────────────
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true }))
}

async fn activate(
    State(state): State<AppState>,
    Json(req): Json<ActivateReq>,
) -> Result<Json<LeaseResp>, (StatusCode, String)> {
    let hwid = req.hwid.trim();
    if hwid.len() < 8 || hwid.len() > 128 {
        return Err((StatusCode::BAD_REQUEST, "invalid hwid".into()));
    }
    let t = now();
    let row = {
        let db = state.db.lock().unwrap();
        get_or_create(&db, &state.cfg, hwid, req.app_version.as_deref(), t)
    };
    let lease = lease_for(&row, &state.cfg, t);
    Ok(signed_response(&state, hwid, lease))
}

async fn redeem(
    State(state): State<AppState>,
    Json(req): Json<RedeemReq>,
) -> Result<Json<LeaseResp>, (StatusCode, String)> {
    let hwid = req.hwid.trim();
    let code = req.code.trim().to_uppercase();
    let t = now();
    let row = {
        let db = state.db.lock().unwrap();
        // ensure a row exists so redeeming works even before first activate
        let base = get_or_create(&db, &state.cfg, hwid, None, t);

        // claim the code atomically: only marks it used if still unused
        let claimed = db
            .execute(
                "UPDATE codes SET used_at=?2, used_by=?3 WHERE code=?1 AND used_at IS NULL",
                rusqlite::params![code, t, hwid],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if claimed == 0 {
            // either unknown or already used — allow re-verify if THIS hwid used it
            let owned: bool = db
                .query_row(
                    "SELECT 1 FROM codes WHERE code=?1 AND used_by=?2",
                    rusqlite::params![code, hwid],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if !owned {
                return Err((StatusCode::BAD_REQUEST, "invalid or already-used code".into()));
            }
        }
        db.execute(
            "UPDATE licenses SET status='paid', paid_at=?2, redeemed=?3 WHERE hwid=?1",
            rusqlite::params![hwid, t, code],
        )
        .ok();
        Row { status: "paid".into(), trial_start: base.trial_start }
    };
    let lease = lease_for(&row, &state.cfg, t);
    Ok(signed_response(&state, hwid, lease))
}

async fn mint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MintReq>,
) -> Result<Json<MintResp>, (StatusCode, String)> {
    let want = headers.get("x-admin-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    match &state.cfg.admin_token {
        Some(tok) if !tok.is_empty() && want == tok => {}
        _ => return Err((StatusCode::UNAUTHORIZED, "bad admin token".into())),
    }
    let n = req.count.unwrap_or(1).clamp(1, 100);
    let t = now();
    let mut codes = Vec::new();
    let db = state.db.lock().unwrap();
    for _ in 0..n {
        let code = new_code();
        db.execute(
            "INSERT INTO codes (code, created_at) VALUES (?1, ?2)",
            rusqlite::params![code, t],
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        codes.push(code);
    }
    Ok(Json(MintResp { codes }))
}

/// A human-typable purchase code like CUTLASS-4KF9-2QX7-M3PD (no 0/O/1/I).
fn new_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let group = |rng: &mut rand::rngs::ThreadRng| -> String {
        (0..4).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
    };
    format!("CUTLASS-{}-{}-{}", group(&mut rng), group(&mut rng), group(&mut rng))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let key_b64 = env::var("CUTLASS_LICENSE_PRIVATE_KEY")
        .map_err(|_| anyhow::anyhow!("set CUTLASS_LICENSE_PRIVATE_KEY (base64 Ed25519 private key)"))?;
    let key = signing_key_from_b64(&key_b64)
        .ok_or_else(|| anyhow::anyhow!("CUTLASS_LICENSE_PRIVATE_KEY is not a valid 32-byte base64 key"))?;

    let cfg = Config {
        trial_secs: env_days("CUTLASS_TRIAL_DAYS", 7),
        trial_grace_secs: env_days("CUTLASS_TRIAL_GRACE_DAYS", 3),
        paid_grace_secs: env_days("CUTLASS_PAID_GRACE_DAYS", 30),
        owner_hwids: env::var("CUTLASS_OWNER_HWIDS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        admin_token: env::var("CUTLASS_ADMIN_TOKEN").ok(),
    };

    let db_path = env::var("CUTLASS_DB_PATH").unwrap_or_else(|_| "cutlass-license.db".into());
    let conn = Connection::open(&db_path)?;
    init_db(&conn);

    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        cfg: Arc::new(cfg),
        key: Arc::new(key),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/activate", post(activate))
        .route("/redeem", post(redeem))
        .route("/admin/mint", post(mint))
        .with_state(state);

    // Honor the platform's assigned port (Railway/Render/Fly set $PORT) before
    // our own override, so the service is reachable without extra config.
    let port: u16 = env::var("CUTLASS_PORT")
        .ok()
        .or_else(|| env::var("PORT").ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(8787);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    println!("cutlass-license-server listening on :{port} (db: {db_path})");
    axum::serve(listener, app).await?;
    Ok(())
}
