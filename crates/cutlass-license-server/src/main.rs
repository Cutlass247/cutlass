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

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
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
    anthropic_key: Option<String>,
    groq_key: Option<String>,
    /// Monthly AI processing allowance in SECONDS (0 = unlimited / track-only).
    ai_monthly_secs: f64,
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
         -- allowance is spent; granted via /admin/grant.
         CREATE TABLE IF NOT EXISTS ai_credits (
            hwid         TEXT PRIMARY KEY,
            seconds      REAL NOT NULL DEFAULT 0
         );",
    )
    .expect("init db");
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

/// Entitlement + usage gate for the AI endpoints. Returns Ok(is_owner) when the
/// call may proceed, or an error status. Owners are unlimited; a monthly cap of
/// 0 means "track usage but never block" (the beta default).
fn ai_gate(
    state: &AppState,
    hwid: &str,
    ver: Option<&str>,
    t: i64,
) -> Result<bool, (StatusCode, String)> {
    let db = state.db.lock().unwrap();
    let row = get_or_create(&db, &state.cfg, hwid, ver, t);
    let is_owner = row.status == "owner";
    if !lease_for(&row, &state.cfg, t).is_active_at(t) {
        return Err((StatusCode::PAYMENT_REQUIRED, "no active licence for AI features".into()));
    }
    if !is_owner && state.cfg.ai_monthly_secs > 0.0 {
        let period = year_month(t);
        let used = ai_used_secs(&db, hwid, &period);
        let credits = ai_credit_secs(&db, hwid);
        if used >= state.cfg.ai_monthly_secs && credits <= 0.0 {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "monthly AI limit reached — add credits or switch to Private (on-device) transcription".into(),
            ));
        }
    }
    Ok(is_owner)
}

/// Record `cost_secs` of AI media processing against the monthly allowance,
/// spilling the overflow onto the persistent credit balance. No-op for owners.
fn ai_charge(state: &AppState, hwid: &str, is_owner: bool, cost_secs: f64, t: i64) {
    if is_owner || cost_secs <= 0.0 {
        return;
    }
    let db = state.db.lock().unwrap();
    let period = year_month(t);
    let cap = state.cfg.ai_monthly_secs;
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
    let is_owner = ai_gate(&state, &hwid, req.app_version.as_deref(), t)?;

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
    ai_charge(&state, &hwid, is_owner, span.max(0.0), t);
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
    let is_owner = ai_gate(&state, &hwid, q.app_version.as_deref(), t)?;

    let audio = body.to_vec();
    let mut words = tokio::task::spawn_blocking(move || call_groq(&key, &audio))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    // meter this chunk's audio length before stamping absolute time
    let chunk_secs = words.last().map(|w| w.end).unwrap_or(0.0)
        - words.first().map(|w| w.start).unwrap_or(0.0);
    ai_charge(&state, &hwid, is_owner, chunk_secs.max(0.0), t);
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
    let db = state.db.lock().unwrap();
    let used = ai_used_secs(&db, hwid, &period);
    let credits = ai_credit_secs(&db, hwid);
    drop(db);
    let cap = state.cfg.ai_monthly_secs;
    let unlimited = state.cfg.owner_hwids.contains(hwid) || cap <= 0.0;
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
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO ai_credits (hwid, seconds) VALUES (?1, ?2)
         ON CONFLICT(hwid) DO UPDATE SET seconds = seconds + ?2",
        rusqlite::params![hwid, secs],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let bal = ai_credit_secs(&db, hwid);
    Ok(Json(serde_json::json!({ "hwid": hwid, "credit_minutes": (bal / 60.0).round() })))
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
        .route("/admin/grant", post(grant))
        .route("/usage", get(usage))
        .route("/highlights", post(highlights))
        // audio chunks can be a few MB — lift axum's 2 MB default for this route
        .route(
            "/transcribe",
            post(transcribe).layer(DefaultBodyLimit::max(48 * 1024 * 1024)),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(cap_minutes: f64, owners: &[&str]) -> AppState {
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
        };
        // any valid 32-byte key — the signing key is unused by the metering path
        let key = signing_key_from_b64("AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=").unwrap();
        AppState {
            db: Arc::new(Mutex::new(conn)),
            cfg: Arc::new(cfg),
            key: Arc::new(key),
        }
    }

    #[test]
    fn year_month_known_date() {
        // 1_755_000_000 → ~Aug 12 2025 UTC
        assert_eq!(year_month(1_755_000_000), "2025-08");
    }

    #[test]
    fn charge_accumulates_and_cap_blocks() {
        let t = now();
        let s = test_state(10.0, &[]); // 10-minute monthly cap
        let hwid = "unittesthwid00";
        assert!(ai_gate(&s, hwid, None, t).is_ok());
        ai_charge(&s, hwid, false, 6.0 * 60.0, t); // 6 min
        {
            let db = s.db.lock().unwrap();
            assert!((ai_used_secs(&db, hwid, &year_month(t)) - 360.0).abs() < 1.0);
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok()); // still under cap
        ai_charge(&s, hwid, false, 5.0 * 60.0, t); // now 11 min, over cap
        let g = ai_gate(&s, hwid, None, t);
        assert_eq!(g.unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn credits_allow_past_cap_and_drain() {
        let t = now();
        let s = test_state(1.0, &[]); // 1-minute cap
        let hwid = "credittest0000";
        ai_gate(&s, hwid, None, t).unwrap();
        ai_charge(&s, hwid, false, 60.0, t); // exactly at cap
        {
            let db = s.db.lock().unwrap();
            db.execute(
                "INSERT INTO ai_credits (hwid, seconds) VALUES (?1, ?2)",
                rusqlite::params![hwid, 120.0],
            )
            .unwrap();
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok()); // at cap but has credits
        ai_charge(&s, hwid, false, 90.0, t); // spills fully onto credits
        let db = s.db.lock().unwrap();
        assert!((ai_credit_secs(&db, hwid) - 30.0).abs() < 1.0);
    }

    #[test]
    fn owner_is_unlimited() {
        let t = now();
        let s = test_state(1.0, &["ownerhwid00000"]);
        let hwid = "ownerhwid00000";
        let is_owner = ai_gate(&s, hwid, None, t).unwrap();
        assert!(is_owner);
        ai_charge(&s, hwid, true, 9_999.0 * 60.0, t); // no-op for owners
        {
            let db = s.db.lock().unwrap();
            assert_eq!(ai_used_secs(&db, hwid, &year_month(t)), 0.0);
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok());
    }

    #[test]
    fn zero_cap_tracks_but_never_blocks() {
        let t = now();
        let s = test_state(0.0, &[]); // 0 = unlimited / track-only
        let hwid = "betatrackonly0";
        ai_charge(&s, hwid, false, 9_999.0 * 60.0, t);
        {
            let db = s.db.lock().unwrap();
            assert!(ai_used_secs(&db, hwid, &year_month(t)) > 0.0); // usage recorded
        }
        assert!(ai_gate(&s, hwid, None, t).is_ok()); // never blocked
    }
}
