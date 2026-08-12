//! Release signer for ZenDesktop CI pipeline.
//!
//! Signs a file using the Ed25519 private key from SIGNING_KEY env var.
//!
//! Usage:
//!   SIGNING_KEY=<64 hex chars> cargo run --release --bin sign-release -- <file>
//!
//! Produces: <file>.sig (64 bytes, raw Ed25519 signature)

use ed25519_dalek::Signer;
use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: SIGNING_KEY=<hex> {} <file>", args[0]);
        process::exit(1);
    }

    let key_hex = env::var("SIGNING_KEY").unwrap_or_else(|_| {
        eprintln!("ERROR: SIGNING_KEY env var not set");
        process::exit(1);
    });

    let key_bytes = hex_decode(&key_hex).unwrap_or_else(|e| {
        eprintln!("ERROR: invalid SIGNING_KEY hex: {e}");
        process::exit(1);
    });

    let signing_key: ed25519_dalek::SigningKey = ed25519_dalek::SigningKey::from_bytes(&key_bytes);

    let file_path = &args[1];
    let data = fs::read(file_path).unwrap_or_else(|e| {
        eprintln!("ERROR reading {file_path}: {e}");
        process::exit(1);
    });

    let signature = signing_key.sign(&data);
    let sig_path = format!("{file_path}.sig");
    fs::write(&sig_path, signature.to_bytes()).unwrap_or_else(|e| {
        eprintln!("ERROR writing {sig_path}: {e}");
        process::exit(1);
    });

    eprintln!("Signed: {sig_path}");
}

fn hex_decode(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
        bytes[i] = u8::from_str_radix(s, 16).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}
