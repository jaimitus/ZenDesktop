//! ZenDesktop :: settings.rs
//!
//! Ventana de configuracion dibujada de cero con Direct2D + DirectWrite, con
//! la misma estetica de las cajas (cristal oscuro, esquinas redondeadas,
//! acento azul). Nada de controles nativos clasicos: casillas, botones,
//! navegacion lateral y lista de reglas son dibujos propios con hover/foco.
//! Los campos de texto son EDIT nativos con tema oscuro (texto, cursor y
//! portapapeles gratis).
//!
//! Arquitectura:
//!   * Ventana `WS_POPUP` sin marco, arrastrable por la cabecera (WM_NCHITTEST).
//!   * Un `ID2D1HwndRenderTarget` dibuja todo el contenido en DIPs.
//!   * Un pase de layout genera el dibujo y la tabla de regiones de
//!     hit-testing (hover, clic, rueda).
//!   * Los campos de texto son hijos EDIT ocultos/mostrados segun el panel.
//!   * Bucle modal propio que repone WM_QUIT (nunca se traga el cierre).

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_CAP_STYLE_ROUND,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect,
    PAINTSTRUCT, ScreenToClient, SetBkColor, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE, HBRUSH, HDC, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    SHBrowseForFolderW, SHGetPathFromIDListW, SHParseDisplayName, BIF_NEWDIALOGSTYLE,
    BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, VK_DOWN,
    VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP, TME_LEAVE,
    TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{parse_color, wide, Config, LayoutTemplate, Rule};
use crate::i18n::{Lang, Tr};
use crate::ui::App;

// ---------------------------------------------------------------------------
// Identificadores
// ---------------------------------------------------------------------------

const ID_EDIT_MAX_AGE: u16 = 1;
const ID_EDIT_MIN_AGE: u16 = 2;
const ID_EDIT_PURGE: u16 = 3;
const ID_EDIT_R_TITLE: u16 = 4;
const ID_EDIT_R_FOLDER: u16 = 5;
const ID_EDIT_R_COLOR: u16 = 6;
const ID_EDIT_R_EXTS: u16 = 7;
const ID_EDIT_R_PATTERNS: u16 = 8;
const ID_EDIT_R_GROUP_TITLE: u16 = 25;
const ID_EDIT_A_BG: u16 = 9;
const ID_EDIT_A_HOVER: u16 = 10;
const ID_EDIT_A_BORDER: u16 = 11;
const ID_EDIT_A_TITLE: u16 = 12;
const ID_EDIT_A_TEXT: u16 = 13;
const ID_EDIT_A_MUTED: u16 = 14;
const ID_EDIT_A_SHADOW: u16 = 15;
const ID_EDIT_A_RADIUS: u16 = 16;
const ID_EDIT_A_TITLE_SIZE: u16 = 17;
const ID_EDIT_A_TEXT_SIZE: u16 = 18;
const ID_EDIT_A_SNAP: u16 = 19;
const ID_EDIT_G_ROOT: u16 = 20;
const ID_EDIT_G_ARCHIVE: u16 = 21;
const ID_EDIT_A_GRID_SIZE: u16 = 22;
const ID_EDIT_AI_URL: u16 = 23;
const ID_EDIT_AI_MODEL: u16 = 24;
const ID_EDIT_A_GRID_ICON: u16 = 26;
const ID_EDIT_STARTUP_DELAY: u16 = 27;
const ID_EDIT_TEMPLATE_NAME: u16 = 28;
const ID_EDIT_R_MIN_SIZE: u16 = 29;
const ID_EDIT_R_MAX_SIZE: u16 = 30;
const ID_EDIT_R_NEWER: u16 = 31;
const ID_EDIT_R_OLDER: u16 = 32;
const ID_EDIT_R_REGEX: u16 = 33;
const ID_EDIT_AI_EMBED_MODEL: u16 = 35;
const ID_EDIT_R_AI_DESC: u16 = 36;
const ID_EDIT_WIDGET_NAME: u16 = 37;
const ID_EDIT_WIDGET_CODE: u16 = 38;

const ID_CHECK_ORGANIZE_FOLDERS: u16 = 101;
const ID_CHECK_ORGANIZE_START: u16 = 102;
const ID_CHECK_PUBLIC: u16 = 103;
const ID_CHECK_SHORTCUTS: u16 = 104;
const ID_CHECK_STARTUP: u16 = 105;
const ID_CHECK_ZEN_DBL: u16 = 111;
const ID_CHECK_ZEN_HOTKEY: u16 = 112;
const ID_CHECK_ZEN_HIDE: u16 = 113;
const ID_CHECK_ARCHIVE: u16 = 121;
const ID_CHECK_BY_MONTH: u16 = 124;
const ID_CHECK_R_ENABLED: u16 = 131;
const ID_CHECK_R_MOVE: u16 = 132;
const ID_CHECK_R_FOLDERS: u16 = 133;
const ID_CHECK_A_ICONS: u16 = 151;
const ID_CHECK_A_COUNTER: u16 = 152;
const ID_CHECK_A_SEARCH: u16 = 153;
const ID_CHECK_A_GRID: u16 = 154;
const ID_CHECK_AI_ENABLE: u16 = 160;
const ID_CHECK_AUTO_UPDATE: u16 = 161;
const ID_CHECK_WIDGET_ENABLED: u16 = 170;

const ID_BTN_NEW: u16 = 201;
const ID_BTN_DEL: u16 = 202;
const ID_BTN_UP: u16 = 203;
const ID_BTN_DOWN: u16 = 204;
const ID_BTN_OK: u16 = 205;
const ID_BTN_CANCEL: u16 = 206;
const ID_BTN_GROUP: u16 = 208;
const ID_BTN_UNGROUP: u16 = 209;
const ID_BTN_AI_PING: u16 = 210;
const ID_BTN_AI_DETECT_MODELS: u16 = 211;
const ID_BTN_AI_REORGANIZE: u16 = 212;
const ID_BTN_CHECK_UPDATES: u16 = 213;
const ID_BTN_DOWNLOAD_UPDATE: u16 = 214;
const ID_BTN_EXPORT_CFG: u16 = 215;
const ID_BTN_IMPORT_CFG: u16 = 216;
const ID_BTN_TEMPLATE_SAVE: u16 = 217;
const ID_BTN_TEMPLATE_APPLY: u16 = 218;
const ID_BTN_TEMPLATE_DEL: u16 = 219;
const ID_BTN_TEMPLATE_DEFAULT: u16 = 220;
const ID_BTN_R_AI_GEN: u16 = 221;
const ID_BTN_WIDGET_NEW: u16 = 222;
const ID_BTN_WIDGET_DEL: u16 = 223;
const ID_BTN_WIDGET_SAVE: u16 = 224;
const ID_BTN_WIDGET_RELOAD: u16 = 225;
const ID_BTN_WIDGET_ADD: u16 = 226;
const ID_BTN_WIDGET_REMOVE: u16 = 227;

// ---------------------------------------------------------------------------
// Resultados asíncronos (hilo de trabajo -> hilo de UI del diálogo).
// Las llamadas de red (update, Ollama) NUNCA se hacen en el hilo de UI: un
// hilo de trabajo las ejecuta, guarda el resultado en un static y avisa con
// PostMessageW; dlg_proc consume el resultado cuando llega el mensaje.
// ---------------------------------------------------------------------------
const WM_AI_PING_DONE: u32 = WM_APP + 0x41;
const WM_AI_MODELS_DONE: u32 = WM_APP + 0x42;
const WM_AI_CLUSTER_DONE: u32 = WM_APP + 0x43;
const WM_UPDATE_DONE: u32 = WM_APP + 0x44;
const WM_AI_RULE_DONE: u32 = WM_APP + 0x45;

static AI_PING_RESULT: Mutex<Option<bool>> = Mutex::new(None);
static AI_MODELS_RESULT: Mutex<Option<Vec<String>>> = Mutex::new(None);
static AI_CLUSTER_RESULT: Mutex<Option<Vec<crate::ai::AiSuggestedRule>>> = Mutex::new(None);
static AI_RULE_RESULT: Mutex<Option<crate::ai::AiRuleDraft>> = Mutex::new(None);
static UPDATE_RESULT: Mutex<Option<crate::updater::UpdateStatus>> = Mutex::new(None);

/// Guarda anti doble-clic: evita lanzar dos hilos de red a la vez por accion.
static AI_BUSY: AtomicBool = AtomicBool::new(false);
static UPDATE_BUSY: AtomicBool = AtomicBool::new(false);

/// Timer de animacion del spinner mientras hay una operacion de red en curso.
const BUSY_TIMER_ID: usize = 0x4E5;
/// Timer que aplaza la reconstruccion de la vista previa para coalescer
/// rafagas de cambios (tecleo + blur, doble clic, toggles rapidos).
const PREVIEW_TIMER_ID: usize = 0x4E6;
const PREVIEW_DEBOUNCE_MS: u32 = 200;

const ALL_EDITS: [u16; 36] = [
    ID_EDIT_MAX_AGE, ID_EDIT_MIN_AGE, ID_EDIT_PURGE, ID_EDIT_R_TITLE, ID_EDIT_R_FOLDER,
    ID_EDIT_R_COLOR, ID_EDIT_R_EXTS, ID_EDIT_R_PATTERNS, ID_EDIT_R_GROUP_TITLE, ID_EDIT_A_BG, ID_EDIT_A_HOVER,
    ID_EDIT_A_BORDER, ID_EDIT_A_TITLE, ID_EDIT_A_TEXT, ID_EDIT_A_MUTED, ID_EDIT_A_SHADOW,
    ID_EDIT_A_RADIUS, ID_EDIT_A_TITLE_SIZE, ID_EDIT_A_TEXT_SIZE, ID_EDIT_A_SNAP,
    ID_EDIT_G_ROOT, ID_EDIT_G_ARCHIVE,
    ID_EDIT_A_GRID_SIZE, ID_EDIT_AI_URL, ID_EDIT_AI_MODEL, ID_EDIT_A_GRID_ICON,
    ID_EDIT_STARTUP_DELAY,
    ID_EDIT_TEMPLATE_NAME,
    ID_EDIT_R_MIN_SIZE, ID_EDIT_R_MAX_SIZE, ID_EDIT_R_NEWER, ID_EDIT_R_OLDER, ID_EDIT_R_REGEX,
    ID_EDIT_AI_EMBED_MODEL, ID_EDIT_R_AI_DESC, ID_EDIT_WIDGET_NAME,
];

// Valores de teclas para patrones de match (las constantes VK_* no son
// evaluables como patrones, pero sus valores si lo son via const).
const KEY_ESC: u32 = VK_ESCAPE.0 as u32;
const KEY_ENTER: u32 = VK_RETURN.0 as u32;
const KEY_SPACE: u32 = VK_SPACE.0 as u32;
const KEY_UP: u32 = VK_UP.0 as u32;
const KEY_DOWN: u32 = VK_DOWN.0 as u32;
const KEY_LEFT: u32 = VK_LEFT.0 as u32;
const KEY_RIGHT: u32 = VK_RIGHT.0 as u32;

// ---------------------------------------------------------------------------
// Paleta (coincide con la UI de las cajas)
// ---------------------------------------------------------------------------

const C_BG: &str = "#0D1220";
const C_SIDEBAR: &str = "#0A0F1A";
const C_CARD: &str = "#131A2B";
const C_CARD_BORDER: &str = "#26324E";
const C_FIELD: &str = "#1B2436";
const C_FIELD_BORDER: &str = "#3A4A6B";
const C_FIELD_FOCUS: &str = "#38BDF8";
const C_TEXT: &str = "#E6EDF7";
const C_MUTED: &str = "#8FA6C4";
const C_DIM: &str = "#5B6B8C";
const C_ACCENT: &str = "#38BDF8";
const C_ON_ACCENT: &str = "#08131F";
const C_HOVER: &str = "#FFFFFF12";
const C_ACTIVE: &str = "#38BDF82E";
const C_DANGER: &str = "#F472B6";

/// Estilo visual de los botones de la barra inferior.
#[derive(Clone, Copy, PartialEq)]
enum BtnKind {
    /// Secundario silencioso: fondo transparente y borde tenue (Cancelar).
    Ghost,
    /// Accion principal: relleno de acento solido con sombra (Guardar).
    Primary,
}

fn col(hex: &str) -> D2D1_COLOR_F {
    let (r, g, b, a) = parse_color(hex);
    D2D1_COLOR_F { r, g, b, a }
}

fn rgba(hex: &str, a: f32) -> D2D1_COLOR_F {
    let (r, g, b, _) = parse_color(hex);
    D2D1_COLOR_F { r, g, b, a }
}

// ---------------------------------------------------------------------------
// Controles dibujados
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Panel {
    General,
    Rules,
    Appearance,
    Language,
    Ai,
    Updates,
    Widgets,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctrl {
    Nav(Panel),
    Check(u16),
    Field(u16),
    Swatch(u16),
    RuleRow(usize),
    RuleSort(usize),
    RuleView(usize, &'static str),
    RuleIcon(usize),
    Btn(u16),
    Folder(u16),
    Close,
    Minimize,
    Scroll,
    Picker(PickerPart),
    Lang(Lang),
    Template(usize),
    Theme(&'static str),
    WidgetRow(usize),
    None,
}

/// Partes pulsables del selector de color visual (paleta HSB).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickerPart {
    Sv,
    Hue,
    Close,
    Ok,
}

#[derive(Clone, Copy)]
struct Region {
    ctrl: Ctrl,
    r: D2D_RECT_F,
}

// ---------------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------------

/// Rectangulo de diseno (DIP): esquina superior izquierda + tamano.
#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct Settings {
    cfg: Config,
    /// Configuracion tal y como estaba al abrir el dialogo: permite revertir
    /// la vista previa en vivo al cancelar o cerrar sin guardar.
    original_cfg: Config,
    hwnd: HWND,
    scale: f32,
    // Tamaño del lienzo en DIPs (independiente del DPI). Los metodos de
    // layout dibujan en DIPs; la conversion a pixeles ocurre solo en
    // Resize/SetWindowPos/CreateWindowExW y en el hit-testing del raton.
    size: (f32, f32),

    factory: ID2D1Factory,
    // Los recursos que dependen del hwnd se crean tras `CreateWindowExW`;
    // `Option` evita inicializadores invalidos (las interfaces COM no
    // permiten zero-init) y los helpers hacen unwrap en punto de uso.
    target: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    fmt_title: Option<IDWriteTextFormat>,
    fmt_body: Option<IDWriteTextFormat>,
    fmt_small: Option<IDWriteTextFormat>,
    stroke: Option<ID2D1StrokeStyle>,

    // Idioma activo del dialogo (el selector lo cambia en caliente y al
    // guardar se persiste en la configuracion).
    lang: Lang,
    tr: &'static Tr,

    panel: Panel,
    checks: HashMap<u16, bool>,
    edits: HashMap<u16, HWND>,
    edits_shown: Vec<u16>,
    // Anti-parpadeo: solo se reposiciona un EDIT cuando su rect cambia de
    // verdad, y la visibilidad solo cambia en transiciones (no en cada frame).
    edit_rects: HashMap<u16, (i32, i32, i32, i32)>,
    edit_visible: HashSet<u16>,
    /// Rects deseados por campo en el frame actual: se aplican en
    /// `sync_edits_post` tras el present D2D, para no mover ventanas hijas
    /// durante el dibujado (deja residuos negros al hacer scroll).
    edit_next_rects: HashMap<u16, (i32, i32, i32, i32)>,

    regions: Vec<Region>,
    focus_order: Vec<Ctrl>,
    focus: Option<usize>,
    hover: Option<Ctrl>,
    pressed: Option<Ctrl>,
    // Selector de color abierto (None = cerrado).
    picker: Option<PickerState>,
    // Ultimo tono usado: los colores neutros (sin saturacion) reabren el
    // selector en ese tono en vez de saltar al rojo.
    last_hue: f32,
    rules_selected: Option<usize>,
    rules_scroll: usize,
    /// Orden por regla (indice en cfg.rules -> sort_mode).
    rules_sort: std::collections::HashMap<usize, Option<String>>,

    // Widgets (Lua)
    widgets_dir: PathBuf,
    widgets_list: Vec<String>,
    widgets_selected: Option<usize>,
    widgets_scroll: usize,

    // Desplazamiento vertical del contenido (ventana redimensionable).
    scroll: f32,
    scroll_max: f32,
    // Area de contenido sin desplazar (para ocultar EDITs fuera de vista).
    content_rect: (f32, f32, f32, f32),
    // Rect (en coords dibujadas) de la lista de reglas: la rueda sobre ella
    // desplaza la lista interna en vez del panel.
    list_rect: Option<D2D_RECT_F>,
    // Puntero a la app para "Aplicar" (mismo hilo, dialogo modal).
    app: *mut App,

    finished: bool,
    result: bool,
    /// Suprime los MessageBoxW de aviso durante la recogida silenciosa (vista
    /// previa en vivo): un campo a medio editar no debe interrumpir al usuario.
    suppress_warnings: bool,
    /// true si hay cambios en la vista previa aun no persistidos con Guardar.
    dirty: bool,
    target_px_size: (u32, u32),
    // Fase de animacion del spinner (0..1) y si el timer de repintado esta vivo.
    spinner_phase: f32,
    busy_timer: bool,
}

const HEADER_H: f32 = 48.0;
const SIDEBAR_W: f32 = 172.0;
const BAR_H: f32 = 60.0;

/// Seleccion de tipografia para `text` (evita prestar `&self.fmt_*` en el
/// call site, que colisionaria con el `&mut self` del propio metodo).
#[derive(Clone, Copy)]
enum Fmt {
    Title,
    Body,
    Small,
}

// ---------------------------------------------------------------------------
// Primitivas de dibujo
// ---------------------------------------------------------------------------

impl Settings {
    fn target(&self) -> &ID2D1HwndRenderTarget {
        self.target.as_ref().expect("target sin inicializar")
    }

    fn brush(&self) -> &ID2D1SolidColorBrush {
        self.brush.as_ref().expect("brush sin inicializar")
    }

    fn stroke(&self) -> &ID2D1StrokeStyle {
        self.stroke.as_ref().expect("stroke sin inicializar")
    }

    fn set(&mut self, color: D2D1_COLOR_F) {
        unsafe { self.brush().SetColor(&color) };
    }

    fn rr(&self, x: f32, y: f32, w: f32, h: f32, r: f32) -> D2D1_ROUNDED_RECT {
        D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: r,
            radiusY: r,
        }
    }

    fn fill_rr(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: D2D1_COLOR_F) {
        self.set(color);
        unsafe {
            self.target()
                .FillRoundedRectangle(&self.rr(x, y, w, h, r), self.brush());
        }
    }

    fn draw_rr(&mut self, rect: Rect, r: f32, color: D2D1_COLOR_F, width: f32) {
        let Rect { x, y, w, h } = rect;
        self.set(color);
        unsafe {
            self.target()
                .DrawRoundedRectangle(&self.rr(x, y, w, h, r), self.brush(), width, None);
        }
    }

    fn text(&mut self, s: &str, fmt: Fmt, r: D2D_RECT_F, color: D2D1_COLOR_F) {
        // Clonar el formato: la interfaz COM es un contador de refs barato y
        // evita retener un borrow inmutable de self al llamar a `set`/DrawText.
        let format = match fmt {
            Fmt::Title => self.fmt_title.as_ref().expect("fmt_title").clone(),
            Fmt::Body => self.fmt_body.as_ref().expect("fmt_body").clone(),
            Fmt::Small => self.fmt_small.as_ref().expect("fmt_small").clone(),
        };
        self.set(color);
        let text = wide(s);
        unsafe {
            self.target().DrawText(
                &text,
                &format,
                &r,
                self.brush(),
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT | windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: D2D1_COLOR_F, width: f32) {
        self.set(color);
        unsafe {
            self.target().DrawLine(
                D2D_POINT_2F { x: x1, y: y1 },
                D2D_POINT_2F { x: x2, y: y2 },
                self.brush(),
                width,
                Some(self.stroke()),
            );
        }
    }

    /// Icono de cadena: dos eslabones entrelazados en diagonal (regla agrupada).
    fn chain_icon(&mut self, x: f32, y: f32, color: D2D1_COLOR_F) {
        let w = 8.0;
        let h = 5.0;
        let r = 2.5;
        self.draw_rr(Rect { x, y, w, h }, r, color, 1.3);
        self.draw_rr(Rect { x: x + 3.6, y: y + 3.6, w, h }, r, color, 1.3);
    }

    fn add_region(&mut self, ctrl: Ctrl, r: D2D_RECT_F) {
        self.regions.push(Region { ctrl, r });
    }

    fn hit(&self, x: f32, y: f32) -> Option<Ctrl> {
        self.regions
            .iter()
            .rev()
            .find(|reg| x >= reg.r.left && x <= reg.r.right && y >= reg.r.top && y <= reg.r.bottom)
            .map(|reg| reg.ctrl)
    }
}

// ---------------------------------------------------------------------------
// Layout y dibujo de paneles
// ---------------------------------------------------------------------------

impl Settings {
    fn content_area(&self) -> (f32, f32, f32, f32) {
        (
            SIDEBAR_W + 16.0,
            HEADER_H + 12.0,
            self.size.0 - SIDEBAR_W - 32.0,
            self.size.1 - HEADER_H - BAR_H - 24.0,
        )
    }

    /// Fondo del panel de contenido (una sola hoja con borde).
    fn panel_bg(&mut self) {
        let (cx, cy, cw, ch) = self.content_area();
        self.fill_rr(cx, cy, cw, ch, 14.0, col(C_CARD));
        self.draw_rr(Rect { x: cx, y: cy, w: cw, h: ch }, 14.0, rgba(C_CARD_BORDER, 0.7), 1.0);
    }

    /// Titulo de seccion con barra de acento; devuelve el siguiente y.
    fn section(&mut self, y: f32, cx: f32, cw: f32, title: &str) -> f32 {
        self.fill_rr(cx + 16.0, y + 4.0, 3.0, 14.0, 1.5, col(C_ACCENT));
        self.text(
            title,
            Fmt::Small,
            D2D_RECT_F {
                left: cx + 26.0,
                top: y,
                right: cx + cw - 16.0,
                bottom: y + 22.0,
            },
            col(C_DIM),
        );
        y + 26.0
    }

    fn check(&mut self, y: f32, cx: f32, cw: f32, id: u16, label: &str) -> f32 {
        self.checkbox(Rect { x: cx + 16.0, y, w: cw - 32.0, h: 28.0 }, id, label, self.checked(id));
        y + 28.0
    }

    fn check_inline(&mut self, y: f32, x0: f32, x1: f32, id: u16, label: &str) {
        self.checkbox(Rect { x: x0 + 8.0, y, w: (x1 - x0) - 12.0, h: 26.0 }, id, label, self.checked(id));
    }

    fn checked(&self, id: u16) -> bool {
        self.checks.get(&id).copied().unwrap_or(false)
    }

    fn checkbox(&mut self, rect: Rect, id: u16, label: &str, on: bool) {
        let Rect { x, y, w, h } = rect;
        let sw_w = 34.0;
        let sw_h = 18.0;
        let bx = x;
        let by = y + (h - sw_h) / 2.0;
        let over = self.hover == Some(Ctrl::Check(id)) || self.focused(Ctrl::Check(id));

        let track_bg = if on {
            col(C_ACCENT)
        } else if over {
            rgba(C_FIELD_BORDER, 0.9)
        } else {
            col(C_FIELD)
        };
        self.fill_rr(bx, by, sw_w, sw_h, sw_h * 0.5, track_bg);
        self.draw_rr(
            Rect { x: bx, y: by, w: sw_w, h: sw_h }, sw_h * 0.5,
            if on { col(C_ACCENT) } else { rgba(C_FIELD_BORDER, 0.8) },
            1.0,
        );

        let dot_size = 12.0;
        let dot_x = if on { bx + sw_w - dot_size - 3.0 } else { bx + 3.0 };
        let dot_y = by + (sw_h - dot_size) / 2.0;
        let dot_color = if on { col(C_ON_ACCENT) } else { col(C_MUTED) };
        self.fill_rr(dot_x, dot_y, dot_size, dot_size, dot_size * 0.5, dot_color);

        self.text(
            label,
            Fmt::Body,
            D2D_RECT_F {
                left: bx + sw_w + 12.0,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            col(C_TEXT),
        );
        self.add_region(
            Ctrl::Check(id),
            D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
        );
    }

    fn field_row(&mut self, y: f32, content: (f32, f32), id: u16, label: &str, value: &str, swatch: bool) -> f32 {
        let (cx, cw) = content;
        let label_w = 148.0;
        let field_w = (cw - 32.0 - label_w - if swatch { 40.0 } else { 0.0 }).max(90.0);
        self.field_at((cx + 16.0, y), (label_w, field_w), id, label, value, swatch);
        y + 38.0
    }

    /// Fila numerica compacta: etiqueta + campo estrecho (2-3 digitos) para
    /// valores como dias o minutos, sin el hueco gigante de antes.
    fn num_row(&mut self, y: f32, cx: f32, id: u16, label: &str, value: &str) -> f32 {
        self.field_at((cx + 16.0, y), (148.0, 84.0), id, label, value, false);
        y + 38.0
    }

    /// Fila de carpeta: campo de texto + boton de examinar (selector nativo).
    fn folder_row(&mut self, y: f32, cx: f32, cw: f32, id: u16, label: &str, value: &str) -> f32 {
        let total = (430.0_f32).min(cw - 32.0);
        self.field_at((cx + 16.0, y), (148.0, total - 148.0), id, label, value, false);
        let fw = (total - 148.0).max(90.0);
        let bx = cx + 16.0 + 148.0 + fw + 6.0;
        let over = self.hover == Some(Ctrl::Folder(id));
        self.fill_rr(bx, y, 36.0, 26.0, 7.0, if over { col(C_HOVER) } else { col(C_FIELD) });
        self.draw_rr(
            Rect { x: bx, y, w: 36.0, h: 26.0 },
            7.0,
            if over { rgba(C_ACCENT, 0.6) } else { col(C_FIELD_BORDER) },
            1.0,
        );
        self.text(
            "…",
            Fmt::Body,
            D2D_RECT_F {
                left: bx,
                top: y + 3.0,
                right: bx + 36.0,
                bottom: y + 24.0,
            },
            if over { col(C_TEXT) } else { col(C_MUTED) },
        );
        self.add_region(
            Ctrl::Folder(id),
            D2D_RECT_F {
                left: bx,
                top: y,
                right: bx + 36.0,
                bottom: y + 26.0,
            },
        );
        y + 38.0
    }

    fn field_at(&mut self, origin: (f32, f32), dims: (f32, f32), id: u16, label: &str, value: &str, swatch: bool) {
        let (x, y) = origin;
        let (label_w, field_w) = dims;
        self.text(
            label,
            Fmt::Body,
            D2D_RECT_F {
                left: x,
                top: y + 5.0,
                right: x + label_w,
                bottom: y + 26.0,
            },
            col(C_MUTED),
        );
        let fx = x + label_w;
        let focused = self.edits_focused(id);
        self.fill_rr(fx, y, field_w, 26.0, 7.0, col(C_FIELD));
        self.draw_rr(
            Rect { x: fx, y, w: field_w, h: 26.0 },
            7.0,
            if focused { col(C_FIELD_FOCUS) } else { col(C_FIELD_BORDER) },
            1.0,
        );
        if self.picker.is_some() {
            // Con el selector de color (o el formulario de caja) abierto los
            // EDITs del panel se ocultan (los cubriria) y el valor se dibuja
            // en D2D, siempre al dia con los cambios del selector.
            let shown = self.edit_text(id).unwrap_or_else(|| value.to_string());
            self.text(
                &shown,
                Fmt::Body,
                D2D_RECT_F {
                    left: fx + 10.0,
                    top: y + 3.0,
                    right: fx + field_w - 8.0,
                    bottom: y + 24.0,
                },
                col(C_TEXT),
            );
        } else if self.edits.contains_key(&id) {
            let (_, cyt, _, chb) = self.content_rect;
            // Un campo EDIT solo es visible si su bounding box cae integro dentro del area de contenido visible.
            let field_top = y;
            let field_bottom = y + 26.0;
            if field_top >= cyt && field_bottom <= chb {
                let s = self.scale;
                let rect = (
                    ((fx + 8.0) * s) as i32,
                    ((y + 2.5) * s) as i32,
                    (((field_w - 14.0) * s).max(40.0)) as i32,
                    (21.0 * s) as i32,
                );
                // Se registra el rect y se aplica tras el present D2D
                // (sync_edits_post): mover la ventana hija durante el
                // dibujado deja residuos negros al hacer scroll.
                self.edit_next_rects.insert(id, rect);
                self.edits_shown.push(id);
            }
        }
        if swatch {
            let sx = fx + field_w + 10.0;
            let text = self.edit_text(id).unwrap_or_else(|| value.to_string());
            let valid = valid_color(&text);
            self.fill_rr(sx, y + 3.0, 20.0, 20.0, 5.0, if valid { col(&text) } else { col(C_FIELD) });
            self.draw_rr(Rect { x: sx, y: y + 3.0, w: 20.0, h: 20.0 }, 5.0, if valid { rgba(&text, 0.9) } else { rgba(C_DANGER, 0.8) }, 1.0);
            self.add_region(
                Ctrl::Swatch(id),
                D2D_RECT_F {
                    left: sx,
                    top: y + 3.0,
                    right: sx + 20.0,
                    bottom: y + 23.0,
                },
            );
        }
        self.add_region(
            Ctrl::Field(id),
            D2D_RECT_F {
                left: fx,
                top: y,
                right: fx + field_w,
                bottom: y + 26.0,
            },
        );
    }

    fn rule_row(&mut self, r: &D2D_RECT_F, idx: usize) {
        let selected = self.rules_selected == Some(idx);
        let over = self.hover == Some(Ctrl::RuleRow(idx));
        let enabled = self.cfg.rules[idx].enabled;
        // Regla agrupada: se sangra y se dibuja un conector tipo arbol para
        // dejar claro que pertenece a una caja de pestanas.
        let is_grouped = self.cfg.is_grouped(self.cfg.rules[idx].id.as_str());
        let group_label = if is_grouped {
            self.cfg
                .group_of(&self.cfg.rules[idx].id)
                .and_then(|f| f.group_title.clone())
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        };
        let indent = if is_grouped { 14.0 } else { 0.0 };
        let bg = if selected { col(C_ACTIVE) } else if over { col(C_HOVER) } else { col("#131B2E") };
        self.fill_rr(r.left, r.top, r.right - r.left, r.bottom - r.top, 8.0, bg);
        if selected {
            self.draw_rr(Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }, 8.0, rgba(C_ACCENT, 0.5), 1.0);
        }
        if is_grouped {
            // Cadena de eslabones: indica que la regla esta unida a un grupo.
            let cy = r.top + (r.bottom - r.top) * 0.5;
            self.chain_icon(r.left + 6.0, cy - 4.3, col(C_MUTED));
        }
        let color = if valid_color(&self.cfg.rules[idx].color) {
            col(&self.cfg.rules[idx].color)
        } else {
            col(C_DIM)
        };
        self.fill_rr(r.left + 10.0 + indent, r.top + (r.bottom - r.top - 10.0) / 2.0, 10.0, 10.0, 5.0, color);
        self.text(
            &self.cfg.rules[idx].title.clone(),
            Fmt::Body,
            D2D_RECT_F {
                left: r.left + 28.0 + indent,
                top: r.top + 2.0,
                right: r.right - 110.0,
                bottom: r.top + 20.0,
            },
            if enabled { col(C_TEXT) } else { col(C_DIM) },
        );
        let rule = &self.cfg.rules[idx];
        let action = if rule.move_files {
            self.tr.rule_action_move
        } else {
            self.tr.rule_action_list
        };
        let folders = if rule.include_folders {
            self.tr.rule_suffix_folders
        } else {
            ""
        };
        let sub = format!(
            "{} {} · {}{}",
            rule.extensions.len(),
            self.tr.rule_sub_ext,
            action,
            folders,
        );
        let sub = match &group_label {
            Some(g) => format!("{sub} · {g}"),
            None => sub,
        };
        // El subtitulo (que incluye el nombre del grupo) se recorta antes de la
        // pastilla de orden (regla activa) o de la etiqueta "desactivada", para
        // que el nombre del grupo sea siempre visible en ambos casos.
        let sub_right = if enabled {
            r.right - 72.0
        } else {
            r.right - 100.0
        };
        self.text(
            &sub,
            Fmt::Small,
            D2D_RECT_F {
                left: r.left + 28.0 + indent,
                top: r.top + 16.0,
                right: sub_right,
                bottom: r.top + 30.0,
            },
            col(C_DIM),
        );
        if !enabled {
            self.text(
                self.tr.rule_disabled,
                Fmt::Small,
                D2D_RECT_F {
                    left: r.right - 92.0,
                    top: r.top + 7.0,
                    right: r.right - 8.0,
                    bottom: r.top + 25.0,
                },
                col(C_DANGER),
            );
        }
        // Selector de orden (pastilla a la derecha de la fila).
        // Solo se muestra si la regla esta activa; si no, se oculta para no
        // solaparse con la etiqueta "desactivada".
        if enabled {
            let mode = self.rules_sort.get(&idx).and_then(|m| m.as_deref());
            let label = if let Some(m) = mode {
                crate::rules::sort_label(m)
            } else {
                "Global"
            };
            let pw = 56.0;
            let px = r.right - pw - 8.0;
            let over = self.hover == Some(Ctrl::RuleSort(idx));
            let bg = if over { col(C_FIELD) } else { col("#00000000") };
            self.fill_rr(px, r.top + 4.0, pw, r.bottom - r.top - 8.0, 6.0, bg);
            self.draw_rr(Rect { x: px, y: r.top + 4.0, w: pw, h: r.bottom - r.top - 8.0 }, 6.0, rgba(C_FIELD_BORDER, if over { 0.9 } else { 0.5 }), 1.0);
            self.text(
                label,
                Fmt::Small,
                D2D_RECT_F {
                    left: px + 4.0,
                    top: r.top + 5.0,
                    right: px + pw - 4.0,
                    bottom: r.bottom - 4.0,
                },
                if over { col(C_TEXT) } else { col(C_MUTED) },
            );
            self.add_region(
                Ctrl::RuleSort(idx),
                D2D_RECT_F {
                    left: px,
                    top: r.top + 4.0,
                    right: px + pw,
                    bottom: r.bottom - 4.0,
                },
            );
        }
        self.add_region(Ctrl::RuleRow(idx), *r);
    }

    fn icon_button(&mut self, rect: Rect, label: &str, id: u16, enabled: bool) {
        let Rect { x, y, w, h } = rect;
        let over = self.hover == Some(Ctrl::Btn(id));
        let pressed = self.pressed == Some(Ctrl::Btn(id));
        let bg = if pressed && enabled {
            rgba(C_ACCENT, 0.25)
        } else if over && enabled {
            col(C_HOVER)
        } else {
            col("#00000000")
        };
        self.fill_rr(x, y, w, h, 7.0, bg);
        self.draw_rr(Rect { x, y, w, h }, 7.0, rgba(C_FIELD_BORDER, 0.8), 1.0);
        self.text(
            label,
            Fmt::Small,
            D2D_RECT_F {
                left: x + 4.0,
                top: y + 4.0,
                right: x + w - 4.0,
                bottom: y + h - 2.0,
            },
            if enabled { col(C_TEXT) } else { col(C_DIM) },
        );
        self.add_region(
            Ctrl::Btn(id),
            D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
        );
    }

    /// Dibuja un spinner de carga (anillo de segmentos girando) centrado en
    /// (cx, cy). `phase` (0..1) anima la posicion del segmento mas brillante.
    fn spinner(&mut self, cx: f32, cy: f32, r: f32, phase: f32) {
        const SEGS: usize = 8;
        let tau = std::f32::consts::TAU;
        for i in 0..SEGS {
            let t = i as f32 / SEGS as f32;
            let mut d = t - phase;
            d -= d.floor(); // distancia angular normalizada al segmento activo
            let alpha = 1.0 - d; // el que va "detras" se atenua
            let a = t * tau;
            let (sin, cos) = a.sin_cos();
            self.line(
                cx + cos * r * 0.5,
                cy + sin * r * 0.5,
                cx + cos * r,
                cy + sin * r,
                D2D1_COLOR_F { r: 0.49, g: 0.83, b: 1.0, a: alpha },
                2.0,
            );
        }
    }

    /// Arranca/para el timer que anima el spinner segun haya operaciones de
    /// red en curso (AI_BUSY / UPDATE_BUSY). Solo se toca en transiciones.
    fn sync_busy_timer(&mut self) {
        let busy = AI_BUSY.load(Ordering::SeqCst) || UPDATE_BUSY.load(Ordering::SeqCst);
        unsafe {
            if busy && !self.busy_timer {
                let _ = SetTimer(self.hwnd, BUSY_TIMER_ID, 80, None);
                self.busy_timer = true;
            } else if !busy && self.busy_timer {
                let _ = KillTimer(self.hwnd, BUSY_TIMER_ID);
                self.busy_timer = false;
                self.spinner_phase = 0.0;
            }
        }
    }

    fn push_button(&mut self, rect: Rect, label: &str, id: u16, kind: BtnKind) {
        let Rect { x, y, w, h } = rect;
        let over = self.hover == Some(Ctrl::Btn(id));
        let pressed = self.pressed == Some(Ctrl::Btn(id));
        let focused = self.focused(Ctrl::Btn(id));
        let r = 10.0;
        let bw = w.max(96.0);

        // Sombra de elevacion compartida (todos los botones proyectan sombra sutil).
        if !pressed {
            self.fill_rr(x + 1.0, y + 2.0, bw, h, r + 1.0, rgba("#000000", 0.25));
        }
        let (bg, border, text_color) = match kind {
            BtnKind::Ghost => (
                if pressed {
                    rgba(C_HOVER, 0.9)
                } else if over {
                    rgba(C_HOVER, 0.7)
                } else {
                    rgba("#000000", 0.0)
                },
                if over { rgba(C_ACCENT, 0.5) } else { rgba(C_FIELD_BORDER, 0.9) },
                if over { col(C_TEXT) } else { col(C_MUTED) },
            ),
            BtnKind::Primary => (
                if pressed {
                    rgba(C_ACCENT, 0.72)
                } else if over {
                    rgba("#5CC8FA", 0.95)
                } else {
                    col(C_ACCENT)
                },
                rgba("#000000", 0.0),
                col(C_ON_ACCENT),
            ),
        };
        self.fill_rr(x, y, w, h, r, bg);
        if kind != BtnKind::Primary {
            self.draw_rr(Rect { x, y, w: bw, h }, r, border, 1.2);
        }
        // Brillo superior sutil (sensacion de cristal).
        if !pressed {
            self.fill_rr(x + 1.5, y + 1.5, bw - 3.0, (h * 0.35).max(3.0), r - 2.0, rgba("#FFFFFF", if over { 0.15 } else { 0.08 }));
        }
        if focused {
            self.draw_rr(Rect { x: x - 1.5, y: y - 1.5, w: w + 3.0, h: h + 3.0 }, r + 1.0, rgba(C_ACCENT, 0.8), 1.5);
        }
        let ty = if pressed { y + 7.0 } else { y + 5.0 };
        self.text(
            label,
            Fmt::Body,
            D2D_RECT_F {
                left: x + 4.0,
                top: ty,
                right: x + w - 4.0,
                bottom: y + h - 3.0,
            },
            text_color,
        );
        self.add_region(
            Ctrl::Btn(id),
            D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Paneles
// ---------------------------------------------------------------------------

impl Settings {
    fn panel_general(&mut self, cy: f32) {
        let (cx, _, cw, _) = self.content_area();
        let mut y = cy + 10.0;
        y = self.section(y, cx, cw, self.tr.sec_organize);
        for (id, label) in [
            (ID_CHECK_ORGANIZE_FOLDERS, self.tr.chk_organize_folders),
            (ID_CHECK_ORGANIZE_START, self.tr.chk_organize_start),
            (ID_CHECK_PUBLIC, self.tr.chk_public),
            (ID_CHECK_SHORTCUTS, self.tr.chk_shortcuts),
            (ID_CHECK_STARTUP, self.tr.chk_startup),
        ] {
            y = self.check(y, cx, cw, id, label);
        }
        // Retraso antes de mostrar las cajas al arrancar (0 = inmediato).
        y = self.num_row(y, cx, ID_EDIT_STARTUP_DELAY, self.tr.fld_startup_delay, &format!("{}", self.cfg.general.startup_delay_seconds));
        // Carpeta donde se guardan los elementos organizados (cajas fisicas).
        let root_folder = self.cfg.general.root_folder.clone();
        y = self.folder_row(
            y,
            cx,
            cw,
            ID_EDIT_G_ROOT,
            self.tr.fld_root_folder,
            &root_folder,
        );
        y += 10.0;
        y = self.section(y, cx, cw, self.tr.sec_zen);
        for (id, label) in [
            (ID_CHECK_ZEN_DBL, self.tr.chk_zen_dbl),
            (ID_CHECK_ZEN_HOTKEY, self.tr.chk_zen_hotkey),
            (ID_CHECK_ZEN_HIDE, self.tr.chk_zen_hide),
        ] {
            y = self.check(y, cx, cw, id, label);
        }
        y += 10.0;
        y = self.section(y, cx, cw, self.tr.sec_archive);
        y = self.check(y, cx, cw, ID_CHECK_ARCHIVE, self.tr.chk_archive);
        // Carpeta de archivo historico.
        let archive_folder = self.cfg.general.archive_folder.clone();
        y = self.folder_row(
            y,
            cx,
            cw,
            ID_EDIT_G_ARCHIVE,
            self.tr.fld_archive_folder,
            &archive_folder,
        );
        y = self.num_row(y, cx, ID_EDIT_MAX_AGE, self.tr.fld_max_age, &format!("{}", self.cfg.ephemeral.max_age_days));
        y = self.num_row(y, cx, ID_EDIT_MIN_AGE, self.tr.fld_min_age, &format!("{}", self.cfg.ephemeral.min_age_minutes));
        y = self.check(y, cx, cw, ID_CHECK_BY_MONTH, self.tr.chk_by_month);
        y = self.num_row(y, cx, ID_EDIT_PURGE, self.tr.fld_purge, &format!("{}", self.cfg.ephemeral.purge_archive_after_days));
        y += 10.0;
        y = self.section(y, cx, cw, self.tr.sec_backup);
        let bx0 = cx + 16.0;
        self.icon_button(Rect { x: bx0, y, w: 220.0, h: 32.0 }, self.tr.btn_export_cfg, ID_BTN_EXPORT_CFG, true);
        self.icon_button(Rect { x: bx0 + 230.0, y, w: 220.0, h: 32.0 }, self.tr.btn_import_cfg, ID_BTN_IMPORT_CFG, true);
        y += 42.0;
        y = self.section(y, cx, cw, self.tr.sec_templates);
        // Nombre de la plantilla a guardar/aplicar/borrar.
        y = self.field_row(y, (cx, cw), ID_EDIT_TEMPLATE_NAME, self.tr.fld_template_name, "default", false);
        let bx1 = cx + 16.0;
        self.icon_button(Rect { x: bx1, y, w: 130.0, h: 30.0 }, self.tr.btn_template_save, ID_BTN_TEMPLATE_SAVE, true);
        self.icon_button(Rect { x: bx1 + 140.0, y, w: 130.0, h: 30.0 }, self.tr.btn_template_apply, ID_BTN_TEMPLATE_APPLY, true);
        self.icon_button(Rect { x: bx1 + 280.0, y, w: 130.0, h: 30.0 }, self.tr.btn_template_del, ID_BTN_TEMPLATE_DEL, !self.cfg.templates.is_empty());
        self.icon_button(Rect { x: bx1 + 420.0, y, w: 130.0, h: 30.0 }, self.tr.btn_template_default, ID_BTN_TEMPLATE_DEFAULT, !self.cfg.templates.is_empty());
        y += 40.0;
        // Chips de las plantillas guardadas: clic = aplicar. Los nombres se
        // clonan antes de dibujar para evitar el doble borrow con self.
        let template_names: Vec<String> = self.cfg.templates.iter().map(|t| t.name.clone()).collect();
        if !template_names.is_empty() {
            let chip_gap = 8.0;
            let x0 = cx + 16.0;
            let chip_w = (cw - 32.0 - 2.0 * chip_gap) / 3.0;
            for (i, name) in template_names.iter().enumerate() {
                let col = i % 3;
                let row = i / 3;
                let is_default = self.cfg.templates.get(i).is_some_and(|t| t.default);
                self.template_chip(
                    Rect {
                        x: x0 + col as f32 * (chip_w + chip_gap),
                        y: y + row as f32 * 34.0,
                        w: chip_w,
                        h: 26.0,
                    },
                    i,
                    name,
                    is_default,
                );
            }
        }
    }

    /// Chip de plantilla guardada (clic = aplicar y rellenar el campo nombre).
    /// La plantilla por defecto se resalta con borde de acento y una estrella.
    fn template_chip(&mut self, r: Rect, idx: usize, name: &str, is_default: bool) {
        let over = self.hover == Some(Ctrl::Template(idx));
        let bg = if over { col(C_HOVER) } else { col("#00000000") };
        self.fill_rr(r.x, r.y, r.w, r.h, 7.0, bg);
        if is_default {
            self.draw_rr(r, 7.0, rgba(C_ACCENT, 0.85), 1.5);
        } else {
            self.draw_rr(r, 7.0, rgba(C_FIELD_BORDER, 0.6), 1.0);
        }
        let label = if is_default {
            format!("★ {name}")
        } else {
            name.to_string()
        };
        self.text(
            &label,
            Fmt::Small,
            D2D_RECT_F {
                left: r.x + 4.0,
                top: r.y + 4.0,
                right: r.x + r.w - 4.0,
                bottom: r.y + r.h - 2.0,
            },
            if is_default { col(C_ACCENT) } else { col(C_MUTED) },
        );
        self.add_region(
            Ctrl::Template(idx),
            D2D_RECT_F {
                left: r.x,
                top: r.y,
                right: r.x + r.w,
                bottom: r.y + r.h,
            },
        );
    }

    /// Card de idioma con insignia/bandera visual dibujada con Direct2D.
    fn lang_card(&mut self, x: f32, y: f32, w: f32, h: f32, lang: Lang) {
        let selected = self.lang == lang;
        let over = self.hover == Some(Ctrl::Lang(lang));
        let bg = if selected {
            col(C_ACTIVE)
        } else if over {
            col(C_HOVER)
        } else {
            col(C_FIELD)
        };
        self.fill_rr(x, y, w, h, 10.0, bg);
        self.draw_rr(
            Rect { x, y, w, h },
            10.0,
            if selected { rgba(C_ACCENT, 0.85) } else { rgba(C_FIELD_BORDER, 0.6) },
            if selected { 1.5 } else { 1.0 },
        );

        // Insignia de Bandera Visual Dibujada en D2D
        let bx = x + 12.0;
        let by = y + (h - 20.0) / 2.0;
        let (bw, bh) = (30.0, 20.0);

        match lang {
            Lang::Es => {
                // España: Rojo - Amarillo - Rojo
                self.fill_rr(bx, by, bw, bh, 3.0, col("#AA151B"));
                self.fill_rr(bx, by + 5.0, bw, 10.0, 0.0, col("#F1BF00"));
            }
            Lang::En => {
                // Reino Unido / Union Jack: Azul fondo, diagonales blancas/rojas y cruz central
                self.fill_rr(bx, by, bw, bh, 3.0, col("#00247D"));
                // Diagonales (St Andrew / St Patrick)
                self.line(bx, by, bx + bw, by + bh, col("#FFFFFF"), 3.5);
                self.line(bx + bw, by, bx, by + bh, col("#FFFFFF"), 3.5);
                self.line(bx, by, bx + bw, by + bh, col("#CF142B"), 1.5);
                self.line(bx + bw, by, bx, by + bh, col("#CF142B"), 1.5);
                // Cruz de San Jorge (Borde blanco)
                self.fill_rr(bx + 11.0, by, 8.0, bh, 0.0, col("#FFFFFF"));
                self.fill_rr(bx, by + 6.0, bw, 8.0, 0.0, col("#FFFFFF"));
                // Cruz de San Jorge (Centro rojo)
                self.fill_rr(bx + 13.0, by, 4.0, bh, 0.0, col("#CF142B"));
                self.fill_rr(bx, by + 8.0, bw, 4.0, 0.0, col("#CF142B"));
            }
            Lang::De => {
                // Alemania: Negro - Rojo - Amarillo
                self.fill_rr(bx, by, bw, bh, 3.0, col("#000000"));
                self.fill_rr(bx, by + 6.6, bw, 6.7, 0.0, col("#DD0000"));
                self.fill_rr(bx, by + 13.3, bw, 6.7, 3.0, col("#FFCC00"));
            }
            Lang::Fr => {
                // Francia: Azul - Blanco - Rojo
                self.fill_rr(bx, by, bw, bh, 3.0, col("#0055A5"));
                self.fill_rr(bx + 10.0, by, 10.0, bh, 0.0, col("#FFFFFF"));
                self.fill_rr(bx + 20.0, by, 10.0, bh, 3.0, col("#EF4135"));
            }
            Lang::It => {
                // Italia: Verde - Blanco - Rojo
                self.fill_rr(bx, by, bw, bh, 3.0, col("#009246"));
                self.fill_rr(bx + 10.0, by, 10.0, bh, 0.0, col("#FFFFFF"));
                self.fill_rr(bx + 20.0, by, 10.0, bh, 3.0, col("#CE2B37"));
            }
            Lang::Pt => {
                // Portugal: Verde - Rojo
                self.fill_rr(bx, by, bw, bh, 3.0, col("#006600"));
                self.fill_rr(bx + 12.0, by, 18.0, bh, 3.0, col("#FF0000"));
                self.fill_rr(bx + 9.0, by + 6.0, 6.0, 8.0, 3.0, col("#FFFF00"));
            }
        }
        self.draw_rr(Rect { x: bx, y: by, w: bw, h: bh }, 3.0, rgba("#FFFFFF", 0.3), 1.0);

        // Texto del idioma a la derecha de la bandera dibujada
        self.text(
            lang.label(),
            Fmt::Body,
            D2D_RECT_F {
                left: bx + bw + 12.0,
                top: y + (h - 20.0) / 2.0,
                right: x + w - 12.0,
                bottom: y + h,
            },
            if selected { col(C_TEXT) } else { col(C_MUTED) },
        );
        self.add_region(
            Ctrl::Lang(lang),
            D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
        );
    }

    fn panel_language(&mut self, cy: f32) {
        let (cx, _, cw, _) = self.content_area();
        let mut y = cy + 10.0;
        y = self.section(y, cx, cw, self.tr.sec_language);

        let card_gap = 12.0;
        let x0 = cx + 16.0;
        let card_w = (cw - 32.0 - card_gap) / 2.0;
        let card_h = 44.0;

        for (i, lang) in Lang::ALL.iter().enumerate() {
            let row = i / 2;
            let col = i % 2;
            let px = x0 + col as f32 * (card_w + card_gap);
            let py = y + row as f32 * (card_h + card_gap);
            self.lang_card(px, py, card_w, card_h, *lang);
        }
    }

    fn panel_rules(&mut self, cy: f32) {
        let (cx, _, cw, ch) = self.content_area();
        let mut y = cy + 10.0;

        y = self.section(y, cx, cw, self.tr.sec_rules);
        self.text(
            self.tr.sec_sort,
            Fmt::Small,
            D2D_RECT_F {
                left: cx + 26.0,
                top: y + 2.0,
                right: cx + cw - 100.0,
                bottom: y + 18.0,
            },
            col(C_DIM),
        );
        y += 18.0;
        // Botones de gestion a la derecha del titulo.
        let bx = cx + cw - 14.0;
        let by = y - 26.0;
        let mut cur_x = bx;
        cur_x -= 30.0;
        self.icon_button(Rect { x: cur_x, y: by, w: 30.0, h: 24.0 }, "▲", ID_BTN_UP, self.rule_can_move(-1));
        cur_x -= 36.0;
        self.icon_button(Rect { x: cur_x, y: by, w: 30.0, h: 24.0 }, "▼", ID_BTN_DOWN, self.rule_can_move(1));
        cur_x -= 62.0;
        self.icon_button(Rect { x: cur_x, y: by, w: 56.0, h: 24.0 }, self.tr.btn_new, ID_BTN_NEW, true);
        cur_x -= 62.0;
        self.icon_button(Rect { x: cur_x, y: by, w: 56.0, h: 24.0 }, self.tr.btn_delete, ID_BTN_DEL, self.rules_selected.is_some());
        cur_x -= 86.0;
        self.icon_button(Rect { x: cur_x, y: by, w: 80.0, h: 24.0 }, self.tr.btn_ungroup, ID_BTN_UNGROUP, self.selected_is_grouped());
        cur_x -= 70.0;
        self.icon_button(Rect { x: cur_x, y: by, w: 64.0, h: 24.0 }, self.tr.btn_group, ID_BTN_GROUP, self.rule_can_group());

        // Zona de lista (se recuerda el rect para el scroll por rueda).
        let list_top = y + 4.0;
        let list_bottom = (list_top + 210.0).min(cy + ch - 6.0);
        self.list_rect = Some(D2D_RECT_F {
            left: cx + 14.0,
            top: list_top,
            right: cx + cw - 14.0,
            bottom: list_bottom,
        });
        let rows = D2D_RECT_F {
            left: cx + 22.0,
            top: list_top + 6.0,
            right: cx + cw - 34.0,
            bottom: list_bottom - 6.0,
        };
        self.fill_rr(cx + 14.0, list_top, cw - 28.0, list_bottom - list_top, 10.0, col("#10162A"));
        self.draw_rr(Rect { x: cx + 14.0, y: list_top, w: cw - 28.0, h: list_bottom - list_top }, 10.0, rgba(C_FIELD_BORDER, 0.6), 1.0);

        let row_h = 32.0;
        let visible = ((rows.bottom - rows.top) / row_h).floor() as usize;
        let total = self.cfg.rules.len();
        let max_scroll = total.saturating_sub(visible);
        self.rules_scroll = self.rules_scroll.min(max_scroll);
        unsafe {
            self.target().PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: rows.left.floor(),
                    top: rows.top.floor(),
                    right: rows.right.ceil(),
                    bottom: rows.bottom.ceil(),
                },
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }
        self.fill_rr(rows.left, rows.top, rows.right - rows.left, rows.bottom - rows.top, 0.0, col("#10162A"));
        for offset in 0..visible.min(total.saturating_sub(self.rules_scroll)) {
            let idx = self.rules_scroll + offset;
            let r = D2D_RECT_F {
                left: rows.left,
                top: rows.top + offset as f32 * row_h,
                right: rows.right,
                bottom: rows.top + (offset as f32 + 1.0) * row_h,
            };
            self.rule_row(&r, idx);
        }
        unsafe {
            self.target().PopAxisAlignedClip();
        }
        if total == 0 {
            self.text(
                self.tr.rules_empty,
                Fmt::Small,
                D2D_RECT_F {
                    left: rows.left + 6.0,
                    top: rows.top + 10.0,
                    right: rows.right,
                    bottom: rows.top + 30.0,
                },
                col(C_DIM),
            );
        }
        if max_scroll > 0 {
            let track_h = rows.bottom - rows.top;
            let thumb_h = (track_h * (visible as f32 / total as f32)).max(24.0);
            let thumb_y = rows.top + (track_h - thumb_h) * (self.rules_scroll as f32 / max_scroll as f32);
            self.fill_rr(rows.right + 3.0, rows.top, 4.0, track_h, 2.0, rgba(C_FIELD_BORDER, 0.5));
            self.fill_rr(rows.right + 3.0, thumb_y, 4.0, thumb_h, 2.0, col(C_MUTED));
        }

        // Tarjeta de edicion.
        let edit_y = list_bottom + 14.0;
        y = self.section(edit_y, cx, cw, self.tr.sec_edit_rule);
        let Some(rule) = self.selected_rule().cloned() else {
            self.text(
                self.tr.select_rule,
                Fmt::Small,
                D2D_RECT_F {
                    left: cx + 16.0,
                    top: y,
                    right: cx + cw - 16.0,
                    bottom: y + 24.0,
                },
                col(C_DIM),
            );
            return;
        };
        // Tres casillas en fila.
        let col_w = (cw - 40.0) / 3.0;
        self.check_inline(y, cx, cx + col_w, ID_CHECK_R_ENABLED, self.tr.chk_r_enabled);
        self.check_inline(y, cx + col_w, cx + 2.0 * col_w, ID_CHECK_R_MOVE, self.tr.chk_r_move);
        self.check_inline(y, cx + 2.0 * col_w, cx + cw - 16.0, ID_CHECK_R_FOLDERS, self.tr.chk_r_folders);
        y += 28.0;
        // Vista de la caja: auto (sigue Apariencia), lista o cuadricula.
        let idx = self.rules_selected.unwrap_or(0);
        self.text(
            self.tr.fld_view,
            Fmt::Small,
            D2D_RECT_F {
                left: cx + 16.0,
                top: y + 6.0,
                right: cx + 140.0,
                bottom: y + 26.0,
            },
            col(C_MUTED),
        );
        let vg = 8.0;
        let vw = 80.0;
        let vx = cx + 148.0;
        self.view_chip(Rect { x: vx, y, w: vw, h: 26.0 }, idx, "auto", self.tr.view_auto);
        self.view_chip(Rect { x: vx + vw + vg, y, w: vw, h: 26.0 }, idx, "list", self.tr.view_list);
        self.view_chip(Rect { x: vx + 2.0 * (vw + vg), y, w: vw, h: 26.0 }, idx, "grid", self.tr.view_grid);
        y += 32.0;
        // Tamano de icono por caja: un clic cicla Auto -> 16 -> ... -> 96.
        self.text(
            self.tr.fld_grid_icon,
            Fmt::Small,
            D2D_RECT_F { left: cx + 16.0, top: y + 6.0, right: cx + 144.0, bottom: y + 26.0 },
            col(C_MUTED),
        );
        self.icon_size_chip(Rect { x: vx, y, w: vw, h: 26.0 }, idx);
        y += 32.0;
        y = self.field_row(y, (cx, cw), ID_EDIT_R_TITLE, self.tr.fld_title, &rule.title, false);
        // Generacion de regla con IA desde lenguaje natural (texto transitorio).
        y = self.field_row(y, (cx, cw), ID_EDIT_R_AI_DESC, self.tr.fld_ai_rule_desc, "", false);
        self.icon_button(
            Rect { x: cx + 16.0, y, w: 200.0, h: 28.0 },
            self.tr.btn_ai_gen_rule,
            ID_BTN_R_AI_GEN,
            self.rules_selected.is_some(),
        );
        if AI_BUSY.load(Ordering::SeqCst) {
            self.spinner(cx + 226.0, y + 14.0, 8.0, self.spinner_phase);
        }
        y += 38.0;
        // Titulo del grupo: solo se muestra si la regla seleccionada esta
        // agrupada (el campo nativo se oculta/limpieza solo al no renderizarse).
        let group_title = self
            .cfg
            .group_of(&rule.id)
            .and_then(|f| f.group_title.clone())
            .unwrap_or_default();
        if self.cfg.is_grouped(&rule.id) {
            y = self.field_row(y, (cx, cw), ID_EDIT_R_GROUP_TITLE, self.tr.fld_group, &group_title, false);
        }
        y = self.field_row(y, (cx, cw), ID_EDIT_R_FOLDER, self.tr.fld_folder, &rule.folder, false);
        y = self.field_row(y, (cx, cw), ID_EDIT_R_COLOR, self.tr.fld_color, &rule.color, true);
        y = self.field_row(y, (cx, cw), ID_EDIT_R_EXTS, self.tr.fld_exts, &rule.extensions.join(", "), false);
        y = self.field_row(y, (cx, cw), ID_EDIT_R_PATTERNS, self.tr.fld_patterns, &rule.name_patterns.join(", "), false);
        // Filtros avanzados: tamano, fecha y regex.
        let min_size = rule.min_size_bytes.map(format_size).unwrap_or_default();
        let max_size = rule.max_size_bytes.map(format_size).unwrap_or_default();
        let newer = rule.newer_than_days.map(|d| d.to_string()).unwrap_or_default();
        let older = rule.older_than_days.map(|d| d.to_string()).unwrap_or_default();
        let regex_str = rule.regex.clone().unwrap_or_default();
        y = self.field_row(y, (cx, cw), ID_EDIT_R_MIN_SIZE, self.tr.fld_min_size, &min_size, false);
        y = self.field_row(y, (cx, cw), ID_EDIT_R_MAX_SIZE, self.tr.fld_max_size, &max_size, false);
        y = self.field_row(y, (cx, cw), ID_EDIT_R_NEWER, self.tr.fld_newer_than, &newer, false);
        y = self.field_row(y, (cx, cw), ID_EDIT_R_OLDER, self.tr.fld_older_than, &older, false);
        let _ = self.field_row(y, (cx, cw), ID_EDIT_R_REGEX, self.tr.fld_regex, &regex_str, false);
    }

    fn panel_appearance(&mut self, cy: f32) {
        let (cx, _, cw, _) = self.content_area();
        let mut y = cy + 10.0;
        let col_w = (cw - 40.0) / 2.0;

        y = self.section(y, cx, cw, self.tr.sec_colors);
        // Se clonan los valores para no retener borrows de `self.cfg` mientras
        // `field_at` toma `&mut self`.
        let colors: [(u16, &'static str, String); 7] = [
            (ID_EDIT_A_BG, self.tr.fld_bg, self.cfg.appearance.background.clone()),
            (ID_EDIT_A_HOVER, self.tr.fld_bg_hover, self.cfg.appearance.background_hover.clone()),
            (ID_EDIT_A_BORDER, self.tr.fld_border, self.cfg.appearance.border.clone()),
            (ID_EDIT_A_TITLE, self.tr.fld_title_color, self.cfg.appearance.title_color.clone()),
            (ID_EDIT_A_TEXT, self.tr.fld_text_color, self.cfg.appearance.text_color.clone()),
            (ID_EDIT_A_MUTED, self.tr.fld_muted, self.cfg.appearance.muted_color.clone()),
            (ID_EDIT_A_SHADOW, self.tr.fld_shadow, self.cfg.appearance.shadow.clone()),
        ];
        for (i, (id, name, value)) in colors.iter().enumerate() {
            let (dx, row) = (if i % 2 == 0 { 0.0 } else { col_w + 20.0 }, i / 2);
            self.field_at((cx + 16.0 + dx, y + row as f32 * 32.0), (148.0, (col_w - 36.0 - 148.0 - 40.0).max(60.0)), *id, name, value, true);
        }
        y += 4.0 * 32.0 + 12.0;

        y = self.section(y, cx, cw, self.tr.sec_measure);
        let nums: [(u16, &'static str, String); 4] = [
            (ID_EDIT_A_RADIUS, self.tr.fld_radius, format!("{}", self.cfg.appearance.corner_radius)),
            (ID_EDIT_A_TITLE_SIZE, self.tr.fld_title_size, format!("{}", self.cfg.appearance.title_size)),
            (ID_EDIT_A_TEXT_SIZE, self.tr.fld_text_size, format!("{}", self.cfg.appearance.text_size)),
            (ID_EDIT_A_SNAP, self.tr.fld_snap, format!("{}", self.cfg.appearance.snap_grid)),
        ];
        for (i, (id, name, value)) in nums.iter().enumerate() {
            let (dx, row) = (if i % 2 == 0 { 0.0 } else { col_w + 20.0 }, i / 2);
            self.field_at((cx + 16.0 + dx, y + row as f32 * 32.0), (148.0, (col_w - 36.0 - 148.0).max(60.0)), *id, name, value, false);
        }
        y += 2.0 * 32.0 + 12.0;

        y = self.section(y, cx, cw, self.tr.sec_show);
        y = self.check(y, cx, cw, ID_CHECK_A_ICONS, self.tr.chk_a_icons);
        y = self.check(y, cx, cw, ID_CHECK_A_COUNTER, self.tr.chk_a_counter);
        y = self.check(y, cx, cw, ID_CHECK_A_SEARCH, self.tr.chk_search);
        y += 10.0;
        y = self.section(y, cx, cw, self.tr.sec_grid);
        y = self.check(y, cx, cw, ID_CHECK_A_GRID, self.tr.chk_grid);
        y = self.num_row(y, cx, ID_EDIT_A_GRID_SIZE, self.tr.fld_grid_size, &format!("{}", self.cfg.appearance.grid_item_size as u32));
        y = self.num_row(y, cx, ID_EDIT_A_GRID_ICON, self.tr.fld_grid_icon, &format!("{}", self.cfg.appearance.grid_icon_size as u32));
        y += 10.0;
        // Selector de tema visual: chips debajo de todo.
        y = self.section(y, cx, cw, self.tr.sec_theme);
        let chips = crate::config::Appearance::preset_names();
        let chip_gap = 8.0;
        let x0 = cx + 16.0;
        let chip_w = ((cw - 32.0 - (chips.len() - 1) as f32 * chip_gap) / chips.len() as f32).min(120.0);
        for (i, key) in chips.iter().enumerate() {
            let label = crate::config::Appearance::preset_label(key);
            self.theme_chip(x0 + i as f32 * (chip_w + chip_gap), y, chip_w, 26.0, key, label);
        }
    }

    /// Chip de tema visual (igual que los chips de idioma).
    fn theme_chip(&mut self, x: f32, y: f32, w: f32, h: f32, key: &'static str, label: &str) {
        let selected = self.cfg.appearance.theme_preset == key;
        let over = self.hover == Some(Ctrl::Theme(key));
        let bg = if selected {
            col(C_ACTIVE)
        } else if over {
            col(C_HOVER)
        } else {
            col("#00000000")
        };
        self.fill_rr(x, y, w, h, 7.0, bg);
        self.draw_rr(
            Rect { x, y, w, h }, 7.0,
            if selected { rgba(C_ACCENT, 0.7) } else { rgba(C_FIELD_BORDER, 0.6) },
            1.0,
        );
        self.text(
            label,
            Fmt::Small,
            D2D_RECT_F { left: x + 4.0, top: y + 4.0, right: x + w - 4.0, bottom: y + h - 2.0 },
            if selected { col(C_TEXT) } else { col(C_MUTED) },
        );
        self.add_region(
            Ctrl::Theme(key),
            D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
        );
    }

    /// Chip de vista de la caja (auto / lista / cuadricula) para una regla.
    fn view_chip(&mut self, rect: Rect, idx: usize, value: &'static str, label: &str) {
        let Rect { x, y, w, h } = rect;
        let current = self.cfg.rules.get(idx).map(|r| r.view_mode.as_str()).filter(|s| !s.is_empty()).unwrap_or("auto");
        let selected = current == value;
        let over = self.hover == Some(Ctrl::RuleView(idx, value));
        let bg = if selected { col(C_ACTIVE) } else if over { col(C_HOVER) } else { col("#00000000") };
        self.fill_rr(x, y, w, h, 7.0, bg);
        self.draw_rr(
            Rect { x, y, w, h }, 7.0,
            if selected { rgba(C_ACCENT, 0.7) } else { rgba(C_FIELD_BORDER, 0.6) },
            1.0,
        );
        self.text(
            label,
            Fmt::Small,
            D2D_RECT_F { left: x + 4.0, top: y + 4.0, right: x + w - 4.0, bottom: y + h - 2.0 },
            if selected { col(C_TEXT) } else { col(C_MUTED) },
        );
        self.add_region(
            Ctrl::RuleView(idx, value),
            D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
        );
    }

    /// Chip del tamano de icono por caja: un clic cicla Auto -> 16 -> ... -> 96.
    fn icon_size_chip(&mut self, rect: Rect, idx: usize) {
        let Rect { x, y, w, h } = rect;
        let over = self.hover == Some(Ctrl::RuleIcon(idx));
        let label = match self.cfg.rules.get(idx).and_then(|r| r.icon_size) {
            Some(v) => format!("{:.0} px", v),
            None => self.tr.view_auto.to_string(),
        };
        let bg = if over { col(C_HOVER) } else { col("#00000000") };
        self.fill_rr(x, y, w, h, 7.0, bg);
        self.draw_rr(Rect { x, y, w, h }, 7.0, rgba(C_FIELD_BORDER, 0.6), 1.0);
        self.text(
            &label,
            Fmt::Small,
            D2D_RECT_F { left: x + 4.0, top: y + 4.0, right: x + w - 4.0, bottom: y + h - 2.0 },
            col(C_TEXT),
        );
        self.add_region(
            Ctrl::RuleIcon(idx),
            D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
        );
    }

    fn panel_ai(&mut self, cy: f32) {
        let (cx, _, cw, _) = self.content_area();
        let mut y = cy + 10.0;
        let tr = self.tr;
        y = self.section(y, cx, cw, tr.sec_ai_title);
        y = self.check(y, cx, cw, ID_CHECK_AI_ENABLE, tr.chk_ai_enable);

        y += 10.0;
        y = self.section(y, cx, cw, tr.sec_ai_connection);
        let url = self.cfg.ai.ollama_url.clone();
        y = self.field_row(y, (cx, cw), ID_EDIT_AI_URL, tr.fld_ai_url, &url, false);

        let model = self.cfg.ai.model.clone();
        y = self.field_row(y, (cx, cw), ID_EDIT_AI_MODEL, tr.fld_ai_model, &model, false);

        let embed = self.cfg.ai.embed_model.clone();
        y = self.field_row(y, (cx, cw), ID_EDIT_AI_EMBED_MODEL, tr.fld_ai_embed_model, &embed, false);

        y += 8.0;
        let bx0 = cx + 16.0;
        self.icon_button(Rect { x: bx0, y, w: 200.0, h: 30.0 }, tr.btn_ai_ping, ID_BTN_AI_PING, true);
        self.icon_button(Rect { x: bx0 + 210.0, y, w: 220.0, h: 30.0 }, tr.btn_ai_detect, ID_BTN_AI_DETECT_MODELS, true);

        y += 42.0;
        y = self.section(y, cx, cw, tr.sec_ai_organize);
        self.icon_button(Rect { x: bx0, y, w: 430.0, h: 34.0 }, tr.btn_ai_reorganize, ID_BTN_AI_REORGANIZE, true);
        if AI_BUSY.load(Ordering::SeqCst) {
            self.spinner(bx0 + 448.0, y + 17.0, 8.0, self.spinner_phase);
        }
    }

    fn panel_updates(&mut self, cy: f32) {
        let (cx, _, cw, _) = self.content_area();
        let mut y = cy + 10.0;
        y = self.section(y, cx, cw, self.tr.sec_updates);

        let ver = crate::updater::current_version();
        let ver_label = format!("{}: v{}", self.tr.lbl_version, ver);
        self.text(
            &ver_label,
            Fmt::Body,
            D2D_RECT_F {
                left: cx + 16.0,
                top: y + 4.0,
                right: cx + cw - 16.0,
                bottom: y + 28.0,
            },
            col(C_TEXT),
        );
        y += 34.0;

        y = self.check(y, cx, cw, ID_CHECK_AUTO_UPDATE, self.tr.chk_auto_check_updates);

        let bx0 = cx + 16.0;
        y += 8.0;
        self.icon_button(Rect { x: bx0, y, w: 220.0, h: 32.0 }, self.tr.btn_check_updates, ID_BTN_CHECK_UPDATES, true);
        self.icon_button(Rect { x: bx0 + 230.0, y, w: 220.0, h: 32.0 }, self.tr.btn_download_update, ID_BTN_DOWNLOAD_UPDATE, true);
        if UPDATE_BUSY.load(Ordering::SeqCst) {
            self.spinner(bx0 + 236.0, y + 16.0, 8.0, self.spinner_phase);
        }
    }

    fn panel_widgets(&mut self, cy: f32) {
        let (cx, _, cw, ch) = self.content_area();
        let mut y = cy + 10.0;
        y = self.section(y, cx, cw, self.tr.sec_widgets);

        // Botones de gestion del widget.
        let bx = cx + 16.0;
        self.icon_button(Rect { x: bx, y, w: 100.0, h: 28.0 }, self.tr.btn_new, ID_BTN_WIDGET_NEW, true);
        self.icon_button(Rect { x: bx + 108.0, y, w: 100.0, h: 28.0 }, self.tr.btn_delete, ID_BTN_WIDGET_DEL, self.widgets_selected.is_some());
        self.icon_button(Rect { x: bx + 216.0, y, w: 100.0, h: 28.0 }, self.tr.btn_widget_reload, ID_BTN_WIDGET_RELOAD, true);
        self.icon_button(Rect { x: bx + 324.0, y, w: 150.0, h: 28.0 }, self.tr.btn_widget_add, ID_BTN_WIDGET_ADD, self.widget_can_add());
        self.icon_button(Rect { x: bx + 482.0, y, w: 135.0, h: 28.0 }, self.tr.btn_widget_remove, ID_BTN_WIDGET_REMOVE, self.widget_can_remove());
        y += 38.0;

        // Lista de scripts instalados (nombres .lua).
        let list_h = 118.0;
        let list = D2D_RECT_F {
            left: cx + 16.0,
            top: y,
            right: cx + cw - 16.0,
            bottom: y + list_h,
        };
        self.fill_rr(list.left, list.top, list.right - list.left, list.bottom - list.top, 10.0, col("#10162A"));
        self.draw_rr(Rect { x: list.left, y: list.top, w: list.right - list.left, h: list.bottom - list.top }, 10.0, rgba(C_FIELD_BORDER, 0.6), 1.0);
        self.list_rect = Some(list);

        let row_h = 28.0;
        let rows = D2D_RECT_F {
            left: list.left + 8.0,
            top: list.top + 6.0,
            right: list.right - 8.0,
            bottom: list.bottom - 6.0,
        };
        let visible = ((rows.bottom - rows.top) / row_h).floor().max(0.0) as usize;
        let total = self.widgets_list.len();
        let max_scroll = total.saturating_sub(visible);
        self.widgets_scroll = self.widgets_scroll.min(max_scroll);
        unsafe {
            self.target().PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: rows.left.floor(),
                    top: rows.top.floor(),
                    right: rows.right.ceil(),
                    bottom: rows.bottom.ceil(),
                },
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }
        for offset in 0..visible.min(total.saturating_sub(self.widgets_scroll)) {
            let idx = self.widgets_scroll + offset;
            let r = D2D_RECT_F {
                left: rows.left,
                top: rows.top + offset as f32 * row_h,
                right: rows.right,
                bottom: rows.top + (offset as f32 + 1.0) * row_h,
            };
            self.widget_row(&r, idx);
        }
        unsafe {
            self.target().PopAxisAlignedClip();
        }
        if total == 0 {
            self.text(
                self.tr.msg_widget_empty,
                Fmt::Small,
                D2D_RECT_F {
                    left: rows.left + 6.0,
                    top: rows.top + 10.0,
                    right: rows.right,
                    bottom: rows.top + 30.0,
                },
                col(C_DIM),
            );
        }
        y += list_h + 12.0;

        // Nombre y estado del widget seleccionado.
        let name = self
            .widgets_selected
            .and_then(|i| self.widgets_list.get(i).cloned())
            .unwrap_or_default();
        y = self.field_row(y, (cx, cw), ID_EDIT_WIDGET_NAME, self.tr.fld_widget_name, &name, false);
        y = self.check(y, cx, cw, ID_CHECK_WIDGET_ENABLED, self.tr.chk_widget_enabled);
        y += 4.0;

        // Editor de codigo Lua (multilinea nativo).
        self.text(
            self.tr.lbl_widget_code,
            Fmt::Small,
            D2D_RECT_F {
                left: cx + 26.0,
                top: y,
                right: cx + cw - 16.0,
                bottom: y + 18.0,
            },
            col(C_DIM),
        );
        y += 20.0;
        let code_h = (ch - (y - cy) - 44.0).max(60.0);
        self.widget_code_edit(cx + 16.0, y, cw - 32.0, code_h);
        y += code_h + 10.0;
        self.icon_button(Rect { x: cx + 16.0, y, w: 140.0, h: 28.0 }, self.tr.btn_widget_save, ID_BTN_WIDGET_SAVE, self.widgets_selected.is_some());
    }

    fn widget_row(&mut self, r: &D2D_RECT_F, idx: usize) {
        let name = self.widgets_list[idx].clone();
        let selected = self.widgets_selected == Some(idx);
        let over = self.hover == Some(Ctrl::WidgetRow(idx));
        let enabled = !self.cfg.widgets_disabled.iter().any(|d| d == &name);
        let bg = if selected { col(C_ACTIVE) } else if over { col(C_HOVER) } else { col("#131B2E") };
        self.fill_rr(r.left, r.top, r.right - r.left, r.bottom - r.top, 8.0, bg);
        if selected {
            self.draw_rr(Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }, 8.0, rgba(C_ACCENT, 0.5), 1.0);
        }
        self.text(
            &name,
            Fmt::Body,
            D2D_RECT_F {
                left: r.left + 12.0,
                top: r.top + 4.0,
                right: r.right - 40.0,
                bottom: r.top + 22.0,
            },
            if enabled { col(C_TEXT) } else { col(C_DIM) },
        );
        if !enabled {
            self.text(
                "✕",
                Fmt::Small,
                D2D_RECT_F {
                    left: r.right - 26.0,
                    top: r.top + 4.0,
                    right: r.right - 8.0,
                    bottom: r.top + 22.0,
                },
                col(C_DIM),
            );
        }
        self.add_region(Ctrl::WidgetRow(idx), *r);
    }

    /// Fondo del editor de codigo y registro de la posicion del EDIT multilinea
    /// (misma estrategia que `field_at`: se aplica tras el present D2D).
    fn widget_code_edit(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let focused = self.edits_focused(ID_EDIT_WIDGET_CODE);
        self.fill_rr(x, y, w, h, 10.0, col(C_FIELD));
        self.draw_rr(Rect { x, y, w, h }, 10.0, if focused { col(C_FIELD_FOCUS) } else { col(C_FIELD_BORDER) }, 1.0);
        if self.edits.contains_key(&ID_EDIT_WIDGET_CODE) {
            let (_, cyt, _, chb) = self.content_rect;
            if y >= cyt && y + h <= chb {
                let s = self.scale;
                let rect = (
                    ((x + 6.0) * s) as i32,
                    ((y + 6.0) * s) as i32,
                    ((w - 12.0) * s).max(40.0) as i32,
                    ((h - 12.0) * s).max(20.0) as i32,
                );
                self.edit_next_rects.insert(ID_EDIT_WIDGET_CODE, rect);
                self.edits_shown.push(ID_EDIT_WIDGET_CODE);
            }
        }
        self.add_region(
            Ctrl::Field(ID_EDIT_WIDGET_CODE),
            D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
        );
    }

    fn widget_select(&mut self, idx: usize) {
        self.widgets_selected = Some(idx);
        self.refresh_widget_fields();
        self.invalidate();
    }

    fn widget_code(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.widgets_dir.join(format!("{name}.lua"))).ok()
    }

    fn refresh_widget_fields(&mut self) {
        let name = self
            .widgets_selected
            .and_then(|i| self.widgets_list.get(i).cloned());
        match name {
            Some(name) => {
                if let Some(&hwnd) = self.edits.get(&ID_EDIT_WIDGET_NAME) {
                    set_text(hwnd, &name);
                }
                let code = self.widget_code(&name).unwrap_or_default();
                if let Some(&hwnd) = self.edits.get(&ID_EDIT_WIDGET_CODE) {
                    set_text(hwnd, &code);
                }
                let enabled = !self.cfg.widgets_disabled.iter().any(|d| d == &name);
                self.checks.insert(ID_CHECK_WIDGET_ENABLED, enabled);
            }
            None => {
                if let Some(&hwnd) = self.edits.get(&ID_EDIT_WIDGET_NAME) {
                    set_text(hwnd, "");
                }
                if let Some(&hwnd) = self.edits.get(&ID_EDIT_WIDGET_CODE) {
                    set_text(hwnd, "");
                }
                self.checks.insert(ID_CHECK_WIDGET_ENABLED, false);
            }
        }
    }

    fn widget_new(&mut self) {
        if std::fs::create_dir_all(&self.widgets_dir).is_err() {
            return;
        }
        let mut n = 1;
        let name = loop {
            let candidate = format!("widget_{n}");
            if !self.widgets_list.iter().any(|w| w == &candidate) {
                break candidate;
            }
            n += 1;
        };
        let template = r#"TITLE = "Mi widget"

function render(ctx)
    local w = ctx:width()
    local h = ctx:height()
    ctx:fill_rect(0, 0, w, h, 0x00000000)
    ctx:text(24, h * 0.4, "Hola", 20, 0xFFFFFFFF)
end
"#;
        let _ = std::fs::write(self.widgets_dir.join(format!("{name}.lua")), template);
        self.widget_reload();
        if let Some(idx) = self.widgets_list.iter().position(|w| w == &name) {
            self.widget_select(idx);
        }
    }

    fn widget_delete(&mut self) {
        let Some(idx) = self.widgets_selected else { return };
        let name = self.widgets_list[idx].clone();
        let _ = std::fs::remove_file(self.widgets_dir.join(format!("{name}.lua")));
        self.cfg.widgets_disabled.retain(|d| d != &name);
        self.widget_reload();
        self.preview_apply();
    }

    fn widget_save(&mut self) {
        let Some(idx) = self.widgets_selected else { return };
        let old_name = self.widgets_list[idx].clone();
        let new_name = self.edit_text(ID_EDIT_WIDGET_NAME).unwrap_or_default().trim().to_string();
        if new_name.is_empty() || new_name.chars().any(|c| "\\/:*?\"<>|".contains(c)) {
            return;
        }
        let code = self.edit_text(ID_EDIT_WIDGET_CODE).unwrap_or_default();
        if std::fs::write(self.widgets_dir.join(format!("{new_name}.lua")), &code).is_err() {
            return;
        }
        if new_name != old_name {
            let _ = std::fs::remove_file(self.widgets_dir.join(format!("{old_name}.lua")));
            if let Some(pos) = self.cfg.widgets_disabled.iter().position(|d| d == &old_name) {
                self.cfg.widgets_disabled[pos] = new_name.clone();
            }
        }
        if !self.app.is_null() {
            unsafe { (*self.app).reload_widgets(); }
        }
        self.widget_reload();
        self.preview_apply();
    }

    /// Abre el formulario de posicion/tamano para el widget seleccionado:
    /// guarda el script, y si la caja ya existe precarga su geometria actual
    /// para editarla; si no, propone una por defecto en cascada.
    /// Crea la caja del widget seleccionado al instante (sin formulario):
    /// habilita el widget, añade la entrada `[[fences]] widget = ...` en una
    /// posicion en cascada y aplica directo a la app. Para recolocar o
    /// redimensionar se arrastra la caja en el escritorio como cualquier otra.
    fn widget_add_as_box(&mut self) {
        self.widget_save();
        let Some(name) = self
            .widgets_selected
            .and_then(|i| self.widgets_list.get(i).cloned())
        else {
            return;
        };
        if self.widget_has_box(&name) {
            return;
        }
        // Habilita el widget: lo saca de la lista de desactivados.
        self.cfg.widgets_disabled.retain(|d| d != &name);
        self.checks.insert(ID_CHECK_WIDGET_ENABLED, true);
        let n = self.cfg.fences.iter().filter(|f| f.widget.is_some()).count() as i32;
        let layout = crate::config::FenceLayout {
            id: crate::config::String32::new(&format!("widget:{name}")),
            x: 120 + n * 32,
            y: 120 + n * 32,
            width: 320,
            height: 240,
            widget: Some(name.clone()),
            ..Default::default()
        };
        self.cfg.fences.push(layout);
        if !self.app.is_null() {
            unsafe {
                (*self.app).reload_widgets();
                // Aplica directo (sin esperar a la vista previa): la caja
                // aparece ya, aunque otro campo de config este a medio editar.
                (*self.app).apply_config(self.cfg.clone());
            }
        }
        self.preview_apply();
    }

    /// Quita la caja instanciada para el widget seleccionado: elimina la
    /// entrada `[[fences]] widget = ...` (el script .lua se conserva intacto).
    fn widget_remove_box(&mut self) {
        let Some(name) = self
            .widgets_selected
            .and_then(|i| self.widgets_list.get(i).cloned())
        else {
            return;
        };
        let before = self.cfg.fences.len();
        self.cfg.fences.retain(|f| f.widget.as_deref() != Some(name.as_str()));
        if self.cfg.fences.len() != before {
            if !self.app.is_null() {
                unsafe { (*self.app).apply_config(self.cfg.clone()); }
            }
            self.preview_apply();
        }
    }

    fn widget_reload(&mut self) {
        let keep = self
            .widgets_selected
            .and_then(|i| self.widgets_list.get(i).cloned());
        self.widgets_list.clear();
        if let Ok(entries) = std::fs::read_dir(&self.widgets_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("lua") {
                    if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                        self.widgets_list.push(name.to_string());
                    }
                }
            }
        }
        self.widgets_list.sort();
        self.widgets_list.dedup();
        self.widgets_scroll = 0;
        self.widgets_selected = keep
            .as_ref()
            .and_then(|k| self.widgets_list.iter().position(|w| w == k));
        self.refresh_widget_fields();
        self.invalidate();
    }

    /// ¿Ya existe una caja (`[[fences]] widget = ...`) para este script?
    fn widget_has_box(&self, name: &str) -> bool {
        self.cfg.fences.iter().any(|f| f.widget.as_deref() == Some(name))
    }

    /// El boton "Añadir como caja" esta activo con un script seleccionado que
    /// aun no tiene caja.
    fn widget_can_add(&self) -> bool {
        self.widgets_selected
            .and_then(|i| self.widgets_list.get(i))
            .is_some_and(|name| !self.widget_has_box(name))
    }

    /// El boton "Quitar caja" esta activo con un script seleccionado que ya
    /// tiene una caja instanciada.
    fn widget_can_remove(&self) -> bool {
        self.widgets_selected
            .and_then(|i| self.widgets_list.get(i))
            .is_some_and(|name| self.widget_has_box(name))
    }

    /// Refleja el estado del checkbox del widget seleccionado en
    /// `cfg.widgets_disabled` (idempotente: se recomputa en cada recogida).
    fn sync_widgets_disabled(&mut self) {
        let Some(idx) = self.widgets_selected else { return };
        let Some(name) = self.widgets_list.get(idx).cloned() else { return };
        let enabled = self.checked(ID_CHECK_WIDGET_ENABLED);
        let contains = self.cfg.widgets_disabled.iter().any(|d| d == &name);
        if enabled && contains {
            self.cfg.widgets_disabled.retain(|d| d != &name);
        } else if !enabled && !contains {
            self.cfg.widgets_disabled.push(name);
        }
    }
}

// ---------------------------------------------------------------------------
// Cabecera, navegacion y barra de acciones
// ---------------------------------------------------------------------------

impl Settings {
    fn draw_header(&mut self) {
        let w = self.size.0;
        let colors = ["#7DD3FC", "#C4B5FD", "#F9A8D4", "#6EE7B7"];
        let (s, gap) = (7.0, 3.0);
        let x0 = 20.0;
        let y0 = HEADER_H / 2.0 - s - gap / 2.0;
        for (i, c) in colors.iter().enumerate() {
            let (dx, dy) = (i as f32 % 2.0, i as f32 / 2.0);
            self.fill_rr(x0 + dx * (s + gap), y0 + dy * (s + gap), s, s, 2.0, col(c));
        }
        let tx = x0 + 2.0 * (s + gap) + 12.0;
        self.text(
            self.tr.window_title,
            Fmt::Title,
            D2D_RECT_F { left: tx, top: 7.0, right: w - 60.0, bottom: 28.0 },
            col(C_TEXT),
        );
        self.text(
            self.tr.window_subtitle,
            Fmt::Small,
            D2D_RECT_F { left: tx, top: 28.0, right: w - 60.0, bottom: 44.0 },
            col(C_MUTED),
        );
        self.draw_rr(Rect { x: 0.0, y: HEADER_H - 1.0, w, h: 1.0 }, 0.0, rgba(C_CARD_BORDER, 0.8), 1.0);

        // Botones de ventana: minimizar (—) y cerrar (×).
        let (b, s2) = (w - 44.0, 26.0);
        let by = (HEADER_H - s2) / 2.0;
        let mb = b - 40.0;
        if self.hover == Some(Ctrl::Minimize) {
            self.fill_rr(mb, by, s2, s2, 13.0, col(C_HOVER));
        }
        self.line(mb + 8.0, by + 13.0, mb + 18.0, by + 13.0, col(C_MUTED), 1.6);
        self.add_region(
            Ctrl::Minimize,
            D2D_RECT_F {
                left: mb,
                top: by,
                right: mb + s2,
                bottom: by + s2,
            },
        );
        if self.hover == Some(Ctrl::Close) {
            self.fill_rr(b, by, s2, s2, 13.0, col(C_HOVER));
        }
        self.line(b + 8.0, by + 8.0, b + 18.0, by + 18.0, col(C_MUTED), 1.6);
        self.line(b + 18.0, by + 8.0, b + 8.0, by + 18.0, col(C_MUTED), 1.6);
        self.add_region(Ctrl::Close, D2D_RECT_F { left: b, top: by, right: b + s2, bottom: by + s2 });
    }

    fn draw_sidebar(&mut self) {
        let y0 = HEADER_H;
        let h = self.size.1 - HEADER_H - BAR_H;
        self.fill_rr(0.0, y0, SIDEBAR_W, h, 0.0, col(C_SIDEBAR));
        self.draw_rr(Rect { x: SIDEBAR_W - 1.0, y: y0, w: 1.0, h }, 0.0, rgba(C_CARD_BORDER, 0.6), 1.0);
        let items: [(Panel, &'static str); 7] = [
            (Panel::General, self.tr.nav_general),
            (Panel::Rules, self.tr.nav_rules),
            (Panel::Appearance, self.tr.nav_appearance),
            (Panel::Language, "🌐 Idioma"),
            (Panel::Ai, "🤖 IA (Ollama)"),
            (Panel::Updates, "🔄 Updates"),
            (Panel::Widgets, "🧩 Widgets"),
        ];
        let mut y = y0 + 16.0;
        for (panel, label) in items {
            let (x, w) = (12.0, SIDEBAR_W - 24.0);
            let selected = self.panel == panel;
            let over = self.hover == Some(Ctrl::Nav(panel));
            let bg = if selected { col(C_ACTIVE) } else if over { col(C_HOVER) } else { col("#00000000") };
            self.fill_rr(x, y, w, 34.0, 9.0, bg);
            if selected {
                self.fill_rr(x, y + 7.0, 3.0, 20.0, 1.5, col(C_ACCENT));
            }
            self.text(
                label,
                Fmt::Body,
                D2D_RECT_F { left: x + 14.0, top: y + 7.0, right: x + w - 10.0, bottom: y + 30.0 },
                if selected { col(C_TEXT) } else { col(C_MUTED) },
            );
            self.add_region(Ctrl::Nav(panel), D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + 34.0 });
            y += 40.0;
        }
        // Pie de la barra lateral: version.
        self.text(
            concat!("v", env!("CARGO_PKG_VERSION")),
            Fmt::Small,
            D2D_RECT_F {
                left: 12.0,
                top: y0 + h - 30.0,
                right: SIDEBAR_W - 12.0,
                bottom: y0 + h - 10.0,
            },
            col(C_DIM),
        );
    }

    fn draw_bar(&mut self) {
        let w = self.size.0;
        let y0 = self.size.1 - BAR_H;
        self.fill_rr(0.0, y0, w, BAR_H, 0.0, col("#0B101D"));
        self.draw_rr(Rect { x: 0.0, y: y0, w, h: 1.0 }, 0.0, rgba(C_CARD_BORDER, 0.8), 1.0);
        let (bw, bh) = (112.0, 36.0);
        let bx = w - 24.0 - bw;
        let by = y0 + (BAR_H - bh) / 2.0;
        // Indicador de cambios sin guardar (vista previa aun no persistida).
        if self.dirty {
            let dot = 6.0;
            let dot_y = y0 + (BAR_H - dot) / 2.0;
            self.fill_rr(24.0, dot_y, dot, dot, dot / 2.0, col(C_ACCENT));
            self.text(
                self.tr.unsaved_changes,
                Fmt::Small,
                D2D_RECT_F {
                    left: 24.0 + dot + 8.0,
                    top: y0,
                    right: bx - 16.0 - bw - 12.0,
                    bottom: y0 + BAR_H,
                },
                col(C_MUTED),
            );
        }
        // Los cambios se aplican en vivo; Cancelar revierte y Guardar persiste.
        self.push_button(Rect { x: bx - 16.0 - bw, y: by, w: bw, h: bh }, self.tr.btn_cancel, ID_BTN_CANCEL, BtnKind::Ghost);
        self.push_button(Rect { x: bx, y: by, w: bw, h: bh }, self.tr.btn_save, ID_BTN_OK, BtnKind::Primary);
    }

    // --- Selector de color visual (HSB) ---

    /// Rectangulo (DIPs) del panel del selector, centrado en el area de contenido.
    fn picker_rect(&self) -> D2D_RECT_F {
        let (cx, cy, cw, ch) = self.content_area();
        let (pw, ph) = (264.0, 290.0);
        D2D_RECT_F {
            left: cx + (cw - pw) / 2.0,
            top: cy + (ch - ph) / 2.0,
            right: cx + (cw - pw) / 2.0 + pw,
            bottom: cy + (ch - ph) / 2.0 + ph,
        }
    }

    fn draw_picker(&mut self) {
        let Some(p) = self.picker else { return };
        let pr = self.picker_rect();
        let (x0, y0) = (pr.left, pr.top);
        let (pw, ph) = (264.0, 290.0);

        self.fill_rr(x0, y0, pw, ph, 14.0, col("#10162A"));
        self.draw_rr(Rect { x: x0, y: y0, w: pw, h: ph }, 14.0, col(C_FIELD_BORDER), 1.0);
        self.text(
            self.tr.picker_title,
            Fmt::Small,
            D2D_RECT_F {
                left: x0 + 16.0,
                top: y0 + 12.0,
                right: x0 + 190.0,
                bottom: y0 + 34.0,
            },
            col(C_TEXT),
        );
        // Cerrar (x).
        let (bx, bs) = (x0 + pw - 42.0, 26.0);
        if self.hover == Some(Ctrl::Picker(PickerPart::Close)) {
            self.fill_rr(bx, y0 + 9.0, bs, bs, 13.0, col(C_HOVER));
        }
        self.line(bx + 8.0, y0 + 17.0, bx + 18.0, y0 + 27.0, col(C_MUTED), 1.6);
        self.line(bx + 18.0, y0 + 17.0, bx + 8.0, y0 + 27.0, col(C_MUTED), 1.6);
        self.add_region(
            Ctrl::Picker(PickerPart::Close),
            D2D_RECT_F {
                left: bx,
                top: y0 + 9.0,
                right: bx + bs,
                bottom: y0 + 9.0 + bs,
            },
        );
        self.draw_rr(Rect { x: x0 + 10.0, y: y0 + 42.0, w: pw - 20.0, h: 1.0 }, 0.0, rgba(C_FIELD_BORDER, 0.8), 1.0);

        // Cuadro Saturacion/Valor: rejilla de celdas calculada al vuelo.
        let (sx, sy, sw, sh) = (x0 + 14.0, y0 + 52.0, 200.0, 170.0);
        let (cols, rows) = (20, 17);
        for j in 0..rows {
            for i in 0..cols {
                let sat = (i as f32 + 0.5) / cols as f32;
                let val = 1.0 - (j as f32 + 0.5) / rows as f32;
                let (r, g, b) = hsv_to_rgb(p.hue, sat, val);
                self.fill_rr(
                    sx + i as f32 * 10.0,
                    sy + j as f32 * 10.0,
                    10.0,
                    10.0,
                    0.0,
                    D2D1_COLOR_F { r, g, b, a: 1.0 },
                );
            }
        }
        self.draw_rr(Rect { x: sx, y: sy, w: sw, h: sh }, 4.0, rgba(C_FIELD_BORDER, 0.9), 1.0);
        let ix = sx + p.sat * sw;
        let iy = sy + (1.0 - p.val) * sh;
        self.draw_rr(Rect { x: ix - 4.5, y: iy - 4.5, w: 9.0, h: 9.0 }, 4.5, D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.95 }, 1.5);
        self.add_region(
            Ctrl::Picker(PickerPart::Sv),
            D2D_RECT_F {
                left: sx,
                top: sy,
                right: sx + sw,
                bottom: sy + sh,
            },
        );

        // Tira de tono (Hue): bandas finas para un degradado suave.
        let (hx, hy, hw, hh) = (x0 + 230.0, y0 + 52.0, 20.0, 170.0);
        let hue_rows = 68;
        for j in 0..hue_rows {
            let hue = (j as f32 + 0.5) / hue_rows as f32 * 360.0;
            let (r, g, b) = hsv_to_rgb(hue, 1.0, 1.0);
            self.fill_rr(hx, hy + j as f32 * 2.5, hw, 2.5, 0.0, D2D1_COLOR_F { r, g, b, a: 1.0 });
        }
        self.draw_rr(Rect { x: hx, y: hy, w: hw, h: hh }, 4.0, rgba(C_FIELD_BORDER, 0.9), 1.0);
        let hyy = hy + (p.hue / 360.0) * hh;
        self.draw_rr(Rect { x: hx - 2.0, y: hyy - 4.0, w: hw + 4.0, h: 8.0 }, 4.0, D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.95 }, 1.5);
        self.add_region(
            Ctrl::Picker(PickerPart::Hue),
            D2D_RECT_F {
                left: hx,
                top: hy,
                right: hx + hw,
                bottom: hy + hh,
            },
        );

        // Muestra + hex + boton Usar.
        let hex = hsv_to_hex(p.hue, p.sat, p.val);
        self.fill_rr(x0 + 16.0, y0 + 236.0, 26.0, 26.0, 6.0, col(&hex));
        self.draw_rr(Rect { x: x0 + 16.0, y: y0 + 236.0, w: 26.0, h: 26.0 }, 6.0, rgba(C_FIELD_BORDER, 0.9), 1.0);
        self.text(
            &hex,
            Fmt::Body,
            D2D_RECT_F {
                left: x0 + 52.0,
                top: y0 + 236.0,
                right: x0 + 185.0,
                bottom: y0 + 262.0,
            },
            col(C_TEXT),
        );
        let (bw, bh) = (64.0, 28.0);
        let (bxx, byy) = (x0 + pw - 78.0, y0 + 232.0);
        let over = self.hover == Some(Ctrl::Picker(PickerPart::Ok));
        self.fill_rr(bxx, byy, bw, bh, 8.0, if over { rgba(C_ACCENT, 0.9) } else { col(C_ACCENT) });
        self.text(
            self.tr.btn_use,
            Fmt::Body,
            D2D_RECT_F {
                left: bxx + 4.0,
                top: byy + 5.0,
                right: bxx + bw - 4.0,
                bottom: byy + bh - 3.0,
            },
            col(C_ON_ACCENT),
        );
        self.add_region(
            Ctrl::Picker(PickerPart::Ok),
            D2D_RECT_F {
                left: bxx,
                top: byy,
                right: bxx + bw,
                bottom: byy + bh,
            },
        );
    }

    fn open_picker(&mut self, field: u16) {
        // Arranca con el color actual del campo (si es valido).
        let hex = self.edit_text(field).unwrap_or_default();
        let parsed = hex_to_hsv(&hex);
        let (h, s, v) = match parsed {
            // Neutros (gris/blanco/negro) no tienen tono: reusar el ultimo.
            Some((_, s, _)) if s <= 0.001 => (self.last_hue, 0.85, parsed.map(|x| x.2).unwrap_or(1.0)),
            Some(t) => t,
            None => (self.last_hue, 0.85, 1.0),
        };
        self.picker = Some(PickerState {
            field,
            hue: h,
            sat: s,
            val: v,
            drag: None,
        });
        self.invalidate();
    }

    fn close_picker(&mut self) {
        if self.picker.is_some() {
            self.picker = None;
            // El color elegido ya se escribio en el EDIT: aplicarlo en vivo.
            self.preview_apply();
        }
    }

    fn picker_start(&mut self, part: PickerPart, x: f32, y: f32) {
        if let Some(p) = self.picker.as_mut() {
            p.drag = Some(part);
        }
        self.picker_set(part, x, y);
        unsafe { let _ = SetCapture(self.hwnd); }
    }

    fn picker_set(&mut self, part: PickerPart, x: f32, y: f32) {
        let pr = self.picker_rect();
        let (sx, sy, sw, sh) = (pr.left + 14.0, pr.top + 52.0, 200.0, 170.0);
        let Some(p) = self.picker.as_mut() else { return };
        match part {
            PickerPart::Sv => {
                p.sat = ((x - sx) / sw).clamp(0.0, 1.0);
                p.val = 1.0 - ((y - sy) / sh).clamp(0.0, 1.0);
            }
            PickerPart::Hue => {
                p.hue = ((y - sy) / sh).clamp(0.0, 1.0) * 360.0;
            }
            _ => {}
        }
        self.last_hue = p.hue;
        // El valor se escribe al EDIT para que quede sincronizado aunque el
        // selector se cierre despues (clic fuera, ESC, Usar...).
        let hex = hsv_to_hex(p.hue, p.sat, p.val);
        if let Some(edit) = self.edits.get(&p.field).copied() {
            set_text(edit, &hex);
        }
        self.invalidate();
    }

    fn render(&mut self) {
        self.regions.clear();
        self.edits_shown.clear();
        self.edit_next_rects.clear();
        // El timer del spinner se sincroniza cada frame, independiente del panel.
        self.sync_busy_timer();

        let mut r = RECT::default();
        unsafe { let _ = GetClientRect(self.hwnd, &mut r); }
        // GetClientRect devuelve pixeles; el layout trabaja en DIPs.
        let size = (
            (r.right - r.left) as f32 / self.scale,
            (r.bottom - r.top) as f32 / self.scale,
        );
        self.size = size;
        if size.0 <= 0.0 || size.1 <= 0.0 {
            return;
        }
        let px_w = (size.0 * self.scale) as u32;
        let px_h = (size.1 * self.scale) as u32;
        unsafe {
            if (px_w, px_h) != self.target_px_size {
                self.target_px_size = (px_w, px_h);
                let _ = self.target().Resize(&D2D_SIZE_U {
                    width: px_w,
                    height: px_h,
                });
            }
            self.target().BeginDraw();
            self.target().Clear(Some(&col(C_BG)));
        }

        self.build_focus_order();
        self.draw_header();
        self.draw_sidebar();
        // Area de contenido (sin desplazar) y recorte del scroll.
        let (cx, cy, cw, ch) = self.content_area();
        self.content_rect = (cx, cy, cx + cw, cy + ch);
        self.list_rect = None;
        unsafe {
            self.target().PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: cx.floor(),
                    top: cy.floor(),
                    right: (cx + cw).ceil(),
                    bottom: (cy + ch).ceil(),
                },
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }
        self.panel_bg();
        // Los paneles dibujan desplazados: si la ventana se encoge, el
        // contenido sobrante queda oculto bajo el borde inferior en vez de
        // solaparse con la barra de acciones.
        let panel_start = self.regions.len();
        let scy = cy - self.scroll;
        match self.panel {
            Panel::General => self.panel_general(scy),
            Panel::Rules => self.panel_rules(scy),
            Panel::Appearance => self.panel_appearance(scy),
            Panel::Language => self.panel_language(scy),
            Panel::Ai => self.panel_ai(scy),
            Panel::Updates => self.panel_updates(scy),
            Panel::Widgets => self.panel_widgets(scy),
        }
        // Limite de desplazamiento = el punto mas bajo del contenido dibujado.
        let max_bottom = self.regions[panel_start..]
            .iter()
            .map(|r| r.r.bottom)
            .fold(cy, f32::max);
        self.scroll_max = (max_bottom + 10.0 - (cy + ch) + self.scroll).max(0.0);
        self.scroll = self.scroll.min(self.scroll_max);
        // Barra de desplazamiento vertical (solo si hay contenido oculto).
        if self.scroll_max > 0.0 {
            let track_h = ch - 8.0;
            let thumb_h = (track_h * (ch / (ch + self.scroll_max))).max(24.0);
            let thumb_y = cy + 4.0 + (track_h - thumb_h) * (self.scroll / self.scroll_max);
            let sx = cx + cw - 6.0;
            self.fill_rr(sx, cy + 4.0, 3.0, track_h, 1.5, rgba(C_FIELD_BORDER, 0.5));
            self.fill_rr(sx, thumb_y, 3.0, thumb_h, 1.5, col(C_MUTED));
            self.add_region(
                Ctrl::Scroll,
                D2D_RECT_F {
                    left: sx - 8.0,
                    top: thumb_y,
                    right: sx + 3.0,
                    bottom: thumb_y + thumb_h,
                },
            );
        }
        unsafe {
            self.target().PopAxisAlignedClip();
        }
        self.draw_bar();
        // El selector de color flota sobre todo el panel.
        if self.picker.is_some() {
            self.draw_picker();
        }
        // El formulario "Añadir como caja" flota por encima de todo.

        // Sincroniza los EDIT nativos FUERA del ciclo de dibujado D2D: primero
        // se ocultan los que van a moverse o desaparecer (con WS_CLIPCHILDREN,
        // el present de EndDraw respeta las regiones de los hijos visibles; si
        // un EDIT se mueve a mitad de ciclo su posicion antigua quedaria sin
        // pintar, dejando un residuo negro), y tras EndDraw se reposicionan y
        // muestran los visibles.
        self.sync_edits_pre();
        unsafe {
            let _ = self.target().EndDraw(None, None);
        }
        self.sync_edits_post();
    }

    /// Oculta los EDIT que van a moverse o desaparecer ANTES del present D2D.
    fn sync_edits_pre(&mut self) {
        let next_visible: HashSet<u16> = self.edits_shown.iter().copied().collect();
        for id in self.edit_visible.iter().copied().collect::<Vec<_>>() {
            let must_hide = !next_visible.contains(&id);
            let moved = next_visible.contains(&id)
                && self.edit_next_rects.get(&id).copied()
                    != self.edit_rects.get(&id).copied();
            if must_hide || moved {
                if let Some(hwnd) = self.edits.get(&id).copied() {
                    unsafe { let _ = ShowWindow(hwnd, SW_HIDE); }
                }
                self.edit_visible.remove(&id);
            }
        }
    }

    /// Tras el present D2D, reposiciona y muestra los EDIT visibles.
    fn sync_edits_post(&mut self) {
        let next_visible: HashSet<u16> = self.edits_shown.iter().copied().collect();
        for (id, &hwnd) in &self.edits {
            if next_visible.contains(id) {
                if let Some(&rect) = self.edit_next_rects.get(id) {
                    if self.edit_rects.get(id) != Some(&rect) {
                        unsafe {
                            let _ = SetWindowPos(
                                hwnd,
                                HWND_TOP,
                                rect.0,
                                rect.1,
                                rect.2,
                                rect.3,
                                SWP_NOZORDER | SWP_NOACTIVATE,
                            );
                        }
                        self.edit_rects.insert(*id, rect);
                    }
                }
                if !self.edit_visible.contains(id) {
                    unsafe { let _ = ShowWindow(hwnd, SW_SHOWNA); }
                    self.edit_visible.insert(*id);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Foco, acciones y consultas
// ---------------------------------------------------------------------------

impl Settings {
    fn focused(&self, c: Ctrl) -> bool {
        self.focus.map(|i| self.focus_order.get(i) == Some(&c)).unwrap_or(false)
    }

    /// Con el formulario "Añadir como caja" abierto, los EDITs del panel
    /// subyacente se ocultan para que no asomen por encima del modal (los
    /// hijos nativos se pintan sobre el D2D). Los campos del propio formulario
    /// (X/Y/Ancho/Alto) siguen siendo EDITs editables.
    fn edits_focused(&self, id: u16) -> bool {
        let focus = unsafe { GetFocus() };
        self.edits.get(&id).map(|e| *e == focus).unwrap_or(false)
    }

    fn selected_rule(&self) -> Option<&Rule> {
        self.rules_selected.and_then(|i| self.cfg.rules.get(i))
    }

    fn rule_can_move(&self, delta: isize) -> bool {
        match self.rules_selected {
            Some(i) => {
                let len = self.cfg.rules.len() as isize;
                let i = i as isize;
                (delta < 0 && i > 0) || (delta > 0 && i + 1 < len)
            }
            None => false,
        }
    }

    /// true si la regla seleccionada puede agruparse con la que esta por
    /// encima en la lista (necesita una regla anterior).
    fn rule_can_group(&self) -> bool {
        match self.rules_selected {
            Some(i) => i > 0,
            None => false,
        }
    }

    /// true si la regla seleccionada forma parte de un grupo de pestanas.
    fn selected_is_grouped(&self) -> bool {
        match self.rules_selected {
            Some(i) => self.cfg.is_grouped(self.cfg.rules[i].id.as_str()),
            None => false,
        }
    }

    fn edit_text(&self, id: u16) -> Option<String> {
        let hwnd = self.edits.get(&id).copied()?;
        Some(get_text(hwnd))
    }

    fn build_focus_order(&mut self) {
        let mut order = vec![
            Ctrl::Nav(Panel::General),
            Ctrl::Nav(Panel::Rules),
            Ctrl::Nav(Panel::Appearance),
            Ctrl::Nav(Panel::Language),
            Ctrl::Nav(Panel::Ai),
            Ctrl::Nav(Panel::Updates),
        ];
        match self.panel {
            Panel::General => {
                order.extend([
                    Ctrl::Check(ID_CHECK_ORGANIZE_FOLDERS),
                    Ctrl::Check(ID_CHECK_ORGANIZE_START),
                    Ctrl::Check(ID_CHECK_PUBLIC),
                    Ctrl::Check(ID_CHECK_SHORTCUTS),
                    Ctrl::Check(ID_CHECK_STARTUP),
                    Ctrl::Field(ID_EDIT_STARTUP_DELAY),
                    Ctrl::Check(ID_CHECK_ZEN_DBL),
                    Ctrl::Check(ID_CHECK_ZEN_HOTKEY),
                    Ctrl::Check(ID_CHECK_ZEN_HIDE),
                    Ctrl::Check(ID_CHECK_ARCHIVE),
                    Ctrl::Field(ID_EDIT_MAX_AGE),
                    Ctrl::Field(ID_EDIT_MIN_AGE),
                    Ctrl::Check(ID_CHECK_BY_MONTH),
                    Ctrl::Field(ID_EDIT_PURGE),
                    Ctrl::Btn(ID_BTN_EXPORT_CFG),
                    Ctrl::Btn(ID_BTN_IMPORT_CFG),
                    Ctrl::Field(ID_EDIT_TEMPLATE_NAME),
                    Ctrl::Btn(ID_BTN_TEMPLATE_SAVE),
                    Ctrl::Btn(ID_BTN_TEMPLATE_APPLY),
                    Ctrl::Btn(ID_BTN_TEMPLATE_DEL),
                    Ctrl::Btn(ID_BTN_TEMPLATE_DEFAULT),
                ]);
            }
            Panel::Rules => {
                order.extend([
                    Ctrl::Btn(ID_BTN_NEW),
                    Ctrl::Btn(ID_BTN_DEL),
                    Ctrl::Btn(ID_BTN_GROUP),
                    Ctrl::Btn(ID_BTN_UNGROUP),
                    Ctrl::Btn(ID_BTN_UP),
                    Ctrl::Btn(ID_BTN_DOWN),
                    Ctrl::RuleRow(0),
                    Ctrl::Check(ID_CHECK_R_ENABLED),
                    Ctrl::Check(ID_CHECK_R_MOVE),
                    Ctrl::Check(ID_CHECK_R_FOLDERS),
                    Ctrl::Field(ID_EDIT_R_AI_DESC),
                    Ctrl::Btn(ID_BTN_R_AI_GEN),
                    Ctrl::Field(ID_EDIT_R_TITLE),
                    Ctrl::Field(ID_EDIT_R_GROUP_TITLE),
                    Ctrl::Field(ID_EDIT_R_FOLDER),
                    Ctrl::Field(ID_EDIT_R_COLOR),
                    Ctrl::Field(ID_EDIT_R_EXTS),
                    Ctrl::Field(ID_EDIT_R_PATTERNS),
                    Ctrl::Field(ID_EDIT_R_MIN_SIZE),
                    Ctrl::Field(ID_EDIT_R_MAX_SIZE),
                    Ctrl::Field(ID_EDIT_R_NEWER),
                    Ctrl::Field(ID_EDIT_R_OLDER),
                    Ctrl::Field(ID_EDIT_R_REGEX),
                ]);
            }
            Panel::Appearance => {
                order.extend([
                    Ctrl::Field(ID_EDIT_A_BG),
                    Ctrl::Field(ID_EDIT_A_HOVER),
                    Ctrl::Field(ID_EDIT_A_BORDER),
                    Ctrl::Field(ID_EDIT_A_TITLE),
                    Ctrl::Field(ID_EDIT_A_TEXT),
                    Ctrl::Field(ID_EDIT_A_MUTED),
                    Ctrl::Field(ID_EDIT_A_SHADOW),
                    Ctrl::Field(ID_EDIT_A_RADIUS),
                    Ctrl::Field(ID_EDIT_A_TITLE_SIZE),
                    Ctrl::Field(ID_EDIT_A_TEXT_SIZE),
                    Ctrl::Field(ID_EDIT_A_SNAP),
                    Ctrl::Check(ID_CHECK_A_ICONS),
                    Ctrl::Check(ID_CHECK_A_COUNTER),
                    Ctrl::Check(ID_CHECK_A_SEARCH),
                    Ctrl::Check(ID_CHECK_A_GRID),
                ]);
            }
            Panel::Language => {}
            Panel::Ai => {
                order.extend([
                    Ctrl::Check(ID_CHECK_AI_ENABLE),
                    Ctrl::Field(ID_EDIT_AI_URL),
                    Ctrl::Field(ID_EDIT_AI_MODEL),
                    Ctrl::Field(ID_EDIT_AI_EMBED_MODEL),
                    Ctrl::Btn(ID_BTN_AI_PING),
                ]);
            }
            Panel::Updates => {
                order.extend([
                    Ctrl::Check(ID_CHECK_AUTO_UPDATE),
                    Ctrl::Btn(ID_BTN_CHECK_UPDATES),
                    Ctrl::Btn(ID_BTN_DOWNLOAD_UPDATE),
                ]);
            }
            Panel::Widgets => {
                order.extend([
                    Ctrl::Btn(ID_BTN_WIDGET_NEW),
                    Ctrl::Btn(ID_BTN_WIDGET_DEL),
                    Ctrl::Btn(ID_BTN_WIDGET_RELOAD),
                    Ctrl::Btn(ID_BTN_WIDGET_ADD),
                    Ctrl::Btn(ID_BTN_WIDGET_REMOVE),
                    Ctrl::WidgetRow(0),
                    Ctrl::Field(ID_EDIT_WIDGET_NAME),
                    Ctrl::Check(ID_CHECK_WIDGET_ENABLED),
                    Ctrl::Field(ID_EDIT_WIDGET_CODE),
                    Ctrl::Btn(ID_BTN_WIDGET_SAVE),
                ]);
            }
        }
        order.push(Ctrl::Btn(ID_BTN_OK));
        order.push(Ctrl::Btn(ID_BTN_CANCEL));
        self.focus_order = order;
        if self.focus.is_none() {
            self.focus = Some(0);
        }
    }

    fn advance_focus(&mut self, delta: isize) {
        if self.focus_order.is_empty() {
            return;
        }
        let len = self.focus_order.len() as isize;
        let cur = self.focus.unwrap_or(0) as isize;
        let step = if delta == 0 { 1 } else { delta };
        let mut next = (cur + step).rem_euclid(len);
        for _ in 0..len {
            let c = self.focus_order[next as usize];
            if self.focusable(c) {
                if let Ctrl::Field(id) = c {
                    if let Some(edit) = self.edits.get(&id).copied() {
                        unsafe { let _ = SetFocus(edit); }
                    }
                    self.focus = None;
                    return;
                }
                // Al volver del campo de texto a un control custom, devolver
                // el foco real al dialogo para que el anillo y el teclado
                // (flechas/espacio/enter) apunten al control correcto.
                unsafe { let _ = SetFocus(self.hwnd); }
                self.focus = Some(next as usize);
                return;
            }
            next = (next + step).rem_euclid(len);
        }
        self.focus = None;
    }

    fn focusable(&self, c: Ctrl) -> bool {
        match c {
            Ctrl::RuleRow(0) => !self.cfg.rules.is_empty(),
            Ctrl::RuleSort(0) => !self.cfg.rules.is_empty(),
            Ctrl::WidgetRow(0) => !self.widgets_list.is_empty(),
            Ctrl::Btn(ID_BTN_DEL) => self.rules_selected.is_some(),
            Ctrl::Btn(ID_BTN_WIDGET_DEL) | Ctrl::Btn(ID_BTN_WIDGET_SAVE) => self.widgets_selected.is_some(),
            Ctrl::Btn(ID_BTN_WIDGET_ADD) => self.widget_can_add(),
            Ctrl::Btn(ID_BTN_WIDGET_REMOVE) => self.widget_can_remove(),
            Ctrl::Btn(ID_BTN_GROUP) => self.rule_can_group(),
            Ctrl::Btn(ID_BTN_UNGROUP) => self.selected_is_grouped(),
            Ctrl::Btn(ID_BTN_UP) | Ctrl::Btn(ID_BTN_DOWN) => self.rule_can_move(if c == Ctrl::Btn(ID_BTN_UP) { -1 } else { 1 }),
            _ => true,
        }
    }

    fn activate(&mut self, ctrl: Ctrl) {
        match ctrl {
            Ctrl::Close | Ctrl::Btn(ID_BTN_CANCEL) => {
                self.result = false;
                self.revert_preview();
                self.finish();
            }
            Ctrl::Minimize => {
                unsafe { let _ = ShowWindow(self.hwnd, SW_MINIMIZE); }
            }
            Ctrl::Nav(panel) => {
                self.panel = panel;
                self.scroll = 0.0;
                self.invalidate();
            }
            Ctrl::Check(id) => {
                let v = self.checked(id);
                self.checks.insert(id, !v);
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_OK) => self.save_and_close(),
            Ctrl::Btn(ID_BTN_AI_PING) => self.test_ollama_connection(),
            Ctrl::Btn(ID_BTN_AI_DETECT_MODELS) => self.detect_models(),
            Ctrl::Btn(ID_BTN_AI_REORGANIZE) => self.reorganize_with_ai(),
            Ctrl::Btn(ID_BTN_R_AI_GEN) => self.generate_rule_with_ai(),
            Ctrl::Btn(ID_BTN_CHECK_UPDATES) => self.check_for_updates(),
            Ctrl::Btn(ID_BTN_DOWNLOAD_UPDATE) => self.download_update(),
            Ctrl::Btn(ID_BTN_EXPORT_CFG) => self.export_config(),
            Ctrl::Btn(ID_BTN_IMPORT_CFG) => self.import_config(),
            Ctrl::Btn(ID_BTN_TEMPLATE_SAVE) => self.template_save(),
            Ctrl::Btn(ID_BTN_TEMPLATE_APPLY) => self.template_apply(),
            Ctrl::Btn(ID_BTN_TEMPLATE_DEL) => self.template_delete(),
            Ctrl::Btn(ID_BTN_TEMPLATE_DEFAULT) => self.template_set_default(),
            Ctrl::Template(i) => self.template_apply_index(i),
            Ctrl::WidgetRow(idx) => self.widget_select(idx),
            Ctrl::Btn(ID_BTN_WIDGET_NEW) => self.widget_new(),
            Ctrl::Btn(ID_BTN_WIDGET_DEL) => self.widget_delete(),
            Ctrl::Btn(ID_BTN_WIDGET_SAVE) => self.widget_save(),
            Ctrl::Btn(ID_BTN_WIDGET_RELOAD) => self.widget_reload(),
            Ctrl::Btn(ID_BTN_WIDGET_ADD) => self.widget_add_as_box(),
            Ctrl::Btn(ID_BTN_WIDGET_REMOVE) => self.widget_remove_box(),
            Ctrl::Folder(id) => {
                self.pick_folder(id);
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_NEW) => {
                self.new_rule();
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_DEL) => {
                self.delete_rule();
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_GROUP) => {
                self.group_rule();
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_UNGROUP) => {
                self.ungroup_rule();
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_UP) => {
                self.move_rule(-1);
                self.preview_apply();
            }
            Ctrl::Btn(ID_BTN_DOWN) => {
                self.move_rule(1);
                self.preview_apply();
            }
            Ctrl::RuleSort(idx) => {
                    let current = self.rules_sort.get(&idx).and_then(|m| m.as_deref()).unwrap_or(&self.cfg.appearance.sort_by);
                    let next = crate::rules::next_sort_mode(current);
                    // "global" -> None: borra la preferencia por caja y usa el valor de Apariencia.
                    if next == "global" {
                        self.rules_sort.insert(idx, None);
                    } else {
                        self.rules_sort.insert(idx, Some(next.to_string()));
                    }
                    self.preview_apply();
                }
            Ctrl::RuleView(idx, value) => {
                if let Some(rule) = self.cfg.rules.get_mut(idx) {
                    rule.view_mode = value.to_string();
                }
                self.preview_apply();
            }
            Ctrl::RuleIcon(idx) => {
                if let Some(rule) = self.cfg.rules.get_mut(idx) {
                    rule.icon_size = next_icon_size(rule.icon_size);
                }
                self.preview_apply();
            }
            Ctrl::RuleRow(idx) => {
                self.sync_rule_fields();
                self.rules_selected = Some(idx);
                self.refresh_rule_fields();
                self.invalidate();
            }
            Ctrl::Field(id) => {
                if let Some(edit) = self.edits.get(&id).copied() {
                    unsafe { let _ = SetFocus(edit); }
                }
            }
            Ctrl::Lang(lang) => {
                self.set_lang(lang);
                self.preview_apply();
            }
            Ctrl::Theme(key) => {
                self.cfg.appearance.apply_preset(key);
                self.refresh_appearance_fields();
                self.hover = None;
                self.preview_apply();
            }
            _ => {}
        }
    }

    fn test_ollama_connection(&self) {
        if AI_BUSY.swap(true, Ordering::SeqCst) {
            return; // Ya hay una operacion de IA en curso.
        }
        let host_url = self.edit_text(ID_EDIT_AI_URL).unwrap_or_else(|| self.cfg.ai.ollama_url.clone());
        let host_clean = host_url
            .replace("http://", "")
            .replace("https://", "");
        let parts: Vec<&str> = host_clean.split(':').collect();
        let host = parts.first().copied().unwrap_or("127.0.0.1").trim().to_string();
        let port = parts.get(1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(11434);
        let model = self.edit_text(ID_EDIT_AI_MODEL).unwrap_or_else(|| self.cfg.ai.model.clone());
        let hwnd = self.hwnd.0 as usize;

        // El ping es de red: fuera del hilo de UI para no congelar el diálogo.
        let _ = std::thread::spawn(move || {
            let hwnd = HWND(hwnd as *mut c_void);
            let client = crate::ai::AiClient { host, port, model };
            let ok = client.ping();
            if let Ok(mut slot) = AI_PING_RESULT.lock() {
                *slot = Some(ok);
            }
            let _ = unsafe { PostMessageW(hwnd, WM_AI_PING_DONE, WPARAM(0), LPARAM(0)) };
        });
    }

    fn on_ai_ping_done(&self) {
        AI_BUSY.store(false, Ordering::SeqCst);
        // Sin resultado (dialogo cerrado o mensaje duplicado): ignorar.
        let Some(ok) = AI_PING_RESULT.lock().ok().and_then(|mut s| s.take()) else { return };
        let msg = if ok {
            "🟢 Conexión exitosa con Ollama!\n\nEl servidor responde correctamente en la dirección configurada."
        } else {
            "🔴 No se pudo conectar a Ollama.\n\nAsegúrate de que el servidor Ollama esté en marcha en tu equipo o red local."
        };
        unsafe {
            let body = crate::config::wide(msg);
            let title = crate::config::wide("ZenDesktop :: Ollama IA Test");
            MessageBoxW(
                self.hwnd,
                PCWSTR(body.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | if ok { MB_ICONINFORMATION } else { MB_ICONWARNING },
            );
        }
    }

    fn detect_models(&mut self) {
        if AI_BUSY.swap(true, Ordering::SeqCst) {
            return; // Ya hay una operacion de IA en curso.
        }
        let host_url = self.edit_text(ID_EDIT_AI_URL).unwrap_or_else(|| self.cfg.ai.ollama_url.clone());
        let host_clean = host_url.replace("http://", "").replace("https://", "");
        let parts: Vec<&str> = host_clean.split(':').collect();
        let host = parts.first().copied().unwrap_or("127.0.0.1").trim().to_string();
        let port = parts.get(1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(11434);
        let hwnd = self.hwnd.0 as usize;

        // Listar modelos es una llamada de red: fuera del hilo de UI.
        let _ = std::thread::spawn(move || {
            let hwnd = HWND(hwnd as *mut c_void);
            let client = crate::ai::AiClient {
                host,
                port,
                model: String::from("llama3.2"),
            };
            let models = client.list_models();
            if let Ok(mut slot) = AI_MODELS_RESULT.lock() {
                *slot = Some(models);
            }
            let _ = unsafe { PostMessageW(hwnd, WM_AI_MODELS_DONE, WPARAM(0), LPARAM(0)) };
        });
    }

    fn on_ai_models_done(&mut self) {
        AI_BUSY.store(false, Ordering::SeqCst);
        // Sin resultado (dialogo cerrado o mensaje duplicado): ignorar.
        let Some(models) = AI_MODELS_RESULT.lock().ok().and_then(|mut s| s.take()) else { return };
        if !models.is_empty() {
            let selected = if models.len() == 1 {
                models[0].clone()
            } else if let Some(chosen) = self.pick_model_popup(&models) {
                chosen
            } else {
                models[0].clone()
            };

            if let Some(&edit) = self.edits.get(&ID_EDIT_AI_MODEL) {
                set_text(edit, &selected);
            }
            self.cfg.ai.model = selected.clone();
            self.invalidate();
        } else {
            unsafe {
                let body = crate::config::wide("🔴 No se encontraron modelos cargados en Ollama. Asegúrate de ejecutar 'ollama pull llama3.2' o similar en tu consola.");
                let title = crate::config::wide("ZenDesktop :: Sin Modelos");
                MessageBoxW(
                    self.hwnd,
                    PCWSTR(body.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    }

    fn pick_model_popup(&self, models: &[String]) -> Option<String> {
        if models.is_empty() { return None; }
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            for (i, m) in models.iter().enumerate() {
                let item_text = crate::config::wide(m);
                let _ = AppendMenuW(menu, MF_STRING, i + 1, PCWSTR(item_text.as_ptr()));
            }

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);

            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN,
                pt.x,
                pt.y,
                0,
                self.hwnd,
                None,
            );

            let _ = DestroyMenu(menu);

            if cmd.0 > 0 && (cmd.0 as usize) <= models.len() {
                Some(models[cmd.0 as usize - 1].clone())
            } else {
                None
            }
        }
    }

    fn reorganize_with_ai(&mut self) {
        if self.app.is_null() { return; }
        if AI_BUSY.swap(true, Ordering::SeqCst) {
            return; // Ya hay una operacion de IA en curso.
        }
        let app = unsafe { &mut *self.app };
        
        let host_url = self.edit_text(ID_EDIT_AI_URL).unwrap_or_else(|| self.cfg.ai.ollama_url.clone());
        let host_clean = host_url.replace("http://", "").replace("https://", "");
        let parts: Vec<&str> = host_clean.split(':').collect();
        let host = parts.first().copied().unwrap_or("127.0.0.1").trim();
        let port = parts.get(1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(11434);

        let client_host = host.to_string();
        let client_port = port;
        let client_model = self.edit_text(ID_EDIT_AI_MODEL).unwrap_or_else(|| self.cfg.ai.model.clone());
        let embed_model = self.edit_text(ID_EDIT_AI_EMBED_MODEL).unwrap_or_else(|| self.cfg.ai.embed_model.clone());
        let language = self.cfg.language.clone();
        let hwnd = self.hwnd.0 as usize;

        // Recopilar nombres de archivos del escritorio
        let desktop_path = app.desktop.clone();
        let mut filenames = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&desktop_path) {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if !name.starts_with('.') && name != "desktop.ini" {
                        filenames.push(name);
                    }
                }
            }
        }

        if filenames.is_empty() {
            unsafe {
                let body = crate::config::wide("ℹ️ Tu escritorio ya está limpio o no contiene archivos sueltos para reorganizar.");
                let title = crate::config::wide("ZenDesktop :: IA");
                MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
            }
            return;
        }

        // La generación con el modelo (puede tardar segundos, más si Ollama
        // está en otro equipo) corre en un hilo; la UI queda libre.
        let _ = std::thread::spawn(move || {
            let hwnd = HWND(hwnd as *mut c_void);
            let client = crate::ai::AiClient {
                host: client_host,
                port: client_port,
                model: client_model,
            };
            let suggestions = client.auto_cluster_desktop(&filenames, &embed_model, &language);
            if let Ok(mut slot) = AI_CLUSTER_RESULT.lock() {
                *slot = Some(suggestions);
            }
            let _ = unsafe { PostMessageW(hwnd, WM_AI_CLUSTER_DONE, WPARAM(0), LPARAM(0)) };
        });
    }

    fn on_ai_cluster_done(&mut self) {
        AI_BUSY.store(false, Ordering::SeqCst);
        if self.app.is_null() { return; }
        let app = unsafe { &mut *self.app };
        let Some(suggestions) = AI_CLUSTER_RESULT.lock().ok().and_then(|mut s| s.take()) else { return };
        if suggestions.is_empty() {
            unsafe {
                let body = crate::config::wide("🔴 No se pudo obtener la propuesta de categorías de Ollama. Verifica que el modelo esté activo.");
                let title = crate::config::wide("ZenDesktop :: Error IA");
                MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONWARNING);
            }
            return;
        }

        // Aplicar sugerencias creando nuevas reglas
        let mut new_rules = Vec::new();
        for (i, sug) in suggestions.iter().enumerate() {
            new_rules.push(Rule {
                id: format!("ai-rule-{}", i + 1),
                title: sug.title.clone(),
                enabled: true,
                extensions: sug.extensions.clone(),
                name_patterns: sug.name_patterns.clone(),
                folder: sug.folder.clone(),
                move_files: true,
                include_folders: true,
                color: if sug.color.starts_with('#') { sug.color.clone() } else { "#38BDF8".into() },
                view_mode: "auto".into(),
                icon_size: None,
                min_size_bytes: None,
                max_size_bytes: None,
                newer_than_days: None,
                older_than_days: None,
                regex: None,
                pinned: Vec::new(),
            });
        }

        self.cfg.rules = new_rules;
        self.cfg.ai.enabled = true;
        self.cfg.general.organize_folders = true;
        self.cfg.general.keep_shortcuts = false;
        self.rules_selected = Some(0);
        self.refresh_rule_fields();

        // Aplicar la nueva configuración inmediatamente a la app y organizar en vivo
        let cfg_to_apply = self.cfg.clone();
        app.apply_dialog_cfg(cfg_to_apply, true);

        self.invalidate();

        unsafe {
            let msg = format!("✨ La IA de Ollama ha creado {} cajas nuevas adaptadas a tu escritorio y ha reestructurado todo.", suggestions.len());
            let body = crate::config::wide(&msg);
            let title = crate::config::wide("ZenDesktop :: Reorganización IA Completada");
            MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
        }
    }

    /// Genera una regla a partir de la descripcion en lenguaje natural escrita
    /// en el panel Reglas (modelo local, fuera del hilo de UI).
    fn generate_rule_with_ai(&mut self) {
        if self.rules_selected.is_none() { return; }
        if AI_BUSY.swap(true, Ordering::SeqCst) {
            return; // Ya hay una operacion de IA en curso.
        }
        let desc = self.edit_text(ID_EDIT_R_AI_DESC).unwrap_or_default();
        if desc.trim().is_empty() {
            AI_BUSY.store(false, Ordering::SeqCst);
            unsafe {
                let body = crate::config::wide("ℹ️ Escribe primero una descripción de la regla (ej. 'los archivos que parezcan facturas van a Facturas').");
                let title = crate::config::wide("ZenDesktop :: IA");
                MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
            }
            return;
        }

        let host_url = self.edit_text(ID_EDIT_AI_URL).unwrap_or_else(|| self.cfg.ai.ollama_url.clone());
        let host_clean = host_url.replace("http://", "").replace("https://", "");
        let parts: Vec<&str> = host_clean.split(':').collect();
        let host = parts.first().copied().unwrap_or("127.0.0.1").trim().to_string();
        let port = parts.get(1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(11434);
        let client_model = self.edit_text(ID_EDIT_AI_MODEL).unwrap_or_else(|| self.cfg.ai.model.clone());
        let language = self.cfg.language.clone();
        let hwnd = self.hwnd.0 as usize;

        let _ = std::thread::spawn(move || {
            let hwnd = HWND(hwnd as *mut c_void);
            let client = crate::ai::AiClient {
                host,
                port,
                model: client_model,
            };
            let draft = client.generate_rule_from_text(&desc, &language);
            if let Ok(mut slot) = AI_RULE_RESULT.lock() {
                *slot = draft;
            }
            let _ = unsafe { PostMessageW(hwnd, WM_AI_RULE_DONE, WPARAM(0), LPARAM(0)) };
        });
    }

    fn on_ai_rule_done(&mut self) {
        AI_BUSY.store(false, Ordering::SeqCst);
        let Some(draft) = AI_RULE_RESULT.lock().ok().and_then(|mut s| s.take()) else { return };
        let Some(index) = self.rules_selected else {
            unsafe {
                let body = crate::config::wide("🔴 No hay ninguna regla seleccionada para aplicar la propuesta.");
                let title = crate::config::wide("ZenDesktop :: Error IA");
                MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONWARNING);
            }
            return;
        };
        // Aplicar el borrador a la regla seleccionada y refrescar los campos.
        let rule = &mut self.cfg.rules[index];
        rule.title = draft.title;
        rule.folder = draft.folder;
        rule.extensions = draft.extensions;
        rule.name_patterns = draft.name_patterns;
        rule.regex = draft.regex;
        self.refresh_rule_fields();
        self.invalidate();
    }

    fn check_for_updates(&self) {
        if UPDATE_BUSY.swap(true, Ordering::SeqCst) {
            return; // Ya hay una comprobacion en curso.
        }
        let hwnd = self.hwnd.0 as usize;

        // La comprobación es una llamada de red (reintentos incluidos):
        // fuera del hilo de UI para que el diálogo no se congele.
        let _ = std::thread::spawn(move || {
            let hwnd = HWND(hwnd as *mut c_void);
            let status = crate::updater::check_update();
            if let Ok(mut slot) = UPDATE_RESULT.lock() {
                *slot = Some(status);
            }
            let _ = unsafe { PostMessageW(hwnd, WM_UPDATE_DONE, WPARAM(0), LPARAM(0)) };
        });
    }

    fn on_update_done(&self) {
        UPDATE_BUSY.store(false, Ordering::SeqCst);
        // Sin resultado (dialogo cerrado o mensaje duplicado): ignorar.
        let Some(status) = UPDATE_RESULT.lock().ok().and_then(|mut s| s.take()) else { return };
        match status {
            crate::updater::UpdateStatus::UpToDate => {
                // Sin modales: un toast verde confirma que esta al dia.
                if !self.app.is_null() {
                    unsafe {
                        (*self.app).show_toast(self.tr.toast_up_to_date, crate::ui::TOAST_DROP);
                    }
                }
            }
            crate::updater::UpdateStatus::UpdateAvailable { version, url, sig_url, .. } => {
                // El toast es clicable: un clic instala la actualizacion.
                crate::updater::set_pending_update(url, sig_url);
                if !self.app.is_null() {
                    let msg = self.tr.toast_update.replacen("{0}", &version, 1);
                    unsafe {
                        (*self.app).show_toast_glyph(&msg, crate::ui::TOAST_UPDATE, '\u{2193}');
                    }
                }
            }
            crate::updater::UpdateStatus::Error(e) => {
                // Error de red real: aviso no intrusivo, no "estas al dia".
                if !self.app.is_null() {
                    let msg = format!("{}: {}", self.tr.toast_update_failed, e);
                    unsafe {
                        (*self.app).show_toast_glyph(&msg, crate::ui::TOAST_ERROR, '\u{2715}');
                    }
                }
            }
        }
    }

    /// Exporta la configuracion actual a un archivo TOML elegido por el usuario.
    fn template_name(&self) -> String {
        self.edit_text(ID_EDIT_TEMPLATE_NAME).unwrap_or_default().trim().to_string()
    }

    fn template_save(&mut self) {
        let name = self.template_name();
        if name.is_empty() {
            return;
        }
        if !self.app.is_null() {
            unsafe {
                (*self.app).save_layout_template(&name);
            }
            // Refrescar el cfg local para que el chip aparezca al instante.
            let layouts = unsafe { (*self.app).capture_layouts() };
            if let Some(t) = self.cfg.templates.iter_mut().find(|t| t.name == name) {
                t.layouts = layouts;
            } else {
                self.cfg.templates.push(LayoutTemplate {
                    name: name.clone(),
                    layouts,
                    default: false,
                });
            }
        }
        self.invalidate();
    }

    fn template_apply(&mut self) {
        let name = self.template_name();
        if name.is_empty() {
            return;
        }
        self.template_apply_named(&name);
    }

    fn template_apply_index(&mut self, i: usize) {
        if let Some(t) = self.cfg.templates.get(i) {
            let name = t.name.clone();
            if let Some(edit) = self.edits.get(&ID_EDIT_TEMPLATE_NAME).copied() {
                set_text(edit, &name);
            }
            self.template_apply_named(&name);
        }
    }

    fn template_apply_named(&mut self, name: &str) {
        if self.app.is_null() {
            return;
        }
        let ok = unsafe { (*self.app).apply_layout_template(name) };
        if !ok {
            self.warn(&self.tr.warn_template_missing.replace("{name}", name));
        }
    }

    fn template_delete(&mut self) {
        let name = self.template_name();
        if name.is_empty() {
            return;
        }
        self.cfg.templates.retain(|t| t.name != name);
        if !self.app.is_null() {
            unsafe {
                (*self.app).delete_layout_template(&name);
            }
        }
        self.invalidate();
    }

    /// Marca la plantilla nombrada como por defecto (se aplicara sola al
    /// arrancar o al conectar su disposicion de monitores).
    fn template_set_default(&mut self) {
        let name = self.template_name();
        if name.is_empty() {
            return;
        }
        if !self.cfg.templates.iter().any(|t| t.name == name) {
            self.warn(&self.tr.warn_template_missing.replace("{name}", &name));
            return;
        }
        for t in &mut self.cfg.templates {
            t.default = t.name == name;
        }
        if !self.app.is_null() {
            unsafe {
                (*self.app).set_default_template(&name);
            }
        }
        self.invalidate();
    }

    fn export_config(&mut self) {
        let Some(path) = save_file_dialog(self.hwnd, self.tr.btn_export_cfg, "zendesktop-config.toml") else {
            return;
        };
        // Exporta lo que hay en pantalla (incluidos los cambios sin guardar).
        let cfg = self.collect_cfg().unwrap_or_else(|| self.cfg.clone());
        match cfg.save(&path) {
            Ok(()) => {
                if !self.app.is_null() {
                    unsafe {
                        (*self.app).show_toast(self.tr.msg_cfg_exported, crate::ui::TOAST_DROP);
                    }
                }
            }
            Err(e) => {
                let msg = format!("Export failed:\n{e}");
                unsafe {
                    let body = crate::config::wide(&msg);
                    MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), w!("ZenDesktop"), MB_OK | MB_ICONERROR);
                }
            }
        }
    }

    /// Importa una configuracion desde un archivo TOML y la aplica en caliente.
    fn import_config(&mut self) {
        let Some(path) = open_file_dialog(self.hwnd, self.tr.btn_import_cfg) else {
            return;
        };
        match Config::reload(&path) {
            Err(e) => {
                let msg = self.tr.err_cfg_invalid.replacen("{0}", &format!("{e}"), 1);
                unsafe {
                    let body = crate::config::wide(&msg);
                    MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), w!("ZenDesktop"), MB_OK | MB_ICONERROR);
                }
            }
            Ok(cfg) => {
                self.cfg = cfg;
                self.lang = self.cfg.lang();
                self.tr = Tr::get(self.lang);
                self.checks = initial_checks(&self.cfg);
                self.rules_selected = None;
                self.rules_scroll = 0;
                self.rules_sort.clear();
                self.refresh_rule_fields();
                seed_edits(self);
                self.build_focus_order();
                if !self.app.is_null() {
                    // Guarda en disco, aplica en caliente y organiza.
                    unsafe { (*self.app).apply_dialog_cfg(self.cfg.clone(), true); }
                }
                self.invalidate();
                unsafe {
                    let body = crate::config::wide(self.tr.msg_cfg_imported);
                    MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), w!("ZenDesktop"), MB_OK | MB_ICONINFORMATION);
                }
            }
        }
    }

    fn download_update(&self) {
        // Primero comprobar que hay update disponible
        match crate::updater::check_update() {
            crate::updater::UpdateStatus::UpdateAvailable { url, sig_url, .. } => {
                match crate::updater::download_and_install(&url, &sig_url) {
                    Ok(_path) => {
                        // El ejecutable ya esta reemplazado y la nueva version
                        // lanzada. Cerrar el dialogo y salir: al soltar el mutex
                        // de instancia unica, la nueva toma el relevo.
                        if !self.app.is_null() {
                            unsafe {
                                (*self.app).request_exit_after_settings();
                                let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("Download failed:\n{}", e);
                        unsafe {
                            let body = crate::config::wide(&msg);
                            let title = crate::config::wide("ZenDesktop :: Download Error");
                            MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR);
                        }
                    }
                }
            }
            crate::updater::UpdateStatus::UpToDate => {
                unsafe {
                    let body = crate::config::wide("You already have the latest version.");
                    let title = crate::config::wide("ZenDesktop :: Updates");
                    MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
                }
            }
            crate::updater::UpdateStatus::Error(e) => {
                let msg = format!("Error: {}", e);
                unsafe {
                    let body = crate::config::wide(&msg);
                    let title = crate::config::wide("ZenDesktop :: Error");
                    MessageBoxW(self.hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONWARNING);
                }
            }
        }
    }

    fn refresh_appearance_fields(&mut self) {
        let fields = [
            (ID_EDIT_A_BG, self.cfg.appearance.background.clone()),
            (ID_EDIT_A_HOVER, self.cfg.appearance.background_hover.clone()),
            (ID_EDIT_A_BORDER, self.cfg.appearance.border.clone()),
            (ID_EDIT_A_TITLE, self.cfg.appearance.title_color.clone()),
            (ID_EDIT_A_TEXT, self.cfg.appearance.text_color.clone()),
            (ID_EDIT_A_MUTED, self.cfg.appearance.muted_color.clone()),
            (ID_EDIT_A_SHADOW, self.cfg.appearance.shadow.clone()),
        ];
        for (id, val) in fields {
            if let Some(&hwnd) = self.edits.get(&id) {
                set_text(hwnd, &val);
            }
        }
    }

    /// Cambia el idioma del dialogo en caliente (se persiste al guardar).
    fn set_lang(&mut self, lang: Lang) {
        if self.lang != lang {
            self.lang = lang;
            self.tr = Tr::get(lang);
            self.invalidate();
        }
    }

    fn new_rule(&mut self) {
        self.sync_rule_fields();
        let rule = Rule {
            id: format!("regla-{}", self.cfg.rules.len() + 1),
            title: self.tr.new_rule_title.into(),
            enabled: true,
            move_files: true,
            folder: "Nueva".into(),
            color: "#38BDF8".into(),
            view_mode: "auto".into(),
            ..Default::default()
        };
        self.cfg.rules.push(rule);
        self.rules_selected = Some(self.cfg.rules.len() - 1);
        self.rules_scroll = usize::MAX;
        self.refresh_rule_fields();
        self.invalidate();
    }


    /// Saca `rule_id` de cualquier grupo al que pertenezca. Si el grupo se
    /// queda con una sola pestana, vuelve a ser una caja independiente para esa
    /// pestana; si se queda vacio, se elimina. El `id` de cada grupo se
    /// mantiene siempre alineado con su primera pestana.
    fn remove_rule_from_groups(&mut self, rule_id: &str) {
        let mut touched: Vec<usize> = Vec::new();
        for (i, f) in self.cfg.fences.iter_mut().enumerate() {
            if f.tabs.iter().any(|t| t == rule_id) {
                f.tabs.retain(|t| t != rule_id);
                touched.push(i);
            }
        }
        // De mayor a menor para poder eliminar sin descolocar los indices.
        touched.sort_unstable_by(|a, b| b.cmp(a));
        for i in touched {
            let len = self.cfg.fences[i].tabs.len();
            if len >= 2 {
                let first = self.cfg.fences[i].tabs[0].clone();
                self.cfg.fences[i].id = crate::config::String32::new(&first);
            } else if len == 1 {
                let only = self.cfg.fences[i].tabs[0].clone();
                self.cfg.fences[i].id = crate::config::String32::new(&only);
                self.cfg.fences[i].tabs.clear();
                self.cfg.fences[i].group_title = None;
            } else {
                self.cfg.fences.remove(i);
            }
        }
    }

    /// Agrupa la regla seleccionada con la inmediatamente superior, creando (o
    /// ampliando) una caja de pestanas. Si la regla ya estaba agrupada, sale de
    /// su grupo anterior antes de unirse al nuevo.
    fn group_rule(&mut self) {
        self.sync_rule_fields();
        let Some(idx) = self.rules_selected else { return };
        if idx == 0 {
            return;
        }
        let current_id = self.cfg.rules[idx].id.clone();
        let prev_id = self.cfg.rules[idx - 1].id.clone();
        if current_id == prev_id {
            return;
        }
        // Si ya estan agrupadas juntas, no hay nada que hacer (evita reordenar).
        if self
            .cfg
            .group_of(&current_id)
            .is_some_and(|g| g.tabs.iter().any(|t| t == &prev_id))
        {
            return;
        }

        // 1) Saca `current_id` de cualquier grupo previo.
        self.remove_rule_from_groups(&current_id);

        // 2) Busca la caja de `prev_id`.
        let prev_group = self
            .cfg
            .fences
            .iter()
            .position(|f| f.tabs.iter().any(|t| t == &prev_id));
        let prev_standalone = if prev_group.is_none() {
            self.cfg
                .fences
                .iter()
                .position(|f| f.id.as_str() == prev_id.as_str() && f.tabs.is_empty())
        } else {
            None
        };

        if let Some(gi) = prev_group {
            // prev ya es un grupo: anade current como pestana.
            if !self.cfg.fences[gi].tabs.contains(&current_id) {
                self.cfg.fences[gi].tabs.push(current_id.clone());
            }
        } else if let Some(si) = prev_standalone {
            // prev es una caja independiente: conviertela en grupo [prev, current].
            self.cfg.fences[si].tabs = vec![prev_id.clone(), current_id.clone()];
            self.cfg.fences[si].group_title = None;
        } else {
            // prev no tiene caja persistida: crea un grupo nuevo.
            let fl = crate::config::FenceLayout {
                id: crate::config::String32::new(&prev_id),
                tabs: vec![prev_id.clone(), current_id.clone()],
                ..Default::default()
            };
            self.cfg.fences.push(fl);
        }

        // 3) Elimina la caja independiente de `current` (si existia).
        self.cfg
            .fences
            .retain(|f| !(f.id.as_str() == current_id.as_str() && f.tabs.is_empty()));

        self.refresh_rule_fields();
        self.invalidate();
    }

    /// Desagrupa la regla seleccionada: sale del grupo y se convierte en una
    /// caja independiente (posicionada junto al grupo original).
    fn ungroup_rule(&mut self) {
        self.sync_rule_fields();
        let Some(idx) = self.rules_selected else { return };
        let current_id = self.cfg.rules[idx].id.clone();

        // Geometria del grupo para colocar la nueva caja junto a el.
        let (gx, gy, gw, gh) = match self
            .cfg
            .fences
            .iter()
            .find(|f| f.tabs.iter().any(|t| t == &current_id))
        {
            Some(f) => (f.x, f.y, f.width, f.height),
            None => return, // no esta agrupada
        };

        // Saca la regla del grupo; si queda una sola pestana, el grupo se
        // disuelve en una caja independiente para esa pestana.
        self.remove_rule_from_groups(&current_id);

        // Crea la caja independiente de `current` junto al grupo.
        let fl = crate::config::FenceLayout {
            id: crate::config::String32::new(&current_id),
            x: gx + gw + 20,
            y: gy,
            width: gw.max(320),
            height: gh.max(260),
            ..Default::default()
        };
        self.cfg.fences.push(fl);

        self.refresh_rule_fields();
        self.invalidate();
    }

    fn delete_rule(&mut self) {
        let Some(index) = self.rules_selected else { return };
        let removed_id = self.cfg.rules[index].id.clone();
        self.cfg.rules.remove(index);
        // Limpia grupos y cajas que referencien la regla eliminada.
        self.remove_rule_from_groups(&removed_id);
        self.cfg
            .fences
            .retain(|f| !(f.id.as_str() == removed_id.as_str() && f.tabs.is_empty()));
        self.rules_selected = if self.cfg.rules.is_empty() {
            None
        } else {
            Some(index.min(self.cfg.rules.len() - 1))
        };
        self.refresh_rule_fields();
        self.invalidate();
    }

    fn move_rule(&mut self, delta: isize) {
        let Some(index) = self.rules_selected else { return };
        let target = if delta < 0 {
            index.checked_sub(1)
        } else if index + 1 < self.cfg.rules.len() {
            Some(index + 1)
        } else {
            None
        };
        if let Some(target) = target {
            self.sync_rule_fields();
            self.cfg.rules.swap(index, target);
            self.rules_selected = Some(target);
            self.refresh_rule_fields();
            self.invalidate();
        }
    }

    fn sync_rule_fields(&mut self) {
        let Some(index) = self.rules_selected else { return };
        let mut rule = self.cfg.rules[index].clone();
        rule.enabled = self.checked(ID_CHECK_R_ENABLED);
        rule.move_files = self.checked(ID_CHECK_R_MOVE);
        rule.include_folders = self.checked(ID_CHECK_R_FOLDERS);
        let title = get_text(self.edits.get(&ID_EDIT_R_TITLE).copied().unwrap_or_default());
        if !title.trim().is_empty() {
            rule.title = title.trim().to_string();
        }
        let folder = get_text(self.edits.get(&ID_EDIT_R_FOLDER).copied().unwrap_or_default());
        if !folder.trim().is_empty() {
            rule.folder = folder.trim().to_string();
        }
        let color = get_text(self.edits.get(&ID_EDIT_R_COLOR).copied().unwrap_or_default()).trim().to_string();
        if !color.is_empty() {
            rule.color = color;
        }
        rule.extensions = split_csv(&get_text(self.edits.get(&ID_EDIT_R_EXTS).copied().unwrap_or_default()));
        rule.name_patterns = split_csv(&get_text(self.edits.get(&ID_EDIT_R_PATTERNS).copied().unwrap_or_default()));
        rule.min_size_bytes = parse_size(&get_text(self.edits.get(&ID_EDIT_R_MIN_SIZE).copied().unwrap_or_default()));
        rule.max_size_bytes = parse_size(&get_text(self.edits.get(&ID_EDIT_R_MAX_SIZE).copied().unwrap_or_default()));
        rule.newer_than_days = parse_days(&get_text(self.edits.get(&ID_EDIT_R_NEWER).copied().unwrap_or_default()));
        rule.older_than_days = parse_days(&get_text(self.edits.get(&ID_EDIT_R_OLDER).copied().unwrap_or_default()));
        let re = get_text(self.edits.get(&ID_EDIT_R_REGEX).copied().unwrap_or_default()).trim().to_string();
        if !re.is_empty() {
            // Regex invalido: conservar el anterior para no romper la regla.
            if regex::Regex::new(&re).is_ok() {
                rule.regex = Some(re);
            }
        } else {
            rule.regex = None;
        }
        self.cfg.rules[index] = rule;
        // Persiste el titulo del grupo (solo si la regla esta agrupada).
        let gt = get_text(self.edits.get(&ID_EDIT_R_GROUP_TITLE).copied().unwrap_or_default());
        let rule_id = self.cfg.rules[index].id.clone();
        if let Some(group) = self.cfg.fences.iter_mut().find(|f| f.tabs.iter().any(|t| t == &rule_id)) {
            group.group_title = if gt.trim().is_empty() { None } else { Some(gt.trim().to_string()) };
        }
    }

    fn refresh_rule_fields(&mut self) {
        let rule = self.selected_rule().cloned();
        let r = rule.as_ref();
        self.checks.insert(ID_CHECK_R_ENABLED, r.map(|x| x.enabled).unwrap_or(false));
        self.checks.insert(ID_CHECK_R_MOVE, r.map(|x| x.move_files).unwrap_or(false));
        self.checks.insert(ID_CHECK_R_FOLDERS, r.map(|x| x.include_folders).unwrap_or(false));
        let min_size = r.and_then(|x| x.min_size_bytes).map(format_size).unwrap_or_default();
        let max_size = r.and_then(|x| x.max_size_bytes).map(format_size).unwrap_or_default();
        let newer = r.and_then(|x| x.newer_than_days).map(|d| d.to_string()).unwrap_or_default();
        let older = r.and_then(|x| x.older_than_days).map(|d| d.to_string()).unwrap_or_default();
        let regex_str = r.and_then(|x| x.regex.clone()).unwrap_or_default();
        for (id, value) in [
            (ID_EDIT_R_TITLE, r.map(|x| x.title.as_str()).unwrap_or("")),
            (ID_EDIT_R_FOLDER, r.map(|x| x.folder.as_str()).unwrap_or("")),
            (ID_EDIT_R_COLOR, r.map(|x| x.color.as_str()).unwrap_or("")),
            (ID_EDIT_R_EXTS, r.map(|x| x.extensions.join(", ")).unwrap_or_default().as_str()),
            (ID_EDIT_R_PATTERNS, r.map(|x| x.name_patterns.join(", ")).unwrap_or_default().as_str()),
            (ID_EDIT_R_MIN_SIZE, min_size.as_str()),
            (ID_EDIT_R_MAX_SIZE, max_size.as_str()),
            (ID_EDIT_R_NEWER, newer.as_str()),
            (ID_EDIT_R_OLDER, older.as_str()),
            (ID_EDIT_R_REGEX, regex_str.as_str()),
        ] {
            if let Some(edit) = self.edits.get(&id).copied() {
                set_text(edit, value);
            }
        }
        // Titulo del grupo: se rellena solo si la regla seleccionada esta agrupada.
        let gt = rule
            .as_ref()
            .and_then(|x| self.cfg.group_of(&x.id))
            .and_then(|f| f.group_title.clone())
            .unwrap_or_default();
        if let Some(edit) = self.edits.get(&ID_EDIT_R_GROUP_TITLE).copied() {
            set_text(edit, &gt);
        }
    }

    /// Reune y valida toda la configuracion editada; `None` si hay un error
    /// (muestra el aviso correspondiente). Compartido por Guardar y Aplicar.
    fn collect_cfg(&mut self) -> Option<Config> {
        self.sync_rule_fields();
        self.sync_widgets_disabled();
        let mut cfg = self.cfg.clone();
        let text = |id: u16| self.edit_text(id).unwrap_or_default();
        let bad_number = |name: &'static str| self.tr.warn_not_number.replace("{name}", name);

        cfg.general.organize_folders = self.checked(ID_CHECK_ORGANIZE_FOLDERS);
        cfg.general.organize_on_start = self.checked(ID_CHECK_ORGANIZE_START);
        cfg.general.watch_public_desktop = self.checked(ID_CHECK_PUBLIC);
        cfg.general.keep_shortcuts = self.checked(ID_CHECK_SHORTCUTS);
        cfg.general.start_with_windows = self.checked(ID_CHECK_STARTUP);
        cfg.general.startup_delay_seconds = match text(ID_EDIT_STARTUP_DELAY).trim().parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                self.warn(&bad_number(self.tr.fld_startup_delay));
                return None;
            }
        };
        cfg.general.auto_check_updates = self.checked(ID_CHECK_AUTO_UPDATE);
        cfg.general.zen_double_click = self.checked(ID_CHECK_ZEN_DBL);
        cfg.general.zen_hotkey = self.checked(ID_CHECK_ZEN_HOTKEY);
        cfg.general.zen_hides_desktop_icons = self.checked(ID_CHECK_ZEN_HIDE);
        cfg.ephemeral.enabled = self.checked(ID_CHECK_ARCHIVE);
        cfg.ephemeral.archive_by_month = self.checked(ID_CHECK_BY_MONTH);
        cfg.ai.enabled = self.checked(ID_CHECK_AI_ENABLE);
        let ai_url = text(ID_EDIT_AI_URL);
        if !ai_url.trim().is_empty() {
            cfg.ai.ollama_url = ai_url.trim().to_string();
        }
        let ai_model = text(ID_EDIT_AI_MODEL);
        if !ai_model.trim().is_empty() {
            cfg.ai.model = ai_model.trim().to_string();
        }
        let ai_embed_model = text(ID_EDIT_AI_EMBED_MODEL);
        if !ai_embed_model.trim().is_empty() {
            cfg.ai.embed_model = ai_embed_model.trim().to_string();
        }

        // Carpetas donde se guardan los elementos organizados.
        let root = text(ID_EDIT_G_ROOT).trim().to_string();
        let archive = text(ID_EDIT_G_ARCHIVE).trim().to_string();
        if root.is_empty() || archive.is_empty() {
            self.warn(self.tr.fld_root_folder);
            return None;
        }
        cfg.general.root_folder = root;
        cfg.general.archive_folder = archive;

        cfg.ephemeral.max_age_days = match text(ID_EDIT_MAX_AGE).trim().parse::<f64>() {
            Ok(v) => v,
            Err(_) => {
                self.warn(&bad_number(self.tr.fld_max_age));
                return None;
            }
        };
        cfg.ephemeral.min_age_minutes = match text(ID_EDIT_MIN_AGE).trim().parse::<u64>() {
            Ok(v) => v,
            Err(_) => {
                self.warn(&bad_number(self.tr.fld_min_age));
                return None;
            }
        };
        cfg.ephemeral.purge_archive_after_days = match text(ID_EDIT_PURGE).trim().parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                self.warn(&bad_number(self.tr.fld_purge));
                return None;
            }
        };

        for (id, name) in [
            (ID_EDIT_A_BG, self.tr.fld_bg),
            (ID_EDIT_A_HOVER, self.tr.fld_bg_hover),
            (ID_EDIT_A_BORDER, self.tr.fld_border),
            (ID_EDIT_A_TITLE, self.tr.fld_title_color),
            (ID_EDIT_A_TEXT, self.tr.fld_text_color),
            (ID_EDIT_A_MUTED, self.tr.fld_muted),
            (ID_EDIT_A_SHADOW, self.tr.fld_shadow),
        ] {
            let value = text(id);
            if value.trim().is_empty() {
                continue;
            }
            if !valid_color(&value) {
                self.warn(&self.tr.warn_bad_color.replace("{name}", name));
                return None;
            }
            let value = value.trim().to_string();
            match id {
                ID_EDIT_A_BG => cfg.appearance.background = value,
                ID_EDIT_A_HOVER => cfg.appearance.background_hover = value,
                ID_EDIT_A_BORDER => cfg.appearance.border = value,
                ID_EDIT_A_TITLE => cfg.appearance.title_color = value,
                ID_EDIT_A_TEXT => cfg.appearance.text_color = value,
                ID_EDIT_A_MUTED => cfg.appearance.muted_color = value,
                ID_EDIT_A_SHADOW => cfg.appearance.shadow = value,
                _ => {}
            }
        }
        let num = |id: u16, name: &'static str| -> Result<f32, String> {
            text(id)
                .trim()
                .parse::<f32>()
                .map_err(|_| self.tr.warn_not_number.replace("{name}", name))
        };
        match (
            num(ID_EDIT_A_RADIUS, self.tr.fld_radius),
            num(ID_EDIT_A_TITLE_SIZE, self.tr.fld_title_size),
            num(ID_EDIT_A_TEXT_SIZE, self.tr.fld_text_size),
        ) {
            (Ok(r), Ok(ts), Ok(tx)) => {
                cfg.appearance.corner_radius = r;
                cfg.appearance.title_size = ts;
                cfg.appearance.text_size = tx;
            }
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                self.warn(&e);
                return None;
            }
        }
        cfg.appearance.snap_grid = match text(ID_EDIT_A_SNAP).trim().parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                self.warn(&bad_number(self.tr.fld_snap));
                return None;
            }
        };
        cfg.appearance.show_icons = self.checked(ID_CHECK_A_ICONS);
        cfg.appearance.show_counter = self.checked(ID_CHECK_A_COUNTER);
        cfg.appearance.show_search = self.checked(ID_CHECK_A_SEARCH);
        cfg.appearance.grid_mode = self.checked(ID_CHECK_A_GRID);
        if let Some(v) = self.edit_text(ID_EDIT_A_GRID_SIZE).and_then(|s| s.parse::<f32>().ok()) {
            cfg.appearance.grid_item_size = v.clamp(48.0, 128.0);
        }
        if let Some(v) = self.edit_text(ID_EDIT_A_GRID_ICON).and_then(|s| s.parse::<f32>().ok()) {
            cfg.appearance.grid_icon_size = v.clamp(16.0, 96.0);
        }

        // Idioma elegido en el selector.
        cfg.language = self.lang.code().into();

        cfg.normalize();
        Some(cfg)
    }

    fn save_and_close(&mut self) {
        if let Some(cfg) = self.collect_cfg() {
            self.cfg = cfg;
            self.result = true;
            self.finish();
        }
    }

    /// Recoge la configuracion sin mostrar avisos: un campo a medio editar
    /// devuelve `None` en silencio para no interrumpir la vista previa.
    fn collect_cfg_quiet(&mut self) -> Option<Config> {
        self.suppress_warnings = true;
        let r = self.collect_cfg();
        self.suppress_warnings = false;
        r
    }

    /// Vuelca las preferencias de orden por regla (rules_sort) sobre
    /// `cfg.fences`, creando el FenceLayout si la regla aun no tiene entrada.
    fn sync_rules_sort_into(&self, cfg: &mut Config) {
        for (rule_idx, sort_opt) in self.rules_sort.iter() {
            if let Some(rule) = cfg.rules.get(*rule_idx) {
                match cfg.fences.iter_mut().find(|f| f.id.as_str() == rule.id || f.tabs.iter().any(|t| t == &rule.id)) {
                    Some(slot) => slot.sort_by = sort_opt.clone(),
                    None => {
                        let fl = crate::config::FenceLayout {
                            id: crate::config::String32::new(&rule.id),
                            sort_by: sort_opt.clone(),
                            ..Default::default()
                        };
                        cfg.fences.push(fl);
                    }
                }
            }
        }
    }

    /// Marca la vista previa como sucia y programa la reconstruccion real con
    /// un debounce corto: las rafagas (tecleo + blur, doble clic, toggles
    /// rapidos) coalescen en una sola pasada en vez de reconstruir a cada evento.
    fn preview_apply(&mut self) {
        self.dirty = true;
        self.invalidate();
        unsafe {
            let _ = KillTimer(self.hwnd, PREVIEW_TIMER_ID);
            let _ = SetTimer(self.hwnd, PREVIEW_TIMER_ID, PREVIEW_DEBOUNCE_MS, None);
        }
    }

    /// Aplica de verdad la configuracion editada (sin guardar en disco ni
    /// organizar), al expirar el debounce de la vista previa.
    fn flush_preview(&mut self) {
        let Some(mut cfg) = self.collect_cfg_quiet() else { return };
        self.sync_rules_sort_into(&mut cfg);
        self.cfg = cfg;
        if !self.app.is_null() {
            unsafe { (*self.app).apply_config(self.cfg.clone()); }
        }
    }

    /// Revierte la vista previa devolviendo la app a la configuracion original
    /// (al cancelar o cerrar sin guardar).
    fn revert_preview(&mut self) {
        if !self.app.is_null() {
            unsafe { (*self.app).apply_config(self.original_cfg.clone()); }
        }
    }

    /// Selector nativo de carpeta para un campo de ruta.
    fn pick_folder(&mut self, id: u16) {
        let title = if id == ID_EDIT_G_ROOT {
            self.tr.fld_root_folder
        } else {
            self.tr.fld_archive_folder
        };
        let initial = self.edit_text(id).unwrap_or_default();
        if let Some(path) = browse_folder(self.hwnd, &initial, title) {
            if let Some(edit) = self.edits.get(&id).copied() {
                set_text(edit, &path);
                self.invalidate();
            }
        }
    }

    /// Mueve el scroll del panel al arrastrar el pulgar de la barra.
    fn scroll_drag(&mut self, dy: f32) {
        let (_, cy, _, ch) = self.content_area();
        let track_h = ch - 8.0;
        let thumb_h = (track_h * (ch / (ch + self.scroll_max))).max(24.0);
        if track_h - thumb_h > 1.0 && self.scroll_max > 0.0 {
            let ratio = ((dy - cy - 4.0 - thumb_h / 2.0) / (track_h - thumb_h)).clamp(0.0, 1.0);
            let next = ratio * self.scroll_max;
            if (next - self.scroll).abs() > 0.5 {
                self.scroll = next;
                self.invalidate();
            }
        }
    }

    fn warn(&mut self, message: &str) {
        if self.suppress_warnings {
            return;
        }
        let text = wide(&format!("{message}\n\n{}", self.tr.warn_not_saved));
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), w!("ZenDesktop"), MB_OK | MB_ICONWARNING);
        }
    }

    fn finish(&mut self) {
        self.finished = true;
    }

    fn invalidate(&self) {
        unsafe { let _ = InvalidateRect(self.hwnd, None, false); }
    }
}

// ---------------------------------------------------------------------------
// Procedimiento de ventana
// ---------------------------------------------------------------------------

fn point_of(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i32;
    (x as f32, y as f32)
}

unsafe fn state_from(hwnd: HWND) -> *mut Settings {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Settings
}

extern "system" fn dlg_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_NCCREATE => {
                let cs = lparam.0 as *const CREATESTRUCTW;
                if !cs.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
                }
                LRESULT(1)
            }
            WM_NCHITTEST => {
                let state = &mut *state_from(hwnd);
                let (sx, sy) = point_of(lparam);
                // WM_NCHITTEST llega en coordenadas de PANTALLA; el hit-testing
                // y las regiones trabajan en DIPs relativos a la ventana, asi
                // que primero se convierte a cliente (si no, ningun clic
                // acierta y todo el dialogo se arrastra en vez de pulsarse).
                let mut pt = POINT { x: sx as i32, y: sy as i32 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let s = state.scale;
                let (dx, dy) = (pt.x as f32 / s, pt.y as f32 / s);
                let (ww, wh) = (state.size.0, state.size.1);
                // Bordes y esquinas: redimensionar (se evalua antes que nada
                // para poder estirar la ventana desde cualquier lado).
                let z = 8.0;
                if dx >= ww - z && dy >= wh - z {
                    return LRESULT(HTBOTTOMRIGHT as isize);
                }
                if dx <= z && dy <= z {
                    return LRESULT(HTTOPLEFT as isize);
                }
                if dx >= ww - z && dy <= z {
                    return LRESULT(HTTOPRIGHT as isize);
                }
                if dx <= z && dy >= wh - z {
                    return LRESULT(HTBOTTOMLEFT as isize);
                }
                if dy <= z {
                    return LRESULT(HTTOP as isize);
                }
                if dy >= wh - z {
                    return LRESULT(HTBOTTOM as isize);
                }
                if dx <= z {
                    return LRESULT(HTLEFT as isize);
                }
                if dx >= ww - z {
                    return LRESULT(HTRIGHT as isize);
                }
                // Cabecera (salvo los botones minimizar/cerrar): arrastrar.
                if dy <= HEADER_H && dx < state.size.0 - 84.0 {
                    return LRESULT(HTCAPTION as isize);
                }
                // Zonas vacias arrastran solo con el selector de color cerrado:
                // con el selector abierto, un clic fuera debe cerrarlo (y no
                // secuestrar el clic con el bucle de arrastre del sistema).
                if state.picker.is_none()
                    && !state.regions.is_empty()
                    && state.hit(dx, dy).is_none()
                {
                    return LRESULT(HTCAPTION as isize);
                }
                LRESULT(HTCLIENT as isize)
            }
            WM_GETMINMAXINFO => {
                let state = &mut *state_from(hwnd);
                let mm = lparam.0 as *mut MINMAXINFO;
                if !mm.is_null() {
                    let s = state.scale.max(1.0);
                    (*mm).ptMinTrackSize = POINT {
                        x: (760.0 * s) as i32,
                        y: (560.0 * s) as i32,
                    };
                }
                LRESULT(0)
            }
            WM_SIZE => {
                let state = &mut *state_from(hwnd);
                state.invalidate();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let state = &mut *state_from(hwnd);
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let _ = BeginPaint(hwnd, &mut ps);
                state.render();
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let state = &mut *state_from(hwnd);
                let dpi = (wparam.0 & 0xFFFF) as u32;
                if dpi > 0 {
                    // Misma logica de ajuste al area de trabajo que en la
                    // apertura: al cambiar de monitor el dialogo nunca se
                    // sale de los limites de la pantalla.
                    let mut wa = RECT::default();
                    let _ = SystemParametersInfoW(
                        SPI_GETWORKAREA,
                        0,
                        Some(&mut wa as *mut RECT as *mut c_void),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    );
                    let work_w = (wa.right - wa.left).max(1) as f32;
                    let work_h = (wa.bottom - wa.top).max(1) as f32;
                    let scale = ((dpi as f32 / 96.0).max(1.0))
                        .min(work_h / 780.0)
                        .min(work_w / 880.0);
                    state.scale = scale.max(0.5);
                    state.target().SetDpi(dpi as f32, dpi as f32);
                    // La ventana se redimensiona con la nueva escala para que
                    // el contenido no quede cortado ni se solape al cambiar
                    // de monitor (SWP_NOZORDER: se ignora el insert-after).
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        (880.0 * state.scale) as i32,
                        (780.0 * state.scale) as i32,
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                state.invalidate();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let state = &mut *state_from(hwnd);
                let (x, y) = point_of(lparam);
                // Arrastre del selector de color (cuadro SV o tira de tono).
                if let Some(p) = state.picker.as_mut() {
                    if let Some(part) = p.drag {
                        state.picker_set(part, x / state.scale, y / state.scale);
                        return LRESULT(0);
                    }
                }
                // Arrastre del pulgar de la barra de desplazamiento.
                if state.pressed == Some(Ctrl::Scroll) {
                    state.scroll_drag(y / state.scale);
                    return LRESULT(0);
                }
                // El raton llega en pixeles; el hit-testing trabaja en DIPs.
                let new_hover = state.hit(x / state.scale, y / state.scale);
                if new_hover != state.hover {
                    state.hover = new_hover;
                    state.invalidate();
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                let state = &mut *state_from(hwnd);
                if state.hover.is_some() || state.pressed.is_some() {
                    state.hover = None;
                    state.pressed = None;
                    state.invalidate();
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let state = &mut *state_from(hwnd);
                let (x, y) = point_of(lparam);
                let ctrl = state
                    .hit(x / state.scale, y / state.scale)
                    .unwrap_or(Ctrl::None);
                match ctrl {
                    // Selector de color: el arrastre del cuadro/tira y los
                    // botones responden aqui mismo.
                    Ctrl::Picker(part) => {
                        match part {
                            PickerPart::Sv | PickerPart::Hue => {
                                state.picker_start(part, x / state.scale, y / state.scale)
                            }
                            PickerPart::Close | PickerPart::Ok => state.close_picker(),
                        }
                        let _ = SetFocus(hwnd);
                        state.invalidate();
                        return LRESULT(0);
                    }
                    // Un swatch abre (o reabre) el selector para ese campo.
                    Ctrl::Swatch(id) => {
                        state.open_picker(id);
                        let _ = SetFocus(hwnd);
                        state.invalidate();
                        return LRESULT(0);
                    }
                    _ => {}
                }
                // Cualquier otro clic cierra el selector abierto y sigue con
                // el control normal.
                if state.picker.is_some() {
                    state.close_picker();
                }
                match ctrl {
                    Ctrl::Field(id) => {
                        if let Some(edit) = state.edits.get(&id).copied() {
                            let _ = SetFocus(edit);
                        }
                    }
                    Ctrl::None => {}
                    _ => {
                        let _ = SetFocus(hwnd);
                        state.focus = state.focus_order.iter().position(|c| *c == ctrl);
                        // Todo control pulsable (nav, casilla, fila, boton,
                        // cerrar) entra en estado "pressed" para activarse en
                        // WM_LBUTTONUP si el cursor sigue encima.
                        state.pressed = Some(ctrl);
                        let _ = SetCapture(hwnd);
                    }
                }
                state.invalidate();
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let state = &mut *state_from(hwnd);
                let (x, y) = point_of(lparam);
                // Fin del arrastre del selector de color.
                if let Some(p) = state.picker.as_mut() {
                    if p.drag.take().is_some() {
                        let _ = ReleaseCapture();
                        state.invalidate();
                        return LRESULT(0);
                    }
                }
                if let Some(released) = state.pressed.take() {
                    let _ = ReleaseCapture();
                    if state.hit(x / state.scale, y / state.scale) == Some(released) {
                        state.activate(released);
                    }
                }
                state.invalidate();
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let state = &mut *state_from(hwnd);
                let delta = ((wparam.0 >> 16) as i16) as i32;
                let steps = if delta > 0 { -1 } else { 1 };
                // WM_MOUSEWHEEL llega en coordenadas de PANTALLA: se convierten
                // a cliente antes de comparar con el rect de la lista.
                let (sx, sy) = point_of(lparam);
                let mut pt = POINT { x: sx as i32, y: sy as i32 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let (dx, dy) = (pt.x as f32 / state.scale, pt.y as f32 / state.scale);
                // Sobre la lista de reglas, la rueda desplaza la lista
                // interna; sobre el resto del contenido, desplaza el panel.
                let over_list = (state.panel == Panel::Rules || state.panel == Panel::Widgets)
                    && state
                        .list_rect
                        .map(|r| dx >= r.left && dx <= r.right && dy >= r.top && dy <= r.bottom)
                        .unwrap_or(false);
                if over_list && state.panel == Panel::Widgets {
                    let total = state.widgets_list.len();
                    let max_scroll = total.saturating_sub(4);
                    let next = (state.widgets_scroll as isize + steps as isize)
                        .clamp(0, max_scroll as isize) as usize;
                    if next != state.widgets_scroll {
                        state.widgets_scroll = next;
                        state.invalidate();
                    }
                } else if over_list {
                    let total = state.cfg.rules.len();
                    let max_scroll = total.saturating_sub(5);
                    let next = (state.rules_scroll as isize + steps as isize)
                        .clamp(0, max_scroll as isize) as usize;
                    if next != state.rules_scroll {
                        state.rules_scroll = next;
                        state.invalidate();
                    }
                } else if state.scroll_max > 0.0 {
                    let next = (state.scroll - steps as f32 * 24.0).clamp(0.0, state.scroll_max);
                    if next != state.scroll {
                        state.scroll = next;
                        state.invalidate();
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let state = &mut *state_from(hwnd);
                let vk = wparam.0 as u32;
                // Con el selector de color abierto solo ESC tiene efecto (lo
                // cierra); el teclado no maneja el dialogo mientras el foco
                // vive en los EDITs del modal.
                if state.picker.is_some() && vk != KEY_ESC {
                    return LRESULT(0);
                }
                match vk {
                    KEY_ESC => {
                        // ESC cierra primero el selector de color si esta abierto.
                        if state.picker.is_some() {
                            state.close_picker();
                        } else {
                            state.result = false;
                            state.finish();
                        }
                    }
                    KEY_ENTER | KEY_SPACE => {
                        if let Some(i) = state.focus {
                            if let Some(ctrl) = state.focus_order.get(i).copied() {
                                state.activate(ctrl);
                            }
                        }
                    }
                    KEY_UP | KEY_LEFT => state.advance_focus(-1),
                    KEY_DOWN | KEY_RIGHT => state.advance_focus(1),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CTLCOLOREDIT => {
                let dc = HDC(lparam.0 as *mut c_void);
                let _ = SetTextColor(dc, COLORREF(0x00F7EDE6));
                let _ = SetBkColor(dc, COLORREF(0x0036241B));
                LRESULT(edit_brush().0 as isize)
            }
            WM_TIMER => {
                if wparam.0 == BUSY_TIMER_ID {
                    let state = &mut *state_from(hwnd);
                    state.spinner_phase = (state.spinner_phase + 0.125).fract();
                    state.invalidate();
                } else if wparam.0 == PREVIEW_TIMER_ID {
                    let state = &mut *state_from(hwnd);
                    let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
                    state.flush_preview();
                }
                LRESULT(0)
            }
            WM_AI_PING_DONE => {
                let state = &mut *state_from(hwnd);
                state.on_ai_ping_done();
                LRESULT(0)
            }
            WM_AI_MODELS_DONE => {
                let state = &mut *state_from(hwnd);
                state.on_ai_models_done();
                LRESULT(0)
            }
            WM_AI_CLUSTER_DONE => {
                let state = &mut *state_from(hwnd);
                state.on_ai_cluster_done();
                LRESULT(0)
            }
            WM_AI_RULE_DONE => {
                let state = &mut *state_from(hwnd);
                state.on_ai_rule_done();
                LRESULT(0)
            }
            WM_UPDATE_DONE => {
                let state = &mut *state_from(hwnd);
                state.on_update_done();
                LRESULT(0)
            }
            WM_COMMAND => {
                // Los EDIT nativos notifican al dialogo por WM_COMMAND. Al
                // perder el foco un campo de texto (EN_KILLFOCUS) se aplica su
                // valor en vivo; mientras se teclea no se toca nada para no
                // reconstruir las cajas a cada pulsacion.
                let code = ((wparam.0 as u32) >> 16) & 0xFFFF;
                if code == EN_KILLFOCUS {
                    let state = &mut *state_from(hwnd);
                    state.preview_apply();
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let state = &mut *state_from(hwnd);
                if state.busy_timer {
                    let _ = KillTimer(hwnd, BUSY_TIMER_ID);
                    state.busy_timer = false;
                }
                AI_BUSY.store(false, Ordering::SeqCst);
                UPDATE_BUSY.store(false, Ordering::SeqCst);
                // Cierre sin guardar (Alt+F4 o el flujo de actualizacion):
                // deshacer la vista previa como en Cancelar.
                state.result = false;
                state.revert_preview();
                state.finished = true;
                LRESULT(0)
            }
            WM_DESTROY => {
                // Red de seguridad: si la ventana se destruye por otra via.
                let _ = KillTimer(hwnd, BUSY_TIMER_ID);
                let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
                // Sin PostQuitMessage: el bucle principal de la app decide.
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

// ---------------------------------------------------------------------------
// EDITs nativos (texto, cursor y portapapeles gratis)
// ---------------------------------------------------------------------------

static EDIT_BRUSH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

fn edit_brush() -> HBRUSH {
    let raw = EDIT_BRUSH.get_or_init(|| unsafe { CreateSolidBrush(COLORREF(0x0036241B)).0 as usize });
    HBRUSH(*raw as *mut c_void)
}

/// Siguiente tamano de icono en el ciclo por caja: None (Auto) -> 16 -> 24
/// -> 32 -> 48 -> 64 -> 96 -> None (Auto).
fn next_icon_size(cur: Option<f32>) -> Option<f32> {
    const SIZES: [f32; 6] = [16.0, 24.0, 32.0, 48.0, 64.0, 96.0];
    match cur {
        None => Some(SIZES[0]),
        Some(v) => match SIZES.iter().position(|&s| (s - v).abs() < f32::EPSILON) {
            Some(i) if i + 1 < SIZES.len() => Some(SIZES[i + 1]),
            _ => None,
        },
    }
}

/// Formatea un tamano en bytes como "1.5 MB" / "300 KB" / "512 B".
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else if v >= 10.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Parsea un tamano legible ("5 MB", "1.5 GB", "1024", "300 KB") a bytes.
fn parse_size(s: &str) -> Option<u64> {
    let lower = s.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let (num, mult) = if let Some(rest) = lower.strip_suffix("tb") {
        (rest.trim(), 1024u64.pow(4))
    } else if let Some(rest) = lower.strip_suffix("gb") {
        (rest.trim(), 1024u64.pow(3))
    } else if let Some(rest) = lower.strip_suffix("mb") {
        (rest.trim(), 1024u64.pow(2))
    } else if let Some(rest) = lower.strip_suffix("kb") {
        (rest.trim(), 1024)
    } else if let Some(rest) = lower.strip_suffix('b') {
        (rest.trim(), 1)
    } else {
        (lower.trim(), 1)
    };
    let val: f64 = num.parse().ok()?;
    if !val.is_finite() || val < 0.0 {
        return None;
    }
    Some((val * mult as f64) as u64)
}

/// Parsea un numero de dias; vacio o no valido => None.
fn parse_days(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let v: f64 = t.parse().ok()?;
    if !v.is_finite() || v <= 0.0 {
        None
    } else {
        Some(v)
    }
}

fn get_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn set_text(hwnd: HWND, text: &str) {
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide(text).as_ptr()));
    }
}

/// Dialogo nativo de guardado (GetSaveFileNameW) para exportar la configuracion.
fn save_file_dialog(owner: HWND, title: &str, default_name: &str) -> Option<PathBuf> {
    unsafe {
        // Buffer grande: GetSaveFileNameW necesita espacio para rutas largas.
        let mut file = vec![0u16; 4096];
        let default: Vec<u16> = default_name.encode_utf16().collect();
        let n = default.len().min(4095);
        file[..n].copy_from_slice(&default[..n]);
        let filter: Vec<u16> = "ZenDesktop config (*.toml)\0*.toml\0All files (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let title_w = wide(title);
        // Vivir fuera del statement: lpstrDefExt se lee tras la llamada.
        let def_ext = wide("toml");
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = PCWSTR(filter.as_ptr());
        ofn.lpstrFile = PWSTR(file.as_mut_ptr());
        ofn.nMaxFile = file.len() as u32;
        ofn.lpstrTitle = PCWSTR(title_w.as_ptr());
        ofn.lpstrDefExt = PCWSTR(def_ext.as_ptr());
        ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetSaveFileNameW(&mut ofn).as_bool() {
            let s = String::from_utf16_lossy(&file);
            let s = s.trim_end_matches('\0').to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
        None
    }
}

/// Dialogo nativo de apertura (GetOpenFileNameW) para importar la configuracion.
fn open_file_dialog(owner: HWND, title: &str) -> Option<PathBuf> {
    unsafe {
        let mut file = vec![0u16; 4096];
        let filter: Vec<u16> = "ZenDesktop config (*.toml)\0*.toml\0All files (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let title_w = wide(title);
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = PCWSTR(filter.as_ptr());
        ofn.lpstrFile = PWSTR(file.as_mut_ptr());
        ofn.nMaxFile = file.len() as u32;
        ofn.lpstrTitle = PCWSTR(title_w.as_ptr());
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let s = String::from_utf16_lossy(&file);
            let s = s.trim_end_matches('\0').to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
        None
    }
}

/// Dialogo nativo de seleccion de carpeta (SHBrowseForFolderW), modal sobre la
/// ventana de configuracion y partiendo de la carpeta actual del campo.
fn browse_folder(owner: HWND, initial: &str, title: &str) -> Option<String> {
    unsafe {
        let mut display = [0u16; 260];
        let title = wide(title);
        // Punto de partida: la carpeta ya escrita en el campo (si es valida).
        let mut start_pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if !initial.is_empty() {
            let w = wide(initial);
            let _ = SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut start_pidl, 0, None);
        }
        let bi = BROWSEINFOW {
            hwndOwner: owner,
            pidlRoot: start_pidl,
            pszDisplayName: PWSTR(display.as_mut_ptr()),
            lpszTitle: PCWSTR(title.as_ptr()),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: None,
            lParam: LPARAM(0),
            iImage: 0,
        };
        let pidl = SHBrowseForFolderW(&bi);
        if !start_pidl.is_null() {
            CoTaskMemFree(Some(start_pidl as *const c_void));
        }
        if pidl.is_null() {
            return None;
        }
        let mut out = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, &mut out);
        CoTaskMemFree(Some(pidl as *const c_void));
        if ok.as_bool() {
            let s = String::from_utf16_lossy(&out);
            let s = s.trim_end_matches('\0').to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }
}

fn split_csv(text: &str) -> Vec<String> {
    text.split([',', ';', ' '])
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn valid_color(text: &str) -> bool {
    let s = text.trim().trim_start_matches('#');
    (s.len() == 6 || s.len() == 8) && s.chars().all(|c| c.is_ascii_hexdigit())
}

// --- Conversiones de color HSB <-> RGB <-> hex (para el selector) ---

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = ((h % 360.0) + 360.0) % 360.0 / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

fn hsv_to_hex(h: f32, s: f32, v: f32) -> String {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    format!(
        "#{:02X}{:02X}{:02X}",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8
    )
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hex_to_hsv(text: &str) -> Option<(f32, f32, f32)> {
    if !valid_color(text) {
        return None;
    }
    let s = text.trim().trim_start_matches('#');
    let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
    Some(rgb_to_hsv(r, g, b))
}

/// Estado del selector de color visual (paleta HSB) abierto sobre el panel.
#[derive(Clone, Copy)]
struct PickerState {
    field: u16,
    hue: f32,
    sat: f32,
    val: f32,
    drag: Option<PickerPart>,
}

fn initial_checks(cfg: &Config) -> HashMap<u16, bool> {
    let mut m = HashMap::new();
    m.insert(ID_CHECK_ORGANIZE_FOLDERS, cfg.general.organize_folders);
    m.insert(ID_CHECK_ORGANIZE_START, cfg.general.organize_on_start);
    m.insert(ID_CHECK_PUBLIC, cfg.general.watch_public_desktop);
    m.insert(ID_CHECK_SHORTCUTS, cfg.general.keep_shortcuts);
    m.insert(ID_CHECK_STARTUP, cfg.general.start_with_windows);
    m.insert(ID_CHECK_AUTO_UPDATE, cfg.general.auto_check_updates);
    m.insert(ID_CHECK_ZEN_DBL, cfg.general.zen_double_click);
    m.insert(ID_CHECK_ZEN_HOTKEY, cfg.general.zen_hotkey);
    m.insert(ID_CHECK_ZEN_HIDE, cfg.general.zen_hides_desktop_icons);
    m.insert(ID_CHECK_ARCHIVE, cfg.ephemeral.enabled);
    m.insert(ID_CHECK_BY_MONTH, cfg.ephemeral.archive_by_month);
    m.insert(ID_CHECK_A_ICONS, cfg.appearance.show_icons);
    m.insert(ID_CHECK_A_COUNTER, cfg.appearance.show_counter);
    m.insert(ID_CHECK_A_SEARCH, cfg.appearance.show_search);
    m.insert(ID_CHECK_A_GRID, cfg.appearance.grid_mode);
    m.insert(ID_CHECK_AI_ENABLE, cfg.ai.enabled);
    m
}

// ---------------------------------------------------------------------------
// Punto de entrada
// ---------------------------------------------------------------------------

static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Muestra la ventana de configuracion (modal) y devuelve la configuracion
/// editada si el usuario pulso "Guardar", o `None` si cancelo. `app` permite
/// que el boton "Aplicar" guarde y aplique en caliente sin cerrar el dialogo
/// (mismo hilo: el puntero solo se usa mientras el dialogo esta abierto).
pub fn open_dialog(current: &Config, app: *mut App) -> Option<Config> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
        let class_name = w!("ZenDesktop.Settings");

        if !REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let mut wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(dlg_proc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class_name,
                ..Default::default()
            };
            wc.hIcon = LoadIconW(Some(&instance), PCWSTR(1 as *const u16)).unwrap_or_default();
            wc.hIconSm = wc.hIcon;
            let _ = RegisterClassExW(&wc);
        }

        // Recursos graficos.
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let dpi = GetDpiForWindow(GetDesktopWindow()) as f32;
        let scale = (dpi / 96.0).max(1.0);
        // 880x780: mas ancho (los paneles respiran) y alto de sobra para que
        // ningun panel se solape con la barra de acciones inferior. Si el area
        // de trabajo no cabe a la escala nativa (pantalla pequena o zoom alto)
        // se reduce la escala de TODO el dialogo de forma proporcional: la
        // barra de botones nunca queda cortada por debajo del borde inferior.
        let mut wa = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let work_w = (wa.right - wa.left).max(1) as f32;
        let work_h = (wa.bottom - wa.top).max(1) as f32;
        let scale = scale.min(work_h / 780.0).min(work_w / 880.0);
        let (win_w, win_h) = ((880.0 * scale) as i32, (780.0 * scale) as i32);
        let x = wa.left + ((work_w - win_w as f32) / 2.0) as i32;
        let y = wa.top + ((work_h - win_h as f32) / 2.0) as i32;

        // Lista de widgets instalados (nombres de los scripts .lua).
        let widgets_dir = (*app).widgets_dir();
        let mut widgets_list: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&widgets_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("lua") {
                    if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                        widgets_list.push(name.to_string());
                    }
                }
            }
        }
        widgets_list.sort();
        let widgets_selected = if widgets_list.is_empty() { None } else { Some(0) };

        let mut settings = Settings {
            cfg: current.clone(),
            original_cfg: current.clone(),
            hwnd: HWND::default(),
            scale,
            // Tamaño en DIPs (el lienzo D2D escala solo).
            size: (880.0, 780.0),
            factory,
            target: None,
            brush: None,
            fmt_title: None,
            fmt_body: None,
            fmt_small: None,
            stroke: None,
            lang: current.lang(),
            tr: Tr::get(current.lang()),
            panel: Panel::General,
            checks: initial_checks(current),
            edits: HashMap::new(),
            edits_shown: Vec::new(),
            edit_rects: HashMap::new(),
            edit_visible: HashSet::new(),
            edit_next_rects: HashMap::new(),
            regions: Vec::new(),
            focus_order: Vec::new(),
            focus: None,
            hover: None,
            pressed: None,
            picker: None,
            last_hue: 210.0,
            rules_selected: if current.rules.is_empty() { None } else { Some(0) },
            rules_scroll: 0,
            rules_sort: std::collections::HashMap::new(),
            widgets_dir,
            widgets_list,
            widgets_selected,
            widgets_scroll: 0,
            scroll: 0.0,
            scroll_max: 0.0,
            content_rect: (0.0, 0.0, 0.0, 0.0),
            list_rect: None,
            app,
            finished: false,
            result: false,
            suppress_warnings: false,
            dirty: false,
            target_px_size: (0, 0),
            spinner_phase: 0.0,
            busy_timer: false,
        };

        // Limpieza de flags stale: si una operacion quedo en curso al cerrar el
        // dialogo, su resultado nunca llego y el flag quedaria activo para siempre.
        AI_BUSY.store(false, Ordering::SeqCst);
        UPDATE_BUSY.store(false, Ordering::SeqCst);

        // Se crea sin WS_VISIBLE: si la inicializacion D2D falla, la ventana
        // nunca se muestra y se destruye sin dejar una ventana huerfana.
        // WS_CLIPCHILDREN: el lienzo D2D jamas pinta sobre el area de los
        // EDITs hijos, lo que elimina el parpadeo del fondo al editar texto.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("ZenDesktop"),
            WS_POPUP | WS_CLIPCHILDREN,
            x,
            y,
            win_w,
            win_h,
            None,
            None,
            instance,
            Some(&mut settings as *mut Settings as *const c_void),
        );
        let hwnd = match hwnd {
            Ok(h) => h,
            Err(_) => return None,
        };
        settings.hwnd = hwnd;

        // Target D2D.
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
            },
            dpiX: dpi,
            dpiY: dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: win_w as u32,
                height: win_h as u32,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        // Si cualquiera de estos falla se destruye la ventana (aun no visible)
        // y se abandona; los recursos COM se liberan al salir del scope.
        let target = match settings.factory.CreateHwndRenderTarget(&props, &hwnd_props) {
            Ok(t) => t,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return None;
            }
        };
        let brush = match target.CreateSolidColorBrush(&col(C_TEXT), None) {
            Ok(b) => b,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return None;
            }
        };
        let stroke = match settings.factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                ..Default::default()
            },
            None,
        ) {
            Ok(s) => s,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return None;
            }
        };
        settings.target = Some(target);
        settings.brush = Some(brush);
        settings.stroke = Some(stroke);

        let make_fmt = |size: f32, weight: DWRITE_FONT_WEIGHT| -> windows::core::Result<IDWriteTextFormat> {
            let family = wide("Segoe UI Variable Text");
            let fmt = dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!(""),
            )?;
            fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            Ok(fmt)
        };
        let fmt_title = match make_fmt(13.5, DWRITE_FONT_WEIGHT_SEMI_BOLD) {
            Ok(f) => f,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return None;
            }
        };
        let fmt_body = match make_fmt(12.0, DWRITE_FONT_WEIGHT_NORMAL) {
            Ok(f) => f,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return None;
            }
        };
        let fmt_small = match make_fmt(10.5, DWRITE_FONT_WEIGHT_NORMAL) {
            Ok(f) => f,
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                return None;
            }
        };
        settings.fmt_title = Some(fmt_title);
        settings.fmt_body = Some(fmt_body);
        settings.fmt_small = Some(fmt_small);

        // EDITs nativos para los campos de texto.
        let font: HFONT = CreateFontW(
            -((9 * dpi as i32 + 36) / 72).max(9),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(wide("Segoe UI").as_ptr()),
        );
        for id in ALL_EDITS {
            // Sin WS_EX_CLIENTEDGE: la caja del campo la dibuja D2D (redondeada);
            // un borde bevelado clasico romperia la estetica moderna.
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_TABSTOP | WS_VISIBLE,
                0,
                0,
                10,
                10,
                hwnd,
                HMENU(id as usize as *mut c_void),
                instance,
                None,
            );
            if let Ok(edit) = edit {
                let _ = SendMessageW(edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
                let _ = ShowWindow(edit, SW_HIDE);
                settings.edits.insert(id, edit);
            }
        }

        // Editor de codigo Lua (multilinea, con scroll vertical).
        let code_edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_LEFT) as u32),
            0,
            0,
            10,
            10,
            hwnd,
            HMENU(ID_EDIT_WIDGET_CODE as usize as *mut c_void),
            instance,
            None,
        );
        if let Ok(code_edit) = code_edit {
            let _ = SendMessageW(code_edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            let _ = ShowWindow(code_edit, SW_HIDE);
            settings.edits.insert(ID_EDIT_WIDGET_CODE, code_edit);
        }

        // Valores iniciales.
        settings.refresh_rule_fields();
        settings.refresh_widget_fields();
        seed_edits(&settings);

        let _ = ShowWindow(hwnd, SW_SHOW);
        settings.invalidate();

        // Bucle modal.
        let mut msg = MSG::default();
        while !settings.finished && IsWindow(hwnd).as_bool() {
            let status = GetMessageW(&mut msg, None, 0, 0);
            if status.0 < 0 {
                break;
            }
            if status.0 == 0 {
                // WM_QUIT: se repone para que la app termine de verdad.
                PostQuitMessage(msg.wParam.0 as i32);
                break;
            }
            if msg.message == WM_KEYDOWN {
                let vk = msg.wParam.0 as u32;
                if vk == VK_TAB.0 as u32 {
                    let state = &mut *state_from(hwnd);
                    // En el editor de codigo Lua, Tab inserta una tabulacion
                    // en vez de mover el foco entre controles.
                    if state.edits.get(&ID_EDIT_WIDGET_CODE).copied() == Some(msg.hwnd) {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                        continue;
                    }
                    let shift = (GetKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
                    state.advance_focus(if shift { -1 } else { 1 });
                    continue;
                }
                if vk == KEY_ESC {
                    let state = &mut *state_from(hwnd);
                    state.result = false;
                    state.revert_preview();
                    state.finish();
                    continue;
                }
                if vk == VK_RETURN.0 as u32 && msg.hwnd != hwnd {
                    let state = &mut *state_from(hwnd);
                    // En el editor de codigo Lua, Enter inserta una linea nueva
                    // en vez de aplicar el campo y devolver el foco.
                    if state.edits.get(&ID_EDIT_WIDGET_CODE).copied() == Some(msg.hwnd) {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                        continue;
                    }
                    // Enter dentro de un campo de texto: aplica en vivo y
                    // devuelve el foco al dialogo (sin cerrar).
                    state.preview_apply();
                    let _ = SetFocus(hwnd);
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Sincronizar ordenes por regla -> cfg.fences antes de devolver.
        if settings.result {
            let mut cfg = settings.cfg.clone();
            settings.sync_rules_sort_into(&mut cfg);
            settings.cfg = cfg;
        }
        let result = if settings.result { Some(settings.cfg.clone()) } else { None };
        let _ = DestroyWindow(hwnd);
        let _ = DeleteObject(HGDIOBJ(font.0));
        result
    }
}

fn seed_edits(s: &Settings) {
    let rows: Vec<(u16, String)> = vec![
        (ID_EDIT_G_ROOT, s.cfg.general.root_folder.clone()),
        (ID_EDIT_G_ARCHIVE, s.cfg.general.archive_folder.clone()),
        (ID_EDIT_STARTUP_DELAY, format!("{}", s.cfg.general.startup_delay_seconds)),
        (ID_EDIT_TEMPLATE_NAME, "default".to_string()),
        (ID_EDIT_MAX_AGE, format!("{}", s.cfg.ephemeral.max_age_days)),
        (ID_EDIT_MIN_AGE, format!("{}", s.cfg.ephemeral.min_age_minutes)),
        (ID_EDIT_PURGE, format!("{}", s.cfg.ephemeral.purge_archive_after_days)),
        (ID_EDIT_A_BG, s.cfg.appearance.background.clone()),
        (ID_EDIT_A_HOVER, s.cfg.appearance.background_hover.clone()),
        (ID_EDIT_A_BORDER, s.cfg.appearance.border.clone()),
        (ID_EDIT_A_TITLE, s.cfg.appearance.title_color.clone()),
        (ID_EDIT_A_TEXT, s.cfg.appearance.text_color.clone()),
        (ID_EDIT_A_MUTED, s.cfg.appearance.muted_color.clone()),
        (ID_EDIT_A_SHADOW, s.cfg.appearance.shadow.clone()),
        (ID_EDIT_A_RADIUS, format!("{}", s.cfg.appearance.corner_radius)),
        (ID_EDIT_A_TITLE_SIZE, format!("{}", s.cfg.appearance.title_size)),
        (ID_EDIT_A_TEXT_SIZE, format!("{}", s.cfg.appearance.text_size)),
        (ID_EDIT_A_SNAP, format!("{}", s.cfg.appearance.snap_grid)),
        (ID_EDIT_A_GRID_SIZE, format!("{}", s.cfg.appearance.grid_item_size as u32)),
        (ID_EDIT_A_GRID_ICON, format!("{}", s.cfg.appearance.grid_icon_size as u32)),
        (ID_EDIT_AI_URL, s.cfg.ai.ollama_url.clone()),
        (ID_EDIT_AI_MODEL, s.cfg.ai.model.clone()),
        (ID_EDIT_AI_EMBED_MODEL, s.cfg.ai.embed_model.clone()),
    ];
    for (id, value) in rows {
        if let Some(edit) = s.edits.get(&id).copied() {
            set_text(edit, &value);
        }
    }
}

// ---------------------------------------------------------------------------
// Probe visual (cargo test --release -- --ignored visual_probe)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PW_CLIENTONLY};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowRect, PostMessageW, SendMessageW, HTCLIENT, HTCAPTION, WM_CLOSE,
        WM_MOUSEWHEEL, WM_NCHITTEST,
    };

    /// Abre el dialogo con una config de prueba, captura la ventana a un BMP
    /// (target/settings_probe.bmp) y la cierra. Uso: se lanza y despues se
    /// convierte el BMP a PNG para revisar el diseno.
    #[test]
    #[ignore = "probe visual manual"]
    fn visual_probe() {
        let mut cfg = Config::default();
        cfg.rules.push(Rule {
            title: "Media".into(),
            folder: "Media".into(),
            color: "#7DD3FC".into(),
            extensions: vec!["png".into(), "jpg".into()],
            name_patterns: vec![],
            enabled: true,
            move_files: true,
            include_folders: true,
            ..Rule::default()
        });
        cfg.rules.push(Rule {
            title: "Documentos".into(),
            folder: "Documentos".into(),
            color: "#C4B5FD".into(),
            extensions: vec!["pdf".into(), "docx".into()],
            name_patterns: vec![],
            enabled: true,
            move_files: true,
            include_folders: false,
            ..Rule::default()
        });
        cfg.rules.push(Rule {
            title: "Varios".into(),
            folder: "Varios".into(),
            color: "#6EE7B7".into(),
            extensions: vec![],
            name_patterns: vec![],
            enabled: true,
            move_files: true,
            include_folders: true,
            ..Rule::default()
        });

        let thread_cfg = cfg.clone();
        let handle = std::thread::spawn(move || open_dialog(&thread_cfg, std::ptr::null_mut()));

        // Esperar a que la ventana exista y pintar.
        let mut hwnd = HWND::default();
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            hwnd = unsafe { FindWindowW(w!("ZenDesktop.Settings"), None).unwrap_or_default() };
            if !hwnd.is_invalid() && unsafe { IsWindow(hwnd).as_bool() } {
                break;
            }
        }
        assert!(!hwnd.is_invalid(), "la ventana de configuracion no aparecio");
        std::thread::sleep(std::time::Duration::from_millis(600));

        unsafe {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = (rect.right - rect.left).max(1);
            let h = (rect.bottom - rect.top).max(1);
            let dpi = GetDpiForWindow(hwnd) as f32 / 96.0;

            // Verificacion del fix de clics (WM_NCHITTEST): el mensaje llega en
            // coordenadas de PANTALLA. Un clic sobre un control debe responder
            // HTCLIENT (clic normal); un clic en zona vacia del contenido,
            // HTCAPTION (arrastre de ventana). Antes del fix ambas devolvian
            // HTCAPTION porque las coordenadas de pantalla no coincidian con
            // las regiones relativas a la ventana: nada era clicable.
            let mut rc = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rc);
            let nchit = |dx: f32, dy: f32| {
                let sx = rc.left + (dx * dpi) as i32;
                let sy = rc.top + (dy * dpi) as i32;
                let lparam =
                    LPARAM(((((sy & 0xFFFF) << 16) | (sx & 0xFFFF)) as u32) as i64 as isize);
                SendMessageW(hwnd, WM_NCHITTEST, WPARAM(0), lparam).0 as u32
            };
            assert_eq!(
                nchit(80.0, HEADER_H + 33.0),
                HTCLIENT as u32,
                "clic sobre un control debe ser HTCLIENT"
            );
            assert_eq!(
                nchit(300.0, 615.0),
                HTCAPTION as u32,
                "clic en zona vacia debe arrastrar (HTCAPTION)"
            );

            // Captura el panel actual a un BMP (las filas GDI vienen bottom-up).
            fn capture(hwnd: HWND, name: &str, w: i32, h: i32) {
                unsafe {
                    let hdc = GetDC(hwnd);
                    let mem = CreateCompatibleDC(hdc);
                    let bmp = CreateCompatibleBitmap(hdc, w, h);
                    let _ = SelectObject(mem, HGDIOBJ(bmp.0));
                    let _ = PrintWindow(hwnd, mem, PW_CLIENTONLY);

                    let mut info = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: w,
                            biHeight: -h, // top-down
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let mut pixels = vec![0u8; (w * h * 4) as usize];
                    let _ = GetDIBits(mem, bmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);

                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                    let _ = DeleteDC(mem);
                    let _ = ReleaseDC(hwnd, hdc);

                    // BMP de 32bpp (BGRA).
                    let row_size = w * 4;
                    let data_size = row_size * h;
                    let file_size = 14 + 40 + data_size as usize;
                    let mut file = Vec::with_capacity(file_size);
                    file.extend_from_slice(b"BM");
                    file.extend_from_slice(&(file_size as u32).to_le_bytes());
                    file.extend_from_slice(&0u16.to_le_bytes());
                    file.extend_from_slice(&0u16.to_le_bytes());
                    file.extend_from_slice(&54u32.to_le_bytes());
                    file.extend_from_slice(&40u32.to_le_bytes());
                    file.extend_from_slice(&(w as i32).to_le_bytes());
                    file.extend_from_slice(&(h as i32).to_le_bytes());
                    file.extend_from_slice(&1u16.to_le_bytes());
                    file.extend_from_slice(&32u16.to_le_bytes());
                    file.extend_from_slice(&0u32.to_le_bytes());
                    file.extend_from_slice(&(data_size as u32).to_le_bytes());
                    file.extend_from_slice(&2835i32.to_le_bytes());
                    file.extend_from_slice(&2835i32.to_le_bytes());
                    file.extend_from_slice(&0u32.to_le_bytes());
                    file.extend_from_slice(&0u32.to_le_bytes());
                    file.extend_from_slice(&pixels);
                    let path = format!("target/{name}.bmp");
                    std::fs::write(&path, &file).expect("no se pudo escribir el BMP");
                }
            }

            // Clic en un item de la sidebar (coordenadas DIP -> pixeles).
            fn click_sidebar(hwnd: HWND, item: u32, dpi: f32) {
                let y = ((HEADER_H + 16.0 + item as f32 * 40.0 + 17.0) * dpi) as i32;
                let x = (80.0 * dpi) as i32;
                let packed = ((y & 0xFFFF) << 16) | (x & 0xFFFF);
                let lparam = LPARAM(packed as i64 as isize);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), lparam);
                    let _ = PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), lparam);
                }
            }

            capture(hwnd, "settings_probe_general", w, h);

            // Panel Reglas (item 1) y Apariencia (item 2).
            click_sidebar(hwnd, 1, dpi);
            std::thread::sleep(std::time::Duration::from_millis(400));
            capture(hwnd, "settings_probe_rules", w, h);

            click_sidebar(hwnd, 2, dpi);
            std::thread::sleep(std::time::Duration::from_millis(400));
            capture(hwnd, "settings_probe_appearance", w, h);

            // Selector de color: clic en el swatch del campo "Fondo" (el
            // primero de Apariencia, en (458, 99) DIPs con la ventana de
            // 880px) y luego un clic dentro del cuadro SV del selector.
            // centrado.
            fn click(hwnd: HWND, x: f32, y: f32, dpi: f32) {
                let px = (x * dpi) as i32;
                let py = (y * dpi) as i32;
                let packed = ((py & 0xFFFF) << 16) | (px & 0xFFFF);
                let lparam = LPARAM(packed as i64 as isize);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), lparam);
                    let _ = PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), lparam);
                }
            }
            click(hwnd, 458.0, 99.0, dpi);
            std::thread::sleep(std::time::Duration::from_millis(400));
            capture(hwnd, "settings_probe_picker", w, h);
            click(hwnd, 420.0, 300.0, dpi);
            std::thread::sleep(std::time::Duration::from_millis(300));
            capture(hwnd, "settings_probe_picker2", w, h);

            // i18n: volver a General (el clic en la sidebar cierra el selector).
            // Las filas de carpetas anadidas desplazan la seccion de idioma,
            // asi que se baja con la rueda hasta el final (el chip "Español"
            // queda visible tras el scroll maximo, en ~(363, 605) DIPs).
            click_sidebar(hwnd, 0, dpi);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let wheel_down = WPARAM((((-120i32) as u16 as usize) << 16) as usize);
            for _ in 0..4 {
                let _ = PostMessageW(hwnd, WM_MOUSEWHEEL, wheel_down, LPARAM(0));
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            click(hwnd, 363.0, 605.0, dpi);
            std::thread::sleep(std::time::Duration::from_millis(300));
            capture(hwnd, "settings_probe_es", w, h);

            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }

        let _ = handle.join();
    }
}
