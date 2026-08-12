//! ZenDesktop :: updater.rs
//!
//! Auto-update via GitHub Releases API with Ed25519 signature verification.
//! No heavy dependencies: just ureq + serde_json + ed25519-dalek.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use serde::Deserialize;
use windows::core::{w, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

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

/// Agente HTTP con timeouts: no se cuelga si GitHub tarda en responder.
fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .build()
}

/// Consulta la API de GitHub Releases y compara con la version actual.
/// Reintenta ante cortes transitorios: el CDN/API de GitHub corta conexiones
/// de vez en cuando ("Network Error: Unexpected EOF").
pub fn check_update() -> UpdateStatus {
    let agent = http_agent();
    let mut last_err = String::new();
    for attempt in 1..=4 {
        let body = match (|| -> Result<String, String> {
            let response = agent
                .get(GITHUB_API)
                .set("User-Agent", "ZenDesktop-Updater/1.0")
                .set("Accept", "application/vnd.github.v3+json")
                .call()
                .map_err(|e| format!("Network error: {e}"))?;
            response.into_string().map_err(|e| format!("Read error: {e}"))
        })() {
            Ok(b) => b,
            Err(e) => {
                last_err = e;
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(500 * attempt));
                }
                continue;
            }
        };

        let release: GitHubRelease = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(e) => return UpdateStatus::Error(format!("Parse error: {e}")),
        };

        let latest = release.tag_name.trim_start_matches('v');
        if is_up_to_date(CURRENT_VERSION, latest) {
            return UpdateStatus::UpToDate;
        }

        // Buscar el asset .exe portable y su .sig
        let asset = release.assets.iter().find(|a| a.name == "ZenDesktop.exe");
        let sig_asset = release.assets.iter().find(|a| a.name == "ZenDesktop.exe.sig");

        return match (asset, sig_asset) {
            (Some(a), Some(s)) => UpdateStatus::UpdateAvailable {
                version: latest.to_string(),
                url: a.browser_download_url.clone(),
                sig_url: s.browser_download_url.clone(),
            },
            (Some(_), None) => UpdateStatus::Error("No .sig signature found in release".into()),
            (None, _) => UpdateStatus::Error("No .exe asset found".into()),
        };
    }
    UpdateStatus::Error(format!("Network error after 4 attempts: {last_err}"))
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

/// True si la carpeta permite crear y borrar archivos (instalacion portable).
/// En Program Files devuelve false para un proceso sin elevacion, lo que
/// activa la ruta de actualizacion con UAC.
fn dir_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".zd-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Descarga `url` en `out` reintentando ante errores de red transitorios
/// (el CDN de GitHub corta conexiones de vez en cuando).
fn download_retry(url: &str, out: &Path, label: &str) -> Result<(), String> {
    let agent = http_agent();
    let mut last_err = String::new();
    for attempt in 1..=4 {
        let result = (|| -> Result<(), String> {
            let response = agent
                .get(url)
                .set("User-Agent", "ZenDesktop-Updater/1.0")
                .call()
                .map_err(|e| format!("{label} error: {e}"))?;
            let mut reader = response.into_reader();
            let mut file = std::fs::File::create(out)
                .map_err(|e| format!("Create temp: {e}"))?;
            std::io::copy(&mut reader, &mut file)
                .map_err(|e| format!("Write error: {e}"))?;
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e;
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(500 * attempt));
                }
            }
        }
    }
    Err(format!("{label} failed after 4 attempts: {last_err}"))
}

/// Descarga la firma (64 bytes) reintentando ante cortes de red.
fn download_sig_retry(url: &str) -> Result<[u8; 64], String> {
    let agent = http_agent();
    let mut last_err = String::new();
    for attempt in 1..=4 {
        let result = (|| -> Result<[u8; 64], String> {
            let response = agent
                .get(url)
                .set("User-Agent", "ZenDesktop-Updater/1.0")
                .call()
                .map_err(|e| format!("Signature download error: {e}"))?;
            let mut reader = response.into_reader();
            let mut buf = [0u8; 64];
            let n = std::io::Read::take(&mut reader, 64)
                .read(&mut buf)
                .map_err(|e| format!("Sig read error: {e}"))?;
            if n != 64 {
                return Err(format!(
                    "Invalid signature size: {n} bytes (expected 64)"
                ));
            }
            Ok(buf)
        })();
        match result {
            Ok(b) => return Ok(b),
            Err(e) => {
                last_err = e;
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(500 * attempt));
                }
            }
        }
    }
    Err(format!("Signature download failed after 4 attempts: {last_err}"))
}

/// Descarga el exe y su firma, verificando la Ed25519 con reintentos.
/// Si la descarga se trunca (corte de red) el archivo no verifica: se
/// descarta y se reintenta con backoff antes de rendirse.
fn download_and_verify(url: &str, sig_url: &str, staged: &Path) -> Result<(), String> {
    let mut last_err = String::new();
    for attempt in 1..=4 {
        let result = (|| -> Result<(), String> {
            download_retry(url, staged, "Download")?;
            let sig_bytes = download_sig_retry(sig_url)?;
            let exe_data =
                std::fs::read(staged).map_err(|e| format!("Read temp for verify: {e}"))?;
            if exe_data.len() < 100_000 {
                return Err("Downloaded file too small".into());
            }
            if !verify_signature(&exe_data, &sig_bytes) {
                return Err(
                    "Signature verification failed (download corrupt or incomplete)".into(),
                );
            }
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e;
                let _ = std::fs::remove_file(staged);
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(500 * attempt));
                }
            }
        }
    }
    Err(format!("Update download failed after 4 attempts: {last_err}"))
}

/// Downloads and installs the update, then hands over to the new version.
/// Verifies Ed25519 signature before installing.
///
/// La descarga se hace en %TEMP% (siempre escribible) y el reemplazo del
/// ejecutable depende de los permisos de la carpeta de instalacion:
///  * carpeta escribible (portable)     -> reemplazo directo, sin elevacion.
///  * Program Files / carpeta protegida -> la nueva version (ya descargada y
///    verificada) se relanza elevada (UAC) en modo `--apply-update` para
///    poder escribir sobre el ejecutable instalado.
pub fn download_and_install(url: &str, sig_url: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let app_dir = current.parent().ok_or("current_exe sin carpeta")?.to_path_buf();
    let backup = current.with_extension("exe.bak");

    // --- Fase 1: descargar y verificar en %TEMP% (escribible incluso desde
    // Program Files). El binario instalado no se toca hasta estar verificado.
    let stage_dir = std::env::temp_dir().join("ZenDesktop-update");
    std::fs::create_dir_all(&stage_dir).map_err(|e| format!("Create stage dir: {e}"))?;
    let staged = stage_dir.join("ZenDesktop.exe");

    // Descargar + verificar con reintentos: un corte de red a mitad deja un
    // archivo truncado que no verifica la firma; se descarta y se reintenta.
    download_and_verify(url, sig_url, &staged)?;

    // --- Fase 2: aplicar el reemplazo segun los permisos de la carpeta.
    if dir_writable(&app_dir) {
        // Instalacion portable: swap directo, sin elevacion.
        if backup.exists() {
            let _ = std::fs::remove_file(&backup); // .bak obsoleto de un update previo
        }
        std::fs::rename(&current, &backup)
            .map_err(|e| format!("Backup error: {e}"))?;

        if let Err(e) = std::fs::copy(&staged, &current) {
            let _ = std::fs::rename(&backup, &current);
            return Err(format!("Replace error: {e} (original restored)"));
        }
        let _ = std::fs::remove_file(&staged);

        // Lanza la nueva version en modo "relevo": esperara a que este proceso
        // cierre y suelte el mutex de instancia unica antes de tomar el control.
        // El caller debe cerrar la aplicacion inmediatamente despues.
        let _ = std::process::Command::new(&current).arg("--update-restart").spawn();
    } else {
        // Program Files / carpeta protegida: relanzar la nueva version elevada
        // (UAC) en modo helper. Ella esperara a que este proceso salga (le
        // pasamos nuestro PID) y entonces reemplazara el ejecutable instalado.
        let args = format!(
            "--apply-update \"{}\" \"{}\" {}",
            staged.to_string_lossy(),
            current.to_string_lossy(),
            std::process::id()
        );
        let file_w = crate::config::wide(&staged.to_string_lossy());
        let args_w = crate::config::wide(&args);
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("runas"),
                PCWSTR(file_w.as_ptr()),
                PCWSTR(args_w.as_ptr()),
                None,
                SW_SHOWNORMAL,
            )
        };
        let code = result.0 as isize;
        if code <= 32 {
            let detail = match code {
                1223 => "cancelado por el usuario (UAC)",
                5 => "denegado por UAC o bloqueado por el sistema",
                _ => "error de elevacion",
            };
            return Err(format!(
                "No se pudo elevar la instalacion ({detail}, codigo {code})"
            ));
        }
    }

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

/// True si la version instalada (`current`) ya tiene (o supera) la ultima
/// publicada (`latest`): no hay nada que actualizar.
fn is_up_to_date(current: &str, latest: &str) -> bool {
    version_at_least(current, latest)
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
    use super::{is_up_to_date, version_at_least};

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

    #[test]
    fn update_detection_is_not_inverted() {
        // Regresion: esta comparacion estuvo invertida y la app siempre
        // decia "al dia" aunque hubiera una version nueva publicada.
        assert!(!is_up_to_date("1.0.2", "1.0.3"), "1.0.2 instalada y 1.0.3 publicada => hay update");
        assert!(is_up_to_date("1.0.3", "1.0.3"), "misma version => al dia");
        assert!(is_up_to_date("1.0.4", "1.0.3"), "instalada superior => al dia");
        assert!(!is_up_to_date("1.0.9", "1.0.10"), "multi-digito: 1.0.9 < 1.0.10 => hay update");
    }
}
