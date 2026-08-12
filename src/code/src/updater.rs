//! ZenDesktop :: updater.rs
//!
//! Auto-actualizacion via GitHub Releases API.
//! Sin dependencias pesadas: solo ureq + serde_json.

use std::path::PathBuf;
use serde::Deserialize;

const GITHUB_API: &str = "https://api.github.com/repos/jaimitus/ZenDesktop/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    size: u64,
}

/// Resultado de la comprobacion de actualizaciones.
#[derive(Debug)]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable { version: String, url: String, size: u64 },
    Error(String),
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
    if latest == CURRENT_VERSION {
        return UpdateStatus::UpToDate;
    }

    // Buscar el asset .exe portable
    let asset = release.assets.iter().find(|a| a.name == "ZenDesktop.exe");
    match asset {
        Some(a) => UpdateStatus::UpdateAvailable {
            version: latest.to_string(),
            url: a.browser_download_url.clone(),
            size: a.size,
        },
        None => UpdateStatus::Error("No .exe asset found".into()),
    }
}

/// Descarga e instala la actualizacion.
/// Devuelve la ruta al nuevo ejecutable.
pub fn download_and_install(url: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let backup = current.with_extension("exe.bak");
    let temp = current.with_extension("exe.new");

    // Descargar
    let response = ureq::get(url)
        .set("User-Agent", "ZenDesktop-Updater/1.0")
        .call()
        .map_err(|e| format!("Download error: {e}"))?;

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&temp)
        .map_err(|e| format!("Create temp: {e}"))?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|e| format!("Write error: {e}"))?;

    // Verificar que el archivo se descargo completo
    let meta = std::fs::metadata(&temp).map_err(|e| format!("metadata: {e}"))?;
    if meta.len() < 100_000 {
        let _ = std::fs::remove_file(&temp);
        return Err("Downloaded file too small".into());
    }

    // Renombrar actual -> backup, nuevo -> actual
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    std::fs::rename(&current, &backup)
        .map_err(|e| format!("Backup error: {e}"))?;
    std::fs::rename(&temp, &current)
        .map_err(|e| format!("Replace error: {e}"))?;

    // Limpiar backup
    let _ = std::fs::remove_file(&backup);

    Ok(current)
}

/// Devuelve la version actual del binario.
pub fn current_version() -> &'static str {
    CURRENT_VERSION
}
