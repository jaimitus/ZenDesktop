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
    SHGetKnownFolderPath, FOLDERID_Desktop, FOLDERID_PublicDesktop, KF_FLAG_DEFAULT,
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
    /// Idioma de la interfaz: en, es, de, fr, pt, it (ingles por defecto).
    #[serde(default = "default_language")]
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub enabled: bool,
    pub ollama_url: String,
    pub model: String,
    pub timeout_ms: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: false,
            ollama_url: String::from("http://127.0.0.1:11434"),
            model: String::from("llama3.2"),
            timeout_ms: 1500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Carpeta raiz (relativa al Escritorio) donde viven las cajas fisicas.
    pub root_folder: String,
    /// Carpeta de archivos caducados (relativa al Escritorio).
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// true  -> los archivos se MUEVEN a <Escritorio>\<root_folder>\<folder>
    /// false -> caja virtual: los archivos siguen en el escritorio, solo se listan.
    pub move_files: bool,
    pub folder: String,
    pub color: String,
    /// Incluir carpetas ademas de archivos en la caja virtual.
    pub include_folders: bool,
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
    /// Orden de los items: "name" | "size" | "type" | "modified" | "custom"
    /// None => hereda el orden global de `appearance.sort_by`.
    pub sort_by: Option<String>,
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
            sort_by: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Valores por defecto
// ---------------------------------------------------------------------------

impl Default for General {
    fn default() -> Self {
        General {
            root_folder: "ZenDesktop".into(),
            archive_folder: "ZenArchive".into(),
            watch_public_desktop: true,
            debounce_ms: 400,
            organize_on_start: true,
            sweep_interval_minutes: 30,
            zen_double_click: true,
            zen_hotkey: true,
            zen_hides_desktop_icons: true,
            start_with_windows: false,
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
            language: "en".into(),
        }
    }
}

fn default_language() -> String {
    "en".into()
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
        if g.root_folder.trim().is_empty() {
            g.root_folder = "ZenDesktop".into();
        }
        if g.archive_folder.trim().is_empty() {
            g.archive_folder = "ZenArchive".into();
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
        if a.font_family.trim().is_empty() {
            a.font_family = "Segoe UI".into();
        }

        let e = &mut self.ephemeral;
        if !e.max_age_days.is_finite() || e.max_age_days <= 0.0 {
            e.max_age_days = 14.0;
        }
        e.max_age_days = e.max_age_days.clamp(0.02, 3650.0);

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
        }
        // Descarta geometrias huerfanas de reglas eliminadas.
        let ids: Vec<String> = self.rules.iter().map(|r| r.id.clone()).collect();
        self.fences
            .retain(|f| ids.iter().any(|id| id == f.id.as_str()));
    }



    pub fn layout_of(&self, id: &str) -> Option<FenceLayout> {
        self.fences.iter().find(|f| f.id.as_str() == id).cloned()
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

    /// Duracion maxima de vida de un archivo en el escritorio.
    /// Idioma de la interfaz, normalizado a un valor conocido.
    pub fn lang(&self) -> crate::i18n::Lang {
        crate::i18n::Lang::from_code(&self.language)
    }

    pub fn max_age(&self) -> std::time::Duration {
        let secs = (self.ephemeral.max_age_days * 86_400.0).max(60.0);
        std::time::Duration::from_secs_f64(secs)
    }

    pub fn root_dir(&self, desktop: &Path) -> PathBuf {
        desktop.join(&self.general.root_folder)
    }

    pub fn archive_dir(&self, desktop: &Path) -> PathBuf {
        desktop.join(&self.general.archive_folder)
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

pub fn public_desktop_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_PublicDesktop).ok()
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
}
