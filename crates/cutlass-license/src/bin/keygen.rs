//! Generate a fresh Ed25519 licensing keypair.
//!
//!   cargo run -p cutlass-license --features sign --bin cutlass-keygen
//!
//! Prints both keys as base64. The PUBLIC key is embedded in the client; the
//! PRIVATE key is a server secret — store it in a password manager and NEVER
//! commit it. Prints to stdout only (no files written) so it can't be left on
//! disk by accident.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::SigningKey;

fn main() {
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let vk = sk.verifying_key();
    println!("# Cutlass licensing keypair (Ed25519)");
    println!("# PUBLIC KEY  — embed in the client (safe to commit):");
    println!("PUBLIC={}", B64.encode(vk.to_bytes()));
    println!("# PRIVATE KEY — server secret. Store securely, NEVER commit:");
    println!("PRIVATE={}", B64.encode(sk.to_bytes()));
}
