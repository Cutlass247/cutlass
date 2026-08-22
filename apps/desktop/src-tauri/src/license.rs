//! Client-side licensing: fingerprint the machine, ask the server for a signed
//! lease on launch, cache it (verified against the embedded public key), and
//! decide whether the editor may run.
//!
//! The server is the source of truth for trial-start, so uninstall + reinstall
//! can't reset the trial. The cached lease only enables *offline* use within
//! the grace window the server stamped into it.

use cutlass_license::{verifying_key_from_b64, SignedLease, Status};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Public half of the licensing keypair. Safe to ship; verifies leases offline.
const PUBLIC_KEY_B64: &str = "evVP402EDTzW7mbeHCJlPFANQEAZNe5FCRfdT3Vt+eM=";
/// Default license server. Overridable at runtime with CUTLASS_LICENSE_URL
/// (handy for testing against a local server).
const DEFAULT_SERVER: &str = "https://cutlass-production.up.railway.app";

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn server_url() -> String {
    std::env::var("CUTLASS_LICENSE_URL").unwrap_or_else(|_| DEFAULT_SERVER.to_string())
}

/// What the frontend needs to gate the app.
#[derive(Serialize, Clone, Debug)]
pub struct LicenseInfo {
    /// "trial" | "paid" | "owner" | "expired" | "offline" | "error"
    pub status: String,
    /// May the editor be used right now?
    pub active: bool,
    /// Whole days remaining in a trial (only for `trial`).
    pub days_left: Option<i64>,
    /// This machine's fingerprint (shown so the user can be granted owner/support).
    pub machine_id: String,
    /// Human explanation, shown on the lock / reconnect screen.
    pub message: String,
    /// True when first launch needs internet to start the trial.
    pub needs_online: bool,
}

impl LicenseInfo {
    fn owner(hwid: &str) -> Self {
        Self {
            status: "owner".into(),
            active: true,
            days_left: None,
            machine_id: hwid.into(),
            message: "Creator edition — full access.".into(),
            needs_online: false,
        }
    }
    fn error(hwid: &str, msg: &str) -> Self {
        Self {
            status: "error".into(),
            active: false,
            days_left: None,
            machine_id: hwid.into(),
            message: msg.into(),
            needs_online: false,
        }
    }
}

// ── hardware fingerprint ─────────────────────────────────────────────
#[cfg(windows)]
fn machine_guid() -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey(r"SOFTWARE\Microsoft\Cryptography").ok()?;
    key.get_value("MachineGuid").ok()
}
#[cfg(not(windows))]
fn machine_guid() -> Option<String> {
    None
}

/// A stable, non-reversible id for this machine. Derived from the Windows
/// install GUID + computer name, hashed so we never send raw identifiers.
pub fn machine_id() -> String {
    let mut h = Sha256::new();
    h.update(machine_guid().unwrap_or_default().as_bytes());
    h.update(b"|");
    h.update(std::env::var("COMPUTERNAME").unwrap_or_default().as_bytes());
    h.update(b"|cutlass-hwid-v1");
    let digest = h.finalize();
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

// ── lease cache ──────────────────────────────────────────────────────
fn cache_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("Cutlass").join("lease.json")
}

fn load_cache() -> Option<SignedLease> {
    let bytes = std::fs::read(cache_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_cache(signed: &SignedLease) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec(signed) {
        let _ = std::fs::write(path, bytes);
    }
}

// ── server calls ─────────────────────────────────────────────────────
#[derive(serde::Deserialize)]
struct LeaseResp {
    lease: SignedLease,
}

fn agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout(Duration::from_secs(8));
    // Use the OS TLS stack (SChannel on Windows) for https.
    if let Ok(connector) = native_tls::TlsConnector::new() {
        builder = builder.tls_connector(std::sync::Arc::new(connector));
    }
    builder.build()
}

fn post_lease(path: &str, body: serde_json::Value) -> Option<SignedLease> {
    let url = format!("{}{}", server_url().trim_end_matches('/'), path);
    let resp: LeaseResp = agent().post(&url).send_json(body).ok()?.into_json().ok()?;
    Some(resp.lease)
}

// ── decision ─────────────────────────────────────────────────────────
fn info_from_lease(lease: &cutlass_license::Lease, hwid: &str, t: i64) -> LicenseInfo {
    let active = lease.is_active_at(t);
    let (status, message) = match lease.status {
        Status::Owner => ("owner", "Creator edition — full access.".to_string()),
        Status::Paid => ("paid", "Licensed — thank you.".to_string()),
        Status::Trial if active => (
            "trial",
            format!("{} days left in your free trial.", lease.trial_days_left(t).unwrap_or(0)),
        ),
        Status::Trial => (
            "expired",
            "Your 7-day free trial has ended. Purchase to keep editing.".to_string(),
        ),
    };
    LicenseInfo {
        status: status.into(),
        active,
        days_left: lease.trial_days_left(t),
        machine_id: hwid.into(),
        message,
        needs_online: false,
    }
}

fn offline(hwid: &str, msg: &str) -> LicenseInfo {
    LicenseInfo {
        status: "offline".into(),
        active: false,
        days_left: None,
        machine_id: hwid.into(),
        message: msg.into(),
        needs_online: true,
    }
}

/// Resolve the current entitlement: try the server, fall back to a cached lease
/// within its offline grace, else require connectivity.
pub fn resolve() -> LicenseInfo {
    let hwid = machine_id();

    // Creator edition never touches the server.
    if cfg!(feature = "owner") {
        return LicenseInfo::owner(&hwid);
    }

    let Some(vk) = verifying_key_from_b64(PUBLIC_KEY_B64) else {
        return LicenseInfo::error(&hwid, "Built with an invalid license key.");
    };
    let t = now();
    let app_version = env!("CARGO_PKG_VERSION");

    // 1) Ask the server. A fresh, verified, machine-bound lease wins.
    if let Some(signed) =
        post_lease("/activate", serde_json::json!({ "hwid": hwid, "app_version": app_version }))
    {
        if let Some(lease) = signed.verify(&vk) {
            if lease.hwid == hwid {
                save_cache(&signed);
                return info_from_lease(&lease, &hwid, t);
            }
        }
    }

    // 2) Offline: trust the cached lease only within the grace window.
    if let Some(signed) = load_cache() {
        if let Some(lease) = signed.verify(&vk) {
            if lease.hwid == hwid && t < lease.lease_expires_at {
                return info_from_lease(&lease, &hwid, t);
            }
        }
        return offline(&hwid, "Reconnect to the internet to verify your license.");
    }

    // 3) Never activated and can't reach the server.
    offline(&hwid, "Connect to the internet to start your 7-day free trial.")
}

/// Redeem a purchase code, then re-resolve entitlement.
pub fn redeem(code: &str) -> LicenseInfo {
    let hwid = machine_id();
    if cfg!(feature = "owner") {
        return LicenseInfo::owner(&hwid);
    }
    let Some(vk) = verifying_key_from_b64(PUBLIC_KEY_B64) else {
        return LicenseInfo::error(&hwid, "Built with an invalid license key.");
    };
    match post_lease("/redeem", serde_json::json!({ "hwid": hwid, "code": code })) {
        Some(signed) => match signed.verify(&vk) {
            Some(lease) if lease.hwid == hwid => {
                save_cache(&signed);
                info_from_lease(&lease, &hwid, now())
            }
            _ => LicenseInfo::error(&hwid, "The server returned an invalid response."),
        },
        None => LicenseInfo::error(&hwid, "That code wasn't accepted. Check it and try again."),
    }
}

// ── AI highlights (transcript → server → Claude → best moments) ───────
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Moment {
    pub start: f64,
    pub end: f64,
    pub title: String,
    #[serde(default)]
    pub reason: String,
}

/// Send the transcript (text only — never the video) to the server, which asks
/// Claude for the best moments. Long timeout; the model call takes a while.
pub fn ai_highlights(transcript: Vec<TWord>, count: u32) -> Result<Vec<Moment>, String> {
    let hwid = machine_id();
    let url = format!("{}/highlights", server_url().trim_end_matches('/'));
    let body = serde_json::json!({
        "hwid": hwid,
        "transcript": transcript,
        "count": count,
        "app_version": env!("CARGO_PKG_VERSION"),
    });
    match agent().post(&url).timeout(Duration::from_secs(120)).send_json(body) {
        Ok(resp) => resp.into_json::<Vec<Moment>>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default();
            Err(format!("{code}: {}", msg.chars().take(200).collect::<String>()))
        }
        Err(e) => Err(format!("Couldn't reach the highlights service: {e}")),
    }
}

// ── Cloud transcription (Groq Whisper — audio chunk → word timestamps) ───
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SttWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// Upload one compressed audio chunk to the server (→ Groq) and get back its
/// words, already stamped into absolute time by `offset` (seconds). Only the
/// audio leaves the machine — never the video.
pub fn cloud_transcribe_chunk(audio: Vec<u8>, offset: f64) -> Result<Vec<SttWord>, String> {
    let hwid = machine_id();
    let url = format!(
        "{}/transcribe?hwid={}&offset={}&app_version={}",
        server_url().trim_end_matches('/'),
        hwid,
        offset,
        env!("CARGO_PKG_VERSION"),
    );
    match agent()
        .post(&url)
        .timeout(Duration::from_secs(180))
        .set("Content-Type", "audio/flac")
        .send_bytes(&audio)
    {
        Ok(resp) => resp.into_json::<Vec<SttWord>>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default();
            Err(format!("{code}: {}", msg.chars().take(200).collect::<String>()))
        }
        Err(e) => Err(format!("Couldn't reach the transcription service: {e}")),
    }
}
