//! ZenDesktop :: config.rs
//!
//! Configuracion 100% portable en TOML. El archivo se busca (en orden):
//!   1. Junto al ejecutable  -> <dir_exe>\config.toml      (modo pendrive / portable)
//!   2. %APPDATA%\ZenDesktop\config.toml                   (fallback si 1 es de solo lectura)
//!
//! Si no existe se genera automaticamente con valores por defecto y cabecera
//! documentada. Toda la escritura es atomica (fichero temporal + rename) para
//! que un corte de energia jamas deje un config.toml corrupto.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use windows::core::{PCWSTR, GUID};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    SHGetKnownFolderPath, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_PublicDesktop,
    KF_FLAG_DEFAULT,
};

pub const CONFIG_FILE: &str = "config.toml";
pub const APP_NAME: &str = "ZenDesktop";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

const HEADER: &str = "\
# ============================================================================
#  ZenDesktop - configuracion portable
#  Editable en caliente: guarda el archivo y usa \"Recargar configuracion\"
#  en el menu contextual (o el icono de la bandeja) para aplicar los cambios.
#  Colores en formato #RRGGBB o #RRGGBBAA.
# ============================================================================

";

// ---------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(toml::de::Error),
    Encode(toml::ser::Error),
    Shell(windows::core::Error),
    NoWritableLocation,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "error de E/S: {e}"),
            ConfigError::Parse(e) => write!(f, "config.toml invalido: {e}"),
            ConfigError::Encode(e) => write!(f, "no se pudo serializar la configuracion: {e}"),
            ConfigError::Shell(e) => write!(f, "error de la API de Windows: {e}"),
            ConfigError::NoWritableLocation => {
                write!(f, "no se encontro ninguna ubicacion escribible para config.toml")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self {
        ConfigError::Io(e)
    }
}
impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Parse(e)
    }
}
impl From<toml::ser::Error> for ConfigError {
    fn from(e: toml::ser::Error) -> Self {
        ConfigError::Encode(e)
    }
}
impl From<windows::core::Error> for ConfigError {
    fn from(e: windows::core::Error) -> Self {
        ConfigError::Shell(e)
    }
}

// ---------------------------------------------------------------------------
// Estructuras serializables
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub appearance: Appearance,
    pub ephemeral: Ephemeral,
    pub ai: AiConfig,
    /// Reglas de clasificacion, evaluadas en orden (la primera que casa gana).
    pub rules: Vec<Rule>,
    /// Geometria persistida de cada caja (se reescribe al arrastrar/redimensionar).
    pub fences: Vec<FenceLayout>,
    /// Plantillas de layout: snapshots con nombre de la geometria de todas las
    /// cajas, para guardar y reaplicar perfiles (trabajo/ocio, etc.).
    #[serde(default)]
    pub templates: Vec<LayoutTemplate>,
    /// Firma de monitores de la ultima sesion: al arrancar se compara con la
    /// disposicion actual para saber si la pantalla cambio (y solo entonces
    /// aplicar la plantilla por defecto, p.ej. al reconectar un dock).
    #[serde(default)]
    pub last_monitors: Vec<MonitorRect>,
    /// Idioma de la interfaz: en, es, de, fr, pt, it (ingles por defecto).
    #[serde(default = "default_language")]
    pub language: String,
    /// Nombres de widgets desactivados (no se crea su caja).
    #[serde(default)]
    pub widgets_disabled: Vec<String>,
    /// Configuracion del widget de Spotify (OAuth PKCE).
    pub spotify: SpotifyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub enabled: bool,
    pub ollama_url: String,
    pub model: String,
    /// Modelo de embeddings para clustering semantico (pequeno y rapido).
    pub embed_model: String,
    pub timeout_ms: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: false,
            ollama_url: String::from("http://127.0.0.1:11434"),
            model: String::from("llama3.2"),
            embed_model: String::from("nomic-embed-text"),
            timeout_ms: 1500,
        }
    }
}

/// Redirect URI por defecto del widget de Spotify (debe coincidir EXACTAMENTE
/// con la registrada en developer.spotify.com; editable desde Configuracion).
pub const SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8899/callback";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotifyConfig {
    /// true => la caja widget de Spotify se crea en el escritorio.
    pub enabled: bool,
    /// Client ID de la aplicacion registrada en developer.spotify.com.
    /// Se configura desde Configuracion -> Spotify (nada hardcodeado).
    pub client_id: String,
    /// Client Secret de la misma aplicacion (el flujo OAuth usa PKCE y no lo
    /// necesita, pero se persiste por si se cambia a flujo con secreto).
    pub client_secret: String,
    /// Redirect URI exacta registrada en el dashboard (debe coincidir al
    /// caracter; el puerto se usa para el listener local de autorizacion).
    pub redirect_uri: String,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        SpotifyConfig {
            enabled: true,
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: SPOTIFY_REDIRECT_URI.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Carpeta raiz donde viven las cajas fisicas. Por defecto es la ruta
    /// absoluta de Mis Documentos\ZenDesktop; admite rutas absolutas o
    /// relativas a Mis Documentos.
    pub root_folder: String,
    /// Carpeta de archivos caducados. Por defecto Mis Documentos\ZenArchive.
    pub archive_folder: String,
    /// Vigilar tambien el escritorio publico (C:\Users\Public\Desktop).
    pub watch_public_desktop: bool,
    /// Ventana de coalescencia de eventos de disco (ms). Evita rafagas de repintado.
    pub debounce_ms: u64,
    /// Clasificar todo el escritorio al arrancar.
    pub organize_on_start: bool,
    /// Periodo del barrido de caducidad, en minutos (timer de bajo coste).
    pub sweep_interval_minutes: u32,
    /// Doble clic en zona vacia del escritorio => Modo Zen.
    pub zen_double_click: bool,
    /// Atajo global Ctrl+Alt+Z para el Modo Zen.
    pub zen_hotkey: bool,
    /// En Modo Zen tambien se ocultan los iconos nativos del escritorio.
    pub zen_hides_desktop_icons: bool,
    /// Registrar la app en HKCU\...\Run.
    pub start_with_windows: bool,
    /// Retraso (segundos) antes de mostrar las cajas al arrancar; 0 = inmediato.
    /// Evita que las cajas tapan el escritorio mientras la sesion de Windows
    /// sigue cargando programas de inicio.
    #[serde(default)]
    pub startup_delay_seconds: u32,
    /// Mover tambien las carpetas del escritorio a sus cajas (solo reglas con
    /// `include_folders` y `move_files`; nunca se copian entre volumenes).
    #[serde(default = "default_true")]
    pub organize_folders: bool,
    /// Nombres/patrones que nunca se mueven (soporta * y ?).
    pub protected: Vec<String>,
    /// Los accesos directos (.lnk) permanecen siempre en el escritorio.
    pub keep_shortcuts: bool,
    /// Volcado de diagnostico a zendesktop.log (desactivado = 0 E/S).
    pub log_enabled: bool,
    /// Comprobar actualizaciones en segundo plano al arrancar y avisar si hay.
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    /// Ultima version para la que se mostro el dialogo "What's New".
    /// Se actualiza automaticamente tras mostrar el changelog de una nueva version.
    #[serde(default)]
    pub last_seen_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub background: String,
    pub background_hover: String,
    pub border: String,
    pub title_color: String,
    pub text_color: String,
    pub muted_color: String,
    pub shadow: String,
    pub font_family: String,
    pub title_size: f32,
    pub text_size: f32,
    pub corner_radius: f32,
    pub border_width: f32,
    pub padding: f32,
    pub header_height: f32,
    pub row_height: f32,
    pub show_icons: bool,
    pub show_counter: bool,
    /// Rejilla de imantacion al soltar una caja (px, 0 = libre).
    pub snap_grid: u32,
    /// Orden: "name" | "modified" | "size" | "extension" | "custom".
    pub sort_by: String,
    /// Mostrar miniaturas al pasar el raton sobre imagenes.
    pub show_thumbnails: bool,
    /// Tema visual predefinido: "custom" | "nocturno" | "oceano" | "bosque" | "minimal" | "clasico".
    /// "custom" = el usuario ha modificado colores manualmente.
    pub theme_preset: String,
    /// Mostrar una barra de busqueda en la cabecera de cada caja.
    pub show_search: bool,
    /// Vista en cuadricula de iconos en lugar de lista.
    pub grid_mode: bool,
    /// Tamano de cada celda en el modo cuadricula (px, 48..128).
    pub grid_item_size: f32,
    /// Tamano del icono dentro de cada celda (px, 16..96).
    pub grid_icon_size: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ephemeral {
    pub enabled: bool,
    /// Edad maxima antes de archivar. Admite decimales (0.5 = 12 h).
    pub max_age_days: f64,
    /// Edad minima absoluta en minutos: red de seguridad para descargas recientes.
    pub min_age_minutes: u64,
    /// Considerar tambien la fecha de ultimo acceso, no solo la de modificacion.
    pub use_access_time: bool,
    /// Subcarpetas AAAA-MM dentro del archivo.
    pub archive_by_month: bool,
    /// Extensiones que nunca caducan.
    pub never_expire: Vec<String>,
    /// Vaciar del archivo lo que supere estos dias (0 = nunca borrar nada).
    pub purge_archive_after_days: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Identificador estable, usado para casar la geometria de la caja.
    pub id: String,
    pub title: String,
    pub enabled: bool,
    /// Extensiones sin punto. "*" actua como comodin (regla cajon de sastre).
    pub extensions: Vec<String>,
    /// Patrones de nombre adicionales (soporta * y ?), p. ej. "factura*".
    pub name_patterns: Vec<String>,
    /// true  -> los archivos se MUEVEN a <Documentos>\<root_folder>\<folder>
    /// false -> caja virtual: los archivos siguen en el escritorio, solo se listan.
    pub move_files: bool,
    pub folder: String,
    pub color: String,
    /// Incluir carpetas ademas de archivos en la caja virtual.
    pub include_folders: bool,
    /// Vista de la caja: "auto" (sigue el ajuste global de Apariencia),
    /// "list" (lista) o "grid" (cuadricula de iconos).
    #[serde(default = "default_view_mode")]
    pub view_mode: String,
    /// Tamano del icono en modo cuadricula, en px. `None` = sigue el ajuste
    /// global de Apariencia. Se cicla con valores predefinidos (16..96).
    #[serde(default)]
    pub icon_size: Option<f32>,
    // --- Filtros avanzados (ninguno = no filtra por ese criterio) ---
    /// Tamano minimo en bytes (None = sin minimo).
    #[serde(default)]
    pub min_size_bytes: Option<u64>,
    /// Tamano maximo en bytes (None = sin maximo).
    #[serde(default)]
    pub max_size_bytes: Option<u64>,
    /// Solo archivos mas nuevos de N dias.
    #[serde(default)]
    pub newer_than_days: Option<f64>,
    /// Solo archivos mas viejos de N dias.
    #[serde(default)]
    pub older_than_days: Option<f64>,
    /// Patron regex sobre el nombre completo del archivo (sin ruta).
    #[serde(default)]
    pub regex: Option<String>,
    /// Rutas completas de los archivos fijados (favoritos): siempre flotan
    /// arriba de la caja, por encima del orden normal.
    #[serde(default)]
    pub pinned: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FenceLayout {
    pub id: String32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub collapsed: bool,
    pub hidden: bool,
    /// Caja anclada: ni se arrastra ni se redimensiona (candado en la cabecera).
    pub locked: bool,
    /// Caja "siempre visible": flota por encima de cualquier app (chincheta en
    /// la cabecera). Independiente del candado de anclaje.
    pub pinned: bool,
    /// Orden de los items: "name" | "size" | "type" | "modified" | "custom"
    /// None => hereda el orden global de `appearance.sort_by`.
    pub sort_by: Option<String>,
    pub group_title: Option<String>,
    pub tabs: Vec<String>,
    /// Nombre del widget (script .lua en la carpeta widgets/) si esta caja es
    /// un widget en vez de una caja de archivos. None => caja normal.
    pub widget: Option<String>,
    /// Monitor donde vive la caja (limites de pantalla completa): lo rellena
    /// la captura de plantillas para reposicionar en disposiciones distintas.
    pub monitor: Option<MonitorRect>,
}

/// Cadena corta de tamano fijo: evita una asignacion por caja y mantiene
/// `FenceLayout` como `Copy` (la geometria se copia en cada frame de arrastre).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct String32 {
    buf: [u8; 32],
    len: u8,
}

impl String32 {
    pub fn new(s: &str) -> Self {
        let bytes = s.as_bytes();
        let len = bytes.len().min(32);
        let mut buf = [0u8; 32];
        buf[..len].copy_from_slice(&bytes[..len]);
        String32 { buf, len: len as u8 }
    }
    pub fn as_str(&self) -> &str {
        // Invariante: el buffer se rellena siempre desde un &str y se corta en
        // un limite valido salvo truncado; from_utf8 protege ese caso extremo.
        std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

impl Default for String32 {
    fn default() -> Self {
        String32::new("")
    }
}

impl fmt::Display for String32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for String32 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for String32 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(String32::new(&s))
    }
}

/// Limites de un monitor en coordenadas de pantalla virtual. Se guardan en las
/// plantillas para poder reposicionar las cajas cuando la disposicion de
/// monitores cambia entre la captura y la aplicacion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MonitorRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl MonitorRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// Traslada la posicion guardada `(x, y)` (con tamano `w` x `h`) a la
    /// disposicion de monitores actual `now`:
    ///   1. Monitor identico  -> desplazamiento exacto.
    ///   2. Mismo tamano      -> el candidato mas cercano conserva el offset
    ///      relativo dentro del monitor (monitores movidos o permutados).
    ///   3. Monitor perdido   -> relativo al monitor primario.
    ///
    /// En los casos 2 y 3 la posicion se recorta para que la caja nunca quede
    /// fuera de la pantalla. Sin monitores, devuelve la posicion original.
    pub fn translate(&self, x: i32, y: i32, w: i32, h: i32, now: &[MonitorRect]) -> (i32, i32) {
        if let Some(m) = now.iter().find(|m| **m == *self) {
            return (x + (m.left - self.left), y + (m.top - self.top));
        }
        let same_size = now
            .iter()
            .filter(|m| m.width() == self.width() && m.height() == self.height())
            .min_by_key(|m| (m.left - self.left).abs() + (m.top - self.top).abs());
        let target = same_size
            .or_else(|| now.iter().find(|m| m.left == 0 && m.top == 0))
            .or_else(|| now.first());
        let Some(t) = target else { return (x, y) };
        let x_max = (t.right - w).max(t.left);
        let y_max = (t.bottom - h).max(t.top);
        let nx = (t.left + (x - self.left)).clamp(t.left, x_max);
        let ny = (t.top + (y - self.top)).clamp(t.top, y_max);
        (nx, ny)
    }

}

/// Plantilla de layout: snapshot con nombre de la geometria de todas las cajas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutTemplate {
    pub name: String,
    pub layouts: Vec<FenceLayout>,
    /// Plantilla por defecto: se aplica sola al arrancar o al conectar un
    /// monitor de su disposicion. Solo una puede estar marcada.
    #[serde(default)]
    pub default: bool,
}

impl LayoutTemplate {
    /// true si todos los monitores que usa esta plantilla estan presentes en
    /// la disposicion actual `now` con los mismos limites (monitor conocido
    /// reconectado en el mismo sitio). La coincidencia es exacta a proposito:
    /// con solo mismo tamano se dispararia al desconectar un monitor. Las
    /// plantillas sin informacion de monitores nunca coinciden solas.
    pub fn matches_monitors(&self, now: &[MonitorRect]) -> bool {
        let saved: Vec<MonitorRect> = self.layouts.iter().filter_map(|l| l.monitor).collect();
        !saved.is_empty() && saved.iter().all(|m| now.contains(m))
    }
}

impl Default for FenceLayout {
    fn default() -> Self {
        FenceLayout {
            id: String32::default(),
            x: 40,
            y: 40,
            width: 320,
            height: 260,
            collapsed: false,
            hidden: false,
            locked: false,
            pinned: false,
            sort_by: None,
            tabs: Vec::new(),
            group_title: None,
            widget: None,
            monitor: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Valores por defecto
// ---------------------------------------------------------------------------

impl Default for General {
    fn default() -> Self {
        General {
            root_folder: default_boxes_path("ZenDesktop"),
            archive_folder: default_boxes_path("ZenArchive"),
            watch_public_desktop: true,
            debounce_ms: 400,
            organize_on_start: true,
            sweep_interval_minutes: 30,
            zen_double_click: true,
            zen_hotkey: true,
            zen_hides_desktop_icons: true,
            start_with_windows: false,
            startup_delay_seconds: 0,
            organize_folders: true,
            protected: vec![
                "desktop.ini".into(),
                "*.crdownload".into(),
                "*.part".into(),
                "*.tmp".into(),
                "~$*".into(),
            ],
            keep_shortcuts: true,
            log_enabled: false,
            auto_check_updates: true,
            last_seen_version: String::new(),
        }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            background: "#12161FC4".into(),
            background_hover: "#1E2635E0".into(),
            border: "#8FA6C433".into(),
            title_color: "#E6EDF7".into(),
            text_color: "#C7D2E1".into(),
            muted_color: "#7C8AA0".into(),
            shadow: "#00000055".into(),
            font_family: "Segoe UI Variable Text".into(),
            title_size: 13.0,
            text_size: 12.0,
            corner_radius: 14.0,
            border_width: 1.0,
            padding: 10.0,
            header_height: 34.0,
            row_height: 24.0,
            show_icons: true,
            show_counter: true,
            snap_grid: 8,
            sort_by: "name".into(),
            show_thumbnails: true,
            theme_preset: "nocturno".into(),
            show_search: true,
            grid_mode: false,
            grid_item_size: 72.0,
            grid_icon_size: 48.0,
        }
    }
}

impl Default for Ephemeral {
    fn default() -> Self {
        Ephemeral {
            enabled: true,
            max_age_days: 14.0,
            min_age_minutes: 60,
            use_access_time: true,
            archive_by_month: true,
            never_expire: vec!["lnk".into(), "url".into()],
            purge_archive_after_days: 0,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: General::default(),
            appearance: Appearance::default(),
            ephemeral: Ephemeral::default(),
            ai: AiConfig::default(),
            rules: default_rules(),
            fences: Vec::new(),
            templates: Vec::new(),
            last_monitors: Vec::new(),
            language: "en".into(),
            widgets_disabled: Vec::new(),
            spotify: SpotifyConfig::default(),
        }
    }
}

fn default_language() -> String {
    "en".into()
}

fn default_view_mode() -> String {
    "auto".into()
}

/// Reglas por defecto: Media, Documentos, Instaladores y Varios (cajon de sastre
/// virtual, no mueve nada). Ampliables sin recompilar desde config.toml.
/// Valor por defecto para opciones booleanas nuevas que deben nacer activadas.
fn default_true() -> bool {
    true
}

pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: "media".into(),
            title: "Media".into(),
            enabled: true,
            extensions: vec![
                "jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "tiff", "svg", "psd", "raw",
                "mp4", "mkv", "avi", "mov", "webm", "m4v", "wmv", "mp3", "wav", "flac", "aac",
                "ogg", "m4a",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            name_patterns: vec!["Captura de pantalla*".into(), "Screenshot*".into()],
            move_files: true,
            folder: "Media".into(),
            color: "#38BDF8".into(),
            include_folders: false,
            view_mode: "auto".into(),
            icon_size: None,
            min_size_bytes: None,
            max_size_bytes: None,
            newer_than_days: None,
            older_than_days: None,
            regex: None,
            pinned: Vec::new(),
        },
        Rule {
            id: "docs".into(),
            title: "Documentos".into(),
            enabled: true,
            extensions: vec![
                "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "xls", "xlsx", "ods", "csv",
                "ppt", "pptx", "odp", "epub", "mobi", "tex",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            name_patterns: Vec::new(),
            move_files: true,
            folder: "Documentos".into(),
            color: "#A78BFA".into(),
            include_folders: false,
            view_mode: "auto".into(),
            icon_size: None,
            min_size_bytes: None,
            max_size_bytes: None,
            newer_than_days: None,
            older_than_days: None,
            regex: None,
            pinned: Vec::new(),
        },
        Rule {
            id: "setup".into(),
            title: "Instaladores".into(),
            enabled: true,
            extensions: vec![
                "exe", "msi", "msix", "appx", "msu", "zip", "rar", "7z", "tar", "gz", "xz", "iso",
                "img", "cab",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            name_patterns: vec!["setup*".into(), "install*".into()],
            move_files: true,
            folder: "Instaladores".into(),
            color: "#F472B6".into(),
            include_folders: false,
            view_mode: "auto".into(),
            icon_size: None,
            min_size_bytes: None,
            max_size_bytes: None,
            newer_than_days: None,
            older_than_days: None,
            regex: None,
            pinned: Vec::new(),
        },
        Rule {
            id: "misc".into(),
            title: "Varios".into(),
            enabled: true,
            extensions: vec!["*".into()],
            name_patterns: Vec::new(),
            // Cajon de sastre fisico: recoge todo lo que no casa con otra regla,
            // ficheros y carpetas. Desactivable desde el menu de configuracion.
            move_files: true,
            folder: "Varios".into(),
            color: "#34D399".into(),
            include_folders: true,
            view_mode: "auto".into(),
            icon_size: None,
            min_size_bytes: None,
            max_size_bytes: None,            newer_than_days: None,
            older_than_days: None,
            regex: None,
            pinned: Vec::new(),
        },


    ]
}

// ---------------------------------------------------------------------------
// Carga / guardado
// ---------------------------------------------------------------------------

impl Config {
    /// Devuelve la configuracion y la ruta efectiva del config.toml, creandolo
    /// con valores por defecto la primera vez que se ejecuta la aplicacion.
    pub fn load_or_create() -> Result<(Config, PathBuf), ConfigError> {
        let path = Config::resolve_path()?;
        if path.is_file() {
            let raw = fs::read_to_string(&path)?;
            let mut cfg: Config = toml::from_str(&raw)?;
            cfg.normalize();
            Ok((cfg, path))
        } else {
            let cfg = Config::default();
            cfg.save(&path)?;
            Ok((cfg, path))
        }
    }

    /// Relee el archivo sin tocar el estado si hay un error de sintaxis.
    pub fn reload(path: &Path) -> Result<Config, ConfigError> {
        let raw = fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&raw)?;
        cfg.normalize();
        Ok(cfg)
    }

    /// Escritura atomica: <config>.tmp -> rename. Nunca deja el archivo a medias.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let body = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(HEADER.as_bytes())?;
            f.write_all(body.as_bytes())?;
            f.sync_all()?;
        }
        // rename sobre un destino existente falla en Win32: se elimina antes.
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Portable primero; si el directorio del ejecutable no admite escritura
    /// (Archivos de programa, unidad de red de solo lectura) cae a %APPDATA%.
    fn resolve_path() -> Result<PathBuf, ConfigError> {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join(CONFIG_FILE);
                if candidate.is_file() || dir_is_writable(dir) {
                    return Ok(candidate);
                }
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            let dir = PathBuf::from(appdata).join(APP_NAME);
            if fs::create_dir_all(&dir).is_ok() {
                return Ok(dir.join(CONFIG_FILE));
            }
        }
        Err(ConfigError::NoWritableLocation)
    }

    /// Sanea valores fuera de rango para que un config.toml editado a mano
    /// jamas pueda romper el renderizador ni disparar divisiones por cero.
    pub fn normalize(&mut self) {
        let g = &mut self.general;
        g.debounce_ms = g.debounce_ms.clamp(50, 10_000);
        g.sweep_interval_minutes = g.sweep_interval_minutes.clamp(1, 24 * 60);
        g.startup_delay_seconds = g.startup_delay_seconds.clamp(0, 600);
        if g.root_folder.trim().is_empty() {
            g.root_folder = default_boxes_path("ZenDesktop");
        }
        if g.archive_folder.trim().is_empty() {
            g.archive_folder = default_boxes_path("ZenArchive");
        }

        // Plantillas sin nombre (config editada a mano) se descartan.
        self.templates.retain(|t| !t.name.is_empty());
        // A lo sumo una plantilla por defecto: la primera marcada gana.
        let mut seen_default = false;
        for t in &mut self.templates {
            if t.default {
                if seen_default {
                    t.default = false;
                } else {
                    seen_default = true;
                }
            }
        }

        self.spotify.client_id = self.spotify.client_id.trim().to_string();
        self.spotify.client_secret = self.spotify.client_secret.trim().to_string();
        if self.spotify.redirect_uri.trim().is_empty() {
            self.spotify.redirect_uri = SPOTIFY_REDIRECT_URI.into();
        }

        let a = &mut self.appearance;
        a.title_size = a.title_size.clamp(8.0, 40.0);
        a.text_size = a.text_size.clamp(7.0, 32.0);
        a.corner_radius = a.corner_radius.clamp(0.0, 40.0);
        a.border_width = a.border_width.clamp(0.0, 6.0);
        a.padding = a.padding.clamp(0.0, 40.0);
        a.header_height = a.header_height.clamp(18.0, 80.0);
        a.row_height = a.row_height.clamp(14.0, 64.0);
        a.snap_grid = a.snap_grid.min(128);
        a.grid_item_size = a.grid_item_size.clamp(48.0, 128.0);
        a.grid_icon_size = a.grid_icon_size.clamp(16.0, 96.0);
        if a.font_family.trim().is_empty() {
            a.font_family = "Segoe UI".into();
        }

        let e = &mut self.ephemeral;
        if !e.max_age_days.is_finite() || e.max_age_days <= 0.0 {
            e.max_age_days = 14.0;
        }
        e.max_age_days = e.max_age_days.clamp(0.02, 3650.0);

        self.spotify.client_id = self.spotify.client_id.trim().to_string();

        // Idioma valido: desconocido o vacio -> ingles.
        self.language = crate::i18n::Lang::from_code(&self.language).code().into();

        if self.rules.is_empty() {
            self.rules = default_rules();
        }
        for r in &mut self.rules {
            if r.id.trim().is_empty() {
                r.id = slug(&r.title);
            }
            if r.folder.trim().is_empty() {
                r.folder = r.title.clone();
            }
            for ext in &mut r.extensions {
                *ext = ext.trim_start_matches('.').to_ascii_lowercase();
            }
            if !matches!(r.view_mode.as_str(), "auto" | "list" | "grid") {
                r.view_mode = "auto".into();
            }
            // El tamano de icono por caja solo acepta valores razonables.
            if let Some(v) = r.icon_size {
                if !(16.0..=96.0).contains(&v) {
                    r.icon_size = None;
                }
            }
            // Filtros avanzados: descartar valores invalidos (regex que no
            // compila, dias negativos, rango de tamano invertido).
            if let Some(re) = &r.regex {
                if re.trim().is_empty() || regex::Regex::new(re).is_err() {
                    r.regex = None;
                }
            }
            if let Some(v) = r.newer_than_days {
                if !v.is_finite() || v <= 0.0 {
                    r.newer_than_days = None;
                }
            }
            if let Some(v) = r.older_than_days {
                if !v.is_finite() || v <= 0.0 {
                    r.older_than_days = None;
                }
            }
            if let (Some(min), Some(max)) = (r.min_size_bytes, r.max_size_bytes) {
                if min > max {
                    r.max_size_bytes = None;
                }
            }
        }
        // Descarta geometrias huerfanas de reglas eliminadas. Las cajas de
        // widgets (id "widget:<script>") no corresponden a reglas y se
        // conservan siempre: si no, un normalize() (p.ej. al recoger la
        // config del dialogo) las borra y la caja desaparece tras crearla.
        let ids: Vec<String> = self.rules.iter().map(|r| r.id.clone()).collect();
        self.fences.retain(|f| {
            f.widget.is_some() || f.tabs.iter().any(|t| ids.iter().any(|id| id == t))
                || ids.iter().any(|id| id == f.id.as_str())
        });
    }



    pub fn set_layout(&mut self, layout: FenceLayout) {
        match self
            .fences
            .iter_mut()
            .find(|f| f.id.as_str() == layout.id.as_str())
        {
            Some(slot) => *slot = layout,
            None => self.fences.push(layout),
        }
    }

    /// Devuelve la caja agrupada (si la hay) que contiene `rule_id` como
    /// pestana. `None` si la regla no esta agrupada (es independiente).
    pub fn group_of(&self, rule_id: &str) -> Option<&FenceLayout> {
        self.fences
            .iter()
            .find(|f| f.tabs.iter().any(|t| t == rule_id))
    }

    /// true si la regla `rule_id` forma parte de un grupo de pestanas.
    pub fn is_grouped(&self, rule_id: &str) -> bool {
        self.group_of(rule_id).is_some()
    }

    /// Duracion maxima de vida de un archivo en el escritorio.
    /// Idioma de la interfaz, normalizado a un valor conocido.
    pub fn lang(&self) -> crate::i18n::Lang {
        crate::i18n::Lang::from_code(&self.language)
    }

    pub fn max_age(&self) -> std::time::Duration {
        let secs = (self.ephemeral.max_age_days * 86_400.0).max(60.0);
        std::time::Duration::from_secs_f64(secs)
    }

    /// Carpeta raiz de las cajas fisicas (por defecto en "Mis Documentos").
    /// Si `root_folder` es una ruta absoluta, se usa tal cual.
    pub fn root_dir(&self) -> PathBuf {
        resolve_folder(&self.general.root_folder)
    }

    /// Carpeta de archivo historico (por defecto en "Mis Documentos").
    /// Si `archive_folder` es una ruta absoluta, se usa tal cual.
    pub fn archive_dir(&self) -> PathBuf {
        resolve_folder(&self.general.archive_folder)
    }

    /// true si cambio algo que afecta a la organizacion (reglas, carpetas de
    /// destino/archivo, protecciones o caducidad). Permite saltarse pasadas de
    /// organizacion innecesarias al guardar cambios puramente visuales.
    pub fn organize_relevant_changed(&self, other: &Config) -> bool {
        self.general.root_folder != other.general.root_folder
            || self.general.archive_folder != other.general.archive_folder
            || self.general.organize_folders != other.general.organize_folders
            || self.general.protected != other.general.protected
            || self.general.keep_shortcuts != other.general.keep_shortcuts
            || self.rules != other.rules
            || self.ephemeral != other.ephemeral
    }
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".zendesktop-write-probe");
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Color: "#RRGGBB" / "#RRGGBBAA" -> (r, g, b, a) normalizado 0.0..1.0
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Temas visuales predefinidos
// ---------------------------------------------------------------------------

/// Paleta de colores de un tema (Background, Hover, Border, Title, Text, Muted, Shadow).
pub struct ThemePalette {
    pub background: &'static str,
    pub background_hover: &'static str,
    pub border: &'static str,
    pub title_color: &'static str,
    pub text_color: &'static str,
    pub muted_color: &'static str,
    pub shadow: &'static str,
}

impl Appearance {
    /// Lista de presets disponibles. El primero es el predeterminado.
    pub fn preset_names() -> &'static [&'static str] {
        &["nocturno", "oceano", "bosque", "minimal", "clasico", "custom"]
    }

    /// Etiqueta visible del preset (segun idioma, pero las claves son las
    /// mismas en todos los idiomas).
    pub fn preset_label(key: &str) -> &'static str {
        match key {
            "nocturno" => "Nocturno",
            "oceano" => "Océano",
            "bosque" => "Bosque",
            "minimal" => "Minimal",
            "clasico" => "Clásico",
            _ => "Personalizado",
        }
    }

    /// Devuelve la paleta del preset indicado.
    pub fn palette_for(name: &str) -> ThemePalette {
        match name {
            "nocturno" => ThemePalette {
                background: "#12161FC4",
                background_hover: "#1E2635E0",
                border: "#8FA6C433",
                title_color: "#E6EDF7",
                text_color: "#C7D2E1",
                muted_color: "#7C8AA0",
                shadow: "#00000055",
            },
            "oceano" => ThemePalette {
                background: "#0F1B2DC4",
                background_hover: "#162440E0",
                border: "#3B82F633",
                title_color: "#93C5FD",
                text_color: "#BFDBFE",
                muted_color: "#6B9FD4",
                shadow: "#1E3A5F55",
            },
            "bosque" => ThemePalette {
                background: "#0F1A13C4",
                background_hover: "#162618E0",
                border: "#34D39933",
                title_color: "#A7F3D0",
                text_color: "#D1FAE5",
                muted_color: "#6BAF8A",
                shadow: "#0A2A1E55",
            },
            "minimal" => ThemePalette {
                background: "#F1F5F9D0",
                background_hover: "#E2E8F0E0",
                border: "#94A3B833",
                title_color: "#1E293B",
                text_color: "#334155",
                muted_color: "#64748B",
                shadow: "#00000022",
            },
            "clasico" => ThemePalette {
                background: "#1A1410C4",
                background_hover: "#2A1F18E0",
                border: "#D9770633",
                title_color: "#FDE68A",
                text_color: "#FEF3C7",
                muted_color: "#B89B6A",
                shadow: "#3A200A55",
            },
            _ => ThemePalette {
                // custom: devuelve transparente-negro para que el usuario no
                // vea cambios al seleccionar "Personalizado".
                background: "#00000000",
                background_hover: "#00000000",
                border: "#00000000",
                title_color: "#000000",
                text_color: "#000000",
                muted_color: "#000000",
                shadow: "#00000000",
            },
        }
    }

    /// Aplica los colores de un preset a esta configuracion (no persiste).
    pub fn apply_preset(&mut self, name: &str) {
        if name == "custom" {
            self.theme_preset = "custom".into();
            return;
        }
        let p = Self::palette_for(name);
        self.background = p.background.into();
        self.background_hover = p.background_hover.into();
        self.border = p.border.into();
        self.title_color = p.title_color.into();
        self.text_color = p.text_color.into();
        self.muted_color = p.muted_color.into();
        self.shadow = p.shadow.into();
        self.theme_preset = name.into();
    }
}

pub fn parse_color(hex: &str) -> (f32, f32, f32, f32) {
    let s = hex.trim().trim_start_matches('#');
    let parse = |i: usize| -> f32 {
        u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0) as f32 / 255.0
    };
    match s.len() {
        6 => (parse(0), parse(2), parse(4), 1.0),
        8 => (parse(0), parse(2), parse(4), parse(6)),
        _ => (1.0, 1.0, 1.0, 1.0),
    }
}

// ---------------------------------------------------------------------------
// Carpetas conocidas del shell (sin hardcodear rutas ni %USERPROFILE%)
// ---------------------------------------------------------------------------

pub fn desktop_dir() -> Result<PathBuf, ConfigError> {
    known_folder(&FOLDERID_Desktop)
}

pub fn documents_dir() -> Result<PathBuf, ConfigError> {
    known_folder(&FOLDERID_Documents)
}

pub fn public_desktop_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_PublicDesktop).ok()
}

/// Base de las carpetas internas (ZenDesktop / ZenArchive). Por defecto en
/// "Mis Documentos", con retroceso al Escritorio si falla la resolucion.
fn boxes_base() -> PathBuf {
    documents_dir()
        .or_else(|_| desktop_dir())
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Ruta por defecto (absoluta) de una carpeta interna dentro de Mis Documentos.
fn default_boxes_path(name: &str) -> String {
    boxes_base().join(name).to_string_lossy().into_owned()
}

/// Resuelve la carpeta interna: una ruta absoluta se usa tal cual (permite a
/// los tests aislarse en un directorio temporal); una relativa se cuelga de
/// la base (Mis Documentos).
fn resolve_folder(folder: &str) -> PathBuf {
    let p = Path::new(folder);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        boxes_base().join(p)
    }
}

fn known_folder(id: &GUID) -> Result<PathBuf, ConfigError> {
    unsafe {
        let pw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, HANDLE::default())?;
        if pw.is_null() {
            return Err(ConfigError::NoWritableLocation);
        }
        let text = pw.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pw.0 as *const core::ffi::c_void));
        if text.is_empty() {
            Err(ConfigError::NoWritableLocation)
        } else {
            Ok(PathBuf::from(text))
        }
    }
}

// ---------------------------------------------------------------------------
// Arranque con Windows (HKCU, sin privilegios de administrador)
// ---------------------------------------------------------------------------

pub fn apply_autostart(enabled: bool) -> Result<(), ConfigError> {
    let exe = std::env::current_exe()?;
    let command = format!("\"{}\"", exe.display());
    unsafe {
        let mut key = HKEY::default();
        let sub = wide(RUN_KEY);
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            return Err(ConfigError::Shell(windows::core::Error::from_hresult(
                status.to_hresult(),
            )));
        }
        let name = wide(APP_NAME);
        let result = if enabled {
            let value = wide(&command);
            let bytes = std::slice::from_raw_parts(
                value.as_ptr() as *const u8,
                value.len() * std::mem::size_of::<u16>(),
            );
            RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes))
        } else {
            // Borrar un valor que no existe no es un error: se normaliza a exito.
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
            ERROR_SUCCESS
        };
        let _ = RegCloseKey(key);
        if result != ERROR_SUCCESS {
            return Err(ConfigError::Shell(windows::core::Error::from_hresult(
                result.to_hresult(),
            )));
        }
    }
    Ok(())
}

/// Helper UTF-16 terminado en NUL para las APIs W.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_serializes_language() {
        let toml = toml::to_string(&Config::default()).unwrap();
        assert!(
            toml.contains("language = \"en\""),
            "la config por defecto debe persistir el idioma: {toml}"
        );
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        let mut cfg = Config::default();
        cfg.language = "xx".into();
        cfg.normalize();
        assert_eq!(cfg.lang().code(), "en");

        // Los config.toml antiguos sin el campo se cargan como ingles.
        let raw = "[general]\n";
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.lang().code(), "en");
    }

    #[test]
    fn fence_layout_monitor_round_trips() {
        let fl = FenceLayout {
            monitor: Some(MonitorRect { left: -1920, top: 0, right: 0, bottom: 1080 }),
            ..Default::default()
        };
        let toml = toml::to_string(&fl).unwrap();
        let back: FenceLayout = toml::from_str(&toml).unwrap();
        assert_eq!(back.monitor, fl.monitor);
        // Las cajas antiguas sin el campo cargan como None.
        let back: FenceLayout = toml::from_str("id = \"x\"\n").unwrap();
        assert_eq!(back.monitor, None);
    }

    #[test]
    fn monitor_translate_same_layout_keeps_position() {
        let saved = MonitorRect { left: -1920, top: 0, right: 0, bottom: 1080 };
        let now = vec![
            MonitorRect { left: -1920, top: 0, right: 0, bottom: 1080 },
            MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 },
        ];
        assert_eq!(saved.translate(-1500, 300, 320, 240, &now), (-1500, 300));
    }

    #[test]
    fn monitor_translate_same_size_moved_picks_nearest() {
        let saved = MonitorRect { left: -1920, top: 0, right: 0, bottom: 1080 };
        let now = vec![
            MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 },
            MonitorRect { left: 1920, top: 0, right: 3840, bottom: 1080 },
        ];
        assert_eq!(saved.translate(-1500, 300, 320, 240, &now), (420, 300));
    }

    #[test]
    fn monitor_translate_lost_monitor_clamps_to_primary() {
        let saved = MonitorRect { left: 0, top: 0, right: 2560, bottom: 1440 };
        let now = vec![MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 }];
        assert_eq!(saved.translate(2000, 1200, 600, 400, &now), (1320, 680));
    }

    #[test]
    fn monitor_translate_wide_fence_clamps_without_panic() {
        // Caja mas ancha que el monitor destino: el recorte no debe entrar en
        // panico (min > max en clamp) y deja la caja visible a la izquierda.
        let saved = MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 };
        let now = vec![MonitorRect { left: 0, top: 0, right: 1280, bottom: 800 }];
        let (x, y) = saved.translate(100, 100, 3000, 500, &now);
        assert_eq!((x, y), (0, 100));
    }

    #[test]
    fn monitor_translate_no_monitors_keeps_position() {
        let saved = MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 };
        assert_eq!(saved.translate(100, 100, 320, 240, &[]), (100, 100));
    }

    #[test]
    fn template_matches_monitors_when_all_present() {
        let fl = |id: &str, m: MonitorRect| FenceLayout {
            id: String32::new(id),
            monitor: Some(m),
            ..Default::default()
        };
        let t = LayoutTemplate {
            name: "dock".into(),
            layouts: vec![
                fl("a", MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 }),
                fl("b", MonitorRect { left: 1920, top: 0, right: 3840, bottom: 1080 }),
            ],
            default: true,
        };
        // Ambos monitores conectados -> coincide.
        let dual = vec![
            MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 },
            MonitorRect { left: 1920, top: 0, right: 3840, bottom: 1080 },
        ];
        assert!(t.matches_monitors(&dual));
        // Solo uno conectado -> no coincide.
        let single = vec![MonitorRect { left: 0, top: 0, right: 1920, bottom: 1080 }];
        assert!(!t.matches_monitors(&single));
        // Plantilla sin informacion de monitores -> nunca coincide sola.
        let old = LayoutTemplate {
            name: "old".into(),
            layouts: vec![FenceLayout::default()],
            default: false,
        };
        assert!(!old.matches_monitors(&dual));
    }

    #[test]
    fn normalize_keeps_single_default_template() {
        let mut cfg = Config::default();
        cfg.templates.push(LayoutTemplate {
            name: "a".into(),
            layouts: vec![],
            default: true,
        });
        cfg.templates.push(LayoutTemplate {
            name: "b".into(),
            layouts: vec![],
            default: true,
        });
        cfg.normalize();
        let defaults: Vec<&str> = cfg
            .templates
            .iter()
            .filter(|t| t.default)
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(defaults, vec!["a"]);
    }

    #[test]
    fn organize_relevant_changed_detects_rule_and_folder_edits_only() {
        let base = Config::default();

        // Sin cambios -> false.
        assert!(!base.organize_relevant_changed(&base.clone()));

        // Cambio puramente visual (color) -> false.
        let mut visual = Config::default();
        visual.appearance.background = "#112233".into();
        assert!(!visual.organize_relevant_changed(&base));

        // Cambio en una regla -> true.
        let mut with_rule = Config::default();
        with_rule.rules.push(Rule {
            id: "r".into(),
            folder: "docs".into(),
            ..Default::default()
        });
        assert!(with_rule.organize_relevant_changed(&base));

        // Cambio en la carpeta raiz -> true.
        let mut root = Config::default();
        root.general.root_folder = "otra".into();
        assert!(root.organize_relevant_changed(&base));

        // Cambio en caducidad (ephemeral) -> true.
        let mut ephemeral = Config::default();
        ephemeral.ephemeral.max_age_days = 7.0;
        assert!(ephemeral.organize_relevant_changed(&base));
    }

    #[test]
    fn normalize_keeps_widget_fences() {
        // Regression: "Añadir como caja" creaba la caja widget y un
        // normalize() posterior (collect_cfg -> flush_preview) la borraba
        // porque su id "widget:<script>" no coincide con ninguna regla.
        let mut cfg = Config::default();
        cfg.fences.push(FenceLayout {
            id: String32::new("widget:clima"),
            widget: Some("clima".into()),
            ..Default::default()
        });
        cfg.normalize();
        assert_eq!(
            cfg.fences
                .iter()
                .filter(|f| f.widget.is_some())
                .count(),
            1,
            "normalize() no debe borrar las cajas de widgets"
        );

        // Las geometrias huerfanas de reglas eliminadas siguen descartandose.
        let mut cfg = Config::default();
        cfg.fences.push(FenceLayout {
            id: String32::new("regla-eliminada"),
            ..Default::default()
        });
        cfg.normalize();
        assert!(cfg.fences.is_empty());
    }
}
