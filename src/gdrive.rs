//! ZenDesktop :: gdrive.rs
//!
//! Backend del widget de Google Drive (nativo, no Lua). Implementa el flujo OAuth
//! 2.0 con PKCE: se abre el navegador una vez, el codigo
//! de autorizacion llega al puerto de redireccion local y se intercambia por
//! un par de tokens. El `refresh_token` se guarda en disco y refresca solo.
//!
//! No hay dependencia de la UI: `GDrive` expone el estado, la URL de
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

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const API_BASE: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3";

/// Scopes para listar, descargar y subir archivos + leer email del usuario.
const SCOPES: &str = "https://www.googleapis.com/auth/drive https://www.googleapis.com/auth/userinfo.email";

/// Token de acceso persistido en disco. `expires_at` es un timestamp Unix en segundos.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

/// Un archivo/carpeta listado de la cuenta de Google Drive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GDriveEntry {
    pub id: String,
    pub name: String,
    pub is_dir: bool,
    /// Tamaño en bytes (0 para carpetas).
    pub size: u64,
    pub mime_type: String,
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
    /// Sin Client ID configurado: el widget no puede conectar.
    Unconfigured,
    /// Client ID presente pero sin sesion: hay que conectar con Google Drive.
    LoggedOut,
    /// Sesion activa (access token disponible, se refresca solo).
    Ready,
}

/// `Clone` comparte el token (Arc<Mutex>): el hilo de sincronizacion y la UI
/// refrescan y guardan sobre la misma sesion.
#[derive(Clone)]
pub struct GDrive {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    token: Arc<Mutex<Option<Token>>>,
    verifier: Arc<Mutex<String>>,
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
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", b);
            }
        }
    }
    out
}

impl GDrive {
    pub fn new(client_id: String, store_path: PathBuf) -> Self {
        let token = Self::load_token(&store_path);
        GDrive {
            client_id,
            client_secret: String::new(),
            redirect_uri: "http://127.0.0.1:8898/callback".into(),
            token: Arc::new(Mutex::new(token)),
            verifier: Arc::new(Mutex::new(String::new())),
            store_path,
        }
    }

    pub fn set_redirect_uri(&mut self, uri: String) {
        let uri = uri.trim().to_string();
        if !uri.is_empty() {
            self.redirect_uri = uri;
        }
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn set_client_id(&mut self, id: String) {
        self.client_id = id.trim().to_string();
    }

    pub fn set_client_secret(&mut self, secret: String) {
        self.client_secret = secret.trim().to_string();
    }

    pub fn status(&self) -> Status {
        if self.client_id.trim().is_empty() {
            return Status::Unconfigured;
        }
        let token = self.token.lock().unwrap().clone();
        match token {
            Some(t) if !t.access_token.is_empty() => Status::Ready,
            _ => Status::LoggedOut,
        }
    }

    /// Devuelve la URL de autorizacion que el host debe abrir en el navegador.
    pub fn authorize_url(&mut self) -> Result<String, String> {
        if self.client_id.trim().is_empty() {
            return Err("gdrive: client_id not configured".into());
        }
        let verifier_str = generate_verifier();
        *self.verifier.lock().unwrap() = verifier_str.clone();
        let challenge = code_challenge(&verifier_str);
        Ok(format!(
            "{AUTHORIZE_URL}?client_id={}&response_type=code&redirect_uri={}\
             &scope={}&code_challenge_method=S256&code_challenge={challenge}\
             &access_type=offline&prompt=consent",
            self.client_id.trim(),
            urlencode(&self.redirect_uri),
            urlencode(SCOPES),
        ))
    }

    /// Intercambia el `code` devuelto por Google por el par de tokens y lo
    /// persiste en disco.
    pub fn complete_auth(&mut self, code: &str) -> Result<(), String> {
        let verifier = self.verifier.lock().unwrap().clone();
        let mut form_data: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("client_id", self.client_id.trim()),
            ("code_verifier", verifier.as_str()),
        ];
        if !self.client_secret.trim().is_empty() {
            form_data.push(("client_secret", self.client_secret.trim()));
        }

        let resp = match ureq::post(TOKEN_URL)
            .timeout(std::time::Duration::from_secs(15))
            .send_form(&form_data) {
                Ok(r) => r,
                Err(ureq::Error::Status(code, resp)) => {
                    let err_body = resp.into_string().unwrap_or_default();
                    return Err(format!("HTTP {code}: {err_body}"));
                }
                Err(e) => return Err(format!("token request failed: {e}")),
            };

        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if let Some(err) = v.get("error") {
            let desc = v.get("error_description").and_then(|d| d.as_str()).unwrap_or("");
            return Err(format!("{err}: {desc}"));
        }
        self.apply_token_response(v)?;
        Ok(())
    }

    fn apply_token_response(&mut self, v: serde_json::Value) -> Result<(), String> {
        let access = v["access_token"].as_str().ok_or("no access_token")?.to_string();
        let refresh = v["refresh_token"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                // Si el refresh_token no vino en esta llamada, conservar el anterior
                self.token.lock().unwrap().as_ref().map(|t| t.refresh_token.clone())
            })
            .unwrap_or_default();
        let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
        let token = Token {
            access_token: access,
            refresh_token: refresh,
            expires_at: now_secs() + expires_in,
        };
        self.save_token(&token);
        *self.token.lock().unwrap() = Some(token);
        Ok(())
    }

    /// Refresca el access token si esta caducado (o a punto de caducar).
    fn ensure_valid(&self) -> Result<String, String> {
        let mut guard = self.token.lock().unwrap();
        let Some(t) = guard.as_ref() else {
            return Err("not logged in".into());
        };
        if t.expires_at > now_secs() + 60 {
            return Ok(t.access_token.clone());
        }

        let mut form_data: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", t.refresh_token.as_str()),
            ("client_id", self.client_id.trim()),
        ];
        if !self.client_secret.trim().is_empty() {
            form_data.push(("client_secret", self.client_secret.trim()));
        }

        let resp = ureq::post(TOKEN_URL)
            .timeout(std::time::Duration::from_secs(15))
            .send_form(&form_data)
            .map_err(|e| format!("refresh failed: {e}"))?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let access = v["access_token"].as_str().ok_or("no access_token")?.to_string();
        let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
        let refresh = v["refresh_token"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| t.refresh_token.clone());
        let nt = Token {
            access_token: access,
            refresh_token: refresh,
            expires_at: now_secs() + expires_in,
        };
        self.save_token(&nt);
        let out = nt.access_token.clone();
        *guard = Some(nt);
        Ok(out)
    }

    /// Lista los archivos de una carpeta (ID `folder_id`, o `'root'`).
    pub fn list_folder(&self, folder_id: &str) -> Result<Vec<GDriveEntry>, String> {
        let access = self.ensure_valid()?;
        let fid = if folder_id.trim().is_empty() || folder_id == "/" {
            "root"
        } else {
            folder_id.trim()
        };

        let q = format!("'{}' in parents and trashed = false", fid);
        let url = format!(
            "{API_BASE}/files?q={}&fields=files(id,name,mimeType,size,modifiedTime)&orderBy=folder,name&pageSize=100",
            urlencode(&q)
        );

        let resp = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
            .map_err(|e| format!("list_folder failed: {e}"))?;

        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;

        let files = v["files"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|f| {
                        let mime = f["mimeType"].as_str().unwrap_or("").to_string();
                        let is_dir = mime == "application/vnd.google-apps.folder";
                        let size = f["size"]
                            .as_str()
                            .and_then(|s| s.parse::<u64>().ok())
                            .or_else(|| f["size"].as_u64())
                            .unwrap_or(0);
                        GDriveEntry {
                            id: f["id"].as_str().unwrap_or("").to_string(),
                            name: f["name"].as_str().unwrap_or("").to_string(),
                            is_dir,
                            size,
                            mime_type: mime,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(files)
    }

    /// Descarga un archivo por su ID de Google Drive hacia una ruta local `dest`.
    pub fn download(&self, file_id: &str, dest: &Path) -> Result<(), String> {
        let access = self.ensure_valid()?;
        if let Some(p) = dest.parent() {
            let _ = std::fs::create_dir_all(p);
        }

        let url = format!("{API_BASE}/files/{}?alt=media", file_id);
        let resp = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(60))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
            .map_err(|e| format!("download failed: {e}"))?;

        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Sube un archivo local a una carpeta de Google Drive (`parent_id`, por defecto `'root'`).
    pub fn upload(&self, src: &Path, parent_id: &str) -> Result<String, String> {
        let access = self.ensure_valid()?;
        let file_name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid file name")?;
        let bytes = std::fs::read(src).map_err(|e| e.to_string())?;

        let pid = if parent_id.trim().is_empty() || parent_id == "/" {
            "root"
        } else {
            parent_id.trim()
        };

        let boundary = "-------ZenDesktopUploadBoundary";
        let metadata = serde_json::json!({
            "name": file_name,
            "parents": [pid]
        })
        .to_string();

        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n").as_bytes());
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let url = format!("{UPLOAD_BASE}/files?uploadType=multipart");
        let resp = ureq::post(&url)
            .timeout(std::time::Duration::from_secs(60))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Type", &format!("multipart/related; boundary={boundary}"))
            .send_bytes(&body)
            .map_err(|e| format!("upload failed: {e}"))?;

        let resp_body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&resp_body).map_err(|e| e.to_string())?;
        let id = v["id"].as_str().unwrap_or("").to_string();
        Ok(id)
    }

    /// Sincroniza la carpeta local con la remota: descarga los nuevos/modificados
    /// y sube los locales que no esten en remoto.
    pub fn sync_folder(&self, local_root: &Path, remote_folder_id: &str) -> SyncReport {
        let mut report = SyncReport::default();
        if !local_root.exists() {
            if let Err(e) = std::fs::create_dir_all(local_root) {
                report.errors.push(format!("no se pudo crear {}: {e}", local_root.display()));
                return report;
            }
        }

        let remote_files = match self.list_folder(remote_folder_id) {
            Ok(f) => f,
            Err(e) => {
                report.errors.push(e);
                return report;
            }
        };

        // 1. Descarga remotos que falten en local
        for entry in &remote_files {
            if entry.is_dir {
                continue;
            }
            let local_path = local_root.join(&entry.name);
            if !local_path.exists() {
                match self.download(&entry.id, &local_path) {
                    Ok(_) => report.downloaded.push(entry.name.clone()),
                    Err(e) => report.errors.push(format!("{}: {e}", entry.name)),
                }
            } else {
                report.skipped += 1;
            }
        }

        // 2. Sube locales que no existan en remoto
        if let Ok(entries) = std::fs::read_dir(local_root) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !remote_files.iter().any(|r| r.name.eq_ignore_ascii_case(name)) {
                        match self.upload(&path, remote_folder_id) {
                            Ok(_) => report.uploaded.push(name.to_string()),
                            Err(err) => report.errors.push(format!("{name}: {err}")),
                        }
                    }
                }
            }
        }

        report
    }

    /// Email de la cuenta vinculada (para mostrarlo en la UI de configuracion).
    pub fn account_email(&self) -> Option<String> {
        let access = self.ensure_valid().ok()?;
        let resp = ureq::get(&format!("{API_BASE}/about?fields=user(emailAddress,displayName)"))
            .timeout(std::time::Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
            .ok()?;
        let body = resp.into_string().ok()?;
        let v: serde_json::Value = serde_json::from_str(&body).ok()?;
        let email = v["user"]["emailAddress"].as_str().map(str::to_string);
        let name = v["user"]["displayName"].as_str();
        match (name, email) {
            (Some(n), Some(e)) if !n.is_empty() => Some(format!("{n} ({e})")),
            (_, Some(e)) => Some(e),
            _ => None,
        }
    }

    /// Olvida la sesion (borra el token de disco y de memoria).
    pub fn sign_out(&mut self) {
        *self.token.lock().unwrap() = None;
        let _ = std::fs::remove_file(&self.store_path);
    }

    fn load_token(path: &PathBuf) -> Option<Token> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_token(&self, token: &Token) {
        if let Ok(bytes) = serde_json::to_vec(token) {
            let _ = std::fs::write(&self.store_path, bytes);
        }
    }
}
