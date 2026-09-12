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
//!   CUTLASS_ANTHROPIC_KEY        Anthropic key for /highlights (AI moments)
//!   CUTLASS_GROQ_KEY             Groq key for /transcribe (cloud STT)
//!   CUTLASS_AI_MONTHLY_MINUTES   per-HWID monthly AI allowance (0/unset =
//!                                unlimited, usage still tracked)
//!   CUTLASS_AI_TRIAL_MINUTES     trial-licence AI allowance (0/unset = use the
//!                                monthly cap); keeps a free trial from draining
//!                                our API credits
//!   CUTLASS_LS_SIGNING_SECRET    Lemon Squeezy webhook HMAC secret (enables it)
//!   CUTLASS_LS_LICENSE_VARIANTS  comma-sep LS variant ids that grant a licence
//!   CUTLASS_LS_CREDIT_VARIANTS   "variantId:minutes,…" credit-pack top-ups
//!   CUTLASS_LS_CHECKOUT_LICENSE  buy-link for the licence (served to the app)
//!   CUTLASS_LS_CHECKOUT_CREDITS  buy-link for a credit pack
//!   CUTLASS_LS_ALLOW_TEST_MODE   1 = honour test-mode orders (leave unset)

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cutlass_license::{issue, signing_key_from_b64, Lease, PrivateKey, SignedLease, Status, NEVER};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Lock the database, ignoring poisoning.
///
/// A poisoned mutex means an earlier request panicked while holding it, and a
/// plain `.lock_ok()` then panics in every later caller *forever*. On a
/// server that is not one failed request -- it is the whole service down, for
/// everyone, with nothing in the logs to explain it, until a human notices and
/// redeploys. The app reports a dead licence server to the user as "Connect to
/// continue", so the blast radius is every customer being locked out of
/// software they have paid for.
///
/// What this guards is a SQLite connection, whose own transactional guarantees
/// are untouched by a panic in the Rust code holding it, so carrying on is
/// strictly better than refusing everyone who comes after. The panic itself is
/// still a bug; this just stops one bug from becoming an outage.
trait LockExt<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A database failure to report to one caller rather than die on.
fn db_err(e: rusqlite::Error) -> (StatusCode, String) {
    eprintln!("db error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
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
    anthropic_key: Option<String>,
    groq_key: Option<String>,
    /// Monthly AI processing allowance in SECONDS (0 = unlimited / track-only).
    ai_monthly_secs: f64,
    /// Trial-license AI allowance in SECONDS (0 = fall back to the monthly cap).
    /// Lets a trial taste the AI without draining our API credits.
    ai_trial_secs: f64,
    /// Lemon Squeezy webhook HMAC signing secret (None = webhook disabled).
    ls_signing_secret: Option<String>,
    /// Lemon Squeezy variant IDs that grant a paid licence.
    ls_license_variants: HashSet<String>,
    /// Lemon Squeezy credit-pack variant IDs → minutes granted.
    ls_credit_variants: HashMap<String, f64>,
    /// Checkout links, served to the app so they are not compiled into it.
    /// Going live on Lemon Squeezy means new products, new variant ids and
    /// new checkout URLs — baked into the app, that would need a new build
    /// and would leave every already-installed copy pointing at a checkout
    /// that cannot take money.
    ls_checkout_license: Option<String>,
    ls_checkout_credits: Option<String>,
    /// Honour orders flagged `test_mode`. Off unless deliberately set: a test
    /// order is not a purchase, and granting a real licence for one would be
    /// indistinguishable in the database from a real sale.
    ls_allow_test_mode: bool,
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
            used_by      TEXT,
            transfers    INTEGER NOT NULL DEFAULT 0
         );
         -- AI usage metering: seconds of media processed per HWID per calendar
         -- month (resets when the YYYY-MM period rolls over).
         CREATE TABLE IF NOT EXISTS ai_usage (
            hwid         TEXT NOT NULL,
            period       TEXT NOT NULL,      -- \"YYYY-MM\" (UTC)
            seconds      REAL NOT NULL DEFAULT 0,
            PRIMARY KEY (hwid, period)
         );
         -- persistent top-up balance (seconds) consumed only after the monthly
         -- allowance is spent; granted via /admin/grant or a paid top-up.
         CREATE TABLE IF NOT EXISTS ai_credits (
            hwid         TEXT PRIMARY KEY,
            seconds      REAL NOT NULL DEFAULT 0
         );
         -- processed payment webhook ids, so retries don't double-grant.
         CREATE TABLE IF NOT EXISTS webhook_events (
            id           TEXT PRIMARY KEY,
            processed_at INTEGER NOT NULL
         );",
    )
    .expect("init db");

    // Migration for a database already on the volume: CREATE TABLE IF NOT
    // EXISTS leaves an existing `codes` table alone, so the column has to be
    // added separately. SQLite has no ADD COLUMN IF NOT EXISTS, so failing
    // because it is already there is the expected path, not a problem.
    let _ = conn.execute(
        "ALTER TABLE codes ADD COLUMN transfers INTEGER NOT NULL DEFAULT 0",
        [],
    );
}

/// UTC calendar year-month ("YYYY-MM") for a unix time — the usage period
/// bucket. Civil-from-days (Howard Hinnant), no date-lib dependency.
fn year_month(unix_secs: i64) -> String {
    let z = unix_secs.div_euclid(86_400) + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}")
}

fn ai_used_secs(db: &Connection, hwid: &str, period: &str) -> f64 {
    db.query_row(
        "SELECT seconds FROM ai_usage WHERE hwid=?1 AND period=?2",
        rusqlite::params![hwid, period],
        |r| r.get(0),
    )
    .unwrap_or(0.0)
}

fn ai_credit_secs(db: &Connection, hwid: &str) -> f64 {
    db.query_row("SELECT seconds FROM ai_credits WHERE hwid=?1", [hwid], |r| r.get(0))
        .unwrap_or(0.0)
}

/// The AI allowance (seconds) for a licence status. Trials get their own,
/// smaller cap (falling back to the monthly cap when unset) so a free trial
/// can taste the AI without draining our credits.
fn effective_cap(cfg: &Config, status: &str) -> f64 {
    match status {
        "trial" if cfg.ai_trial_secs > 0.0 => cfg.ai_trial_secs,
        _ => cfg.ai_monthly_secs, // paid, or trial with no trial-specific cap
    }
}

/// Entitlement + usage gate for the AI endpoints. Returns Ok((is_owner, cap))
/// when the call may proceed, or an error status. Owners are unlimited; a cap of
/// 0 means "track usage but never block" (the beta default).
fn ai_gate(
    state: &AppState,
    hwid: &str,
    ver: Option<&str>,
    t: i64,
) -> Result<(bool, f64), (StatusCode, String)> {
    let db = state.db.lock_ok();
    let row = get_or_create(&db, &state.cfg, hwid, ver, t).map_err(db_err)?;
    let is_owner = row.status == "owner";
    if !lease_for(&row, &state.cfg, t).is_active_at(t) {
        return Err((StatusCode::PAYMENT_REQUIRED, "no active licence for AI features".into()));
    }
    let cap = effective_cap(&state.cfg, &row.status);
    if !is_owner && cap > 0.0 {
        let period = year_month(t);
        let used = ai_used_secs(&db, hwid, &period);
        let credits = ai_credit_secs(&db, hwid);
        if used >= cap && credits <= 0.0 {
            let msg = if row.status == "trial" {
                "trial AI limit reached — buy Cutlass for the full allowance, or use Private (on-device) transcription"
            } else {
                "monthly AI limit reached — add credits or use Private (on-device) transcription"
            };
            return Err((StatusCode::TOO_MANY_REQUESTS, msg.into()));
        }
    }
    Ok((is_owner, cap))
}

/// Record `cost_secs` of AI media processing against the allowance (`cap`),
/// spilling the overflow onto the persistent credit balance. No-op for owners.
fn ai_charge(state: &AppState, hwid: &str, is_owner: bool, cap: f64, cost_secs: f64, t: i64) {
    if is_owner || cost_secs <= 0.0 {
        return;
    }
    let db = state.db.lock_ok();
    let period = year_month(t);
    let (from_monthly, from_credits) = if cap <= 0.0 {
        (cost_secs, 0.0) // track-only: everything visible in monthly, no drain
    } else {
        let room = (cap - ai_used_secs(&db, hwid, &period)).max(0.0);
        let fm = cost_secs.min(room);
        (fm, cost_secs - fm)
    };
    let _ = db.execute(
        "INSERT INTO ai_usage (hwid, period, seconds) VALUES (?1, ?2, ?3)
         ON CONFLICT(hwid, period) DO UPDATE SET seconds = seconds + ?3",
        rusqlite::params![hwid, period, from_monthly],
    );
    if from_credits > 0.0 {
        let _ = db.execute(
            "UPDATE ai_credits SET seconds = MAX(0, seconds - ?2) WHERE hwid = ?1",
            rusqlite::params![hwid, from_credits],
        );
    }
}

struct Row {
    status: String,
    trial_start: i64,
}

/// Fetch the record for `hwid`, creating a fresh trial (or owner) row the first
/// time we ever see it. Owner HWIDs are always owner, even on first contact.
///
/// Returns `Err` rather than panicking when that write fails: this runs with
/// the database lock held, so a panic here poisoned it and took every later
/// request down with it (see [`LockExt`]). A full volume or a locked database
/// is a 500 for one caller, not an outage for everyone.
fn get_or_create(
    conn: &Connection,
    cfg: &Config,
    hwid: &str,
    ver: Option<&str>,
    t: i64,
) -> rusqlite::Result<Row> {
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
        return Ok(Row { status, trial_start });
    }

    let status = if cfg.owner_hwids.contains(hwid) { "owner" } else { "trial" };
    conn.execute(
        "INSERT INTO licenses (hwid, status, trial_start, created_at, last_seen, app_version)
         VALUES (?1, ?2, ?3, ?3, ?3, ?4)",
        rusqlite::params![hwid, status, t, ver],
    )?;
    Ok(Row { status: status.to_string(), trial_start: t })
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
    // The commit is here so a redeploy can be confirmed from outside. It used
    // to answer {"ok":true} regardless of what was running, which meant there
    // was no way to tell whether a deploy had actually landed.
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "commit": std::env::var("CUTLASS_BUILD_SHA").unwrap_or_else(|_| "unknown".into()),
    }))
}

/// Where to send someone who wants to buy.
///
/// Served rather than compiled into the app so the store can move — going
/// live, changing price, replacing a product — without shipping a new build
/// and without stranding every copy already installed.
async fn checkout(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "license": state.cfg.ls_checkout_license,
        "credits": state.cfg.ls_checkout_credits,
    }))
}

/// Send a buyer to the current checkout.
///
/// A plain redirect so the landing page can link to it with an ordinary
/// anchor — no JavaScript, no CORS, and nothing about the store baked into a
/// static page that would need redeploying the day it changes.
///
/// Falls back to the store front when no link is configured, which is better
/// than a dead end: someone who wants to pay still lands somewhere they can.
async fn buy(State(state): State<AppState>, Path(what): Path<String>) -> Response {
    let link = match what.as_str() {
        "credits" => state.cfg.ls_checkout_credits.clone(),
        _ => state.cfg.ls_checkout_license.clone(),
    };
    let to = link.unwrap_or_else(|| "https://cutlass.lemonsqueezy.com".to_string());
    // 302, not 301: browsers cache a permanent redirect, and this target
    // changes the day the store goes live.
    (StatusCode::FOUND, [(axum::http::header::LOCATION, to)]).into_response()
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
        let db = state.db.lock_ok();
        get_or_create(&db, &state.cfg, hwid, req.app_version.as_deref(), t).map_err(db_err)?
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
        let db = state.db.lock_ok();
        // ensure a row exists so redeeming works even before first activate
        let base = get_or_create(&db, &state.cfg, hwid, None, t).map_err(db_err)?;

        // claim the code atomically: only marks it used if still unused
        let claimed = db
            .execute(
                "UPDATE codes SET used_at=?2, used_by=?3 WHERE code=?1 AND used_at IS NULL",
                rusqlite::params![code, t, hwid],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if claimed == 0 {
            // Already used, or unknown. Who holds it?
            let holder: Option<String> = db
                .query_row(
                    "SELECT used_by FROM codes WHERE code=?1",
                    rusqlite::params![code],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            match holder.as_deref() {
                // Unknown code. Nothing to do.
                None => {
                    return Err((StatusCode::BAD_REQUEST, "invalid or already-used code".into()))
                }
                // Same machine re-verifying: fine, carry on.
                Some(h) if h == hwid => {}
                // A different machine. Move the licence rather than refuse it.
                //
                // People replace laptops, reinstall Windows, and change parts,
                // and every one of those changes the machine id — so refusing
                // meant someone who had paid had to email for a rebind. A
                // transfer releases the old machine as it binds the new one, so
                // only ever one machine is licensed by a code; a shared code
                // takes access away from whoever used it last, which is its own
                // deterrent. `transfers` is recorded so unusual churn shows up.
                Some(previous) => {
                    db.execute(
                        "UPDATE licenses SET status='trial', paid_at=NULL, redeemed=NULL                          WHERE hwid=?1",
                        rusqlite::params![previous],
                    )
                    .ok();
                    db.execute(
                        "UPDATE codes SET used_at=?2, used_by=?3, transfers=transfers+1                          WHERE code=?1",
                        rusqlite::params![code, t, hwid],
                    )
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                    eprintln!("code {code} transferred from {previous} to {hwid}");
                }
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
    let db = state.db.lock_ok();
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

// ── AI highlights (Claude reads the transcript, finds the best moments) ─
#[derive(Deserialize)]
struct TWord {
    text: String,
    start: f64,
    end: f64,
}

#[derive(Deserialize)]
struct HighlightsReq {
    hwid: String,
    transcript: Vec<TWord>,
    count: Option<u32>,
    app_version: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Moment {
    start: f64,
    end: f64,
    title: String,
    #[serde(default)]
    reason: String,
}

/// Group words into ~14-word lines, each stamped with its start second, so the
/// model sees where each phrase lands without a token-heavy per-word dump.
fn transcript_lines(words: &[TWord]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < words.len() {
        let start = words[i].start;
        let mut line = String::new();
        let mut n = 0;
        while i < words.len() && n < 14 {
            line.push_str(words[i].text.trim());
            line.push(' ');
            i += 1;
            n += 1;
        }
        out.push_str(&format!("[{start:.1}] {}\n", line.trim()));
    }
    out
}

fn build_prompt(words: &[TWord], count: u32) -> String {
    let total = words.last().map(|w| w.end).unwrap_or(0.0);
    format!(
        "You are a world-class short-form video editor who makes viral YouTube Shorts, \
TikToks, and Instagram Reels. Below is the full timestamped transcript of a {total:.0}-second \
video. Each line is prefixed with its start time in seconds: [SECONDS].\n\n\
Find the {count} BEST standalone moments to cut into vertical short clips. Prioritise moments \
that are genuinely FUNNY, EXCITING, SURPRISING, emotionally powerful, or PIVOTAL — the parts \
that make someone stop scrolling and watch. Skim the ENTIRE video; spread picks across it.\n\n\
Hard rules:\n\
- Each clip must be self-contained and make sense on its own (include enough setup/context).\n\
- Between 15 and 60 seconds long. Start and end on natural sentence boundaries.\n\
- Must open with a strong hook in the first few seconds.\n\
- Do NOT pick overlapping, repetitive, or near-duplicate moments — each must be distinct.\n\
- Rank them best-first.\n\n\
Return ONLY a JSON array (no prose, no markdown fences). Each element is exactly:\n\
{{\"start\": <seconds number>, \"end\": <seconds number>, \"title\": \"<punchy 3-7 word title>\", \
\"reason\": \"<why it stands out, 4-9 words>\"}}\n\n\
Transcript:\n{}",
        transcript_lines(words)
    )
}

/// Call the Anthropic Messages API (blocking; run under spawn_blocking). Retry
/// transient upstream errors (429 rate-limit, 529 overloaded, 5xx) a couple of
/// times with backoff; genuine 4xx (bad key, no credits) fail fast. The model
/// is asked for a bare JSON array in the prompt and the caller extracts the
/// outermost [...] (tolerating any stray markdown fences).
fn call_anthropic(key: &str, prompt: String) -> Result<String, String> {
    let connector = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(90))
        .tls_connector(std::sync::Arc::new(connector))
        .build();
    let body = serde_json::json!({
        "model": "claude-sonnet-5",
        "max_tokens": 3000,
        "messages": [{ "role": "user", "content": prompt }]
    });

    let mut last_err = String::new();
    for attempt in 0..3 {
        let resp = agent
            .post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", key)
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .send_json(body.clone());
        match resp {
            Ok(r) => {
                let v: serde_json::Value = r.into_json().map_err(|e| e.to_string())?;
                let text = v["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("")
                    })
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    return Err("empty response from anthropic".into());
                }
                return Ok(text);
            }
            Err(ureq::Error::Status(code, r)) => {
                let msg = r.into_string().unwrap_or_default();
                last_err = format!("anthropic {code}: {}", msg.chars().take(300).collect::<String>());
                // only retry transient statuses; fail fast on 4xx like 400/401
                let transient = code == 429 || code >= 500;
                if !transient || attempt == 2 {
                    return Err(last_err);
                }
            }
            Err(e) => {
                last_err = format!("anthropic transport error: {e}");
                if attempt == 2 {
                    return Err(last_err);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(600 * (attempt as u64 + 1)));
    }
    Err(last_err)
}

/// Extract the outermost JSON array from a possibly-chatty model response.
fn extract_json_array(s: &str) -> Option<&str> {
    let a = s.find('[')?;
    let b = s.rfind(']')?;
    (b > a).then(|| &s[a..=b])
}

async fn highlights(
    State(state): State<AppState>,
    Json(req): Json<HighlightsReq>,
) -> Result<Json<Vec<Moment>>, (StatusCode, String)> {
    let key = state
        .cfg
        .anthropic_key
        .clone()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "AI highlights are not configured".into()))?;
    if req.transcript.len() < 5 {
        return Err((StatusCode::BAD_REQUEST, "transcript too short to analyse".into()));
    }
    // gate on an active licence + monthly AI allowance
    let t = now();
    let hwid = req.hwid.trim().to_string();
    let (is_owner, cap) = ai_gate(&state, &hwid, req.app_version.as_deref(), t)?;

    let count = req.count.unwrap_or(8).clamp(1, 15);
    let prompt = build_prompt(&req.transcript, count);
    let text = tokio::task::spawn_blocking(move || call_anthropic(&key, prompt))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let json = extract_json_array(&text)
        .ok_or((StatusCode::BAD_GATEWAY, "AI response was not valid JSON".into()))?;
    let mut moments: Vec<Moment> = serde_json::from_str(json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("could not parse AI JSON: {e}")))?;
    // clamp to the real transcript span so a hallucinated time can't overrun
    let dur = req.transcript.last().map(|w| w.end).unwrap_or(0.0);
    for m in moments.iter_mut() {
        if m.start < 0.0 {
            m.start = 0.0;
        }
        if dur > 0.0 && m.end > dur {
            m.end = dur;
        }
    }
    moments.retain(|m| m.end > m.start && m.end - m.start >= 5.0);
    // meter the analysed span (Claude cost is incurred on either transcribe path)
    let span = dur - req.transcript.first().map(|w| w.start).unwrap_or(0.0);
    ai_charge(&state, &hwid, is_owner, cap, span.max(0.0), t);
    Ok(Json(moments))
}

// ── Cloud transcription (Groq Whisper on GPUs — the OpusClip speed trick) ─
// The client extracts + compresses the audio into ~10-min chunks and uploads
// each here in parallel; we forward to Groq and stamp the words back into
// absolute time with the chunk's `offset`. Only the audio ever leaves the
// machine (never the video), and only for licensed apps.
#[derive(Serialize, Deserialize)]
struct SttWord {
    text: String,
    start: f64,
    end: f64,
}

#[derive(Deserialize)]
struct SttQuery {
    hwid: String,
    #[serde(default)]
    offset: f64,
    app_version: Option<String>,
}

fn push_text_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
            .as_bytes(),
    );
}

/// POST one audio chunk to Groq's OpenAI-compatible transcription API and
/// return word-level timestamps (relative to the chunk start).
fn call_groq(key: &str, audio: &[u8]) -> Result<Vec<SttWord>, String> {
    let connector = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(150))
        .tls_connector(std::sync::Arc::new(connector))
        .build();

    let boundary = format!("----cutlass{}", now());
    let mut body: Vec<u8> = Vec::with_capacity(audio.len() + 512);
    push_text_field(&mut body, &boundary, "model", "whisper-large-v3-turbo");
    push_text_field(&mut body, &boundary, "response_format", "verbose_json");
    push_text_field(&mut body, &boundary, "timestamp_granularities[]", "word");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.flac\"\r\nContent-Type: audio/flac\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let resp = match agent
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default();
            return Err(format!("groq {code}: {}", msg.chars().take(300).collect::<String>()));
        }
        Err(e) => return Err(format!("groq transport error: {e}")),
    };
    let v: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    // prefer word-level; fall back to segment-level if the model didn't return words
    let mut words: Vec<SttWord> = v["words"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|w| {
                    Some(SttWord {
                        text: w.get("word").and_then(|x| x.as_str())?.to_string(),
                        start: w.get("start").and_then(|x| x.as_f64())?,
                        end: w.get("end").and_then(|x| x.as_f64())?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if words.is_empty() {
        if let Some(segs) = v["segments"].as_array() {
            words = segs
                .iter()
                .filter_map(|s| {
                    Some(SttWord {
                        text: s.get("text").and_then(|x| x.as_str())?.trim().to_string(),
                        start: s.get("start").and_then(|x| x.as_f64())?,
                        end: s.get("end").and_then(|x| x.as_f64())?,
                    })
                })
                .collect();
        }
    }
    Ok(words)
}

async fn transcribe(
    State(state): State<AppState>,
    Query(q): Query<SttQuery>,
    body: Bytes,
) -> Result<Json<Vec<SttWord>>, (StatusCode, String)> {
    let key = state
        .cfg
        .groq_key
        .clone()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "cloud transcription is not configured".into()))?;
    if body.len() < 512 {
        return Err((StatusCode::BAD_REQUEST, "audio chunk too small".into()));
    }
    // same gate as /highlights — active licence + monthly AI allowance
    let t = now();
    let hwid = q.hwid.trim().to_string();
    let (is_owner, cap) = ai_gate(&state, &hwid, q.app_version.as_deref(), t)?;

    let audio = body.to_vec();
    let mut words = tokio::task::spawn_blocking(move || call_groq(&key, &audio))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    // meter this chunk's audio length before stamping absolute time
    let chunk_secs = words.last().map(|w| w.end).unwrap_or(0.0)
        - words.first().map(|w| w.start).unwrap_or(0.0);
    ai_charge(&state, &hwid, is_owner, cap, chunk_secs.max(0.0), t);
    // stamp the chunk offset back so times are absolute over the whole video
    if q.offset != 0.0 {
        for w in words.iter_mut() {
            w.start += q.offset;
            w.end += q.offset;
        }
    }
    Ok(Json(words))
}

// ── usage metering endpoints ─────────────────────────────────────────
#[derive(Deserialize)]
struct UsageQuery {
    hwid: String,
}

/// GET /usage?hwid=… — how much AI allowance is left this month (for the app
/// to display). remaining_minutes = -1 means unlimited.
async fn usage(State(state): State<AppState>, Query(q): Query<UsageQuery>) -> Json<serde_json::Value> {
    let t = now();
    let period = year_month(t);
    let hwid = q.hwid.trim();
    let db = state.db.lock_ok();
    // pick the cap for THIS licence's status (trial vs paid), defaulting to trial
    let status: String = db
        .query_row("SELECT status FROM licenses WHERE hwid=?1", [hwid], |r| r.get(0))
        .unwrap_or_else(|_| "trial".to_string());
    let used = ai_used_secs(&db, hwid, &period);
    let credits = ai_credit_secs(&db, hwid);
    drop(db);
    let cap = effective_cap(&state.cfg, &status);
    let unlimited = state.cfg.owner_hwids.contains(hwid) || status == "owner" || cap <= 0.0;
    let remaining = if unlimited { -1.0 } else { ((cap - used).max(0.0) + credits) / 60.0 };
    Json(serde_json::json!({
        "period": period,
        "used_minutes": (used / 60.0).round(),
        "cap_minutes": if unlimited { 0.0 } else { (cap / 60.0).round() },
        "credit_minutes": (credits / 60.0).round(),
        "remaining_minutes": remaining.round(),
        "unlimited": unlimited,
    }))
}

#[derive(Deserialize)]
struct GrantReq {
    hwid: String,
    minutes: f64,
}

/// POST /admin/grant {hwid, minutes} — add top-up credits to an HWID (token-gated).
async fn grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<GrantReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let want = headers.get("x-admin-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    match &state.cfg.admin_token {
        Some(tok) if !tok.is_empty() && want == tok => {}
        _ => return Err((StatusCode::UNAUTHORIZED, "bad admin token".into())),
    }
    let hwid = req.hwid.trim();
    let secs = (req.minutes * 60.0).max(0.0);
    let db = state.db.lock_ok();
    db.execute(
        "INSERT INTO ai_credits (hwid, seconds) VALUES (?1, ?2)
         ON CONFLICT(hwid) DO UPDATE SET seconds = seconds + ?2",
        rusqlite::params![hwid, secs],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let bal = ai_credit_secs(&db, hwid);
    Ok(Json(serde_json::json!({ "hwid": hwid, "credit_minutes": (bal / 60.0).round() })))
}

/// POST /admin/reset {hwid} — wipe a machine's licence + credits + usage
/// (token-gated). Support tool: undo a test grant, or reset a machine.
async fn admin_reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UsageQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let want = headers.get("x-admin-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    match &state.cfg.admin_token {
        Some(tok) if !tok.is_empty() && want == tok => {}
        _ => return Err((StatusCode::UNAUTHORIZED, "bad admin token".into())),
    }
    let hwid = req.hwid.trim();
    let db = state.db.lock_ok();
    let licenses = db.execute("DELETE FROM licenses WHERE hwid=?1", [hwid]).unwrap_or(0);
    let credits = db.execute("DELETE FROM ai_credits WHERE hwid=?1", [hwid]).unwrap_or(0);
    let usage = db.execute("DELETE FROM ai_usage WHERE hwid=?1", [hwid]).unwrap_or(0);
    Ok(Json(serde_json::json!({ "hwid": hwid, "licenses": licenses, "credits": credits, "usage": usage })))
}

// ── Lemon Squeezy payment webhook ────────────────────────────────────
// User buys a licence or a credit pack; Lemon Squeezy (merchant of record —
// it handles tax/VAT) POSTs a signed order here. The buyer's machine id rides
// along in the checkout's custom data, so we grant to the right machine.

/// Constant-time verify of Lemon Squeezy's `X-Signature` (hex HMAC-SHA256).
fn ls_verify(secret: &[u8], body: &[u8], sig_hex: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    let Ok(sig) = hex::decode(sig_hex) else {
        return false;
    };
    mac.verify_slice(&sig).is_ok()
}

/// Mark an HWID as paid (create the row if it's the first we've seen it).
fn mark_paid(db: &Connection, hwid: &str, t: i64) {
    let updated = db
        .execute(
            "UPDATE licenses SET status='paid', paid_at=?2, last_seen=?2 WHERE hwid=?1",
            rusqlite::params![hwid, t],
        )
        .unwrap_or(0);
    if updated == 0 {
        let _ = db.execute(
            "INSERT INTO licenses (hwid, status, trial_start, paid_at, created_at, last_seen)
             VALUES (?1, 'paid', ?2, ?2, ?2, ?2)",
            rusqlite::params![hwid, t],
        );
    }
}

async fn lemonsqueezy_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, (StatusCode, String)> {
    let secret = state
        .cfg
        .ls_signing_secret
        .clone()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "payment webhook not configured".into()))?;
    let sig = headers.get("x-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !ls_verify(secret.as_bytes(), &body, sig) {
        return Err((StatusCode::UNAUTHORIZED, "bad signature".into()));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| (StatusCode::BAD_REQUEST, "bad json".into()))?;

    // only a genuinely paid one-time order does anything; ack everything else
    if v["meta"]["event_name"].as_str() != Some("order_created")
        || v["data"]["attributes"]["status"].as_str() != Some("paid")
    {
        return Ok(StatusCode::OK);
    }
    // A test-mode order is signed and says "paid", and once live and test
    // products share a variant id -- which happens the moment one is copied
    // to live mode -- nothing else here would tell them apart.
    let test_order = v["meta"]["test_mode"].as_bool().unwrap_or(false)
        || v["data"]["attributes"]["test_mode"].as_bool().unwrap_or(false);
    if test_order && !state.cfg.ls_allow_test_mode {
        eprintln!("ignored a test-mode order (set CUTLASS_LS_ALLOW_TEST_MODE=1 to honour them)");
        return Ok(StatusCode::OK);
    }
    let order_id = v["data"]["id"].as_str().unwrap_or("").to_string();
    let hwid = v["meta"]["custom_data"]["hwid"].as_str().unwrap_or("").trim().to_string();
    let variant = {
        let vid = &v["data"]["attributes"]["first_order_item"]["variant_id"];
        vid.as_str()
            .map(|s| s.to_string())
            .or_else(|| vid.as_i64().map(|n| n.to_string()))
            .unwrap_or_default()
    };
    if order_id.is_empty() || hwid.is_empty() || variant.is_empty() {
        return Ok(StatusCode::OK);
    }

    let t = now();
    let db = state.db.lock_ok();
    // idempotency: a retried webhook must not double-grant (the mutex serialises
    // the check + grant + record, so concurrent retries can't race either)
    if db.query_row("SELECT 1 FROM webhook_events WHERE id=?1", [&order_id], |_| Ok(())).is_ok() {
        return Ok(StatusCode::OK);
    }
    if state.cfg.ls_license_variants.contains(&variant) {
        mark_paid(&db, &hwid, t);
    } else if let Some(mins) = state.cfg.ls_credit_variants.get(&variant) {
        let _ = db.execute(
            "INSERT INTO ai_credits (hwid, seconds) VALUES (?1, ?2)
             ON CONFLICT(hwid) DO UPDATE SET seconds = seconds + ?2",
            rusqlite::params![hwid, mins * 60.0],
        );
    } else {
        return Ok(StatusCode::OK); // unknown product — ack, grant nothing
    }
    let _ = db.execute(
        "INSERT OR IGNORE INTO webhook_events (id, processed_at) VALUES (?1, ?2)",
        rusqlite::params![order_id, t],
    );
    Ok(StatusCode::OK)
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
        anthropic_key: env::var("CUTLASS_ANTHROPIC_KEY").ok(),
        groq_key: env::var("CUTLASS_GROQ_KEY").ok(),
        ai_monthly_secs: env::var("CUTLASS_AI_MONTHLY_MINUTES")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            * 60.0,
        ai_trial_secs: env::var("CUTLASS_AI_TRIAL_MINUTES")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            * 60.0,
        ls_signing_secret: env::var("CUTLASS_LS_SIGNING_SECRET").ok().filter(|s| !s.is_empty()),
        ls_license_variants: env::var("CUTLASS_LS_LICENSE_VARIANTS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        // "variantId:minutes,variantId:minutes"
        ls_checkout_license: env::var("CUTLASS_LS_CHECKOUT_LICENSE")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        ls_checkout_credits: env::var("CUTLASS_LS_CHECKOUT_CREDITS")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        ls_allow_test_mode: env::var("CUTLASS_LS_ALLOW_TEST_MODE")
            .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes")),
        ls_credit_variants: env::var("CUTLASS_LS_CREDIT_VARIANTS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|pair| {
                let (id, mins) = pair.split_once(':')?;
                Some((id.trim().to_string(), mins.trim().parse::<f64>().ok()?))
            })
            .collect(),
    };

    let db_path = env::var("CUTLASS_DB_PATH").unwrap_or_else(|_| "cutlass-license.db".into());
    let conn = Connection::open(&db_path)?;
    init_db(&conn);

    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        cfg: Arc::new(cfg),
        key: Arc::new(key),
    };

    // Honor the platform's assigned port (Railway/Render/Fly set $PORT) before
    // our own override, so the service is reachable without extra config.
    let port: u16 = env::var("CUTLASS_PORT")
        .ok()
        .or_else(|| env::var("PORT").ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(8787);
    println!("cutlass-license-server db: {db_path}");
    serve(router(state), port).await
}

async fn serve(app: Router, port: u16) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    println!("cutlass-license-server listening on :{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Every route, plus the panic net around them. Separate from `main` so tests
/// can drive real requests through the stack the service actually runs.
fn router(state: AppState) -> Router {
    router_with(state, Router::new())
}

/// `extra` is merged in *before* the panic layer, because a layer only covers
/// routes added ahead of it. Tests add a route through this so the net is
/// proven where it is really applied, not in a copy of it that could drift.
fn router_with(state: AppState, extra: Router<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/checkout", get(checkout))
        .route("/buy/:what", get(buy))
        .route("/activate", post(activate))
        .route("/redeem", post(redeem))
        .route("/admin/mint", post(mint))
        .route("/admin/grant", post(grant))
        .route("/admin/reset", post(admin_reset))
        .route("/webhook/lemonsqueezy", post(lemonsqueezy_webhook))
        .route("/usage", get(usage))
        .route("/highlights", post(highlights))
        // audio chunks can be a few MB — lift axum's 2 MB default for this route
        .route(
            "/transcribe",
            post(transcribe).layer(DefaultBodyLimit::max(48 * 1024 * 1024)),
        )
        .merge(extra)
        // Last resort. Nothing above should panic, but without this one that
        // slips through drops the connection with no reply at all -- which the
        // app shows the user as "offline", a server bug wearing the costume of
        // their own internet.
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(
            |_: Box<dyn std::any::Any + Send + 'static>| {
                eprintln!("handler panicked; answered 500");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            },
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(cap_minutes: f64, trial_minutes: f64, owners: &[&str]) -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn);
        let cfg = Config {
            trial_secs: 7 * 86_400,
            trial_grace_secs: 3 * 86_400,
            paid_grace_secs: 30 * 86_400,
            owner_hwids: owners.iter().map(|s| s.to_string()).collect(),
            admin_token: Some("t".into()),
            anthropic_key: None,
            groq_key: None,
            ai_monthly_secs: cap_minutes * 60.0,
            ai_trial_secs: trial_minutes * 60.0,
            ls_signing_secret: None,
            ls_license_variants: HashSet::new(),
            ls_credit_variants: HashMap::new(),
            ls_checkout_license: None,
            ls_checkout_credits: None,
            ls_allow_test_mode: false,
        };
        // any valid 32-byte key — the signing key is unused by the metering path
        let key = signing_key_from_b64("AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=").unwrap();
        AppState {
            db: Arc::new(Mutex::new(conn)),
            cfg: Arc::new(cfg),
            key: Arc::new(key),
        }
    }

    // ── staying up (see `LockExt`) ────────────────────────────────────
    //
    // These three are one story. A request panics while holding the database
    // lock; before this, that poisoned the mutex and every later request from
    // every other customer panicked too. The service was down for good, and a
    // dead licence server reaches the user as "Connect to continue" -- so one
    // bad request locked every paying customer out of the app. It must not be
    // possible for a single request to do that.

    /// A test-mode order is signed, says "paid", and carries a variant id.
    /// Once a product is copied to live mode the two environments can look
    /// identical to this handler, so the flag is the only thing separating a
    /// rehearsal from a sale.
    #[test]
    fn test_mode_is_not_a_purchase_unless_asked_for() {
        let s = test_state(0.0, 0.0, &[]);
        assert!(!s.cfg.ls_allow_test_mode, "honouring test orders must be opt-in");

        for body in [
            serde_json::json!({"meta": {"test_mode": true}, "data": {"attributes": {}}}),
            serde_json::json!({"meta": {}, "data": {"attributes": {"test_mode": true}}}),
        ] {
            let flagged = body["meta"]["test_mode"].as_bool().unwrap_or(false)
                || body["data"]["attributes"]["test_mode"].as_bool().unwrap_or(false);
            assert!(flagged, "a test order must be recognisable wherever LS puts the flag");
        }

        // and a real order carries neither
        let real = serde_json::json!({"meta": {}, "data": {"attributes": {"status": "paid"}}});
        assert!(!(real["meta"]["test_mode"].as_bool().unwrap_or(false)
            || real["data"]["attributes"]["test_mode"].as_bool().unwrap_or(false)));
    }

    /// The checkout links are served, not compiled in. Unset they are null,
    /// and the app keeps whatever it already knew rather than sending someone
    /// to a blank page.
    #[tokio::test]
    async fn checkout_links_come_from_config() {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let mut st = test_state(0.0, 0.0, &[]);
        let cfg = Arc::get_mut(&mut st.cfg).unwrap();
        cfg.ls_checkout_license = Some("https://example.test/buy/live-licence".into());

        let res = router(st)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/checkout")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["license"], "https://example.test/buy/live-licence");
        assert!(v["credits"].is_null(), "unset means unset, not empty string");
    }

    /// The landing page's Buy button is an ordinary link to this, so it has
    /// to redirect somewhere useful even before the store is live — a button
    /// that 404s is worse than one that lands on the store front.
    #[tokio::test]
    async fn buy_always_sends_someone_somewhere() {
        use tower::ServiceExt;

        let go = |st: AppState, path: &'static str| async move {
            let res = router(st)
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let loc =
                res.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            (res.status(), loc)
        };

        // nothing configured yet: still a redirect, to the store front
        let (status, loc) = go(test_state(0.0, 0.0, &[]), "/buy/license").await;
        assert_eq!(status, StatusCode::FOUND, "must redirect, not 404");
        assert!(loc.starts_with("https://"), "landed nowhere: {loc}");

        // configured: the configured link, and 302 so browsers don't cache a
        // target that changes the day the store goes live
        let mut st = test_state(0.0, 0.0, &[]);
        let cfg = Arc::get_mut(&mut st.cfg).unwrap();
        cfg.ls_checkout_license = Some("https://example.test/buy/live".into());
        cfg.ls_checkout_credits = Some("https://example.test/buy/credits".into());
        assert_eq!(go(st.clone(), "/buy/license").await, (StatusCode::FOUND, "https://example.test/buy/live".into()));
        assert_eq!(go(st, "/buy/credits").await, (StatusCode::FOUND, "https://example.test/buy/credits".into()));
    }

    /// Every lock in this file must go through `lock_ok`, checked by reading
    /// the file's own source.
    ///
    /// The desktop app was converted once by searching for `.lock().unwrap()`
    /// and five were missed, because rustfmt had wrapped them across two lines
    /// and the search was line by line. Nothing caught it: a poisoned mutex is
    /// a runtime state no test reaches by accident, so the survivors passed
    /// every check while doing the exact thing the fix existed to stop. Here
    /// the stakes are a live service, so the same guard.
    #[test]
    fn every_lock_in_this_file_goes_through_lock_ok() {
        let src = include_str!("main.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();

        for (at, _) in code.match_indices(".lock()") {
            let after = code[at + ".lock()".len()..].trim_start();
            // `.unwrap_or_else` is how lock_ok itself is written, and is fine.
            if after.starts_with(".unwrap()") || after.starts_with(".expect(") {
                let line = code[..at].lines().count();
                panic!(
                    "main.rs:{line} panics on a poisoned lock, which takes the \
                     whole service down for every customer. Use `.lock_ok()`."
                );
            }
        }
    }

    /// The reachable trigger: `get_or_create` inserts a row with the lock
    /// held, and used to `.expect("insert license")`, so any failed write --
    /// a full volume, a locked database -- panicked right there.
    #[test]
    fn a_failed_insert_is_an_error_not_a_panic() {
        let s = test_state(0.0, 0.0, &[]);
        let db = s.db.lock_ok();
        // stands in for every reason the write can fail
        db.execute_batch("DROP TABLE licenses").unwrap();

        let r = get_or_create(&db, &s.cfg, "hwid-with-nowhere-to-go", None, now());
        assert!(r.is_err(), "a failed insert must return an error, not panic");
    }

    /// And if something under the lock panics anyway, the next customer
    /// through the door still has to be served.
    #[test]
    fn a_poisoned_lock_does_not_take_the_service_down() {
        let s = test_state(0.0, 0.0, &[]);
        let t = now();
        assert!(ai_gate(&s, "customer-a", None, t).is_ok(), "baseline");

        let db2 = s.db.clone();
        let _ = std::thread::spawn(move || {
            let _held = db2.lock_ok();
            panic!("something under the lock went wrong");
        })
        .join();
        assert!(s.db.lock().is_err(), "the mutex really is poisoned now");

        assert!(ai_gate(&s, "customer-b", None, t).is_ok(), "a later request must still work");
        assert!(ai_gate(&s, "customer-a", None, t).is_ok(), "and so must an existing one");
    }

    /// The net under everything else: a panic reaching the router comes back
    /// as a 500 to that one caller instead of a dropped connection.
    #[tokio::test]
    async fn a_panicking_handler_answers_500() {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        async fn boom() -> &'static str {
            panic!("handler blew up");
        }
        let app = router_with(test_state(0.0, 0.0, &[]), Router::new().route("/boom", get(boom)));

        let res = app
            .oneshot(axum::http::Request::builder().uri("/boom").body(axum::body::Body::empty()).unwrap())
            .await
            .expect("the connection must not be dropped");

        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"internal error");
    }

    /// Sanity: the test above is only worth anything if real routes go
    /// through the same stack it does.
    #[tokio::test]
    async fn health_still_answers_through_the_stack() {
        use tower::ServiceExt;
        let res = router(test_state(0.0, 0.0, &[]))
            .oneshot(axum::http::Request::builder().uri("/health").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[test]
    fn year_month_known_date() {
        // 1_755_000_000 → ~Aug 12 2025 UTC
        assert_eq!(year_month(1_755_000_000), "2025-08");
    }

    #[test]
    fn charge_accumulates_and_cap_blocks() {
        let t = now();
        let s = test_state(10.0, 0.0, &[]); // 10-min cap (trial falls back to it)
        let hwid = "unittesthwid00";
        let (_, cap) = ai_gate(&s, hwid, None, t).unwrap();
        ai_charge(&s, hwid, false, cap, 6.0 * 60.0, t); // 6 min
        {
            let db = s.db.lock_ok();
            assert!((ai_used_secs(&db, hwid, &year_month(t)) - 360.0).abs() < 1.0);
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok()); // still under cap
        ai_charge(&s, hwid, false, cap, 5.0 * 60.0, t); // now 11 min, over cap
        let g = ai_gate(&s, hwid, None, t);
        assert_eq!(g.unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn credits_allow_past_cap_and_drain() {
        let t = now();
        let s = test_state(1.0, 0.0, &[]); // 1-minute cap
        let hwid = "credittest0000";
        let (_, cap) = ai_gate(&s, hwid, None, t).unwrap();
        ai_charge(&s, hwid, false, cap, 60.0, t); // exactly at cap
        {
            let db = s.db.lock_ok();
            db.execute(
                "INSERT INTO ai_credits (hwid, seconds) VALUES (?1, ?2)",
                rusqlite::params![hwid, 120.0],
            )
            .unwrap();
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok()); // at cap but has credits
        ai_charge(&s, hwid, false, cap, 90.0, t); // spills fully onto credits
        let db = s.db.lock_ok();
        assert!((ai_credit_secs(&db, hwid) - 30.0).abs() < 1.0);
    }

    #[test]
    fn owner_is_unlimited() {
        let t = now();
        let s = test_state(1.0, 0.0, &["ownerhwid00000"]);
        let hwid = "ownerhwid00000";
        let (is_owner, cap) = ai_gate(&s, hwid, None, t).unwrap();
        assert!(is_owner);
        ai_charge(&s, hwid, true, cap, 9_999.0 * 60.0, t); // no-op for owners
        {
            let db = s.db.lock_ok();
            assert_eq!(ai_used_secs(&db, hwid, &year_month(t)), 0.0);
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok());
    }

    #[test]
    fn zero_cap_tracks_but_never_blocks() {
        let t = now();
        let s = test_state(0.0, 0.0, &[]); // 0 = unlimited / track-only
        let hwid = "betatrackonly0";
        let (_, cap) = ai_gate(&s, hwid, None, t).unwrap();
        ai_charge(&s, hwid, false, cap, 9_999.0 * 60.0, t);
        {
            let db = s.db.lock_ok();
            assert!(ai_used_secs(&db, hwid, &year_month(t)) > 0.0); // usage recorded
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok()); // never blocked
    }

    #[test]
    fn trial_cap_is_separate_from_paid() {
        let t = now();
        let s = test_state(10.0, 1.0, &[]); // paid 10 min, trial 1 min
        let hwid = "trialuser00001";
        // a fresh HWID is a trial → gets the 1-minute cap
        let (_, cap) = ai_gate(&s, hwid, None, t).unwrap();
        assert!((cap - 60.0).abs() < 0.1);
        ai_charge(&s, hwid, false, cap, 60.0, t); // hit the trial cap
        assert_eq!(ai_gate(&s, hwid, None, t).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        // promote to paid → the 10-minute monthly cap applies, so it's allowed again
        {
            let db = s.db.lock_ok();
            db.execute("UPDATE licenses SET status='paid' WHERE hwid=?1", [hwid]).unwrap();
        }
        let (_, paid_cap) = ai_gate(&s, hwid, None, t).unwrap();
        assert!((paid_cap - 600.0).abs() < 0.1);
        assert!(ai_gate(&s, hwid, None, t).is_ok());
    }

    #[test]
    fn webhook_signature_verifies() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let secret = b"whsec_test";
        let body = br#"{"meta":{"event_name":"order_created"}}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(ls_verify(secret, body, &sig)); // correct signature passes
        assert!(!ls_verify(secret, body, "deadbeef")); // wrong signature fails
        assert!(!ls_verify(b"otherkey", body, &sig)); // wrong secret fails
        assert!(!ls_verify(secret, br#"{"tampered":true}"#, &sig)); // tampered body fails
    }

    #[test]
    fn reset_returns_a_machine_to_a_clean_trial() {
        let t = now();
        let s = test_state(600.0, 30.0, &[]);
        let hwid = "resetme000001";
        {
            let db = s.db.lock_ok();
            mark_paid(&db, hwid, t);
            db.execute("INSERT INTO ai_credits (hwid, seconds) VALUES (?1, 6000)", [hwid]).unwrap();
        }
        // reset (mirrors admin_reset's deletes)
        {
            let db = s.db.lock_ok();
            db.execute("DELETE FROM licenses WHERE hwid=?1", [hwid]).unwrap();
            db.execute("DELETE FROM ai_credits WHERE hwid=?1", [hwid]).unwrap();
            db.execute("DELETE FROM ai_usage WHERE hwid=?1", [hwid]).unwrap();
        }
        // gone: recreated as a fresh trial (30-min cap), zero credits
        let (_, cap) = ai_gate(&s, hwid, None, t).unwrap();
        assert!((cap - 1800.0).abs() < 1.0);
        let db = s.db.lock_ok();
        assert_eq!(ai_credit_secs(&db, hwid), 0.0);
    }

    #[test]
    fn mark_paid_promotes_to_the_paid_cap() {
        let t = now();
        let s = test_state(600.0, 30.0, &[]); // paid 600 min, trial 30 min
        let hwid = "paidbyhook0001";
        {
            let db = s.db.lock_ok();
            mark_paid(&db, hwid, t); // as the webhook would on a licence purchase
            assert_eq!(get_or_create(&db, &s.cfg, hwid, None, t).unwrap().status, "paid");
        }
        // a paid machine gets the 600-min cap, not the 30-min trial cap
        let (_, cap) = ai_gate(&s, hwid, None, t).unwrap();
        assert!((cap - 36_000.0).abs() < 1.0);
    }

    /// Put a code in the table, as /admin/mint does.
    fn mint(s: &AppState, code: &str) {
        let db = s.db.lock_ok();
        db.execute(
            "INSERT INTO codes (code, created_at) VALUES (?1, ?2)",
            rusqlite::params![code, now()],
        )
        .unwrap();
    }

    fn status_of(s: &AppState, hwid: &str) -> Option<String> {
        let db = s.db.lock_ok();
        db.query_row(
            "SELECT status FROM licenses WHERE hwid=?1",
            rusqlite::params![hwid],
            |r| r.get(0),
        )
        .ok()
    }

    /// A machine id changes whenever someone replaces a laptop, reinstalls
    /// Windows, or swaps a part. Refusing the code then means the person who
    /// paid has to email for a rebind, so the code moves instead.
    #[tokio::test]
    async fn a_code_moves_to_a_new_machine_and_releases_the_old_one() {
        let s = test_state(0.0, 0.0, &[]);
        mint(&s, "CUTLASS-AAAA-BBBB-CCCC");

        let first = redeem(
            State(s.clone()),
            Json(RedeemReq { hwid: "old-machine".into(), code: "CUTLASS-AAAA-BBBB-CCCC".into() }),
        )
        .await;
        assert!(first.is_ok(), "the first redemption should work");
        assert_eq!(status_of(&s, "old-machine").as_deref(), Some("paid"));

        // same machine again: still fine, no transfer
        assert!(redeem(
            State(s.clone()),
            Json(RedeemReq { hwid: "old-machine".into(), code: "CUTLASS-AAAA-BBBB-CCCC".into() }),
        )
        .await
        .is_ok());

        // new machine: the licence moves
        let moved = redeem(
            State(s.clone()),
            Json(RedeemReq { hwid: "new-machine".into(), code: "CUTLASS-AAAA-BBBB-CCCC".into() }),
        )
        .await;
        assert!(moved.is_ok(), "a new machine must be able to claim it");
        assert_eq!(status_of(&s, "new-machine").as_deref(), Some("paid"));
        assert_eq!(
            status_of(&s, "old-machine").as_deref(),
            Some("trial"),
            "the machine it moved off must lose the paid licence"
        );

        let (holder, transfers): (String, i64) = {
            let db = s.db.lock_ok();
            db.query_row(
                "SELECT used_by, transfers FROM codes WHERE code=?1",
                rusqlite::params!["CUTLASS-AAAA-BBBB-CCCC"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(holder, "new-machine");
        assert_eq!(transfers, 1, "the move is recorded, so churn is visible");
    }

    /// One code, one machine at a time. Moving it back and forth is allowed --
    /// people do go back to an old laptop -- but it is never live on two.
    #[tokio::test]
    async fn a_code_only_ever_licenses_one_machine_at_a_time() {
        let s = test_state(0.0, 0.0, &[]);
        mint(&s, "CUTLASS-DDDD-EEEE-FFFF");
        for hwid in ["a", "b", "a", "c"] {
            assert!(redeem(
                State(s.clone()),
                Json(RedeemReq { hwid: hwid.into(), code: "CUTLASS-DDDD-EEEE-FFFF".into() }),
            )
            .await
            .is_ok());
        }
        assert_eq!(status_of(&s, "c").as_deref(), Some("paid"), "the last one holds it");
        for loser in ["a", "b"] {
            assert_eq!(status_of(&s, loser).as_deref(), Some("trial"), "{loser} must not still be paid");
        }
    }

    #[tokio::test]
    async fn an_unknown_code_is_still_refused() {
        let s = test_state(0.0, 0.0, &[]);
        let r = redeem(
            State(s.clone()),
            Json(RedeemReq { hwid: "someone".into(), code: "CUTLASS-0000-0000-0000".into() }),
        )
        .await;
        assert!(r.is_err(), "a code that was never minted must not grant anything");
        assert_ne!(status_of(&s, "someone").as_deref(), Some("paid"));
    }
}
