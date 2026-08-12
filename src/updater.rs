//! ZenDesktop :: updater.rs
//!
//! Auto-update via GitHub Releases API with Ed25519 signature verification.
//! No heavy dependencies: just ureq + serde_json + ed25519-dalek.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use serde::Deserialize;

const GITHUB_API: &str = "https://api.github.com/repos/jaimitus/ZenDesktop/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// Ed25519 public key (hex). Generated 2026-08-12 with `cargo run --bin gen-keys`.
// The matching private key is stored as the GitHub Actions secret SIGNING_KEY.
const PUBKEY_HEX: &str = "df1c9091cd42eb37b8986f5df342667c808ac3c2fb27a9e5f1465198c2d17489";

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    body: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

/// Resultado de la comprobacion de actualizaciones.
#[derive(Debug)]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable { version: String, url: String, sig_url: String },
    Error(String),
}

/// Resultado del chequeo en segundo plano, leido por el hilo de UI.
static LAST_CHECK: Mutex<Option<UpdateStatus>> = Mutex::new(None);

/// Almacena el resultado del ultimo chequeo en segundo plano.
pub fn store_last_check(status: UpdateStatus) {
    if let Ok(mut slot) = LAST_CHECK.lock() {
        *slot = Some(status);
    }
}

/// Recupera (y consume) el resultado del chequeo en segundo plano.
pub fn take_last_check() -> Option<UpdateStatus> {
    LAST_CHECK.lock().ok().and_then(|mut slot| slot.take())
}

/// Actualizacion pendiente de confirmar por el usuario (toast clicable).
static PENDING_UPDATE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Almacena la actualizacion pendiente cuando se avisa con un toast.
pub fn set_pending_update(url: String, sig_url: String) {
    if let Ok(mut slot) = PENDING_UPDATE.lock() {
        *slot = Some((url, sig_url));
    }
}

/// Recupera (y consume) la actualizacion pendiente de instalar.
pub fn take_pending_update() -> Option<(String, String)> {
    PENDING_UPDATE.lock().ok().and_then(|mut slot| slot.take())
}

/// Consulta la API de GitHub Releases y compara con la version actual.
pub fn check_update() -> UpdateStatus {
    let response = match ureq::get(GITHUB_API)
        .set("User-Agent", "ZenDesktop-Updater/1.0")
        .set("Accept", "application/vnd.github.v3+json")
        .call()
    {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Error(format!("Network error: {e}")),
    };

    let body = match response.into_string() {
        Ok(b) => b,
        Err(e) => return UpdateStatus::Error(format!("Read error: {e}")),
    };

    let release: GitHubRelease = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Error(format!("Parse error: {e}")),
    };

    let latest = release.tag_name.trim_start_matches('v');
    if version_at_least(latest, CURRENT_VERSION) {
        return UpdateStatus::UpToDate;
    }

    // Buscar el asset .exe portable y su .sig
    let asset = release.assets.iter().find(|a| a.name == "ZenDesktop.exe");
    let sig_asset = release.assets.iter().find(|a| a.name == "ZenDesktop.exe.sig");

    match (asset, sig_asset) {
        (Some(a), Some(s)) => UpdateStatus::UpdateAvailable {
            version: latest.to_string(),
            url: a.browser_download_url.clone(),
            sig_url: s.browser_download_url.clone(),
        },
        (Some(_), None) => UpdateStatus::Error("No .sig signature found in release".into()),
        (None, _) => UpdateStatus::Error("No .exe asset found".into()),
    }
}

/// Decodes the hardcoded public key from hex.
fn pubkey() -> Option<ed25519_dalek::VerifyingKey> {
    let bytes = hex_decode(PUBKEY_HEX)?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok()
}

/// Verifies an Ed25519 signature against the embedded public key.
fn verify_signature(data: &[u8], sig_bytes: &[u8; 64]) -> bool {
    let vk = match pubkey() {
        Some(k) => k,
        None => return false,
    };
    let sig = ed25519_dalek::Signature::from_bytes(sig_bytes);
    vk.verify_strict(data, &sig).is_ok()
}

/// Downloads and installs the update, then auto-restarts the app.
/// Verifies Ed25519 signature before installing.
pub fn download_and_install(url: &str, sig_url: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let backup = current.with_extension("exe.bak");
    let temp = current.with_extension("exe.new");

    // Download .exe
    let response = ureq::get(url)
        .set("User-Agent", "ZenDesktop-Updater/1.0")
        .call()
        .map_err(|e| format!("Download error: {e}"))?;

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&temp)
        .map_err(|e| format!("Create temp: {e}"))?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|e| format!("Write error: {e}"))?;

    // Download .sig
    let sig_response = ureq::get(sig_url)
        .set("User-Agent", "ZenDesktop-Updater/1.0")
        .call()
        .map_err(|e| format!("Signature download error: {e}"))?;

    let mut sig_bytes = [0u8; 64];
    let sig_reader = sig_response.into_reader();
    let n = std::io::Read::take(sig_reader, 64)
        .read(&mut sig_bytes)
        .map_err(|e| format!("Sig read error: {e}"))?;

    if n != 64 {
        return Err(format!("Invalid signature size: {n} bytes (expected 64)"));
    }

    // Verify signature
    let exe_data = std::fs::read(&temp)
        .map_err(|e| format!("Read temp for verify: {e}"))?;

    if !verify_signature(&exe_data, &sig_bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err("Signature verification FAILED — possible tampering!".into());
    }

    // Verify file size sanity
    let meta = std::fs::metadata(&temp).map_err(|e| format!("metadata: {e}"))?;
    if meta.len() < 100_000 {
        let _ = std::fs::remove_file(&temp);
        return Err("Downloaded file too small".into());
    }

    // Rename current -> backup, new -> current
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    std::fs::rename(&current, &backup)
        .map_err(|e| format!("Backup error: {e}"))?;

    // Replace the exe; if this fails, restore the backup so the app
    // stays runnable and the update can be retried.
    if let Err(e) = std::fs::rename(&temp, &current) {
        let _ = std::fs::rename(&backup, &current);
        return Err(format!("Replace error: {e} (original restored)"));
    }

    // El .bak (el ejecutable antiguo, todavia en uso) no se puede borrar
    // ahora: lo limpiara el proceso nuevo al arrancar (ver main.rs).

    // Lanza la nueva version en modo "relevo": esperara a que este proceso
    // cierre y suelte el mutex de instancia unica antes de tomar el control.
    // El caller debe cerrar la aplicacion inmediatamente despues.
    let _ = std::process::Command::new(&current).arg("--update-restart").spawn();

    Ok(current)
}

/// Returns the current version of the binary.
pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

/// True if `a >= b` as semantic versions (handles multi-digit segments,
/// e.g. 1.0.10 > 1.0.9). Missing segments default to 0.
fn version_at_least(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> (u64, u64, u64) {
        let mut it = v.split('.');
        let p = |s: Option<&str>| s.and_then(|x| x.parse().ok()).unwrap_or(0);
        (p(it.next()), p(it.next()), p(it.next()))
    }
    parts(a) >= parts(b)
}

fn hex_decode(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        bytes[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::version_at_least;

    #[test]
    fn version_compares_multi_digit_segments() {
        // String comparison would get this wrong ("1.0.9" > "1.0.10").
        assert!(version_at_least("1.0.10", "1.0.9"));
        assert!(!version_at_least("1.0.9", "1.0.10"));
    }

    #[test]
    fn version_compares_same_and_newer() {
        assert!(version_at_least("1.0.0", "1.0.0"));
        assert!(version_at_least("1.1.0", "1.0.9"));
        assert!(version_at_least("2.0.0", "1.9.9"));
        assert!(!version_at_least("1.0.0", "1.0.1"));
    }

    #[test]
    fn version_handles_missing_segments() {
        assert!(version_at_least("1.0", "0.9.9"));
        assert!(version_at_least("2", "1.9.9"));
    }
}
