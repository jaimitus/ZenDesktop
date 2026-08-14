//! ZenDesktop :: spotify.rs
//!
//! Backend del widget de Spotify (nativo, no Lua). Implementa el flujo OAuth
//! 2.0 con PKCE (sin Client Secret): se abre el navegador una vez, el codigo
//! de autorizacion llega al puerto de redireccion local y se intercambia por
//! un par de tokens. El `refresh_token` se guarda en disco y refresca solo.
//!
//! No hay dependencia de la UI: `Spotify` expone el estado, la URL de
//! autorizacion, la consulta de "now playing" y los controles de reproduccion.
//! Los tokens se comparten con el poller en segundo plano via `Arc<Mutex<..>>`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const API_BASE: &str = "https://api.spotify.com/v1";

/// Token de acceso persistido en disco. `expires_at` es un timestamp Unix en
/// milisegundos (el access token dura ~1 h; el refresh token, indefinidamente).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: String,
    pub progress_ms: u32,
    pub duration_ms: u32,
    pub is_playing: bool,
    pub volume_percent: u8,
    pub device_name: String,
}

/// Una pista de la cola de reproduccion.
#[derive(Debug, Clone, Default)]
pub struct QueueItem {
    pub uri: String,
    pub title: String,
    pub artist: String,
}

/// Cola de reproduccion: contexto actual + siguientes pistas.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    pub context_uri: String,
    pub next: Vec<QueueItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Sin Client ID configurado: el widget no puede conectar.
    Unconfigured,
    /// Client ID presente pero sin sesion: hay que conectar con Spotify.
    LoggedOut,
    /// Sesion activa (access token disponible, se refresca solo).
    Ready,
}

/// `Clone` comparte el token (Arc<Mutex>): el poller y el hilo de interfaz
/// refrescan y guardan sobre la misma sesion.
/// Puerto de la URL de redireccion (para el listener local que captura el
/// codigo de autorizacion). `http://host:puerto/path` -> puerto.
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

#[derive(Clone)]
pub struct Spotify {
    client_id: String,
    /// Client Secret (el flujo actual es PKCE y no lo usa, pero se conserva
    /// en el cliente por si el flujo cambia a authorization-code con secreto).
    client_secret: String,
    /// Redirect URI exacta registrada en el dashboard de Spotify.
    redirect_uri: String,
    token: Arc<Mutex<Option<Token>>>,
    /// code_verifier del flujo PKCE en curso (para canjear el codigo al volver).
    verifier: Arc<Mutex<String>>,
    store_path: PathBuf,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// true si el mensaje de error proviene de un 401/403 de la API de Spotify
/// (el token fue revocado o no tiene permiso). Se usa para forzar el refresh.
fn is_unauthorized_msg(msg: &str) -> bool {
    msg.starts_with("401:") || msg.starts_with("403:")
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

impl Spotify {
    pub fn new(client_id: String, store_path: PathBuf) -> Self {
        let token = Self::load_token(&store_path);
        Spotify {
            client_id,
            client_secret: String::new(),
            redirect_uri: "http://127.0.0.1:8899/callback".into(),
            token: Arc::new(Mutex::new(token)),
            verifier: Arc::new(Mutex::new(String::new())),
            store_path,
        }
    }

    pub fn set_client_secret(&mut self, secret: String) {
        self.client_secret = secret.trim().to_string();
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

    pub fn status(&self) -> Status {
        if self.client_id.trim().is_empty() {
            return Status::Unconfigured;
        }
        match self.token.lock().unwrap().as_ref() {
            Some(_) => Status::Ready,
            None => Status::LoggedOut,
        }
    }

    pub fn set_client_id(&mut self, id: String) {
        self.client_id = id.trim().to_string();
    }

    /// Genera el code_verifier y devuelve la URL de autorizacion que el host
    /// debe abrir en el navegador.
    pub fn authorize_url(&mut self) -> Result<String, String> {
        if self.client_id.trim().is_empty() {
            return Err("spotify: client_id not configured".into());
        }
        let verifier_str = generate_verifier();
        *self.verifier.lock().unwrap() = verifier_str.clone();
        let challenge = code_challenge(&verifier_str);
        let scope = "user-read-playback-state user-modify-playback-state user-read-currently-playing";
        Ok(format!(
            "{AUTHORIZE_URL}?client_id={}&response_type=code&redirect_uri={}\
             &scope={}&code_challenge_method=S256&code_challenge={challenge}",
            self.client_id.trim(),
            urlencode(&self.redirect_uri),
            urlencode(scope),
        ))
    }

    /// Intercambia el `code` devuelto por Spotify por el par de tokens y lo
    /// persiste en disco.
    pub fn complete_auth(&mut self, code: &str) -> Result<(), String> {
        let verifier = self.verifier.lock().unwrap().clone();
        let mut form_data = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("client_id", self.client_id.trim()),
            ("code_verifier", verifier.as_str()),
        ];
        if !self.client_secret.is_empty() {
            form_data.push(("client_secret", self.client_secret.as_str()));
        }
        let resp = match ureq::post(TOKEN_URL)
            .timeout(Duration::from_secs(15))
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

    /// Extrae access/refresh/expires de la respuesta de /api/token y guarda.
    fn apply_token_response(&mut self, v: serde_json::Value) -> Result<(), String> {
        let access = v["access_token"].as_str().ok_or("no access_token")?.to_string();
        let refresh = v["refresh_token"]
            .as_str()
            .map(str::to_string)
            .ok_or("no refresh_token")?;
        let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
        let token = Token {
            access_token: access,
            refresh_token: refresh,
            expires_at: now_ms() + expires_in * 1000,
        };
        self.save_token(&token);
        *self.token.lock().unwrap() = Some(token);
        Ok(())
    }

    /// Intercambia un `refresh_token` por un access token nuevo. No toca el
    /// estado del cliente: solo hace la llamada HTTP y devuelve el `Token`.
    fn refresh_access_token(&self, refresh_token: &str) -> Result<Token, String> {
        let mut form_data = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.client_id.trim()),
        ];
        if !self.client_secret.is_empty() {
            form_data.push(("client_secret", self.client_secret.as_str()));
        }
        let resp = ureq::post(TOKEN_URL)
            .timeout(Duration::from_secs(15))
            .send_form(&form_data)
            .map_err(|e| format!("refresh failed: {e}"))?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let access = v["access_token"].as_str().ok_or("no access_token")?.to_string();
        let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
        let refresh = v["refresh_token"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| refresh_token.to_string());
        Ok(Token {
            access_token: access,
            refresh_token: refresh,
            expires_at: now_ms() + expires_in * 1000,
        })
    }

    /// Refresca el access token si esta caducado (o a punto de caducar).
    fn ensure_valid(&self) -> Result<String, String> {
        let mut guard = self.token.lock().unwrap();
        let Some(t) = guard.as_ref() else {
            return Err("not logged in".into());
        };
        if t.expires_at > now_ms() + 60_000 {
            return Ok(t.access_token.clone());
        }
        let nt = self.refresh_access_token(&t.refresh_token)?;
        self.save_token(&nt);
        let out = nt.access_token.clone();
        *guard = Some(nt);
        Ok(out)
    }

    /// Renueva el access token ignorando `expires_at`. Se usa cuando la API
    /// responde 401 con un token aparentemente valido (p. ej. Spotify lo
    /// revoco al regenerar el Client Secret en el dashboard).
    pub fn force_refresh(&self) -> Result<String, String> {
        let mut guard = self.token.lock().unwrap();
        let Some(t) = guard.as_ref() else {
            return Err("not logged in".into());
        };
        let nt = self.refresh_access_token(&t.refresh_token)?;
        self.save_token(&nt);
        let out = nt.access_token.clone();
        *guard = Some(nt);
        Ok(out)
    }

    /// Pista en reproduccion ahora mismo (None si no hay nada sonando).
    pub fn now_playing(&self) -> Result<Option<NowPlaying>, String> {
        let access = self.ensure_valid()?;
        match self.player_state(&access) {
            // 401/403 => el token fue revocado antes de caducar: se renueva
            // forzosamente y se reintenta una sola vez.
            Err(e) if is_unauthorized_msg(&e) => {
                let access = self.force_refresh()?;
                self.player_state(&access)
            }
            other => other,
        }
    }

    /// GET /me/player -> NowPlaying. Si no hay dispositivo/pista activa (204,
    /// cuerpo vacio o `item: null`) cae a /me/player/currently-playing, que es
    /// mas permisivo. Un 401/403 se propaga con prefijo para poder forzar el
    /// refresh y reintentar desde `now_playing`.
    fn player_state(&self, access: &str) -> Result<Option<NowPlaying>, String> {
        match ureq::get(&format!("{API_BASE}/me/player"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
        {
            Ok(r) => {
                if r.status() == 204 {
                    return self.currently_playing(access);
                }
                let body = r.into_string().map_err(|e| e.to_string())?;
                if body.trim().is_empty() {
                    return self.currently_playing(access);
                }
                let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                if v.get("item").is_none() || v["item"].is_null() {
                    return self.currently_playing(access);
                }
                Ok(Some(parse_now_playing(&v)))
            }
            Err(ureq::Error::Status(code, resp)) if code == 401 || code == 403 => {
                let body = resp.into_string().unwrap_or_default();
                Err(format!("{code}: {body}"))
            }
            Err(_) => self.currently_playing(access),
        }
    }

    /// Pista actual via /me/player/currently-playing (sin informacion de
    /// dispositivo/volumen; se consultan aparte con `active_device`).
    fn currently_playing(&self, access: &str) -> Result<Option<NowPlaying>, String> {
        let resp = ureq::get(&format!("{API_BASE}/me/player/currently-playing"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
            .map_err(|e| format!("now playing failed: {e}"))?;
        if resp.status() == 204 {
            return Ok(None);
        }
        let body = resp.into_string().map_err(|e| e.to_string())?;
        if body.trim().is_empty() {
            return Ok(None);
        }
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if v.get("item").is_none() || v["item"].is_null() {
            return Ok(None);
        }
        Ok(Some(parse_now_playing(&v)))
    }

    /// Dispositivo de reproduccion activo: (volumen 0..100, nombre).
    /// `currently-playing` no incluye el device, asi que se consulta aparte.
    pub fn active_device(&self) -> Option<(u8, String)> {
        let access = self.ensure_valid().ok()?;
        let resp = ureq::get(&format!("{API_BASE}/me/player/devices"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
            .ok()?;
        let body = resp.into_string().ok()?;
        let v: serde_json::Value = serde_json::from_str(&body).ok()?;
        let devices = v["devices"].as_array()?;
        for dev in devices {
            if dev["is_active"].as_bool().unwrap_or(false) {
                let vol = dev["volume_percent"].as_u64().unwrap_or(50).min(100) as u8;
                let name = dev["name"].as_str().unwrap_or("").to_string();
                return Some((vol, name));
            }
        }
        None
    }

    /// Pausa o reanuda la reproduccion segun el estado conocido del widget.
    pub fn set_playing(&self, playing: bool) -> Result<(), String> {
        let access = self.ensure_valid()?;
        let endpoint = if playing { "play" } else { "pause" };
        ureq::put(&format!("{API_BASE}/me/player/{endpoint}"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Length", "0")
            .call()
            .map_err(|e| format!("{endpoint} failed: {e}"))?;
        Ok(())
    }

    pub fn next(&self) -> Result<(), String> {
        let access = self.ensure_valid()?;
        ureq::post(&format!("{API_BASE}/me/player/next"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Length", "0")
            .call()
            .map_err(|e| format!("next failed: {e}"))?;
        Ok(())
    }

    pub fn previous(&self) -> Result<(), String> {
        let access = self.ensure_valid()?;
        ureq::post(&format!("{API_BASE}/me/player/previous"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Length", "0")
            .call()
            .map_err(|e| format!("previous failed: {e}"))?;
        Ok(())
    }

    /// Siguientes pistas en la cola (hasta ~20 que devuelve Spotify).
    pub fn queue(&self) -> Result<Queue, String> {
        let access = self.ensure_valid()?;
        let resp = ureq::get(&format!("{API_BASE}/me/player/queue"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .call()
            .map_err(|e| format!("queue failed: {e}"))?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let context_uri = v["currently_playing"]["context"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let next = v["queue"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|t| QueueItem {
                        uri: t["uri"].as_str().unwrap_or("").to_string(),
                        title: t["name"].as_str().unwrap_or("").to_string(),
                        artist: t["artists"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x["name"].as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Queue { context_uri, next })
    }

    /// Salta a una pista concreta: si hay contexto, usa `context_uri` +
    /// `offset.uri` (mantiene la cola); si no, la reproduce suelta.
    pub fn play_track(&self, context_uri: &str, uri: &str) -> Result<(), String> {
        let access = self.ensure_valid()?;
        let body = if context_uri.is_empty() {
            format!("{{\"uris\":[\"{uri}\"]}}")
        } else {
            format!("{{\"context_uri\":\"{context_uri}\",\"offset\":{{\"uri\":\"{uri}\"}}}}")
        };
        ureq::put(&format!("{API_BASE}/me/player/play"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("play failed: {e}"))?;
        Ok(())
    }

    /// Ajusta el volumen del dispositivo activo (0..100).
    pub fn set_volume(&self, percent: u8) -> Result<(), String> {
        let access = self.ensure_valid()?;
        ureq::put(&format!("{API_BASE}/me/player/volume?volume_percent={}", percent.min(100)))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Length", "0")
            .call()
            .map_err(|e| format!("volume failed: {e}"))?;
        Ok(())
    }

    /// Salta a una posicion en milisegundos de la pista actual.
    pub fn seek(&self, position_ms: u32) -> Result<(), String> {
        let access = self.ensure_valid()?;
        ureq::put(&format!("{API_BASE}/me/player/seek?position_ms={position_ms}"))
            .timeout(Duration::from_secs(8))
            .set("Authorization", &format!("Bearer {access}"))
            .set("Content-Length", "0")
            .call()
            .map_err(|e| format!("seek failed: {e}"))?;
        Ok(())
    }

    /// Olvida la sesion (borra el token de disco y de memoria).
    pub fn sign_out(&mut self) {
        *self.token.lock().unwrap() = None;
        let _ = std::fs::remove_file(&self.store_path);
    }

    // -- persistencia -------------------------------------------------------

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

fn parse_now_playing(v: &serde_json::Value) -> NowPlaying {
    let item = &v["item"];
    let title = item["name"].as_str().unwrap_or("").to_string();
    let artist = item["artists"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let album = item["album"]["name"].as_str().unwrap_or("").to_string();
    let cover_url = item["album"]["images"]
        .as_array()
        .and_then(|imgs| imgs.iter().find_map(|i| i["url"].as_str().map(str::to_string)))
        .unwrap_or_default();
    NowPlaying {
        title,
        artist,
        album,
        cover_url,
        progress_ms: v["progress_ms"].as_u64().unwrap_or(0) as u32,
        duration_ms: item["duration_ms"].as_u64().unwrap_or(0) as u32,
        is_playing: v["is_playing"].as_bool().unwrap_or(false),
        volume_percent: v["device"]["volume_percent"].as_u64().unwrap_or(50).min(100) as u8,
        device_name: v["device"]["name"].as_str().unwrap_or("").to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_base64url_without_padding() {
        let v = generate_verifier();
        assert!((43..=128).contains(&v.len()));
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn challenge_is_sha256_base64url() {
        // Valor de referencia de la RFC 7636 (apendice B).
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(code_challenge(verifier), expected);
    }

    #[test]
    fn parses_now_playing_json() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "is_playing": true,
                "progress_ms": 5000,
                "item": {
                    "name": "Song",
                    "duration_ms": 200000,
                    "artists": [{"name": "Artist A"}, {"name": "Artist B"}],
                    "album": {
                        "name": "Album",
                        "images": [{"url": "https://i.scdn.co/c.jpg", "width": 300}]
                    }
                }
            }"#,
        )
        .unwrap();
        let np = parse_now_playing(&v);
        assert_eq!(np.title, "Song");
        assert_eq!(np.artist, "Artist A, Artist B");
        assert_eq!(np.album, "Album");
        assert_eq!(np.cover_url, "https://i.scdn.co/c.jpg");
        assert_eq!(np.progress_ms, 5000);
        assert_eq!(np.duration_ms, 200000);
        assert!(np.is_playing);
    }

    #[test]
    fn status_depends_on_client_id_and_token() {
        let dir = std::env::temp_dir().join(format!("zendesktop_spotify_test_{}", now_ms()));
        let mut s = Spotify::new(String::new(), dir.clone());
        assert_eq!(s.status(), Status::Unconfigured);
        s.set_client_id("abc".into());
        assert_eq!(s.status(), Status::LoggedOut);
        let _ = std::fs::remove_file(&dir);
    }
}
