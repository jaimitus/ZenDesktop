//! Key generator for ZenDesktop auto-update signing.
//!
//! Generates an Ed25519 keypair and prints hex-encoded keys.
//!
//! Usage:
//!   cargo run --bin gen-keys
//!
//! Output:
//!   PUBLIC_KEY: <64 hex chars>
//!   SECRET_KEY: <64 hex chars>
//!
//! Keep SECRET_KEY private (store as GitHub Secret `SIGNING_KEY`).
//! Hardcode PUBLIC_KEY in src/updater.rs.

use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;

fn main() {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key: VerifyingKey = (&signing_key).into();

    let secret_hex = hex_encode(signing_key.to_bytes().as_slice());
    let public_hex = hex_encode(verifying_key.as_bytes());

    println!("PUBLIC_KEY: {public_hex}");
    println!("SECRET_KEY: {secret_hex}");
    println!();
    println!("Copy PUBLIC_KEY into src/updater.rs constant PUBKEY.");
    println!("Store SECRET_KEY as GitHub Secret named SIGNING_KEY.");
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
