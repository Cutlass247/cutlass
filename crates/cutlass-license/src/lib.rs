//! Shared licensing primitives for Cutlass.
//!
//! The server mints a **lease** — a small, Ed25519-signed statement of a
//! machine's entitlement (trial with an expiry, paid, or owner). The client
//! caches the lease and verifies it offline against an embedded public key, so
//! a brief server outage can't brick anyone mid-trial or a paying customer.
//!
//! Signing (minting) lives behind the `sign` feature so only the server links
//! the private key; the client compiles with verify-only.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};

// Re-export the key types so downstream crates name them via cutlass_license.
pub use ed25519_dalek::VerifyingKey as PublicKey;

/// Sentinel expiry meaning "never" (paid / owner).
pub const NEVER: i64 = i64::MAX;

/// What a machine is entitled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// In the 7-day free trial; access until `expires_at`.
    Trial,
    /// Purchased — full access, no expiry.
    Paid,
    /// The creator's machine — full access, no expiry, no server needed.
    Owner,
}

/// The signed statement of entitlement. `expires_at` gates the app; when the
/// server is unreachable the client keeps trusting a cached lease only until
/// `lease_expires_at` (the offline grace window), then must re-check online.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    /// Hardware fingerprint this lease is bound to.
    pub hwid: String,
    pub status: Status,
    /// Unix seconds the trial began (server's clock, so a client can't rewind it).
    pub trial_start: i64,
    /// Unix seconds access ends. `NEVER` for paid/owner.
    pub expires_at: i64,
    /// Unix seconds until which the client may trust this lease while offline.
    pub lease_expires_at: i64,
    /// Unix seconds this lease was minted.
    pub issued_at: i64,
}

impl Lease {
    /// True if access is currently granted, judged against `now` (unix secs).
    /// Owner/paid are always live; a trial is live until it expires.
    pub fn is_active_at(&self, now: i64) -> bool {
        match self.status {
            Status::Owner | Status::Paid => true,
            Status::Trial => now < self.expires_at,
        }
    }

    /// Whole days of trial left at `now` (0 once expired). Non-trial → None.
    pub fn trial_days_left(&self, now: i64) -> Option<i64> {
        match self.status {
            Status::Trial => Some(((self.expires_at - now).max(0) + 86_399) / 86_400),
            _ => None,
        }
    }
}

/// A lease plus its detached signature, as shipped over the wire and cached on
/// disk. `payload` is the exact signed bytes (base64) so verification never
/// depends on re-serialising the struct identically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedLease {
    /// base64 of the canonical JSON bytes that were signed.
    pub payload: String,
    /// base64 of the 64-byte Ed25519 signature.
    pub sig: String,
}

impl SignedLease {
    /// Verify against the embedded public key and return the lease if the
    /// signature is valid and the payload parses. Any tampering → None.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> Option<Lease> {
        let payload = B64.decode(self.payload.as_bytes()).ok()?;
        let sig_bytes = B64.decode(self.sig.as_bytes()).ok()?;
        let sig_arr: [u8; SIGNATURE_LENGTH] = sig_bytes.try_into().ok()?;
        let sig = Signature::from_bytes(&sig_arr);
        verifying_key.verify_strict(&payload, &sig).ok()?;
        serde_json::from_slice(&payload).ok()
    }
}

/// Parse a 32-byte Ed25519 public key from base64 (how the client embeds it).
pub fn verifying_key_from_b64(s: &str) -> Option<VerifyingKey> {
    let bytes = B64.decode(s.trim().as_bytes()).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

#[cfg(feature = "sign")]
mod signing {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Load a 32-byte Ed25519 private key from base64 (server reads this from a
    /// secret at runtime — it is never compiled into any binary).
    pub fn signing_key_from_b64(s: &str) -> Option<SigningKey> {
        let bytes = B64.decode(s.trim().as_bytes()).ok()?;
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some(SigningKey::from_bytes(&arr))
    }

    /// Mint a signed lease for `lease` with the server's private key.
    pub fn issue(lease: &Lease, signing_key: &SigningKey) -> SignedLease {
        let payload = serde_json::to_vec(lease).expect("lease serialises");
        let sig = signing_key.sign(&payload);
        SignedLease {
            payload: B64.encode(&payload),
            sig: B64.encode(sig.to_bytes()),
        }
    }
}

#[cfg(feature = "sign")]
pub use ed25519_dalek::SigningKey as PrivateKey;
#[cfg(feature = "sign")]
pub use signing::{issue, signing_key_from_b64};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "sign")]
    #[test]
    fn round_trips_and_rejects_tampering() {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::generate(&mut rand::rngs::OsRng);
        let vk = sk.verifying_key();

        let lease = Lease {
            hwid: "abc123".into(),
            status: Status::Trial,
            trial_start: 1_000,
            expires_at: 1_000 + 7 * 86_400,
            lease_expires_at: 1_000 + 3 * 86_400,
            issued_at: 1_000,
        };
        let signed = issue(&lease, &sk);

        // valid signature → parses back
        let got = signed.verify(&vk).expect("verifies");
        assert_eq!(got.hwid, "abc123");
        assert!(got.is_active_at(1_500));
        assert!(!got.is_active_at(1_000 + 7 * 86_400 + 1));
        assert_eq!(got.trial_days_left(1_000), Some(7));

        // flip a payload byte → rejected
        let mut tampered = signed.clone();
        let mut raw = B64.decode(tampered.payload.as_bytes()).unwrap();
        raw[5] ^= 0x01;
        tampered.payload = B64.encode(&raw);
        assert!(tampered.verify(&vk).is_none(), "tampered payload must fail");

        // wrong key → rejected
        let other = SigningKey::generate(&mut rand::rngs::OsRng).verifying_key();
        assert!(signed.verify(&other).is_none(), "wrong key must fail");
    }

    #[test]
    fn owner_and_paid_never_expire() {
        let owner = Lease {
            hwid: "me".into(),
            status: Status::Owner,
            trial_start: 0,
            expires_at: NEVER,
            lease_expires_at: NEVER,
            issued_at: 0,
        };
        assert!(owner.is_active_at(NEVER - 1));
        assert_eq!(owner.trial_days_left(0), None);
    }
}
