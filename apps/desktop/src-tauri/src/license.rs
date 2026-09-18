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

/// A stable, non-reversible id for this machine, from the Windows install GUID,
/// hashed so we never send a raw identifier anywhere.
///
/// The computer name used to be mixed in as well, and must not be: it is the
/// one part a user can change on a whim, from Settings, and changing it moved
/// the machine id. That broke both directions at once — someone who had paid
/// was locked out of what they bought, and anyone could reset an expired trial
/// by renaming their PC, which is exactly what binding the trial to hardware
/// was for. It contributed almost no uniqueness either; the install GUID is
/// already per-installation.
pub fn machine_id() -> String {
    let mut h = Sha256::new();
    h.update(machine_guid().unwrap_or_default().as_bytes());
    h.update(b"|cutlass-hwid-v2");
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

/// Where to send someone who wants to buy.
///
/// Nothing about the store is compiled in here, deliberately. Going live on
/// Lemon Squeezy creates new products with new ids and new checkout links, so
/// anything baked into the app is pointing at the old ones the moment it
/// matters — and the copies already installed would keep opening a checkout
/// that cannot take money. The server owns the current link and redirects to
/// it, which is why this is a URL to open rather than a request to make: no
/// network call here, nothing to fail, and going live needs no new build.
///
/// The machine id travels with it because payment is tied to a machine — the
/// webhook drops an order that arrives without one — and the server will only
/// forward to a real checkout when it has one.
pub fn checkout_url(kind: &str) -> String {
    let kind = if kind == "credits" { "credits" } else { "license" };
    format!(
        "{}/buy/{}?hwid={}",
        server_url().trim_end_matches('/'),
        kind,
        machine_id()
    )
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

/// Decide from a cached lease alone, when the server can't be reached.
///
/// Two things have to hold, and both are security-relevant: the lease must
/// belong to *this* machine, so one cannot be copied to another, and it must
/// still be inside the offline grace window the server stamped into it, so a
/// cached lease can't be trusted indefinitely by staying disconnected.
/// Separated from `resolve` so those rules can actually be tested.
fn from_cached_lease(
    lease: &cutlass_license::Lease,
    hwid: &str,
    t: i64,
) -> Option<LicenseInfo> {
    if lease.hwid == hwid && t < lease.lease_expires_at {
        Some(info_from_lease(lease, hwid, t))
    } else {
        None
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
            if let Some(info) = from_cached_lease(&lease, &hwid, t) {
                return info;
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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Moment {
    pub start: f64,
    pub end: f64,
    pub title: String,
    #[serde(default)]
    pub reason: String,
}

/// Why a highlights request failed.
///
/// The app falls back to its on-device finder for `Offline` and must never do
/// so for `Failed`. That line is the entire reason this type exists: "nothing
/// answered" is a reason to do the work locally, while "the server answered
/// and said no" — no licence, allowance spent, AI not configured — is a gate.
/// Falling back there would hand out the paid feature through another door.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AiError {
    /// Nothing answered: no network, DNS failure, refused connection, timeout.
    Offline { message: String },
    /// The server answered, and the answer was not usable.
    Failed { status: u16, message: String },
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Offline { message } => write!(f, "{message}"),
            AiError::Failed { status, message } => write!(f, "{status}: {message}"),
        }
    }
}

/// Send the transcript (text only — never the video) to the server, which asks
/// Claude for the best moments. Long timeout; the model call takes a while.
pub fn ai_highlights(transcript: Vec<TWord>, count: u32) -> Result<Vec<Moment>, AiError> {
    ai_highlights_at(&server_url(), transcript, count)
}

/// The body of [`ai_highlights`], with the server address passed in.
///
/// Split out so a test can point it at a closed port and check that a refused
/// connection comes back as `Offline`. That mapping is what decides whether
/// the on-device finder runs, and it cannot be checked through `server_url()`
/// without mutating the environment out from under every other test.
fn ai_highlights_at(base: &str, transcript: Vec<TWord>, count: u32) -> Result<Vec<Moment>, AiError> {
    let hwid = machine_id();
    let url = format!("{}/highlights", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "hwid": hwid,
        "transcript": transcript,
        "count": count,
        "app_version": env!("CARGO_PKG_VERSION"),
    });
    match agent().post(&url).timeout(Duration::from_secs(120)).send_json(body) {
        Ok(resp) => {
            // Read the status before the body is consumed. A reply we cannot
            // parse still came from the server, so it is Failed, not Offline —
            // a garbled response must not unlock the local fallback either.
            let status = resp.status();
            resp.into_json::<Vec<Moment>>().map_err(|e| AiError::Failed {
                status,
                message: e.to_string(),
            })
        }
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default();
            Err(AiError::Failed {
                status: code,
                message: msg.chars().take(200).collect::<String>(),
            })
        }
        Err(e) => Err(AiError::Offline {
            message: format!("Couldn't reach the highlights service: {e}"),
        }),
    }
}

// ── AI usage (how much of the monthly AI allowance is left) ──────────────
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AiUsage {
    pub used_minutes: f64,
    pub cap_minutes: f64,
    pub credit_minutes: f64,
    pub remaining_minutes: f64, // -1 = unlimited
    pub unlimited: bool,
    pub period: String,
}

/// Ask the server how much AI processing this machine has used / has left this
/// month. Best-effort (the readout just hides on failure).
pub fn ai_usage() -> Result<AiUsage, String> {
    let hwid = machine_id();
    let url = format!("{}/usage?hwid={}", server_url().trim_end_matches('/'), hwid);
    match agent().get(&url).timeout(Duration::from_secs(15)).call() {
        Ok(resp) => resp.into_json::<AiUsage>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            Err(format!("{code}: {}", r.into_string().unwrap_or_default().chars().take(120).collect::<String>()))
        }
        Err(e) => Err(format!("Couldn't reach the usage service: {e}")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cutlass_license::{Lease, Status, NEVER};

    const DAY: i64 = 86_400;

    /// The Buy button must never lead to a store URL this app chose, and must
    /// always name the machine to grant to.
    ///
    /// Both halves cost money if they regress. A compiled-in store link is
    /// wrong the day the store moves, which is the day it starts mattering;
    /// and a checkout reached without an hwid takes the card and grants
    /// nothing, because the webhook drops an order that cannot be attributed
    /// and still answers 200, so nobody finds out.
    #[test]
    fn buying_goes_through_our_own_server_and_names_the_machine() {
        for kind in ["license", "credits"] {
            let url = checkout_url(kind);
            assert!(
                url.starts_with(&server_url()),
                "the buy link left our server, so the store got compiled in: {url}"
            );
            assert!(
                !url.contains("lemonsqueezy"),
                "a store URL is baked into the app again: {url}"
            );
            assert!(url.contains(&format!("/buy/{kind}")), "wrong product: {url}");
            assert!(
                url.contains(&format!("hwid={}", machine_id())),
                "no machine to grant the purchase to: {url}"
            );
        }
        // An unrecognised kind must not invent a third product or drop the
        // path segment and point at the server root.
        assert!(checkout_url("nonsense").contains("/buy/license"));
    }

    /// The frontend decides whether to run the on-device finder by switching on
    /// `kind`. Rename a variant and one of two things happens silently: the app
    /// stops falling back when offline, or it starts falling back when the
    /// server refused — and the second one gives the paid feature away to
    /// anyone whose licence has lapsed. Neither shows up as a failing build.
    #[test]
    fn being_offline_looks_different_to_being_refused() {
        let offline = serde_json::to_value(AiError::Offline {
            message: "dns error".into(),
        })
        .unwrap();
        assert_eq!(offline["kind"], "offline");

        let refused = serde_json::to_value(AiError::Failed {
            status: 402,
            message: "monthly AI allowance spent".into(),
        })
        .unwrap();
        assert_eq!(refused["kind"], "failed");
        assert_eq!(refused["status"], 402);
    }

    /// The mapping the fallback actually hangs on: a machine that cannot open
    /// the connection must come back `Offline`, so the on-device finder runs.
    /// Port 1 is reserved and nothing listens on it, so this is a refused
    /// connection — the same class of failure as having no network — without
    /// needing the test host to be offline.
    #[test]
    fn a_connection_that_never_opens_is_offline_not_a_refusal() {
        let words = vec![TWord {
            text: "hello".into(),
            start: 0.0,
            end: 0.5,
        }];
        match ai_highlights_at("http://127.0.0.1:1", words, 8) {
            Err(AiError::Offline { .. }) => {}
            other => panic!("expected Offline, got {other:?}"),
        }
    }

    fn lease(status: Status, expires_at: i64, lease_expires_at: i64) -> Lease {
        Lease {
            hwid: "this-machine".into(),
            status,
            trial_start: 0,
            expires_at,
            lease_expires_at,
            issued_at: 0,
        }
    }

    /// The failure that matters most: someone who paid must never be told they
    /// cannot use what they bought, whatever the clock says.
    #[test]
    fn a_paid_licence_is_active_no_matter_the_time() {
        let paid = lease(Status::Paid, NEVER, NEVER);
        for t in [0, DAY, 10_000 * DAY, NEVER - 1] {
            let info = info_from_lease(&paid, "this-machine", t);
            assert!(info.active, "paid licence must stay active at t={t}");
            assert_eq!(info.status, "paid");
            assert_eq!(info.days_left, None, "paid has no trial countdown");
        }
    }

    #[test]
    fn the_owner_edition_is_always_active() {
        let owner = lease(Status::Owner, NEVER, NEVER);
        let info = info_from_lease(&owner, "this-machine", 10_000 * DAY);
        assert!(info.active);
        assert_eq!(info.status, "owner");
    }

    /// The trial boundary, exactly. Off by one here either cuts a trial a day
    /// short or hands out a free day.
    #[test]
    fn a_trial_runs_until_its_expiry_and_not_past_it() {
        let expiry = 7 * DAY;
        let t = lease(Status::Trial, expiry, expiry);

        assert!(info_from_lease(&t, "this-machine", expiry - 1).active, "live one second before");
        assert!(!info_from_lease(&t, "this-machine", expiry).active, "over at the expiry itself");
        assert!(!info_from_lease(&t, "this-machine", expiry + 1).active, "and after");

        let ended = info_from_lease(&t, "this-machine", expiry + DAY);
        assert_eq!(ended.status, "expired");
        assert_eq!(ended.days_left, Some(0), "never a negative countdown");
        assert!(ended.message.contains("Purchase"), "expired should say what to do next");
    }

    #[test]
    fn the_days_remaining_count_rounds_the_way_a_person_would() {
        let expiry = 7 * DAY;
        let t = lease(Status::Trial, expiry, expiry);
        let days = |now: i64| info_from_lease(&t, "this-machine", now).days_left;
        assert_eq!(days(0), Some(7), "a whole 7 days at the start");
        assert_eq!(days(1), Some(7), "a second in is still 'seven days left'");
        assert_eq!(days(6 * DAY), Some(1), "part of a day still reads as a day");
        assert_eq!(days(7 * DAY - 1), Some(1), "the last second is still a day");
        assert_eq!(days(7 * DAY), Some(0));
    }

    /// A cached lease is only good on the machine it was issued to. Otherwise
    /// copying one file to another computer would be a free licence.
    #[test]
    fn a_cached_lease_from_another_machine_is_refused() {
        let l = lease(Status::Paid, NEVER, NEVER); // issued to "this-machine"
        assert!(from_cached_lease(&l, "this-machine", DAY).is_some());
        assert!(
            from_cached_lease(&l, "a-different-machine", DAY).is_none(),
            "a lease must not travel between machines"
        );
    }

    /// Staying offline must not extend a licence forever.
    #[test]
    fn a_cached_lease_expires_out_of_its_offline_grace() {
        let grace_ends = 3 * DAY;
        let l = lease(Status::Paid, NEVER, grace_ends);
        assert!(from_cached_lease(&l, "this-machine", grace_ends - 1).is_some(), "inside the window");
        assert!(from_cached_lease(&l, "this-machine", grace_ends).is_none(), "at the edge");
        assert!(from_cached_lease(&l, "this-machine", grace_ends + DAY).is_none(), "past it");
    }

    /// A trial that ended while offline is still *readable* — the app should
    /// say the trial is over, not that something went wrong.
    #[test]
    fn an_expired_trial_inside_grace_reads_as_expired_not_as_an_error() {
        let l = lease(Status::Trial, 7 * DAY, 30 * DAY);
        let info = from_cached_lease(&l, "this-machine", 8 * DAY).expect("still within grace");
        assert_eq!(info.status, "expired");
        assert!(!info.active);
        assert!(!info.needs_online, "this is a decided answer, not a connectivity problem");
    }

    /// A clock set backwards must not lock anyone out; it is the generous
    /// direction, and users do change their clocks.
    #[test]
    fn a_clock_set_backwards_does_not_lock_anyone_out() {
        let l = lease(Status::Trial, 7 * DAY, 30 * DAY);
        let info = from_cached_lease(&l, "this-machine", -5 * DAY).expect("still granted");
        assert!(info.active, "an earlier clock must not deny access");
    }
}
