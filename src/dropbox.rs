//! ZenDesktop :: dropbox.rs
//!
//! Backend del widget de Dropbox (nativo, no Lua). Implementa el flujo OAuth
//! 2.0 con PKCE (sin Client Secret): se abre el navegador una vez, el codigo
//! de autorizacion llega al puerto de redireccion local y se intercambia por
//! un par de tokens. El `refresh_token` se guarda en disco y refresca solo.
//!
//! No hay dependencia de la UI: `Dropbox` expone el estado, la URL de
//! autorizacion, el listado de carpetas y la descarga/subida de archivos.
//! Los tokens se comparten con el hilo de sincronizacion via `Arc<Mutex<..>>`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";
const AUTHORIZE_URL: &str = "https://www.dropbox.com/oauth2/authorize";
const API_BASE: &str = "https://api.dropboxapi.com/2";
const CONTENT_BASE: &str = "https://content.dropboxapi.com/2";

/// Scopes minimos para listar, descargar y subir archivos + leer la cuenta.
const SCOPES: &str = "files.metadata.read files.content.read files.content.write account_info.read";

/// Token de acceso persistido en disco. `expires_at` es un timestamp Unix en
/// segundos (el access token de Dropbox dura ~4 h; el refresh, indefinido).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

/// Un archivo/carpeta listado de la cuenta Dropbox.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DropboxEntry {
    pub name: String,
    /// Ruta con prefijo `/` (p.ej. `/ZenDesktop/nota.txt`).
    pub path: String,
    pub is_dir: bool,
    /// Tamano en bytes (0 para carpetas).
    pub size: u64,
}

/// Resultado de una pasada de sincronizacion local <-> remoto.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub downloaded: Vec<String>,
    pub uploaded: Vec<String>,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Sin App Key configurado: el widget no puede conectar.
    Unconfigured,
    /// App Key presente pero sin sesion: hay que conectar con Dropbox.
    LoggedOut,
    /// Sesion activa (access token disponible, se refresca solo).
    Ready,
}

/// `Clone` comparte el token (Arc<Mutex>): el hilo de sincronizacion y la UI
/// refrescan y guardan sobre la misma sesion.
#[derive(Clone)]
pub struct Dropbox {
    app_key: String,
    /// App Secret (opcional con OAuth PKCE; se guarda por completitud).
    app_secret: String,
    /// Redirect URI exacta registrada en la App Console de Dropbox.
    redirect_uri: String,
    token: Arc<Mutex<Option<Token>>>,
    /// code_verifier del flujo PKCE en curso (para canjear el codigo al volver).
    verifier: String,
    store_path: PathBuf,
}

pub fn redirect_port(uri: &str) -> u16 {
    let rest = uri
        .strip_prefix("http://")
        .or_else(|| uri.strip_prefix("https://"))
        .unwrap_or(uri);
    let host_part = rest.split('/').next().unwrap_or("");
    host_part
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(80)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// code_verifier PKCE: 64 bytes aleatorios en base64url sin padding (86 chars).
fn generate_verifier() -> String {
    let mut bytes = [0u8; 64];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// code_challenge = base64url(sha256(code_verifier)).
fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn urlencode(s: &str) -> String {
    s.replace(' ', "%20")
}

impl Dropbox {
    pub fn new(app_key: String, store_path: PathBuf) -> Self {
        let token = Self::load_token(&store_path);
        Dropbox {
            app_key,
            app_secret: String::new(),
            redirect_uri: "http://127.0.0.1:8897/callback".into(),
            token: Arc::new(Mutex::new(token)),
            verifier: String::new(),
            store_path,
        }
    }

    pub fn set_redirect_uri(&mut self, uri: String) {
        self.redirect_uri = uri.trim().to_string();
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn set_app_key(&mut self, key: String) {
        self.app_key = key.trim().to_string();
    }

    pub fn set_app_secret(&mut self, secret: String) {
        self.app_secret = secret.trim().to_string();
    }

    pub fn status(&self) -> Status {
        if self.app_key.trim().is_empty() {
            return Status::Unconfigured;
        }
        let token = self.token.lock().unwrap().clone();
        match token {
            Some(t) if !t.access_token.is_empty() => Status::Ready,
            _ => Status::LoggedOut,
        }
    }

    /// Genera el code_verifier y devuelve la URL de autorizacion.
    pub fn authorize_url(&mut self) -> Result<String, String> {
        if self.app_key.trim().is_empty() {
            return Err("dropbox: app_key not configured".into());
        }
        self.verifier = generate_verifier();
        let challenge = code_challenge(&self.verifier);
        Ok(format!(
            "{AUTHORIZE_URL}?client_id={}&redirect_uri={}&response_type=code\
             &token_access_type=offline&scope={}\
             &force_reapprove=true\
             &code_challenge_method=S256&code_challenge={challenge}",
            self.app_key.trim(),
            urlencode(&self.redirect_uri),
            urlencode(SCOPES),
        ))
    }

    /// Intercambia el `code` devuelto por Dropbox por el par de tokens.
    pub fn complete_auth(&mut self, code: &str) -> Result<(), String> {
        let verifier = self.verifier.clone();
        let resp = ureq::post(TOKEN_URL)
            .timeout(std::time::Duration::from_secs(15))
            .send_form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("client_id", self.app_key.trim()),
                ("code_verifier", verifier.as_str()),
            ])
            .map_err(|e| format!("token request failed: {e}"))?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        self.apply_token_response(v)?;
        Ok(())
    }

    fn apply_token_response(&mut self, v: serde_json::Value) -> Result<(), String> {
        let access = v["access_token"].as_str().ok_or("no access_token")?.to_string();
        let refresh = v["refresh_token"]
            .as_str()
            .map(str::to_string)
            .ok_or("no refresh_token")?;
        let expires_in = v["expires_in"].as_u64().unwrap_or(14400);
        let token = Token {
            access_token: access,
            refresh_token: refresh,
            expires_at: now_secs() + expires_in,
        };
        *self.token.lock().unwrap() = Some(token.clone());
        self.save_token(&token);
        Ok(())
    }

    /// Devuelve un access token valido, refrescandolo si ha expirado.
    fn ensure_valid(&self) -> Result<String, String> {
        let token = self.token.lock().unwrap().clone().ok_or("no session")?;
        if !token.access_token.is_empty() && now_secs() < token.expires_at - 60 {
            return Ok(token.access_token);
        }
        if token.refresh_token.is_empty() {
            return Err("no refresh token".into());
        }
        let resp = ureq::post(TOKEN_URL)
            .timeout(std::time::Duration::from_secs(15))
            .send_form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", token.refresh_token.as_str()),
                ("client_id", self.app_key.trim()),
            ])
            .map_err(|e| format!("refresh failed: {e}"))?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let access = v["access_token"].as_str().ok_or("no access_token")?.to_string();
        let expires_in = v["expires_in"].as_u64().unwrap_or(14400);
        let fresh = Token {
            access_token: access,
            refresh_token: token.refresh_token,
            expires_at: now_secs() + expires_in,
        };
        *self.token.lock().unwrap() = Some(fresh.clone());
        self.save_token(&fresh);
        Ok(fresh.access_token)
    }

    /// Lista una carpeta remota (nivel superior, sin recursion).
    pub fn list_folder(&self, path: &str) -> Result<Vec<DropboxEntry>, String> {
        let token = self.ensure_valid()?;
        let body = serde_json::json!({
            "path": if path.trim().is_empty() { "/" } else { path },
            "recursive": false,
            "limit": 500,
        })
        .to_string();
        let resp = ureq::post(&format!("{API_BASE}/files/list_folder"))
            .timeout(std::time::Duration::from_secs(20))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("list_folder failed: {e}"))?;
        let text = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let entries = v["entries"].as_array().ok_or("no entries")?;
        let mut out = Vec::new();
        for e in entries {
            let tag = e[".tag"].as_str().unwrap_or("");
            let name = e["name"].as_str().unwrap_or("").to_string();
            let path_d = e["path_display"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            out.push(DropboxEntry {
                name,
                path: path_d,
                is_dir: tag == "folder",
                size: e["size"].as_u64().unwrap_or(0),
            });
        }
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    /// Descarga un archivo remoto a una ruta local (crea los directorios).
    pub fn download(&self, remote: &str, dest: &Path) -> Result<(), String> {
        let token = self.ensure_valid()?;
        let arg = serde_json::json!({ "path": remote }).to_string();
        let resp = ureq::post(&format!("{CONTENT_BASE}/files/download"))
            .timeout(std::time::Duration::from_secs(120))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Dropbox-API-Arg", &arg)
            .call()
            .map_err(|e| format!("download failed: {e}"))?;
        let bytes = resp.into_reader();
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut std::io::BufReader::new(bytes), &mut file)
            .map_err(|e| format!("write failed: {e}"))?;
        Ok(())
    }

    /// Sube un archivo local a una ruta remota (sobreescribe si existe).
    pub fn upload(&self, src: &Path, remote: &str) -> Result<(), String> {
        let token = self.ensure_valid()?;
        let arg = serde_json::json!({ "path": remote, "mode": "overwrite" }).to_string();
        let bytes = std::fs::read(src).map_err(|e| e.to_string())?;
        ureq::post(&format!("{CONTENT_BASE}/files/upload"))
            .timeout(std::time::Duration::from_secs(120))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Dropbox-API-Arg", &arg)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(&bytes)
            .map_err(|e| format!("upload failed: {e}"))?;
        Ok(())
    }

    /// Sube varios archivos locales a la carpeta remota `remote_dir`
    /// (sobreescribiendo si existen). Devuelve (subidos, total).
    pub fn upload_files(&self, paths: &[std::path::PathBuf], remote_dir: &str) -> (usize, usize) {
        let total = paths.len();
        let mut ok = 0usize;
        for src in paths {
            let Some(name) = src.file_name() else { continue };
            let name = name.to_string_lossy().to_string();
            if name.is_empty() {
                continue;
            }
            let base = remote_dir.trim_end_matches('/');
            let remote = if base.is_empty() {
                format!("/{name}")
            } else {
                format!("{base}/{name}")
            };
            if self.upload(src, &remote).is_ok() {
                ok += 1;
            }
        }
        (ok, total)
    }

    /// Datos de la cuenta (email) para mostrar en el estado.
    pub fn account_email(&self) -> Option<String> {
        let token = self.ensure_valid().ok()?;
        let resp = ureq::post(&format!("{API_BASE}/users/get_current_account"))
            .timeout(std::time::Duration::from_secs(15))
            .set("Authorization", &format!("Bearer {token}"))
            .call()
            .ok()?;
        let text = resp.into_string().ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v["email"].as_str().map(str::to_string)
    }

    /// Sincroniza la carpeta local con una carpeta remota (un solo nivel):
    /// descarga los archivos remotos que faltan o difieren, sube los locales
    /// que no existen en remoto. No borra nada.
    pub fn sync_folder(&self, local: &Path, remote: &str) -> SyncReport {
        let mut report = SyncReport::default();
        if !local.is_dir() {
            report.errors.push(format!("carpeta local no existe: {}", local.display()));
            return report;
        }
        let remote_entries = match self.list_folder(remote) {
            Ok(e) => e,
            Err(e) => {
                report.errors.push(e);
                return report;
            }
        };
        // 1) Descargar remotos que faltan o difieren en tamano.
        for entry in &remote_entries {
            if entry.is_dir {
                continue;
            }
            let dest = local.join(&entry.name);
            let need = match std::fs::metadata(&dest) {
                Ok(m) => m.len() != entry.size,
                Err(_) => true,
            };
            if need {
                if let Err(e) = self.download(&entry.path, &dest) {
                    report.errors.push(format!("{}: {e}", entry.name));
                } else {
                    report.downloaded.push(entry.name.clone());
                }
            } else {
                report.skipped += 1;
            }
        }
        // 2) Subir locales que no estan en remoto (solo archivos, un nivel).
        let remote_names: std::collections::HashSet<String> = remote_entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
        let Ok(entries) = std::fs::read_dir(local) else {
            return report;
        };
        for item in entries.flatten() {
            let Ok(ft) = item.file_type() else { continue };
            if !ft.is_file() {
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if remote_names.contains(&name) {
                continue;
            }
            let remote_path = format!("{}/{name}", remote.trim_end_matches('/'));
            if let Err(e) = self.upload(&item.path(), &remote_path) {
                report.errors.push(format!("{name}: {e}"));
            } else {
                report.uploaded.push(name);
            }
        }
        report
    }

    pub fn sign_out(&mut self) {
        *self.token.lock().unwrap() = None;
        let _ = std::fs::remove_file(&self.store_path);
    }

    fn load_token(path: &PathBuf) -> Option<Token> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn save_token(&self, token: &Token) {
        if let Some(dir) = self.store_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(body) = serde_json::to_string(token) {
            let _ = std::fs::write(&self.store_path, body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_port_parses_port_from_uri() {
        assert_eq!(redirect_port("http://127.0.0.1:8897/callback"), 8897);
        assert_eq!(redirect_port("http://localhost:9000/x"), 9000);
        assert_eq!(redirect_port("https://example.com/cb"), 80);
    }

    #[test]
    fn verifier_is_base64url_without_padding() {
        let v = generate_verifier();
        assert!((43..=128).contains(&v.len()));
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn challenge_is_sha256_base64url() {
        let v = generate_verifier();
        let c = code_challenge(&v);
        assert_eq!(c.len(), 43);
        assert!(c.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn status_depends_on_app_key_and_token() {
        let mut d = Dropbox::new(String::new(), PathBuf::from("/tmp/none.json"));
        assert_eq!(d.status(), Status::Unconfigured);
        d.set_app_key("abc".into());
        assert_eq!(d.status(), Status::LoggedOut);
    }

    #[test]
    fn authorize_url_requires_app_key() {
        let mut d = Dropbox::new(String::new(), PathBuf::from("/tmp/none.json"));
        assert!(d.authorize_url().is_err());
    }

    #[test]
    fn sync_missing_local_folder_reports_error() {
        let d = Dropbox::new("key".into(), PathBuf::from("/tmp/none.json"));
        let report = d.sync_folder(Path::new("/ruta/que/no/existe/xyz"), "/");
        assert_eq!(report.errors.len(), 1);
    }
}
