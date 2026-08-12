//! ZenDesktop :: ui.rs
//!
//! Capa de presentacion: cajas flotantes translucidas dibujadas con Direct2D +
//! DirectWrite sobre ventanas *layered* con canal alfa por pixel.
//!
//! Tecnica de composicion
//! ----------------------
//! Cada caja es una `WS_POPUP | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`.
//! Direct2D dibuja sobre un `ID2D1DCRenderTarget` enlazado a una DIB de 32 bits
//! (BGRA premultiplicado) y el resultado se publica con `UpdateLayeredWindow`.
//! Ventajas frente a una swapchain DXGI por ventana:
//!   * ~0 VRAM reservada por caja y ninguna cadena de intercambio viva.
//!   * Transparencia real por pixel sin `WS_EX_TRANSPARENT` ni color key.
//!   * El repintado ocurre unicamente cuando algo cambia (evento de disco,
//!     hover, scroll o arrastre): en reposo la GPU y la CPU quedan a cero.
//!
//! Modo Zen
//! --------
//! Un hook `WH_MOUSE_LL` sintetiza el doble clic (los hooks de bajo nivel no
//! reciben `WM_LBUTTONDBLCLK`) y, si el punto cae sobre el escritorio y no sobre
//! un icono, publica un mensaje asincrono a la ventana controladora. Toda la
//! logica pesada ocurre fuera del hook para no penalizar la latencia de entrada
//! del sistema.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;

use windows::core::{w, GUID, Interface, Result as WinResult, PCSTR, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, GENERIC_ACCESS_RIGHTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, POINTL,
    RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory1, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ELLIPSE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, Ellipse, GetDC, GetStockObject, GetTextExtentPoint32W,
    ReleaseDC, ScreenToClient, SelectObject, SetBkMode, SetTextColor, ValidateRect, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    DT_CENTER, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, TRANSPARENT, HBITMAP,
    HDC, HGDIOBJ, NULL_PEN,
};
use windows::Win32::System::Com::{
    CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
};

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, IWICBitmapDecoder, IWICBitmapFrameDecode,
    IWICBitmapScaler, IWICFormatConverter, IWICImagingFactory,
    GUID_WICPixelFormat32bppPBGRA, WICBitmapInterpolationMode,
    WICDecodeOptions,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalLock, GlobalUnlock, VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
    PAGE_READWRITE,
};
use windows::Win32::System::Ole::{
    RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_MOVE,
    DROPEFFECT_NONE, IDropTarget,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows::Win32::UI::Controls::{LVHITTESTINFO, LVM_HITTEST};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture, SetFocus,
    TrackMouseEvent, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, TME_LEAVE,
    TRACKMOUSEEVENT, VK_CONTROL, VK_SHIFT, VK_Z,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, HDROP, ShellExecuteW, Shell_NotifyIconW,
    SHBindToParent, SHGetFileInfoW, SHParseDisplayName,
    CMINVOKECOMMANDINFO, CMF_NORMAL,
    IContextMenu, IContextMenu3, IShellFolder, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON,
    SHGFI_USEFILEATTRIBUTES, SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{parse_color, wide, Config, FenceLayout, String32};
use crate::i18n::Tr;
use crate::rules::{self, FenceContent};
use crate::settings;
use crate::updater;
use crate::watcher::DesktopWatcher;

/// Carpetas que debe vigilar el watcher para una configuracion dada:
/// escritorio(s) mas las carpetas fisicas de las reglas activas.
fn watch_paths_for(cfg: &Config, desktop: &Path, extra_desktops: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vec![desktop.to_path_buf()];
    paths.extend(extra_desktops.iter().cloned());
    let root = cfg.root_dir(desktop);
    for rule in cfg.rules.iter().filter(|r| r.enabled && r.move_files) {
        paths.push(root.join(&rule.folder));
    }
    paths
}

// ---------------------------------------------------------------------------
// Mensajes, temporizadores y comandos
// ---------------------------------------------------------------------------

/// Rafaga de cambios en disco detectada por el vigilante.
pub const WM_ZEN_FS: u32 = WM_APP + 0x10;
/// Callback del icono de la bandeja del sistema.
pub const WM_ZEN_TRAY: u32 = WM_APP + 0x11;
/// Doble clic detectado sobre el escritorio (x en WPARAM, y en LPARAM).
pub const WM_ZEN_DBLCLICK: u32 = WM_APP + 0x12;    /// Refresco pendiente tras un DoDragDrop (se postea para que fence_proc retorne
    /// antes de que build_fences destruya la ventana).
    const WM_ZEN_DRAG_DONE: u32 = WM_APP + 0x13;
    /// Resultado del chequeo de updates en segundo plano (hilo de trabajo -> UI).
    pub const WM_ZEN_UPDATE_CHECKED: u32 = WM_APP + 0x14;
const TIMER_SWEEP: usize = 1;
const TIMER_PERSIST: usize = 2;
/// Temporizador de fade-in/fade-out de la miniatura (60 fps).
const TIMER_THUMB_FADE: usize = 3;
const TIMER_TOAST: usize = 4;
/// Temporizador de animaciones suaves (colapsar/expandir, Zen mode).
const TIMER_ANIM: usize = 5;
/// Pasos de interpolacion por animacion (8 pasos a 10ms = 80ms).
const ANIM_STEPS: u8 = 8;
/// Identificador del atajo global Ctrl+Alt+Z ("ZE" en ASCII).
const HOTKEY_ID: i32 = 0x5A45;
/// Identificador unico del icono de bandeja ("ZD" en ASCII).
const TRAY_UID: u32 = 0x5A44;
/// Recurso del icono de bandeja embebido en el .exe (ver assets/zendesktop.rc).
const TRAY_ICON_RES: usize = 2;

const CMD_ZEN: usize = 1;
const CMD_ORGANIZE: usize = 2;
const CMD_SWEEP: usize = 3;
const CMD_REFRESH: usize = 4;
const CMD_OPEN_FOLDER: usize = 5;
const CMD_COLLAPSE: usize = 6;
const CMD_SETTINGS: usize = 7;
const CMD_EDIT_CONFIG: usize = 8;
const CMD_RELOAD: usize = 9;
const CMD_EXIT: usize = 10;
const CMD_LOCK: usize = 11;
const CMD_SORT_NAME: usize = 12;
const CMD_SORT_SIZE: usize = 13;
const CMD_SORT_TYPE: usize = 14;
const CMD_SORT_MODIFIED: usize = 15;
const CMD_SORT_CUSTOM: usize = 16;

const CLASS_CONTROLLER: PCWSTR = w!("ZenDesktop.Controller");
const CLASS_FENCE: PCWSTR = w!("ZenDesktop.Fence");
const CLASS_THUMB: PCWSTR = w!("ZenDesktop.Thumb");
const CLASS_TOAST: PCWSTR = w!("ZenDesktop.Toast");

const GRIP: i32 = 22; // zona inferior derecha para redimensionar
const SCROLLBAR_W: f32 = 3.0;

/// Colores del icono del toast segun contexto.
const TOAST_DROP: COLORREF = COLORREF(0x004CCD3C);      // verde
const TOAST_ORGANIZE: COLORREF = COLORREF(0x003B82F6);  // azul

/// Anade un item de texto al menu contextual (el string se copia en el menu).
fn append_menu(menu: HMENU, id: usize, label: &str) {
    let text = wide(label);
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(text.as_ptr()));
    }
}

/// Rectangulo del candado de anclaje en la cabecera (coordenadas de la caja,
/// sin escalar). Compartido por el render y el hit-testing del clic.
fn lock_rect(w: f32, theme: &Theme, show_counter: bool, item_count: usize) -> D2D_RECT_F {
    let counter_w = if show_counter {
        let count_str = format!("{}", item_count);
        (22.0 + count_str.len() as f32 * 7.5).max(28.0)
    } else {
        0.0
    };
    let right = w - theme.padding - if show_counter { counter_w + 6.0 } else { 0.0 };
    let size = 11.0;
    let top = (theme.header * 0.5 - size / 2.0).max(2.0);
    D2D_RECT_F {
        left: (right - size).max(theme.padding),
        top,
        right: right.max(theme.padding + size),
        bottom: top + size,
    }
}

/// Rectangulo de la barra de busqueda en la cabecera (coordenadas de la caja, sin escalar).
/// Se posiciona a la izquierda del candado y contador, adaptando su tamano para no solaparse nunca.
fn search_rect(w: f32, theme: &Theme, show_counter: bool, item_count: usize) -> Option<D2D_RECT_F> {
    let lock_r = lock_rect(w, theme, show_counter, item_count);
    let right = lock_r.left - 8.0;
    let search_w = (w * 0.28).clamp(65.0, 135.0);
    let left = right - search_w;
    let min_left = theme.padding + 70.0;
    let search_h = 20.0;
    let top = (theme.header * 0.5 - search_h * 0.5).max(2.0);
    if left < min_left {
        let adjusted_w = right - min_left;
        if adjusted_w < 40.0 {
            return None;
        }
        Some(D2D_RECT_F {
            left: min_left,
            top,
            right,
            bottom: top + search_h,
        })
    } else {
        Some(D2D_RECT_F {
            left,
            top,
            right,
            bottom: top + search_h,
        })
    }
}

// ---------------------------------------------------------------------------
// Tema resuelto (colores ya parseados: cero parsing por frame)
// ---------------------------------------------------------------------------

struct Theme {
    background: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    border: D2D1_COLOR_F,
    title: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    muted: D2D1_COLOR_F,
    shadow: D2D1_COLOR_F,
    radius: f32,
    border_width: f32,
    padding: f32,
    header: f32,
    row: f32,
    show_icons: bool,
    show_counter: bool,
    snap: i32,
    grid_mode: bool,
    grid_item_size: f32,
}

fn color(hex: &str) -> D2D1_COLOR_F {
    let (r, g, b, a) = parse_color(hex);
    D2D1_COLOR_F { r, g, b, a }
}

fn with_alpha(c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

impl Theme {
    fn from_config(cfg: &Config) -> Theme {
        let a = &cfg.appearance;
        Theme {
            background: color(&a.background),
            hover: color(&a.background_hover),
            border: color(&a.border),
            title: color(&a.title_color),
            text: color(&a.text_color),
            muted: color(&a.muted_color),
            shadow: color(&a.shadow),
            radius: a.corner_radius,
            border_width: a.border_width,
            padding: a.padding,
            header: a.header_height,
            row: a.row_height,
            show_icons: a.show_icons,
            show_counter: a.show_counter,
            snap: a.snap_grid as i32,
            grid_mode: a.grid_mode,
            grid_item_size: a.grid_item_size,
        }
    }
}

// ---------------------------------------------------------------------------
// Recursos graficos compartidos por todas las cajas
// ---------------------------------------------------------------------------

struct Graphics {
    d2d: ID2D1Factory1,
    title_format: IDWriteTextFormat,
    text_format: IDWriteTextFormat,
    meta_format: IDWriteTextFormat,
    center_meta_format: IDWriteTextFormat,
}

impl Graphics {
    fn new(cfg: &Config) -> WinResult<Graphics> {
        unsafe {
            let d2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            let family = wide(&cfg.appearance.font_family);
            let locale = w!("");

            let title_format = dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                cfg.appearance.title_size,
                locale,
            )?;
            let text_format = dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                cfg.appearance.text_size,
                locale,
            )?;
            let meta_format = dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                (cfg.appearance.text_size - 1.5).max(7.0),
                locale,
            )?;
            let center_meta_format = dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                (cfg.appearance.text_size - 2.0).max(7.5),
                locale,
            )?;

            let ellipsis = dwrite.CreateEllipsisTrimmingSign(&text_format)?;
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };

            for format in [&title_format, &text_format, &meta_format, &center_meta_format] {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                format.SetTrimming(&trimming, &ellipsis)?;
            }
            meta_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            center_meta_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

            Ok(Graphics {
                d2d,
                title_format,
                text_format,
                meta_format,
                center_meta_format,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Superficie de dibujo (DIB 32 bpp + render target de Direct2D)
// ---------------------------------------------------------------------------

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    width: i32,
    height: i32,
}

impl Surface {
    unsafe fn new(gfx: &Graphics, width: i32, height: i32) -> WinResult<Surface> {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let _ = ReleaseDC(None, screen);

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down: primera fila = parte superior
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        // HGDIOBJ(handle.0) en lugar de .into(): conversion estable entre
        // versiones de la crate windows, donde los handles cambian de forma.
        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));

        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0, // el escalado DPI se aplica manualmente por caja
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let target = gfx.d2d.CreateDCRenderTarget(&props)?;
        let brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            None,
        )?;

        Ok(Surface {
            dc,
            bitmap,
            previous,
            target,
            brush,
            width,
            height,
        })
    }
}


// ---------------------------------------------------------------------------
// Superficie GDI simple (DIB 32 bpp, sin D2D) para la miniatura emergente.
// ---------------------------------------------------------------------------

struct SurfaceDib {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    width: i32,
    height: i32,
}

impl SurfaceDib {
    unsafe fn new(width: i32, height: i32) -> Option<SurfaceDib> {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let _ = ReleaseDC(None, screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
        // Rellenar con negro transparente.
        for i in 0..(width * height) as usize {
            *(bits as *mut u32).add(i) = 0;
        }
        Some(SurfaceDib { dc, bitmap, previous, bits: bits as *mut u32, width, height })
    }
}

impl Drop for SurfaceDib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Miniatura cargada (WIC decode -> raw BGRA pixels).
struct ThumbEntry {
    pixels: Vec<u8>,
    w: i32,
    h: i32,
}

/// Cache LRU de miniaturas (hasta 32 entradas).
struct ThumbCache {
    entries: Vec<(PathBuf, ThumbEntry)>,
}

impl ThumbCache {
    fn new() -> Self { ThumbCache { entries: Vec::new() } }

    fn get(&self, path: &Path) -> Option<&ThumbEntry> {
        self.entries.iter().find(|(p, _)| p == path).map(|(_, t)| t)
    }

    fn insert(&mut self, path: PathBuf, entry: ThumbEntry) {
        self.entries.retain(|(p, _)| p != &path);
        self.entries.push((path, entry));
        if self.entries.len() > 32 {
            self.entries.remove(0);
        }
    }

}

/// Carga una miniatura desde disco usando WIC (maximo MAX_DIM px).
const THUMB_MAX: u32 = 160;

unsafe fn load_thumbnail(path: &Path) -> Option<ThumbEntry> {
    let wic: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let wide_path: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let decoder: IWICBitmapDecoder = wic
        .CreateDecoderFromFilename(
            PCWSTR(wide_path.as_ptr()),
            None,
            GENERIC_ACCESS_RIGHTS(0x80000000u32), // GENERIC_READ
            WICDecodeOptions(0), /* WICDecodeMetadataCacheOnDemand */
        )
        .ok()?;
    let frame: IWICBitmapFrameDecode = decoder.GetFrame(0).ok()?;
    // Escalar manteniendo proporcion.
    let (mut fw, mut fh) = (0u32, 0u32);
    frame.GetSize(&mut fw, &mut fh).ok()?;
    let scale = THUMB_MAX as f32 / fw.max(fh).max(1) as f32;
    let scale = if scale >= 1.0 { 1.0 } else { scale };
    let tw = (fw as f32 * scale).max(1.0) as u32;
    let th = (fh as f32 * scale).max(1.0) as u32;
    let scaler: IWICBitmapScaler = wic
        .CreateBitmapScaler()
        .ok()?;
    scaler.Initialize(&frame, tw, th, WICBitmapInterpolationMode(1)).ok()?; // WICBitmapInterpolationModeFant=0x1
    // Convertir a 32bpp PBGRA.
    let converter: IWICFormatConverter = wic.CreateFormatConverter().ok()?;
    converter
        .Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            windows::Win32::Graphics::Imaging::WICBitmapDitherType(0),
            None,
            0.0,
            windows::Win32::Graphics::Imaging::WICBitmapPaletteType(0),
        )
        .ok()?;
    let mut pixels = vec![0u8; (tw * th * 4) as usize];
    let stride = tw * 4;
    // Convertir a BGRA pre-multiplicado (para DIB 32bpp).
    converter.CopyPixels(std::ptr::null(), stride, &mut pixels)
        .ok()?;
    Some(ThumbEntry { pixels, w: tw as i32, h: th as i32 })
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

// ---------------------------------------------------------------------------
// Cache de iconos del shell
// ---------------------------------------------------------------------------

#[derive(Default)]
struct IconCache {
    by_ext: HashMap<String, HICON>,
    by_path: HashMap<PathBuf, HICON>,
}

impl IconCache {
    /// Para la mayoria de extensiones basta con una consulta por tipo
    /// (`SHGFI_USEFILEATTRIBUTES`, sin tocar el disco). Solo ejecutables,
    /// accesos directos e iconos requieren consulta por archivo.
    unsafe fn get(&mut self, path: &Path, ext: &str, is_dir: bool, large: bool) -> Option<HICON> {
        let per_file = matches!(ext, "exe" | "lnk" | "ico" | "msi" | "url");
        if per_file {
            if let Some(icon) = self.by_path.get(path) {
                return Some(*icon);
            }
            if self.by_path.len() > 256 {
                self.purge_paths();
            }
            let icon = query_icon(&wide(&path.to_string_lossy()), false, large)?;
            self.by_path.insert(path.to_path_buf(), icon);
            return Some(icon);
        }

        let key = if is_dir {
            String::from("<dir>")
        } else if ext.is_empty() {
            String::from("<file>")
        } else {
            ext.to_string()
        };
        if let Some(icon) = self.by_ext.get(&key) {
            return Some(*icon);
        }
        let probe = if is_dir {
            wide("C:\\")
        } else {
            wide(&format!("zen.{key}"))
        };
        let icon = query_icon(&probe, true, large)?;
        self.by_ext.insert(key, icon);
        Some(icon)
    }

    fn purge_paths(&mut self) {
        for (_, icon) in self.by_path.drain() {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

impl Drop for IconCache {
    fn drop(&mut self) {
        self.purge_paths();
        for (_, icon) in self.by_ext.drain() {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

unsafe fn query_icon(path: &[u16], use_attributes: bool, large: bool) -> Option<HICON> {
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_ICON | if large { SHGFI_LARGEICON } else { SHGFI_SMALLICON };
    let attributes = if use_attributes {
        flags |= SHGFI_USEFILEATTRIBUTES;
        windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL
    } else {
        windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    let ok = SHGetFileInfoW(
        PCWSTR(path.as_ptr()),
        attributes,
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        flags,
    );
    if ok == 0 || info.hIcon.is_invalid() {
        None
    } else {
        Some(info.hIcon)
    }
}

// ---------------------------------------------------------------------------
// Caja
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum DragMode {
    None,
    Move,
    Resize,
    Select { start_x: f32, start_y: f32, curr_x: f32, curr_y: f32 },
    ItemDrag { item_idx: usize, start_x: i32, start_y: i32 },
    ManualDrag { paths: Vec<PathBuf>, source_fence: usize },
}

struct Fence {
    hwnd: HWND,
    content: FenceContent,
    layout: FenceLayout,
    accent: D2D1_COLOR_F,
    surface: Option<Surface>,
    scroll: i32,
    hover: i32,
    hover_lock: bool,
    selected: HashSet<usize>,
    rubberband: Option<(f32, f32, f32, f32)>,
    search_focused: bool,
    drag: DragMode,
    anchor: POINT,
    origin: FenceLayout,
    scale: f32,
    reorder_drop: Option<usize>,
    sort_mode: Option<String>,
    search_text: String,
    hit_row_h: f32,
}

impl Fence {
    fn visible_height(&self, theme: &Theme) -> i32 {
        if self.layout.collapsed {
            ((theme.header + theme.padding) * self.scale) as i32
        } else {
            self.layout.height
        }
    }

    fn rows_visible(&self, theme: &Theme) -> i32 {
        let usable = self.layout.height as f32 / self.scale - theme.header - theme.padding;
        (usable / theme.row).floor().max(0.0) as i32
    }

    fn max_scroll(&self, theme: &Theme) -> i32 {
        (self.content.items.len() as i32 - self.rows_visible(theme)).max(0)
    }

    fn item_at(&self, theme: &Theme, x: i32, y: i32) -> Option<usize> {
        let fy = y as f32 / self.scale;
        let fx = x as f32 / self.scale;
        if fy < theme.header || fx < 0.0 {
            return None;
        }
        if theme.grid_mode {
            let cell = theme.grid_item_size.max(48.0);
            let pad = theme.padding;
            let w_dip = self.layout.width as f32 / self.scale;
            let grid_cols = ((w_dip - pad) / cell).floor().max(1.0) as usize;
            let row = ((fy - theme.header) / cell).floor() as i32 + self.scroll;
            let col = ((fx - pad * 0.5) / cell).floor() as i32;
            if col >= 0 && col < grid_cols as i32 && row >= 0 {
                let index = row as usize * grid_cols + col as usize;
                if index < self.content.items.len() {
                    return Some(index);
                }
            }
            None
        } else {
            let row_h = if self.hit_row_h > 0.0 { self.hit_row_h } else { theme.row };
            let index = ((fy - theme.header) / row_h).floor() as i32 + self.scroll;
            if index >= 0 && (index as usize) < self.content.items.len() {
                Some(index as usize)
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Aplicacion
// ---------------------------------------------------------------------------

pub struct App {
    cfg: Config,
    cfg_path: PathBuf,
    pub(crate) desktop: PathBuf,
    extra_desktops: Vec<PathBuf>,
    gfx: Graphics,
    theme: Theme,
    fences: Vec<Fence>,
    icons: IconCache,
    zen: bool,
    controller: HWND,
    instance: HINSTANCE,
    hook: HHOOK,
    tray: bool,
    tray_icon: HICON,
    settings_open: bool,
    watcher: Option<DesktopWatcher>,
    /// Textos visibles en el idioma de la configuracion.
    tr: &'static Tr,
    /// El escritorio ya se restauro al cerrar (evita hacerlo dos veces).
    restored: bool,
    /// Menu contextual nativo (IContextMenu) mientras TrackPopupMenu esta
    /// abierto: el proc de la caja reenvia WM_INITMENUPOPUP/DRAWITEM/
    /// MEASUREITEM para que los submenus ("Abrir con"...) funcionen.
    shell_menu: Option<IContextMenu>,
    /// Objetivo OLE de arrastrar y soltar, registrado en cada caja con
    /// RegisterDragDrop (el objeto vive con la aplicacion via Box::leak).
    drop_target: Option<IDropTarget>,
    /// Ventana emergente de miniatura (WS_EX_LAYERED, una sola, reutilizada).
    thumb_hwnd: HWND,
    /// Superficie de la ventana de miniatura.
    thumb_surface: Option<SurfaceDib>,
    /// Alfa actual del fade (0..255).
    thumb_alpha: u8,
    /// True si estamos decrementando el alfa para desaparecer.
    thumb_fading_out: bool,
    /// Posicion de la ventana de miniatura (para re-enviar UpdateLayeredWindow
    /// durante el fade sin volver a calcularla en cada tick del timer).
    thumb_pos: POINT,
    /// Tamano de la ventana de miniatura (para el UpdateLayeredWindow del fade).
    thumb_sz: SIZE,
    toast_hwnd: HWND,
    toast_surface: Option<SurfaceDib>,
    toast_alpha: u8,
    toast_fading_out: bool,
    toast_hold: u32,
    anim_kind: u8,
    anim_step: u8,
    anim_fence: usize,
    anim_zen_alpha: u8,
    /// true mientras DoDragDrop esta en curso (suprime refrescos de UI anidados).
    dragging: bool,
    /// Toast pendiente de mostrar al terminar un DoDragDrop interno.
    pending_toast: Option<String>,
    /// Archivos devueltos al escritorio manualmente: saltar el proximo organize.
    skip_next_organize: bool,
    /// Superficie para la imagen de arrastre manual.
    drag_image_surface: Option<SurfaceDib>,
    /// (fence_idx, item_idx) resaltado durante un drag manual (subcarpeta destino).
    drag_hover_target: Option<(usize, i32)>,
}

/// Puntero estable a la aplicacion, propiedad del hilo de interfaz.
pub struct AppHandle {
    app: *mut App,
}

impl AppHandle {
    pub fn controller(&self) -> HWND {
        unsafe { (*self.app).controller }
    }

    pub fn watch_paths(&self) -> Vec<PathBuf> {
        unsafe { (*self.app).watch_paths() }
    }

    pub fn attach_watcher(&self, watcher: DesktopWatcher) {
        unsafe {
            (*self.app).watcher = Some(watcher);
        }
    }

    /// Libera ventanas, hooks, icono de bandeja y memoria.
    pub fn shutdown(self) {
        unsafe {
            let app = Box::from_raw(self.app);
            drop(app);
        }
    }
}

/// Menu contextual nativo (IContextMenu) mientras TrackPopupMenu esta
/// abierto: prepara el menu para uno o varios archivos usando SHBindToParent.
unsafe fn build_shell_menu_for_paths(paths: &[PathBuf]) -> Result<(IContextMenu, HMENU, *mut ITEMIDLIST), String> {
    if paths.is_empty() {
        return Err("No paths".into());
    }
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    for p in paths {
        let wide_path = wide(&p.to_string_lossy());
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(wide_path.as_ptr()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
            pidls.push(pidl);
        }
    }
    if pidls.is_empty() {
        return Err("SHParseDisplayName".into());
    }

    let mut first_child: *mut ITEMIDLIST = std::ptr::null_mut();
    let parent: IShellFolder = match SHBindToParent(pidls[0], Some(&mut first_child)) {
        Ok(f) => f,
        Err(_) => {
            for p in &pidls { let _ = CoTaskMemFree(Some(*p as *const c_void)); }
            return Err("SHBindToParent".into());
        }
    };

    let mut child_pidls: Vec<*const ITEMIDLIST> = Vec::new();
    let mut child_ptrs: Vec<*mut ITEMIDLIST> = Vec::new();
    for full_pidl in &pidls {
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let res: windows::core::Result<IShellFolder> = SHBindToParent(*full_pidl, Some(&mut child));
        if res.is_ok() && !child.is_null() {
            child_ptrs.push(child);
        }
    }
    for cp in &child_ptrs {
        child_pidls.push(*cp as *const ITEMIDLIST);
    }
    if child_pidls.is_empty() {
        for p in &pidls { let _ = CoTaskMemFree(Some(*p as *const c_void)); }
        return Err("No child pidls".into());
    }

    let menu: IContextMenu = match parent.GetUIObjectOf(HWND::default(), &child_pidls, None) {
        Ok(m) => m,
        Err(_) => {
            for p in &pidls { let _ = CoTaskMemFree(Some(*p as *const c_void)); }
            return Err("GetUIObjectOf".into());
        }
    };

    let hmenu = match CreatePopupMenu() {
        Ok(h) => h,
        Err(_) => {
            for p in &pidls { let _ = CoTaskMemFree(Some(*p as *const c_void)); }
            return Err("CreatePopupMenu".into());
        }
    };

    if menu.QueryContextMenu(hmenu, 0, 0x8000, 0xFFFF, CMF_NORMAL).is_err() {
        let _ = DestroyMenu(hmenu);
        for p in &pidls { let _ = CoTaskMemFree(Some(*p as *const c_void)); }
        return Err("QueryContextMenu".into());
    }

    if GetMenuItemCount(hmenu) <= 0 {
        let _ = DestroyMenu(hmenu);
        for p in &pidls { let _ = CoTaskMemFree(Some(*p as *const c_void)); }
        return Err("GetMenuItemCount==0".into());
    }

    // Liberar los PIDLs extra (pidls[1..]): pidls[0] lo libera el caller.
    for p in pidls.iter().skip(1) {
        let _ = CoTaskMemFree(Some(*p as *const c_void));
    }
    Ok((menu, hmenu, pidls[0]))
}

impl App {
    /// Crea la ventana controladora, las cajas y todos los recursos graficos.
    pub fn launch(
        cfg: Config,
        cfg_path: PathBuf,
        desktop: PathBuf,
        extra_desktops: Vec<PathBuf>,
    ) -> WinResult<AppHandle> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            register_classes(instance)?;

            let gfx = Graphics::new(&cfg)?;
            let theme = Theme::from_config(&cfg);
            let tr = Tr::get(cfg.lang());

            let mut app = Box::new(App {
                cfg,
                cfg_path,
                desktop,
                extra_desktops,
                gfx,
                theme,
                fences: Vec::new(),
                icons: IconCache::default(),
                zen: false,
                controller: HWND::default(),
                instance,
                hook: HHOOK::default(),
                tray: false,
                tray_icon: HICON::default(),
                settings_open: false,
                watcher: None,
                tr,
                restored: false,
                shell_menu: None,
                drop_target: None,
                thumb_hwnd: HWND::default(),
                thumb_surface: None,
                thumb_alpha: 255,
                thumb_fading_out: false,
                thumb_pos: POINT::default(),
                thumb_sz: SIZE::default(),
                toast_hwnd: HWND::default(),
                toast_surface: None,
                toast_alpha: 255,
                toast_fading_out: false,
                toast_hold: 0,
                anim_kind: 0,
                anim_step: 0,
                anim_fence: 0,
                anim_zen_alpha: 255,
                dragging: false,
                pending_toast: None,
                skip_next_organize: false,
                drag_image_surface: None,
                drag_hover_target: None,
            });
            // Objetivo OLE de arrastrar y soltar, compartido por todas las
            // cajas: vive con la aplicacion (Box::leak) y guarda el puntero a
            // `App` para mover los ficheros soltados a la caja correcta.
            let app_ptr: *mut App = &mut *app;
            let drop_obj = Box::leak(Box::new(FenceDropTarget {
                vtbl: &DROP_TARGET_VTBL,
                app: app_ptr,
                accepts_files: std::cell::Cell::new(false),
            }));
            app.drop_target = Some(IDropTarget::from_raw(
                drop_obj as *mut FenceDropTarget as *mut c_void,
            ));
            let ptr = Box::into_raw(app);

            // Ventana controladora: nunca visible, pero de nivel superior para
            // poder recibir hotkeys, mensajes de bandeja y menus contextuales.
            let controller = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_CONTROLLER,
                w!("ZenDesktop"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                Some(ptr as *const c_void),
            )?;
            (*ptr).controller = controller;

            (*ptr).build_fences()?;
            (*ptr).toast_hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                CLASS_TOAST,
                w!(""),
                WS_POPUP,
                0, 0, 1, 1,
                None, None,
                instance,
                None,
            )?;
            (*ptr).thumb_hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_THUMB,
                w!(""),
                WS_POPUP,
                0, 0, 1, 1,
                None, None,
                instance,
                None,
            )?;
            (*ptr).install_tray();
            (*ptr).install_hooks();

            let period = (*ptr).cfg.general.sweep_interval_minutes as u32 * 60_000;
            SetTimer(controller, TIMER_SWEEP, period, None);

            Ok(AppHandle { app: ptr })
        }
    }

    // -- ciclo de vida de las cajas ----------------------------------------

    fn watch_paths(&self) -> Vec<PathBuf> {
        watch_paths_for(&self.cfg, &self.desktop, &self.extra_desktops)
    }

    /// Crea una ventana por regla activa y calcula la geometria inicial.
    unsafe fn build_fences(&mut self) -> WinResult<()> {
        for fence in self.fences.drain(..) {
            let _ = RevokeDragDrop(fence.hwnd);
            let _ = DestroyWindow(fence.hwnd);
        }

        let contents = rules::collect_fences(&self.cfg, &self.desktop, &std::collections::HashMap::new());
        let work = work_area();
        let mut cursor_y = work.top + 32;

        for (index, content) in contents.into_iter().enumerate() {
            let layout = self.cfg.layout_of(&content.id).unwrap_or(FenceLayout {
                id: String32::new(&content.id),
                x: work.left + 32,
                y: {
                    let y = cursor_y;
                    cursor_y += 260;
                    if cursor_y > work.bottom - 80 {
                        cursor_y = work.top + 32;
                    }
                    y
                },
                width: 320,
                height: 240,
                collapsed: false,
                hidden: false,
                locked: false,
                sort_by: None,
            });
            let _ = index;

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_FENCE,
                w!("ZenDesktop Fence"),
                WS_POPUP,
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                None,
                None,
                self.instance,
                Some(self as *mut App as *const c_void),
            )?;

            let dpi = GetDpiForWindow(hwnd) as f32;
            let fence = Fence {
                hwnd,
                accent: color(&content.color),
                content,
                layout: layout.clone(),
                surface: None,
                scroll: 0,
                hover: -1,
                hover_lock: false,
                selected: HashSet::new(),
                rubberband: None,
                search_focused: false,
                drag: DragMode::None,
                anchor: POINT::default(),
                origin: layout.clone(),
                scale: if dpi > 0.0 { dpi / 96.0 } else { 1.0 },
                reorder_drop: None,
                sort_mode: layout.sort_by.clone(),
                search_text: String::new(),
                hit_row_h: self.theme.row,
            };
            self.fences.push(fence);

            let _ = SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNA);

            // Mover layout al final: SetWindowPos de arriba usa layout.x/y/w/h.
            self.fences.last_mut().unwrap().layout = layout;

            // Objetivo de arrastrar y soltar: OLE (RegisterDragDrop) mas el
            // mecanismo clasico WM_DROPFILES como red de seguridad.
            if let Some(target) = &self.drop_target {
                let _ = RegisterDragDrop(hwnd, target);
            }
            let _ = DragAcceptFiles(hwnd, true);
        }

        self.render_all();
        Ok(())
    }

    /// Recalcula el contenido de cada caja tras una rafaga de disco.
    /// Ficheros de un HDROP (WM_DROPFILES o extraidos de IDataObject) que caen
    /// sobre la caja `index`: se mueven a su carpeta fisica y se refresca el
    /// contenido. Si la caja es virtual (sin carpeta) no se hace nada.
    /// Ficheros de un HDROP (WM_DROPFILES) sobre la caja `index`.
    unsafe fn handle_hdrop(&mut self, index: usize, hdrop: HDROP) {
        let count = DragQueryFileW(hdrop, 0xFFFFFFFF, None);
        if count == 0 {
            return;
        }
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut buf = vec![0u16; 4096];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf));
            if n > 0 {
                paths.push(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])));
            }
        }
        self.accept_drop(index, paths);
    }

    /// Mueve rutas soltadas a la carpeta fisica de la caja `index` y refresca
    /// el contenido si algo se movio. Devuelve cuantos ficheros se movieron.
    unsafe fn accept_drop(&mut self, index: usize, paths: Vec<PathBuf>) -> usize {
        let folder = match self.fences.get(index).and_then(|f| f.content.folder.clone()) {
            Some(f) => f,
            None => return 0,
        };
        let mut moved = 0usize;
        for src in paths {
            if rules::move_into_any(&src, &folder).unwrap_or(false) {
                moved += 1;
            }
        }
        if moved > 0 {
            if self.dragging {
                let fname = self.fences[index].content.title.clone();
                self.pending_toast = Some(format!("{} {} → {}", moved, self.tr.toast_dropped, fname));
            } else {
                let fname = self.fences[index].content.title.clone();
                let msg = format!("{} {} → {}", moved, self.tr.toast_dropped, fname);
                self.show_toast(&msg, TOAST_DROP);
                self.refresh_contents();
                let _ = SetTimer(self.controller, TIMER_PERSIST, 800, None);
            }
        }
        moved
    }

    /// Mueve elementos de vuelta a la raíz del Escritorio (<Escritorio>).
    unsafe fn drop_to_desktop(&mut self, paths: Vec<PathBuf>) -> usize {
        let mut moved = 0usize;
        for src in paths {
            if rules::move_into_any(&src, &self.desktop).unwrap_or(false) {
                moved += 1;
            }
        }
        if moved > 0 {
            // Evitar que el organizador automatico devuelva los archivos a la caja.
            self.skip_next_organize = true;
            if self.dragging {
                self.pending_toast = Some(format!("{} elementos devueltos al Escritorio", moved));
            } else {
                let msg = format!("{} elementos devueltos al Escritorio", moved);
                self.show_toast(&msg, TOAST_DROP);
                self.refresh_contents();
                rules::notify_shell();
                let _ = SetTimer(self.controller, TIMER_PERSIST, 800, None);
            }
        }
        moved
    }

    /// Mueve rutas a una carpeta destino concreta (p.ej. una subcarpeta dentro
    /// de una caja) y muestra toast con el nombre de la carpeta.
    unsafe fn move_paths_to(&mut self, dest: &Path, dest_name: &str, paths: Vec<PathBuf>) -> usize {
        let mut moved = 0usize;
        for src in paths {
            if rules::move_into_any(&src, dest).unwrap_or(false) {
                moved += 1;
            }
        }
        if moved > 0 {
            self.skip_next_organize = true;
            if self.dragging {
                self.pending_toast = Some(format!("{} {} → {}", moved, self.tr.toast_dropped, dest_name));
            } else {
                let msg = format!("{} {} → {}", moved, self.tr.toast_dropped, dest_name);
                self.show_toast(&msg, TOAST_DROP);
                self.refresh_contents();
                let _ = SetTimer(self.controller, TIMER_PERSIST, 800, None);
            }
        }
        moved
    }

    /// Extrae la lista de rutas de un IDataObject (CF_HDROP, OLE drag & drop).
    unsafe fn paths_from_idata(data: &IDataObject) -> Vec<PathBuf> {
        let fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = match data.GetData(&fmt) {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        if medium.tymed != TYMED_HGLOBAL.0 as u32 {
            let _ = ReleaseStgMedium(&mut medium);
            return Vec::new();
        }
        let ptr = GlobalLock(medium.u.hGlobal);
        if ptr.is_null() {
            let _ = ReleaseStgMedium(&mut medium);
            return Vec::new();
        }
        let hdrop = HDROP(ptr);
        let mut paths = Vec::new();
        let count = DragQueryFileW(hdrop, 0xFFFFFFFF, None);
        for i in 0..count {
            let mut buf = vec![0u16; 4096];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf));
            if n > 0 {
                paths.push(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])));
            }
        }
        let _ = GlobalUnlock(medium.u.hGlobal);
        let _ = ReleaseStgMedium(&mut medium);
        paths
    }

    fn refresh_contents(&mut self) {
        // No refrescar contenidos durante DoDragDrop: build_fences
        // destruye/crea ventanas y corrompe OLE.
        if self.dragging { return; }
        let mut sort_overrides: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
        for fence in &self.fences {
            sort_overrides.insert(fence.content.id.clone(), fence.sort_mode.clone());
        }
        let contents = rules::collect_fences(&self.cfg, &self.desktop, &sort_overrides);
        let same_shape = contents.len() == self.fences.len()
            && contents
                .iter()
                .zip(self.fences.iter())
                .all(|(c, f)| c.id == f.content.id);

        if !same_shape {
            unsafe {
                let _ = self.build_fences();
            }
            return;
        }
        for (fence, content) in self.fences.iter_mut().zip(contents.into_iter()) {
            fence.content = content;
            let max = fence.max_scroll(&self.theme);
            fence.scroll = fence.scroll.min(max).max(0);
        }
        self.render_all();
    }

    fn render_all(&mut self) {
        for index in 0..self.fences.len() {
            let _ = self.render(index);
        }
    }

    fn index_of(&self, hwnd: HWND) -> Option<usize> {
        self.fences.iter().position(|f| f.hwnd == hwnd)
    }

    /// Limpia el highlight de subcarpeta destino al terminar un drag.
    fn clear_drag_hover(&mut self) {
        if let Some((fi, _)) = self.drag_hover_target.take() {
            self.fences[fi].hover = -1;
            let _ = self.render(fi);
        }
    }

    // -- renderizado --------------------------------------------------------

    fn render(&mut self, index: usize) -> WinResult<()> {
        if self.zen || index >= self.fences.len() {
            return Ok(());
        }
        let theme_ptr: *const Theme = &self.theme;
        let gfx_ptr: *const Graphics = &self.gfx;
        // SEGURIDAD: theme y gfx viven en el mismo `App` y no se mutan durante
        // el dibujado; se toman punteros para poder tomar prestada la caja
        // de forma mutable sin dividir la estructura en dos objetos.
        let theme = unsafe { &*theme_ptr };
        let gfx = unsafe { &*gfx_ptr };
        let icons: *mut IconCache = &mut self.icons;
        let fence = &mut self.fences[index];

        let width = fence.layout.width.max(80);
        let height = fence.visible_height(theme).max(28);
        let scale = fence.scale;

        unsafe {
            let recreate = match &fence.surface {
                Some(s) => s.width != width || s.height != height,
                None => true,
            };
            if recreate {
                fence.surface = Some(Surface::new(gfx, width, height)?);
            }
            let surface = match fence.surface.as_ref() {
                Some(s) => s,
                None => return Ok(()),
            };

            let bounds = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            surface.target.BindDC(surface.dc, &bounds)?;
            surface.target.BeginDraw();
            surface.target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            let w = width as f32 / scale;
            let h = height as f32 / scale;
            let radius = theme.radius;
            let brush = &surface.brush;

            // Sombra falsa: dos rectangulos redondeados con alfa muy bajo.
            // Mas barato que un D2D1Effect de desenfoque gaussiano.
            brush.SetColor(&theme.shadow);
            surface.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(1.5 * scale, 2.5 * scale, (w - 1.5) * scale, (h - 0.5) * scale),
                    radiusX: radius * scale,
                    radiusY: radius * scale,
                },
                brush,
            );

            // Fondo translucido.
            brush.SetColor(&theme.background);
            let body = D2D1_ROUNDED_RECT {
                rect: rect(0.0, 0.0, (w - 1.0) * scale, (h - 1.5) * scale),
                radiusX: radius * scale,
                radiusY: radius * scale,
            };
            surface.target.FillRoundedRectangle(&body, brush);

            // Borde de un pixel logico.
            brush.SetColor(&theme.border);
            surface
                .target
                .DrawRoundedRectangle(&body, brush, theme.border_width * scale, None);

            // Fondo de la cabecera (Header Backdrop Bar)
            let header_h = theme.header * scale;
            brush.SetColor(&with_alpha(theme.text, 0.035));
            surface.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(0.0, 0.0, w * scale, header_h),
                    radiusX: radius * scale,
                    radiusY: radius * scale,
                },
                brush,
            );

            // Linea divisoria inferior de la cabecera
            let pad = theme.padding * scale;
            brush.SetColor(&with_alpha(theme.border, 0.30));
            surface.target.DrawLine(
                D2D_POINT_2F { x: pad * 0.5, y: header_h },
                D2D_POINT_2F { x: (w - theme.padding * 0.5) * scale, y: header_h },
                brush,
                1.0 * scale,
                None,
            );

            // Pill de acento vertical brillante
            let pill_h = 14.0 * scale;
            let pill_w = 3.5 * scale;
            let pill_y = (header_h - pill_h) * 0.5;
            
            brush.SetColor(&with_alpha(fence.accent, 0.25));
            surface.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(pad - 1.0 * scale, pill_y - 1.0 * scale, pad + pill_w + 1.0 * scale, pill_y + pill_h + 1.0 * scale),
                    radiusX: 2.5 * scale,
                    radiusY: 2.5 * scale,
                },
                brush,
            );
            brush.SetColor(&fence.accent);
            surface.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(pad, pill_y, pad + pill_w, pill_y + pill_h),
                    radiusX: 1.75 * scale,
                    radiusY: 1.75 * scale,
                },
                brush,
            );

            let item_cnt = fence.content.items.len();
            let title_right = if self.cfg.appearance.show_search {
                if let Some(sr) = search_rect(w, theme, theme.show_counter, item_cnt) {
                    sr.left - 8.0
                } else {
                    lock_rect(w, theme, theme.show_counter, item_cnt).left - 8.0
                }
            } else {
                lock_rect(w, theme, theme.show_counter, item_cnt).left - 8.0
            };

            let title = wide_str(&fence.content.title);
            brush.SetColor(&theme.title);
            surface.target.DrawText(
                &title,
                &gfx.title_format,
                &rect(
                    pad + 10.0 * scale,
                    0.0,
                    title_right.max(pad + 20.0) * scale,
                    header_h,
                ),
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // Badge / Pildora del contador de elementos
            if theme.show_counter {
                let count_str = format!("{}", item_cnt);
                let counter = wide_str(&count_str);
                let badge_w = (22.0 + count_str.len() as f32 * 7.5) * scale;
                let badge_h = 16.0 * scale;
                let badge_right = (w - theme.padding) * scale;
                let badge_left = badge_right - badge_w;
                let badge_top = (header_h - badge_h) * 0.5;

                brush.SetColor(&with_alpha(theme.text, 0.08));
                surface.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(badge_left, badge_top, badge_right, badge_top + badge_h),
                        radiusX: 8.0 * scale,
                        radiusY: 8.0 * scale,
                    },
                    brush,
                );
                brush.SetColor(&with_alpha(theme.border, 0.35));
                surface.target.DrawRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(badge_left, badge_top, badge_right, badge_top + badge_h),
                        radiusX: 8.0 * scale,
                        radiusY: 8.0 * scale,
                    },
                    brush,
                    0.8 * scale,
                    None,
                );
                brush.SetColor(&with_alpha(theme.text, 0.85));
                surface.target.DrawText(
                    &counter,
                    &gfx.center_meta_format,
                    &rect(badge_left, badge_top, badge_right, badge_top + badge_h),
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Campo de busqueda
            let show_search = self.cfg.appearance.show_search;
            if show_search {
                if let Some(sr) = search_rect(w, theme, theme.show_counter, item_cnt) {
                    let search_x = sr.left * scale;
                    let search_y = sr.top * scale;
                    let search_w = (sr.right - sr.left) * scale;
                    let search_h = (sr.bottom - sr.top) * scale;
                    let is_focused = fence.search_focused;

                    let bg_color = if is_focused {
                        with_alpha(theme.background, 0.90)
                    } else {
                        with_alpha(theme.background, 0.55)
                    };
                    brush.SetColor(&bg_color);
                    surface.target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(search_x, search_y, search_x + search_w, search_y + search_h),
                            radiusX: 6.0 * scale,
                            radiusY: 6.0 * scale,
                        },
                        brush,
                    );
                    let border_col = if is_focused { fence.accent } else { with_alpha(theme.border, 0.45) };
                    let border_w = if is_focused { 1.2 * scale } else { 0.8 * scale };
                    brush.SetColor(&border_col);
                    surface.target.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(search_x, search_y, search_x + search_w, search_y + search_h),
                            radiusX: 6.0 * scale,
                            radiusY: 6.0 * scale,
                        },
                        brush,
                        border_w,
                        None,
                    );

                    let label = if fence.search_text.is_empty() {
                        "🔍 Buscar…"
                    } else {
                        ""
                    };
                    if !label.is_empty() {
                        brush.SetColor(&with_alpha(theme.muted, 0.6));
                        let lbl = wide_str(label);
                        surface.target.DrawText(
                            &lbl,
                            &gfx.meta_format,
                            &rect(
                                search_x + 6.0 * scale,
                                search_y + 1.5 * scale,
                                search_x + search_w - 6.0 * scale,
                                search_y + search_h - 1.0 * scale,
                            ),
                            brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    } else if !fence.search_text.is_empty() {
                        brush.SetColor(&theme.text);
                        let stext = wide_str(&fence.search_text);
                        surface.target.DrawText(
                            &stext,
                            &gfx.text_format,
                            &rect(
                                search_x + 6.0 * scale,
                                search_y + 1.0 * scale,
                                search_x + search_w - 18.0 * scale,
                                search_y + search_h - 1.0 * scale,
                            ),
                            brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );

                        // Boton de borrar texto (x)
                        brush.SetColor(&with_alpha(theme.muted, 0.7));
                        let x_str = wide_str("×");
                        surface.target.DrawText(
                            &x_str,
                            &gfx.meta_format,
                            &rect(
                                search_x + search_w - 16.0 * scale,
                                search_y + 0.5 * scale,
                                search_x + search_w - 2.0 * scale,
                                search_y + search_h,
                            ),
                            brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
            }

            // Candado de anclaje: icono vectorial fino y elegante con efecto hover.
            {
                let lr = lock_rect(w, theme, theme.show_counter, item_cnt);
                let (rx, ry) = (lr.left, lr.top);
                let is_hovered = fence.hover_lock;
                let c = if fence.layout.locked {
                    fence.accent
                } else if is_hovered {
                    with_alpha(theme.text, 0.9)
                } else {
                    with_alpha(theme.muted, 0.40)
                };
                let s = 11.0;
                brush.SetColor(&c);
                
                let is_locked = fence.layout.locked;
                let shackle_left = (rx + 2.5) * scale;
                let shackle_right = if is_locked { (rx + s - 2.5) * scale } else { (rx + s - 1.0) * scale };
                let shackle_top = if is_locked { ry * scale } else { (ry - 2.0) * scale };
                let shackle_bottom = (ry + 5.0) * scale;
                let shackle = D2D1_ROUNDED_RECT {
                    rect: rect(shackle_left, shackle_top, shackle_right, shackle_bottom),
                    radiusX: 2.0 * scale,
                    radiusY: 2.0 * scale,
                };
                surface.target.DrawRoundedRectangle(&shackle, brush, 1.2 * scale, None);

                let body_rect = rect(rx * scale, (ry + 4.5) * scale, (rx + s) * scale, (ry + s) * scale);
                surface.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: body_rect,
                        radiusX: 1.5 * scale,
                        radiusY: 1.5 * scale,
                    },
                    brush,
                );

                brush.SetColor(&with_alpha(theme.background, 0.95));
                surface.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F {
                            x: (rx + s * 0.5) * scale,
                            y: (ry + 7.5) * scale,
                        },
                        radiusX: 0.9 * scale,
                        radiusY: 0.9 * scale,
                    },
                    brush,
                );
            }

            let mut icon_jobs: Vec<(i32, i32, HICON)> = Vec::new();

            if !fence.layout.collapsed {
                let content_rect = rect(0.0, theme.header * scale, w * scale, h * scale);
                surface
                    .target
                    .PushAxisAlignedClip(&content_rect, D2D1_ANTIALIAS_MODE_ALIASED);

                let rows = fence.rows_visible(theme);
                let icon_px = if scale > 1.25 { 20.0 } else { 16.0 };
                // Filtro de busqueda: si hay texto, solo mostrar items que lo contengan.
                let filtered: Vec<usize> = if fence.search_text.is_empty() {
                    (0..fence.content.items.len()).collect()
                } else {
                    let q = fence.search_text.to_lowercase();
                    fence.content.items.iter().enumerate()
                        .filter(|(_, item)| item.name.to_lowercase().contains(&q))
                        .map(|(i, _)| i)
                        .collect()
                };
                let first = fence.scroll.max(0) as usize;
                let _last = (first + rows.max(0) as usize + 1).min(fence.content.items.len());

                let grid_mode = self.cfg.appearance.grid_mode;
                let cell = (self.cfg.appearance.grid_item_size * scale).max(48.0);
                let grid_cols = ((w * scale - pad * 2.0) / cell).floor().max(1.0) as usize;

                if grid_mode && !fence.content.items.is_empty() {
                    fence.hit_row_h = cell / scale;
                    // Calcular scroll en modo cuadricula.
                    let grid_rows = (fence.content.items.len() + grid_cols - 1) / grid_cols;
                    let visible_rows = ((h * scale - theme.header * scale) / cell).floor().max(1.0) as usize;
                    let max_grid_scroll = grid_rows.saturating_sub(visible_rows);
                    fence.scroll = fence.scroll.min(max_grid_scroll as i32).max(0);
                    let scroll_off = fence.scroll as f32 * cell;
                    let first_row = fence.scroll as usize;
                    let last_row = (first_row + visible_rows + 1).min(grid_rows);
                    let cell_pad = 4.0 * scale;
                    let icon_size = (cell * 0.48).min(32.0 * scale).max(20.0 * scale);
                    for row in first_row..last_row {
                        for col in 0..grid_cols {
                            let idx = row * grid_cols + col;
                            if idx >= fence.content.items.len() { break; }
                            let item = &fence.content.items[idx];
                            let cx = pad * 0.5 + col as f32 * cell;
                            let cy_val = theme.header * scale + (row as f32 * cell) - scroll_off;
                            if cy_val + cell > h * scale { break; }
                            let is_sel = fence.selected.contains(&idx);
                            let is_hov = fence.hover == idx as i32;
                            if is_sel || is_hov {
                                let bg = if is_sel { with_alpha(fence.accent, 0.28) } else { theme.hover };
                                brush.SetColor(&bg);
                                surface.target.FillRoundedRectangle(
                                    &D2D1_ROUNDED_RECT {
                                        rect: rect(cx + 1.0, cy_val + 1.0, cx + cell - 1.0, cy_val + cell - 1.0),
                                        radiusX: 6.0 * scale, radiusY: 6.0 * scale,
                                    }, brush,
                                );
                                if is_sel {
                                    brush.SetColor(&with_alpha(fence.accent, 0.7));
                                    surface.target.DrawRoundedRectangle(
                                        &D2D1_ROUNDED_RECT {
                                            rect: rect(cx + 1.0, cy_val + 1.0, cx + cell - 1.0, cy_val + cell - 1.0),
                                            radiusX: 6.0 * scale, radiusY: 6.0 * scale,
                                        }, brush, 1.0 * scale, None,
                                    );
                                }
                            }
                            if theme.show_icons {
                                if let Some(icon) = (*icons).get(&item.path, &item.ext, item.is_dir, scale > 1.25) {
                                    let ix = cx + (cell - icon_size) / 2.0;
                                    icon_jobs.push((ix as i32, (cy_val + cell_pad) as i32, icon));
                                }
                            }
                            brush.SetColor(&theme.text);
                            let name = wide_str(&item.name);
                            surface.target.DrawText(
                                &name, &gfx.meta_format,
                                &rect(cx + cell_pad, cy_val + cell * 0.55, cx + cell - cell_pad, cy_val + cell - cell_pad),
                                brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                    }
                    // Scrollbar en modo cuadricula.
                    let _grid_rows = (fence.content.items.len() + grid_cols - 1) / grid_cols;
                    let _visible_rows = ((h * scale - theme.header * scale) / cell).floor().max(1.0) as usize;
                    let _max_grid = _grid_rows.saturating_sub(_visible_rows);
                    if _max_grid > 0 {
                        let track = (h - theme.header - theme.padding) * scale;
                        let thumb = (track * (_visible_rows as f32 / _grid_rows as f32)).max(18.0 * scale);
                        let progress = fence.scroll as f32 / _max_grid as f32;
                        let top = theme.header * scale + (track - thumb) * progress;
                        brush.SetColor(&theme.muted);
                        surface.target.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: rect((w - SCROLLBAR_W - 3.0) * scale, top, (w - 3.0) * scale, top + thumb),
                                radiusX: SCROLLBAR_W * scale * 0.5, radiusY: SCROLLBAR_W * scale * 0.5,
                            }, brush,
                        );
                    }
                } else if fence.content.items.is_empty() {
                    brush.SetColor(&theme.muted);
                    let empty = wide_str(self.tr.fence_empty);
                    surface.target.DrawText(
                        &empty,
                        &gfx.text_format,
                        &rect(
                            pad,
                            theme.header * scale,
                            (w - theme.padding) * scale,
                            (theme.header + theme.row) * scale,
                        ),
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                let total_filtered = filtered.len();
                if !grid_mode {
                fence.hit_row_h = theme.row;
                let visible_filtered = (rows as usize + 1).min(total_filtered.saturating_sub(first));
                for offset in 0..visible_filtered {
                    let mapped = filtered[first + offset];
                    if mapped >= fence.content.items.len() { break; }
                    let item = &fence.content.items[mapped];
                    let index_abs = mapped;
                    let top = (theme.header + offset as f32 * theme.row) * scale;
                    let bottom = top + theme.row * scale;
                    if top > h * scale {
                        break;
                    }

                    let is_sel = fence.selected.contains(&index_abs);
                    let is_hov = fence.hover == index_abs as i32;
                    if is_sel || is_hov {
                        let bg = if is_sel { with_alpha(fence.accent, 0.28) } else { theme.hover };
                        brush.SetColor(&bg);
                        surface.target.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: rect(
                                    pad * 0.5,
                                    top + 1.0,
                                    (w - theme.padding * 0.5) * scale,
                                    bottom - 1.0,
                                ),
                                radiusX: 6.0 * scale,
                                radiusY: 6.0 * scale,
                            },
                            brush,
                        );
                        if is_sel {
                            brush.SetColor(&with_alpha(fence.accent, 0.7));
                            surface.target.DrawRoundedRectangle(
                                &D2D1_ROUNDED_RECT {
                                    rect: rect(
                                        pad * 0.5,
                                        top + 1.0,
                                        (w - theme.padding * 0.5) * scale,
                                        bottom - 1.0,
                                    ),
                                    radiusX: 6.0 * scale,
                                    radiusY: 6.0 * scale,
                                },
                                brush,
                                1.0 * scale,
                                None,
                            );
                        }
                    }

                    let mut text_left = pad;
                    if theme.show_icons {
                        if let Some(icon) = (*icons).get(
                            &item.path,
                            &item.ext,
                            item.is_dir,
                            scale > 1.25,
                        ) {
                            let iy = top + (theme.row * scale - icon_px * scale) * 0.5;
                            icon_jobs.push((pad as i32, iy as i32, icon));
                        }
                        text_left += (icon_px + 6.0) * scale;
                    }

                    let name = wide_str(&item.name);
                    brush.SetColor(&theme.text);
                    surface.target.DrawText(
                        &name,
                        &gfx.text_format,
                        &rect(
                            text_left,
                            top,
                            (w - theme.padding - 48.0) * scale,
                            bottom,
                        ),
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    let meta = wide_str(&item.size_label());
                    brush.SetColor(&theme.muted);
                    surface.target.DrawText(
                        &meta,
                        &gfx.meta_format,
                        &rect(
                            (w - theme.padding - 46.0) * scale,
                            top,
                            (w - theme.padding) * scale,
                            bottom,
                        ),
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                // Indicador de reordenacion: linea de acento en la posicion de insercion.
                if let Some(drop) = fence.reorder_drop {
                    let effective = if drop > fence.content.items.len() { fence.content.items.len() } else { drop };
                    let y_pos = (theme.header + (effective as f32 - fence.scroll as f32) * theme.row) * scale;
                    let y_pos = y_pos.clamp(theme.header * scale, (h - 1.0) * scale);
                    brush.SetColor(&fence.accent);
                    surface.target.DrawLine(
                        D2D_POINT_2F { x: pad, y: y_pos },
                        D2D_POINT_2F { x: (w - theme.padding) * scale, y: y_pos },
                        brush,
                        2.5 * scale,
                        None,
                    );
                }

                // Indicador de desplazamiento.
                // Indicador de desplazamiento (usa el numero de items filtrados).
                let max_scroll = if fence.search_text.is_empty() {
                    fence.max_scroll(theme)
                } else {
                    (total_filtered as i32 - rows).max(0)
                };
                if max_scroll > 0 {
                    let total = fence.content.items.len() as f32;
                    let visible = rows as f32;
                    let track = (h - theme.header - theme.padding) * scale;
                    let thumb = (track * (visible / total)).max(18.0 * scale);
                    let progress = fence.scroll as f32 / max_scroll as f32;
                    let top = theme.header * scale + (track - thumb) * progress;
                    brush.SetColor(&theme.muted);
                    surface.target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(
                                (w - SCROLLBAR_W - 3.0) * scale,
                                top,
                                (w - 3.0) * scale,
                                top + thumb,
                            ),
                            radiusX: SCROLLBAR_W * scale * 0.5,
                            radiusY: SCROLLBAR_W * scale * 0.5,
                        },
                        brush,
                    );
                }

                } // end if !grid_mode

                if let Some((x1, y1, x2, y2)) = fence.rubberband {
                    let rx1 = x1.min(x2) * scale;
                    let ry1 = y1.min(y2) * scale;
                    let rx2 = x1.max(x2) * scale;
                    let ry2 = y1.max(y2) * scale;
                    let rrect = rect(rx1, ry1, rx2, ry2);
                    brush.SetColor(&with_alpha(fence.accent, 0.25));
                    surface.target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT { rect: rrect, radiusX: 3.0 * scale, radiusY: 3.0 * scale },
                        brush,
                    );
                    brush.SetColor(&with_alpha(fence.accent, 0.8));
                    surface.target.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT { rect: rrect, radiusX: 3.0 * scale, radiusY: 3.0 * scale },
                        brush, 1.0 * scale, None,
                    );
                }

                surface.target.PopAxisAlignedClip();
            }

            // Indicador del grip de redimensionado: tres diagonales tenues en
            // la esquina inferior derecha (solo en cajas desplegadas).
            if !fence.layout.collapsed && !fence.layout.locked {
                let gx = (w - theme.padding - 3.0) * scale;
                let gy = (h - theme.padding - 3.0) * scale;
                brush.SetColor(&with_alpha(theme.muted, 0.35));
                for i in 0..3 {
                    let o = i as f32 * 3.5 * scale;
                    surface.target.DrawLine(
                        D2D_POINT_2F {
                            x: gx - 12.0 * scale + o,
                            y: gy,
                        },
                        D2D_POINT_2F {
                            x: gx,
                            y: gy - 12.0 * scale + o,
                        },
                        brush,
                        1.0 * scale,
                        None,
                    );
                }
            }

            surface.target.EndDraw(None, None)?;

            // Los iconos del shell son HICON de GDI: se estampan sobre la misma
            // DIB despues de que Direct2D haya volcado su contenido.
            let icon_px = if scale > 1.25 { 20 } else { 16 };
            for (x, y, icon) in icon_jobs {
                let _ = DrawIconEx(surface.dc, x, y, icon, icon_px, icon_px, 0, None, DI_NORMAL);
            }

            // Publicacion atomica del frame con alfa por pixel.
            let screen = GetDC(None);
            let position = POINT {
                x: fence.layout.x,
                y: fence.layout.y,
            };
            let size = SIZE {
                cx: width,
                cy: height,
            };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let result = UpdateLayeredWindow(
                fence.hwnd,
                screen,
                Some(&position as *const POINT),
                Some(&size as *const SIZE),
                surface.dc,
                Some(&source as *const POINT),
                COLORREF(0),
                Some(&blend as *const BLENDFUNCTION),
                ULW_ALPHA,
            );
            let _ = ReleaseDC(None, screen);
            result?;
        }
        Ok(())
    }

    // -- acciones -----------------------------------------------------------

    /// Muestra u oculta la miniatura emergente junto al cursor.
    unsafe fn show_thumb(&mut self, path: &Path) {
        if !self.cfg.appearance.show_thumbnails {
            self.hide_thumb();
            return;
        }
        // Asegurar que la entrada este en la cache (solo carga si no existe).
        static mut CACHE: Option<ThumbCache> = None;
        // SAFETY: CACHE solo se accede desde el hilo de UI (single-threaded).
        let cache = unsafe {
            let ptr: *mut Option<ThumbCache> = &raw mut CACHE;
            if (*ptr).is_none() {
                *ptr = Some(ThumbCache::new());
            }
            (*ptr).as_mut().unwrap()
        };
        if cache.get(path).is_none() {
            if let Some(entry) = load_thumbnail(path) {
                cache.insert(path.to_path_buf(), entry);
            } else {
                self.hide_thumb();
                return;
            }
        }
        // Clonar los pixeles SOLO cuando vamos a pintar (no en cada WM_MOUSEMOVE).
        let entry = cache.get(path).unwrap();
        let pixels = entry.pixels.clone();
        let (tw, th) = (entry.w, entry.h);
        // Posicionar junto al cursor (offset 12px a la derecha).
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let pad: i32 = 8;
        let w = tw + pad * 2;
        let h = th + pad * 2;
        let x = cursor.x + 12;
        // La DIB debe tener el mismo tamano que la ventana (UpdateLayeredWindow
        // hace 1:1, no estira). Los pixeles de la miniatura se copian con un
        // margen de `pad` pixeles; el resto queda en negro transparente.
        let need_new = match &self.thumb_surface {
            Some(s) => s.width != w || s.height != h,
            None => true,
        };
        if need_new {
            self.thumb_surface = SurfaceDib::new(w, h);
        }
        let surface = match &self.thumb_surface {
            Some(s) => s,
            None => return,
        };
fn color_to_bgra_u32(c: D2D1_COLOR_F, alpha_override: Option<f32>) -> u32 {
    let a = alpha_override.unwrap_or(c.a).clamp(0.0, 1.0);
    let r = (c.r * a * 255.0).clamp(0.0, 255.0) as u32;
    let g = (c.g * a * 255.0).clamp(0.0, 255.0) as u32;
    let b = (c.b * a * 255.0).clamp(0.0, 255.0) as u32;
    let a_byte = (a * 255.0).clamp(0.0, 255.0) as u32;
    (a_byte << 24) | (r << 16) | (g << 8) | b
}

        // Pintar la tarjeta de fondo (redondeada con el tema activo) y la miniatura encima.
        if !surface.bits.is_null() {
            let bits = unsafe { std::slice::from_raw_parts_mut(surface.bits, (w * h) as usize) };
            bits.fill(0);
            let card_margin = pad / 2;
            let radius: i32 = 12;
            let bg_u32 = color_to_bgra_u32(self.theme.background, Some(0.95));
            let border_u32 = color_to_bgra_u32(self.theme.border, Some(0.98));

            let card_left = card_margin;
            let card_top = card_margin;
            let card_right = w - card_margin;
            let card_bottom = h - card_margin;
            for row in card_top..card_bottom {
                for col in card_left..card_right {
                    let idx = (row * w + col) as usize;
                    let is_border = row == card_top || row == card_bottom - 1 || col == card_left || col == card_right - 1;
                    let mut inside = true;
                    if row < card_top + radius && col < card_left + radius {
                        let dx = (card_left + radius - col) as f32;
                        let dy = (card_top + radius - row) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if row < card_top + radius && col >= card_right - radius {
                        let dx = (col - (card_right - radius)) as f32;
                        let dy = (card_top + radius - row) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if row >= card_bottom - radius && col < card_left + radius {
                        let dx = (card_left + radius - col) as f32;
                        let dy = (row - (card_bottom - radius)) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if row >= card_bottom - radius && col >= card_right - radius {
                        let dx = (col - (card_right - radius)) as f32;
                        let dy = (row - (card_bottom - radius)) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if inside {
                        bits[idx] = if is_border { border_u32 } else { bg_u32 };
                    }
                }
            }
            // Copiar la miniatura encima con offset (pad, pad).
            let src_u32 = unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u32, (tw * th) as usize) };
            for row in 0..th {
                let src_row = row as usize * tw as usize;
                let dst_row = (row + pad) as usize * w as usize + pad as usize;
                bits[dst_row..dst_row + tw as usize].copy_from_slice(&src_u32[src_row..src_row + tw as usize]);
            }
        }
        let y = cursor.y - h / 2;
        let screen = GetDC(None);
        let position = POINT { x, y };
        let size = SIZE { cx: w, cy: h };
        let source = POINT { x: 0, y: 0 };
        self.thumb_pos = position;
        self.thumb_sz = size;
        // Iniciar fade-in: empezar con alfa bajo y un timer a ~60 fps.
        self.thumb_alpha = 32;
        self.thumb_fading_out = false;
        let _ = SetWindowPos(
            self.thumb_hwnd,
            HWND_TOPMOST,
            x, y, w, h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: self.thumb_alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            self.thumb_hwnd,
            screen,
            Some(&position),
            Some(&size),
            surface.dc,
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        let _ = ReleaseDC(None, screen);
        let _ = SetTimer(self.controller, TIMER_THUMB_FADE, 16, None);
    }

    unsafe fn show_toast(&mut self, message: &str, icon_color: COLORREF) {
        if self.toast_hwnd.is_invalid() {
            return;
        }
        // --- Medir texto con GDI para calcular el tamano real ---
        let font_name: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        let font = CreateFontW(
            16, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
            1, 0, 0, 4, 0,
            PCWSTR(font_name.as_ptr()),
        );
        let measure_dc = CreateCompatibleDC(None);
        let old_font_m = SelectObject(measure_dc, HGDIOBJ(font.0));
        let wide_msg: Vec<u16> = message.encode_utf16().collect();
        let mut text_sz = SIZE::default();
        let _ = GetTextExtentPoint32W(measure_dc, &wide_msg, &mut text_sz);
        SelectObject(measure_dc, old_font_m);
        let _ = DeleteDC(measure_dc);
        let _ = DeleteObject(HGDIOBJ(font.0));

        let icon_w: i32 = 16;
        let pad_x: i32 = 28 + icon_w; // espacio para icono + padding
        let pad_y: i32 = 14;
        let tw = (text_sz.cx + pad_x).clamp(170, 600);
        let th = (text_sz.cy + pad_y).max(36);
        // --- Crear superficie al tamano medido ---
        let surface = match SurfaceDib::new(tw, th) {
            Some(s) => s,
            None => return,
        };
        if !surface.bits.is_null() {
            let bits = std::slice::from_raw_parts_mut(surface.bits, (tw * th) as usize);
            bits.fill(0);
            let bg: u32 = 0xF02B1A13;
            let radius: i32 = 10;
            for row in 0..th {
                for col in 0..tw {
                    let mut inside = true;
                    if row < radius && col < radius {
                        let dx = (radius - col) as f32; let dy = (radius - row) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if row < radius && col >= tw - radius {
                        let dx = (col - (tw - radius)) as f32; let dy = (radius - row) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if row >= th - radius && col < radius {
                        let dx = (radius - col) as f32; let dy = (row - (th - radius)) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if row >= th - radius && col >= tw - radius {
                        let dx = (col - (tw - radius)) as f32; let dy = (row - (th - radius)) as f32;
                        inside = dx * dx + dy * dy <= (radius * radius) as f32;
                    }
                    if inside { bits[(row * tw + col) as usize] = bg; }
                }
            }
            // Icono: circulo verde con checkmark blanco.
            let icon_cx = 16 + icon_w / 2;
            let icon_cy = th / 2;
            let icon_r = icon_w / 2;
            let green = CreateSolidBrush(icon_color);
            let old_brush = SelectObject(surface.dc, HGDIOBJ(green.0));
            let null_pen = GetStockObject(NULL_PEN);
            let old_pen = SelectObject(surface.dc, null_pen);
            let _ = Ellipse(surface.dc, icon_cx - icon_r, icon_cy - icon_r, icon_cx + icon_r, icon_cy + icon_r);
            SelectObject(surface.dc, old_pen);
            SelectObject(surface.dc, old_brush);
            let _ = DeleteObject(HGDIOBJ(green.0));
            // Checkmark ✓ centrado en el circulo.
            let check_font = CreateFontW(
                14, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0,
                1, 0, 0, 4, 0,
                PCWSTR(font_name.as_ptr()),
            );
            if !check_font.is_invalid() {
                let old_font = SelectObject(surface.dc, HGDIOBJ(check_font.0));
                let _ = SetBkMode(surface.dc, TRANSPARENT);
                let _ = SetTextColor(surface.dc, COLORREF(0x00FFFFFF));
                let mut checkmark: Vec<u16> = "\u{2713}".encode_utf16().collect();
                let mut crc = RECT { left: icon_cx - icon_r, top: icon_cy - icon_r, right: icon_cx + icon_r, bottom: icon_cy + icon_r };
                let _ = DrawTextW(surface.dc, &mut checkmark, &mut crc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(surface.dc, old_font);
                let _ = DeleteObject(HGDIOBJ(check_font.0));
            }
            // Dibujar texto principal.
            let font2 = CreateFontW(
                16, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
                1, 0, 0, 4, 0,
                PCWSTR(font_name.as_ptr()),
            );
            if !font2.is_invalid() {
                let old_font = SelectObject(surface.dc, HGDIOBJ(font2.0));
                let _ = SetBkMode(surface.dc, TRANSPARENT);
                let _ = SetTextColor(surface.dc, COLORREF(0x00FFFFFF));
                let mut text: Vec<u16> = message.encode_utf16().collect();
                let text_left = pad_x / 2 + icon_w + 4;
                let mut rc = RECT { left: text_left, top: 0, right: tw - 8, bottom: th };
                let _ = DrawTextW(surface.dc, &mut text, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(surface.dc, old_font);
                let _ = DeleteObject(HGDIOBJ(font2.0));
            }
        }

        self.toast_surface = Some(surface);
        let work = work_area();
        let x = work.left + ((work.right - work.left) - tw) / 2;
        let y = work.bottom - th - 40;
        let screen = GetDC(None);
        let position = POINT { x, y };
        let size = SIZE { cx: tw, cy: th };
        let source = POINT { x: 0, y: 0 };
        self.toast_alpha = 32;
        self.toast_fading_out = false;
        self.toast_hold = 188;
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: self.toast_alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let surface = self.toast_surface.as_ref().unwrap();
        let _ = SetWindowPos(self.toast_hwnd, HWND_TOPMOST, x, y, tw, th, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = UpdateLayeredWindow(self.toast_hwnd, screen, Some(&position), Some(&size), surface.dc, Some(&source), COLORREF(0), Some(&blend), ULW_ALPHA);
        let _ = ReleaseDC(None, screen);
        let _ = SetTimer(self.controller, TIMER_TOAST, 16, None);
    }


    unsafe fn toast_tick(&mut self) {
        if self.toast_hwnd.is_invalid() || self.toast_surface.is_none() {
            let _ = KillTimer(self.controller, TIMER_TOAST);
            return;
        }
        let surface = self.toast_surface.as_ref().unwrap();
        if self.toast_fading_out {
            if self.toast_alpha <= 32 {
                self.toast_alpha = 0;
                let _ = ShowWindow(self.toast_hwnd, SW_HIDE);
                let _ = KillTimer(self.controller, TIMER_TOAST);
                self.toast_fading_out = false;
                self.toast_hold = 0;
                return;
            }
            self.toast_alpha = self.toast_alpha.saturating_sub(32);
        } else {
            if self.toast_alpha < 255 - 32 {
                self.toast_alpha = (self.toast_alpha + 32).min(255);
            } else if self.toast_alpha < 255 {
                self.toast_alpha = 255;
            } else {
                if self.toast_hold > 0 { self.toast_hold -= 1; return; }
                self.toast_fading_out = true;
                return;
            }
        }
        let screen = GetDC(None);
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: self.toast_alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = UpdateLayeredWindow(self.toast_hwnd, screen, None, None, surface.dc, Some(&source), COLORREF(0), Some(&blend), ULW_ALPHA);
        let _ = ReleaseDC(None, screen);
    }

    fn start_collapse_anim(&mut self, index: usize) {
        if index >= self.fences.len() { return; }
        let fence = &mut self.fences[index];
        fence.layout.collapsed = !fence.layout.collapsed;
        self.anim_fence = index;
        self.anim_kind = if fence.layout.collapsed { 1 } else { 2 };
        self.anim_step = ANIM_STEPS;
        unsafe { let _ = SetTimer(self.controller, TIMER_ANIM, 10, None); }
    }

    fn anim_tick(&mut self) {
        if self.anim_step == 0 {
            unsafe { let _ = KillTimer(self.controller, TIMER_ANIM); }
            if self.anim_kind == 3 {
                unsafe {
                    for fence in &self.fences {
                        let _ = ShowWindow(fence.hwnd, SW_HIDE);
                    }
                    if self.cfg.general.zen_hides_desktop_icons {
                        if let Some(lv) = desktop_listview() {
                            let _ = ShowWindow(lv, SW_HIDE);
                        }
                    }
                }
            }
            self.anim_kind = 0;
            return;
        }
        self.anim_step -= 1;
        let progress = (ANIM_STEPS - self.anim_step) as f32 / ANIM_STEPS as f32;
        match self.anim_kind {
            1 | 2 => {
                let idx = self.anim_fence;
                if idx >= self.fences.len() { self.anim_step = 0; return; }
                let fence = &self.fences[idx];
                let hwnd = fence.hwnd;
                let full_h = fence.layout.height;
                let collapsed_h = ((fence.scale * (self.theme.header + self.theme.padding)) as i32).max(28);
                let (from, to) = if self.anim_kind == 1 { (full_h, collapsed_h) } else { (collapsed_h, full_h) };
                let cur = from + ((to - from) as f32 * progress) as i32;
                let _ = unsafe { SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, fence.layout.width, cur, SWP_NOMOVE | SWP_NOACTIVATE) };
                let _ = self.render(idx);
                if self.anim_step == 0 {
                    self.persist_layout();
                }
            }
            3 => {
                self.anim_zen_alpha = ((1.0 - progress) * 255.0) as u8;
                self.render_all();
            }
            4 => {
                self.anim_zen_alpha = (progress * 255.0) as u8;
                self.render_all();
            }
            _ => { self.anim_step = 0; }
        }
    }

    /// Muestra el icono de arrastre (sin fade, para drag manual).
    unsafe fn show_drag_image(&mut self, path: &Path) {
        // Cargar icono del archivo (32x32).
        let mut info = SHFILEINFOW::default();
        let flags = SHGFI_ICON | SHGFI_LARGEICON;
        if SHGetFileInfoW(PCWSTR(wide(&path.to_string_lossy()).as_ptr()), windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut info), std::mem::size_of::<SHFILEINFOW>() as u32, flags) == 0 {
            return;
        }
        if info.hIcon.is_invalid() { return; }
        // Crear DIB 32x32 y pintar el icono.
        let tw: i32 = 32; let th: i32 = 32;
        let surface = match SurfaceDib::new(tw, th) { Some(s) => s, None => return };
        if !surface.bits.is_null() {
            let bits = std::slice::from_raw_parts_mut(surface.bits, (tw * th) as usize);
            bits.fill(0);
            // Dibujar icono sobre el DC.
            let _ = DrawIconEx(surface.dc, 0, 0, info.hIcon, tw, th, 0, None, DI_NORMAL);
            let _ = DestroyIcon(info.hIcon);
        }
        self.drag_image_surface = Some(surface);
        // Posicionar en el cursor.
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let x = cursor.x + 12;
        let y = cursor.y + 12;
        let screen = GetDC(None);
        let position = POINT { x, y };
        let size = SIZE { cx: tw, cy: th };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 200, AlphaFormat: AC_SRC_ALPHA as u8 };
        let s = self.drag_image_surface.as_ref().unwrap();
        let _ = SetWindowPos(self.thumb_hwnd, HWND_TOPMOST, x, y, tw, th, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = UpdateLayeredWindow(self.thumb_hwnd, screen, Some(&position), Some(&size), s.dc, Some(&source), COLORREF(0), Some(&blend), ULW_ALPHA);
        let _ = ReleaseDC(None, screen);
    }

    unsafe fn update_drag_image_position(&mut self) {
        if self.drag_image_surface.is_none() { return; }
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let x = cursor.x + 12;
        let y = cursor.y + 12;
        let screen = GetDC(None);
        let position = POINT { x, y };
        let size = SIZE { cx: 32, cy: 32 };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 200, AlphaFormat: AC_SRC_ALPHA as u8 };
        let s = self.drag_image_surface.as_ref().unwrap();
        let _ = UpdateLayeredWindow(self.thumb_hwnd, screen, Some(&position), Some(&size), s.dc, Some(&source), COLORREF(0), Some(&blend), ULW_ALPHA);
        let _ = ReleaseDC(None, screen);
    }

    unsafe fn hide_drag_image(&mut self) {
        self.drag_image_surface = None;
        let _ = ShowWindow(self.thumb_hwnd, SW_HIDE);
    }

    unsafe fn hide_thumb(&mut self) {
        if !self.thumb_hwnd.is_invalid() {
            // Si ya esta oculta o en fade-out, no hacer nada.
            if !IsWindowVisible(self.thumb_hwnd).as_bool() {
                return;
            }
            // Iniciar fade-out: decrementar alfa hasta 0 y luego ocultar.
            self.thumb_fading_out = true;
            let _ = SetTimer(self.controller, TIMER_THUMB_FADE, 16, None);
        }
    }

    /// Tick del fade: incrementa (fade-in) o decrementa (fade-out) el alfa
    /// y re-publica la ventana de miniatura con UpdateLayeredWindow.
    unsafe fn thumb_fade_tick(&mut self) {
        if self.thumb_hwnd.is_invalid() || self.thumb_surface.is_none() {
            let _ = KillTimer(self.controller, TIMER_THUMB_FADE);
            return;
        }
        let surface = self.thumb_surface.as_ref().unwrap();
        if self.thumb_fading_out {
            if self.thumb_alpha <= 32 {
                self.thumb_alpha = 0;
                let _ = ShowWindow(self.thumb_hwnd, SW_HIDE);
                let _ = KillTimer(self.controller, TIMER_THUMB_FADE);
                self.thumb_fading_out = false;
                return;
            }
            self.thumb_alpha = self.thumb_alpha.saturating_sub(32);
        } else {
            if self.thumb_alpha >= 255 - 32 {
                self.thumb_alpha = 255;
                let _ = KillTimer(self.controller, TIMER_THUMB_FADE);
                // Ya esta completamente visible: no seguimos gastando timer.
                return;
            }
            self.thumb_alpha = (self.thumb_alpha + 32).min(255);
        }
        let screen = GetDC(None);
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: self.thumb_alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            self.thumb_hwnd,
            screen,
            Some(&self.thumb_pos),
            Some(&self.thumb_sz),
            surface.dc,
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        let _ = ReleaseDC(None, screen);
    }

    fn organize_now(&mut self) {
        let report = rules::organize(&self.cfg, &self.desktop);
        let sweep = rules::sweep_ephemeral(&self.cfg, &self.desktop);
        let total = report.organized + sweep.archived;
        if total > 0 {
            let msg = format!("{} {}", total, self.tr.toast_organized);
            unsafe { self.show_toast(&msg, TOAST_ORGANIZE); }
        }
        self.refresh_contents();
    }

    fn sweep_now(&mut self) {
        // No barrer durante un DoDragDrop: el bombeo de mensajes
        // puede disparar WM_TIMER y build_fences destruiria cajas
        // mientras el drag esta activo.
        if self.dragging { return; }
        let report = rules::sweep_ephemeral(&self.cfg, &self.desktop);
        if report.archived > 0 {
            let msg = format!("{} {}", report.archived, self.tr.toast_archived);
            unsafe { self.show_toast(&msg, TOAST_ORGANIZE); }
        }
        self.refresh_contents();
    }

    fn toggle_zen(&mut self) {
        self.zen = !self.zen;
        unsafe {
            for fence in &self.fences {
                let _ = ShowWindow(fence.hwnd, if self.zen { SW_HIDE } else { SW_SHOWNA });
            }
            if self.cfg.general.zen_hides_desktop_icons {
                if let Some(listview) = desktop_listview() {
                    let _ = ShowWindow(listview, if self.zen { SW_HIDE } else { SW_SHOW });
                }
            }
        }
        if !self.zen {
            self.render_all();
        }
    }

    fn reload_config(&mut self) {
        if let Ok(cfg) = Config::reload(&self.cfg_path) {
            self.apply_config(cfg);
        }
    }

    /// Aplica una configuracion en caliente: tema, recursos graficos, arbol de
    /// carpetas, reconstruccion de las cajas y sincronizacion del vigilante
    /// con las carpetas fisicas (nuevas reglas con `move_files` se vigilan ya,
    /// sin esperar a reiniciar).
    pub(crate) fn apply_config(&mut self, cfg: Config) {
        let old_paths = self.watch_paths();
        let new_paths = watch_paths_for(&cfg, &self.desktop, &self.extra_desktops);

        self.cfg = cfg;
        self.theme = Theme::from_config(&self.cfg);
        self.tr = Tr::get(self.cfg.lang());
        if let Ok(gfx) = Graphics::new(&self.cfg) {
            self.gfx = gfx;
        }
        let _ = rules::ensure_layout(&self.cfg, &self.desktop);
        unsafe {
            let _ = self.build_fences();
        }

        if let Some(watcher) = self.watcher.as_mut() {
            for path in &new_paths {
                if !old_paths.iter().any(|p| p == path) {
                    let _ = watcher.watch_extra(path);
                }
            }
            for path in &old_paths {
                if !new_paths.iter().any(|p| p == path) {
                    watcher.unwatch(path);
                }
            }
        }
    }

    /// Ventana de configuracion. Si el usuario acepta, se guarda el TOML y se
    /// aplica en caliente, incluyendo una pasada de organizacion para que los
    /// cambios (reglas, carpetas...) surtan efecto de inmediato.
    fn open_settings(&mut self) {
        // Reentrada (clic en la bandeja mientras el dialogo esta abierto).
        if self.settings_open {
            return;
        }
        self.settings_open = true;
        // El dialogo recibe el puntero a la app para que "Aplicar" guarde y
        // aplique en caliente sin cerrar la ventana (mismo hilo, dialogo
        // modal, asi que el puntero es seguro mientras dura la llamada).
        let app_ptr = self as *mut App;
        let chosen = settings::open_dialog(&self.cfg, app_ptr);
        self.settings_open = false;
        let Some(mut cfg) = chosen else {
            return; // cancelado
        };
        // Fusiona las preferencias de orden del dialogo con la geometria actual.
        for df in &cfg.fences {
            if let Some(lf) = self.cfg.fences.iter_mut().find(|f| f.id.as_str() == df.id.as_str()) {
                lf.sort_by = df.sort_by.clone();
            }
        }
        // Conserva la geometria actual de las cajas, que el dialogo no toca.
        cfg.fences = self.cfg.fences.clone();
        self.apply_dialog_cfg(cfg);
    }

    /// Aplica una configuracion ya validada por el dialogo (autostart, tema,
    /// cajas, vigilante, guardado en disco y una pasada de organizacion). La
    /// usan tanto "Guardar" (cierra el dialogo) como "Aplicar" (sin cerrar).
    pub(crate) fn apply_dialog_cfg(&mut self, cfg: Config) {
        if cfg.general.start_with_windows != self.cfg.general.start_with_windows {
            let _ = crate::config::apply_autostart(cfg.general.start_with_windows);
        }
        self.apply_config(cfg);
        let _ = self.cfg.save(&self.cfg_path);
        self.organize_now();
    }

    fn persist_layout(&mut self) {
        for fence in &self.fences {
            self.cfg.set_layout(fence.layout.clone());
        }
        let _ = self.cfg.save(&self.cfg_path);
    }

    fn open_item(&self, path: &Path) {
        let target = wide(&path.to_string_lossy());
        unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            );
        }
    }

    fn delete_to_recycle_bin(&self, owner: HWND, paths: &[PathBuf]) {
        let mut double_null_utf16: Vec<u16> = Vec::new();
        for p in paths {
            double_null_utf16.extend(p.to_string_lossy().encode_utf16());
            double_null_utf16.push(0);
        }
        double_null_utf16.push(0);

        let mut op = SHFILEOPSTRUCTW {
            hwnd: owner,
            wFunc: FO_DELETE,
            pFrom: PCWSTR(double_null_utf16.as_ptr()),
            pTo: PCWSTR(std::ptr::null()),
            fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0) as u16,
            ..Default::default()
        };
        unsafe {
            let _ = SHFileOperationW(&mut op);
        }
    }

    // -- bandeja y hooks ----------------------------------------------------

    unsafe fn install_tray(&mut self) {
        // Icono embebido en el .exe (recurso 2, variante de bandeja): el shell
        // lo escala desde el .ico multiresolucion. Cae al icono generico solo
        // si el recurso no esta disponible (binario compilado sin build.rs).
        // El recurso vive en el modulo del .exe. `LoadImageW` exige
        // `Param<HINSTANCE>`: en windows-core 0.58 solo `Option<&T>` lo
        // satisface para handles, asi que se pasa por referencia.
        let module: HINSTANCE = GetModuleHandleW(None)
            .map(|h| h.into())
            .unwrap_or_default();
        // LoadImageW devuelve un icono propio (destruible con DestroyIcon);
        // LoadIconW devolveria un icono compartido del sistema. En el fallback
        // se usa tambien LoadImageW para mantener la propiedad del handle.
        let load = |module: Option<&HINSTANCE>, name: PCWSTR| {
            LoadImageW(module, name, IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR)
                .map(|handle| HICON(handle.0))
        };
        let hicon = load(Some(&module), PCWSTR(TRAY_ICON_RES as *const u16))
            .or_else(|_| load(None, IDI_APPLICATION))
            .unwrap_or_default();
        self.tray_icon = hicon;

        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.controller,
            uID: TRAY_UID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_ZEN_TRAY,
            hIcon: hicon,
            ..Default::default()
        };
        let tip = wide(self.tr.tray_tip);
        let len = tip.len().min(data.szTip.len());
        data.szTip[..len].copy_from_slice(&tip[..len]);
        self.tray = Shell_NotifyIconW(NIM_ADD, &data).as_bool();
    }

    unsafe fn install_hooks(&mut self) {
        if self.cfg.general.zen_double_click {
            HOOK_STATE.with(|state| {
                let mut s = state.get();
                s.controller = Some(self.controller);
                state.set(s);
            });
            if let Ok(hook) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) {
                self.hook = hook;
            }
        }
        if self.cfg.general.zen_hotkey {
            let _ = RegisterHotKey(
                self.controller,
                HOTKEY_ID,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_Z.0 as u32,
            );
        }
    }

        // -- menu contextual ----------------------------------------------------

    /// Menu contextual nativo del shell (Explorer) para un elemento de una
    /// caja: Abrir, Abrir con, Copiar, Cortar, Eliminar, Propiedades... igual
    /// que si se pulsara el boton derecho sobre el icono en el escritorio.
    unsafe fn show_shell_menu_for_paths(&mut self, owner: HWND, paths: &[PathBuf]) {
        let (menu, hmenu, pidl) = match build_shell_menu_for_paths(paths) {
            Ok(triple) => triple,
            Err(_) => return,
        };
        let first = 0x8000u32;

        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let _ = SetForegroundWindow(owner);
        self.shell_menu = Some(menu.clone());
        let choice = TrackPopupMenu(
            hmenu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            cursor.x,
            cursor.y,
            0,
            owner,
            None,
        );
        self.shell_menu = None;
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);

        if choice.0 != 0 && choice.0 as u32 >= first {
            let verb = (choice.0 as u32 - first) as u16 as usize as *const u8;
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                fMask: 0,
                hwnd: owner,
                lpVerb: PCSTR(verb),
                lpParameters: PCSTR(std::ptr::null()),
                lpDirectory: PCSTR(std::ptr::null()),
                nShow: SW_SHOWNORMAL.0 as i32,
                dwHotKey: 0,
                hIcon: HANDLE::default(),
            };
            let _ = menu.InvokeCommand(&info);
        }
        let _ = CoTaskMemFree(Some(pidl as *const c_void));
    }

    unsafe fn show_menu(&mut self, owner: HWND, fence_index: Option<usize>) {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        append_menu(menu, CMD_ZEN, if self.zen { self.tr.menu_exit_zen } else { self.tr.menu_zen });
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        append_menu(menu, CMD_ORGANIZE, self.tr.menu_organize);
        append_menu(menu, CMD_SWEEP, self.tr.menu_sweep);
        append_menu(menu, CMD_REFRESH, self.tr.menu_refresh);
        if let Some(index) = fence_index {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            if self.fences[index].content.folder.is_some() {
                append_menu(menu, CMD_OPEN_FOLDER, self.tr.menu_open_folder);
            }
            let label = if self.fences[index].layout.collapsed {
                self.tr.menu_expand
            } else {
                self.tr.menu_collapse
            };
            append_menu(menu, CMD_COLLAPSE, label);
            let lock_label = if self.fences[index].layout.locked {
                self.tr.menu_unlock
            } else {
                self.tr.menu_lock
            };
            append_menu(menu, CMD_LOCK, lock_label);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let current_sort = self.fences[fence_index.unwrap()].sort_mode.as_deref().unwrap_or(&self.cfg.appearance.sort_by);
            append_menu(menu, CMD_SORT_NAME, &format!("    {}", if current_sort == "name" { "✓ A-Z" } else { "  A-Z" }));
            append_menu(menu, CMD_SORT_SIZE, &format!("    {}", if current_sort == "size" { "✓ Tamano" } else { "  Tamano" }));
            append_menu(menu, CMD_SORT_TYPE, &format!("    {}", if current_sort == "type" { "✓ Tipo" } else { "  Tipo" }));
            append_menu(menu, CMD_SORT_MODIFIED, &format!("    {}", if current_sort == "modified" { "✓ Fecha" } else { "  Fecha" }));
            append_menu(menu, CMD_SORT_CUSTOM, &format!("    {}", if current_sort == "custom" { "✓ Manual" } else { "  Manual" }));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        append_menu(menu, CMD_SETTINGS, self.tr.menu_settings);
        append_menu(menu, CMD_EDIT_CONFIG, self.tr.menu_edit_config);
        append_menu(menu, CMD_RELOAD, self.tr.menu_reload);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        append_menu(menu, CMD_EXIT, self.tr.menu_exit);

        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // Requisito documentado para que el menu se cierre al perder el foco.
        let _ = SetForegroundWindow(owner);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            cursor.x,
            cursor.y,
            0,
            owner,
            None,
        );
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);

        match choice.0 as usize {
            CMD_ZEN => self.toggle_zen(),
            CMD_ORGANIZE => self.organize_now(),
            CMD_SWEEP => self.sweep_now(),
            CMD_REFRESH => self.refresh_contents(),
            CMD_OPEN_FOLDER => {
                if let Some(index) = fence_index {
                    if let Some(folder) = self.fences[index].content.folder.clone() {
                        self.open_item(&folder);
                    }
                }
            }
            CMD_COLLAPSE => {
                if let Some(index) = fence_index {
                    let collapsed = !self.fences[index].layout.collapsed;
                    self.fences[index].layout.collapsed = collapsed;
                    let height = self.fences[index].visible_height(&self.theme);
                    let hwnd = self.fences[index].hwnd;
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_BOTTOM,
                        0,
                        0,
                        self.fences[index].layout.width,
                        height,
                        SWP_NOMOVE | SWP_NOACTIVATE,
                    );
                    let _ = self.render(index);
                    self.persist_layout();
                }
            }
            CMD_LOCK => {
                if let Some(index) = fence_index {
                    self.fences[index].layout.locked = !self.fences[index].layout.locked;
                    self.fences[index].drag = DragMode::None;
                    let _ = ReleaseCapture();
                    let _ = self.render(index);
                    self.persist_layout();
                }
            }
            CMD_SORT_NAME | CMD_SORT_SIZE | CMD_SORT_TYPE | CMD_SORT_MODIFIED | CMD_SORT_CUSTOM => {
                if let Some(index) = fence_index {
                    let mode = match choice.0 as usize {
                        CMD_SORT_NAME => "name",
                        CMD_SORT_SIZE => "size",
                        CMD_SORT_TYPE => "extension",
                        CMD_SORT_MODIFIED => "modified",
                        _ => "custom",
                    };
                    self.fences[index].sort_mode = Some(mode.to_string());
                    rules::sort_items_slice(&mut self.fences[index].content.items, mode);
                    self.fences[index].scroll = 0;
                    let _ = self.render(index);
                    self.persist_layout();
                }
            }
            CMD_SETTINGS => self.open_settings(),
            CMD_EDIT_CONFIG => {
                let path = self.cfg_path.clone();
                self.open_item(&path);
            }
            CMD_RELOAD => self.reload_config(),
            CMD_EXIT => {
                let _ = PostMessageW(self.controller, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            _ => {}
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if !self.hook.is_invalid() {
                let _ = UnhookWindowsHookEx(self.hook);
            }
            let _ = UnregisterHotKey(self.controller, HOTKEY_ID);
            if self.tray {
                let data = NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: self.controller,
                    uID: TRAY_UID,
                    ..Default::default()
                };
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            if !self.tray_icon.is_invalid() {
                let _ = DestroyIcon(self.tray_icon);
            }
            // Al salir, el escritorio vuelve a su estado original: los ficheros
            // que ZenDesktop habia movido a las cajas y al archivo regresan a
            // la raiz. Nunca borra nada y resuelve colisiones con " (n)".
            // Normalmente la restauracion ya ocurrio en WM_CLOSE; aqui actua
            // como red de seguridad para rutas de salida alternativas.
            if !self.restored {
                let _ = rules::restore_desktop(&self.cfg, &self.desktop);
            }
            // Restaura los iconos nativos si se sale estando en Modo Zen.
            if self.zen && self.cfg.general.zen_hides_desktop_icons {
                if let Some(listview) = desktop_listview() {
                    let _ = ShowWindow(listview, SW_SHOW);
                }
            }
            if !self.thumb_hwnd.is_invalid() {
                let _ = DestroyWindow(self.thumb_hwnd);
            }
            for fence in self.fences.drain(..) {
                let _ = DestroyWindow(fence.hwnd);
            }
            self.watcher.take();
        }
    }
}

// ---------------------------------------------------------------------------
// Registro de clases y procedimientos de ventana
// ---------------------------------------------------------------------------

unsafe fn register_classes(instance: HINSTANCE) -> WinResult<()> {
    let controller = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(controller_proc),
        hInstance: instance,
        lpszClassName: CLASS_CONTROLLER,
        ..Default::default()
    };
    if RegisterClassExW(&controller) == 0 {
        return Err(windows::core::Error::from_win32());
    }

    let fence = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS, // necesario para recibir WM_LBUTTONDBLCLK
        lpfnWndProc: Some(fence_proc),
        hInstance: instance,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        lpszClassName: CLASS_FENCE,
        ..Default::default()
    };
    if RegisterClassExW(&fence) == 0 {
        return Err(windows::core::Error::from_win32());
    }

    let thumb = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(thumb_proc),
        hInstance: instance,
        lpszClassName: CLASS_THUMB,
        ..Default::default()
    };
    if RegisterClassExW(&thumb) == 0 {
        return Err(windows::core::Error::from_win32());
    }

    let toast = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(toast_proc),
        hInstance: instance,
        lpszClassName: CLASS_TOAST,
        ..Default::default()
    };
    if RegisterClassExW(&toast) == 0 {
        return Err(windows::core::Error::from_win32());
    }

    Ok(())
}

unsafe fn app_from(hwnd: HWND) -> Option<&'static mut App> {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if raw.is_null() {
        None
    } else {
        Some(&mut *raw)
    }
}

unsafe fn store_app(hwnd: HWND, lparam: LPARAM) {
    let cs = lparam.0 as *const CREATESTRUCTW;
    if !cs.is_null() {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
    }
}

extern "system" fn controller_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_NCCREATE => {
                store_app(hwnd, lparam);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_ZEN_FS => {
                if let Some(app) = app_from(hwnd) {
                    if !app.dragging {
                        // Si el usuario acaba de soltar archivos al escritorio,
                        // no reorganizar (el organizador los devolveria a su caja).
                        if app.skip_next_organize {
                            app.skip_next_organize = false;
                        } else {
                            let _ = rules::organize(&app.cfg, &app.desktop);
                        }
                        app.refresh_contents();
                    }
                }
                LRESULT(0)
            }
            WM_ZEN_DRAG_DONE => {
                if let Some(app) = app_from(hwnd) {
                    app.refresh_contents();
                    rules::notify_shell();
                    if let Some(msg) = app.pending_toast.take() {
                        app.show_toast(&msg, TOAST_DROP);
                    }
                    let _ = SetTimer(app.controller, TIMER_PERSIST, 800, None);
                }
                LRESULT(0)
            }
            WM_ZEN_DBLCLICK => {
                if let Some(app) = app_from(hwnd) {
                    let point = POINT {
                        x: wparam.0 as i32,
                        y: lparam.0 as i32,
                    };
                    if is_empty_desktop(point) {
                        app.toggle_zen();
                    }
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                if wparam.0 as i32 == HOTKEY_ID {
                    if let Some(app) = app_from(hwnd) {
                        app.toggle_zen();
                    }
                }
                LRESULT(0)
            }
            WM_ZEN_TRAY => {
                if let Some(app) = app_from(hwnd) {
                    match lparam.0 as u32 {
                        WM_RBUTTONUP | WM_CONTEXTMENU => app.show_menu(hwnd, None),
                        WM_LBUTTONDBLCLK => app.toggle_zen(),
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_ZEN_UPDATE_CHECKED => {
                // Resultado del chequeo en segundo plano (hilo -> UI). Solo se
                // avisa si hay una version nueva; los fallos de red son silenciosos.
                if let Some(updater::UpdateStatus::UpdateAvailable { version, url, sig_url, .. }) =
                    updater::take_last_check()
                {
                    let msg = format!(
                        "ZenDesktop {version} is available.\n\nDownload and install now?"
                    );
                    let title = wide("ZenDesktop :: Update Available");
                    let body = wide(&msg);
                    let answer = MessageBoxW(
                        hwnd,
                        PCWSTR(body.as_ptr()),
                        PCWSTR(title.as_ptr()),
                        MB_YESNO | MB_ICONINFORMATION | MB_DEFBUTTON2,
                    );
                    if answer == IDYES {
                        if let Err(e) = updater::download_and_install(&url, &sig_url) {
                            let err = wide(&format!("Update failed:\n{e}"));
                            let et = wide("ZenDesktop :: Update Error");
                            MessageBoxW(
                                hwnd,
                                PCWSTR(err.as_ptr()),
                                PCWSTR(et.as_ptr()),
                                MB_OK | MB_ICONERROR,
                            );
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if let Some(app) = app_from(hwnd) {
                    match wparam.0 {
                        TIMER_SWEEP => app.sweep_now(),
                        TIMER_PERSIST => {
                            let _ = KillTimer(hwnd, TIMER_PERSIST);
                            app.persist_layout();
                        }
                        TIMER_THUMB_FADE => {
                            app.thumb_fade_tick();
                        }
                        TIMER_TOAST => {
                            app.toast_tick();
                        }
                        TIMER_ANIM => {
                            app.anim_tick();
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            // Cierre de sesion / apagado del sistema: restaurar el escritorio
            // tambien en esta ruta (el tiempo es limitado, pero es mejor que
            // dejar los ficheros en las cajas).
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION => {
                if wparam.0 != 0 {
                    if let Some(app) = app_from(hwnd) {
                        if !app.restored {
                            app.restored = true;
                            let _ = rules::restore_desktop(&app.cfg, &app.desktop);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                if let Some(app) = app_from(hwnd) {
                    app.render_all();
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                if let Some(app) = app_from(hwnd) {
                    app.persist_layout();
                    // El escritorio vuelve a su estado original en el momento
                    // del cierre (y no solo al soltar la ultima referencia):
                    // garantiza la restauracion en cualquier ruta de salida.
                    if !app.restored {
                        app.restored = true;
                        // Parar el vigilante antes de restaurar: asi ningun
                        // evento de disco pendiente puede re-organizar lo que
                        // acaba de volver al escritorio.
                        app.watcher.take();
                        let _ = rules::restore_desktop(&app.cfg, &app.desktop);
                    }
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

// ---------------------------------------------------------------------------
// Objetivo de arrastrar y soltar OLE (IDropTarget implementado a mano)
// ---------------------------------------------------------------------------
//
// Cada caja se registra como destino de arrastre con RegisterDragDrop. Al
// soltar ficheros de Explorer sobre una caja, el shell llama a estos callbacks
// en el hilo de interfaz y los ficheros se mueven a la carpeta fisica de la
// caja (se refresca el contenido y se programa el guardado del layout).

#[repr(C)]
struct FenceDropTarget {
    vtbl: *const FenceDropVtbl,
    app: *mut App,
    /// Cache de DragEnter: true si el arrastre trae ficheros (CF_HDROP).
    accepts_files: std::cell::Cell<bool>,
}

#[repr(C)]
struct FenceDropVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> windows::core::HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    drag_enter: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        MODIFIERKEYS_FLAGS,
        POINTL,
        *mut DROPEFFECT,
    ) -> windows::core::HRESULT,
    drag_over: unsafe extern "system" fn(
        *mut c_void,
        MODIFIERKEYS_FLAGS,
        POINTL,
        *mut DROPEFFECT,
    ) -> windows::core::HRESULT,
    drag_leave: unsafe extern "system" fn(*mut c_void) -> windows::core::HRESULT,
    drop: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        MODIFIERKEYS_FLAGS,
        POINTL,
        *mut DROPEFFECT,
    ) -> windows::core::HRESULT,
}

static DROP_TARGET_VTBL: FenceDropVtbl = FenceDropVtbl {
    query_interface: drop_target_qi,
    add_ref: drop_target_addref,
    release: drop_target_release,
    drag_enter: drop_target_drag_enter,
    drag_over: drop_target_drag_over,
    drag_leave: drop_target_drag_leave,
    drop: drop_target_drop,
};

unsafe extern "system" fn drop_target_qi(
    this: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> windows::core::HRESULT {
    const IID_IUNKNOWN: GUID = GUID::from_u128(0x0000_0000_0000_0000_c000_000000000046);
    const IID_IDROPTARGET: GUID = GUID::from_u128(0x0000_0122_0000_0000_c000_000000000046);
    if !riid.is_null() && !ppv.is_null() {
        if *riid == IID_IUNKNOWN || *riid == IID_IDROPTARGET {
            *ppv = this;
            return windows::core::HRESULT(0); // S_OK
        }
    }
    windows::core::HRESULT(0x8000_4002u32 as i32) // E_NOINTERFACE
}

unsafe extern "system" fn drop_target_addref(_this: *mut c_void) -> u32 {
    1 // el objeto vive mientras la aplicacion (Box::leak)
}

unsafe extern "system" fn drop_target_release(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn drop_target_drag_enter(
    this: *mut c_void,
    pdataobj: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _pt: POINTL,
    pdweffect: *mut DROPEFFECT,
) -> windows::core::HRESULT {
    let target = &*(this as *const FenceDropTarget);
    let _app = &*target.app;
    // Solo se aceptan arrastres de ficheros: preguntar al IDataObject si
    // puede entregar CF_HDROP (evita el cursor de "mover" sobre texto, etc.).
    let files = if pdataobj.is_null() {
        false
    } else {
        let data = &*(pdataobj as *const IDataObject);
        let fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        data.QueryGetData(&fmt).is_ok()
    };
    target.accepts_files.set(files);
    if !pdweffect.is_null() {
        *pdweffect = if files {
            DROPEFFECT_MOVE
        } else {
            DROPEFFECT_NONE
        };
    }
    windows::core::HRESULT(0)
}

unsafe extern "system" fn drop_target_drag_over(
    this: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _pt: POINTL,
    pdweffect: *mut DROPEFFECT,
) -> windows::core::HRESULT {
    let target = &*(this as *const FenceDropTarget);
    if !pdweffect.is_null() {
        *pdweffect = if target.accepts_files.get() {
            DROPEFFECT_MOVE
        } else {
            DROPEFFECT_NONE
        };
    }
    windows::core::HRESULT(0)
}

unsafe extern "system" fn drop_target_drag_leave(_this: *mut c_void) -> windows::core::HRESULT {
    windows::core::HRESULT(0)
}

unsafe extern "system" fn drop_target_drop(
    this: *mut c_void,
    pdataobj: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    pt: POINTL,
    pdweffect: *mut DROPEFFECT,
) -> windows::core::HRESULT {
    let app = &mut *(*(this as *const FenceDropTarget)).app;
    let hwnd = WindowFromPoint(POINT { x: pt.x, y: pt.y });
    let mut moved = 0usize;
    if !pdataobj.is_null() {
        let data = &*(pdataobj as *const IDataObject);
        let paths = App::paths_from_idata(data);
        if let Some(index) = app.index_of(hwnd) {
            moved = app.accept_drop(index, paths);
        } else {
            moved = app.drop_to_desktop(paths);
        }
    }
    if !pdweffect.is_null() {
        *pdweffect = if moved > 0 { DROPEFFECT_MOVE } else { DROPEFFECT_NONE };
    }
    windows::core::HRESULT(0)
}

unsafe extern "system" fn thumb_proc(
    hwnd: HWND,
    message: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, _wparam, _lparam),
    }
}

unsafe extern "system" fn toast_proc(
    hwnd: HWND,
    message: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, _wparam, _lparam),
    }
}

extern "system" fn fence_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            store_app(hwnd, lparam);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }

        let app = match app_from(hwnd) {
            Some(a) => a,
            None => return DefWindowProcW(hwnd, message, wparam, lparam),
        };
        let index = match app.index_of(hwnd) {
            Some(i) => i,
            None => return DefWindowProcW(hwnd, message, wparam, lparam),
        };

        match message {
            // Las cajas viven siempre en el fondo del orden Z: forman parte del
            // escritorio, nunca tapan a las ventanas del usuario.
            WM_WINDOWPOSCHANGING => {
                let pos = lparam.0 as *mut WINDOWPOS;
                if !pos.is_null() && ((*pos).flags.0 & SWP_NOZORDER.0) == 0 {
                    (*pos).hwndInsertAfter = HWND_BOTTOM;
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let theme_header = (app.theme.header * app.fences[index].scale) as i32;
                let fence = &app.fences[index];
                let w_dip = fence.layout.width as f32 / fence.scale;
                let item_cnt = fence.content.items.len();
                if app.cfg.appearance.show_search && pt.y <= theme_header {
                    if let Some(sr) = search_rect(w_dip, &app.theme, app.theme.show_counter, item_cnt) {
                        let s = fence.scale;
                        let sx1 = (sr.left * s) as i32;
                        let sx2 = (sr.right * s) as i32;
                        let sy1 = (sr.top * s) as i32;
                        let sy2 = (sr.bottom * s) as i32;
                        if pt.x >= sx1 && pt.x <= sx2 && pt.y >= sy1 && pt.y <= sy2 {
                            return LRESULT(MA_ACTIVATE as isize);
                        }
                    }
                }
                LRESULT(MA_NOACTIVATE as isize)
            }
            WM_SETCURSOR => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let fence = &app.fences[index];
                let in_grip = pt.x > fence.layout.width - GRIP
                    && pt.y > fence.visible_height(&app.theme) - GRIP
                    && !fence.layout.collapsed
                    && !fence.layout.locked;
                if in_grip {
                    let _ = SetCursor(LoadCursorW(None, IDC_SIZENWSE).unwrap_or_default());
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_of(lparam);
                let theme_header = (app.theme.header * app.fences[index].scale) as i32;

                // Clic sobre la barra de busqueda: dar foco para escribir texto.
                if app.cfg.appearance.show_search && y <= theme_header {
                    let fence = &mut app.fences[index];
                    let s = fence.scale;
                    let w_dip = fence.layout.width as f32 / s;
                    let item_cnt = fence.content.items.len();
                    if let Some(sr) = search_rect(w_dip, &app.theme, app.theme.show_counter, item_cnt) {
                        let sx1 = (sr.left * s) as i32;
                        let sx2 = (sr.right * s) as i32;
                        let sy1 = (sr.top * s) as i32;
                        let sy2 = (sr.bottom * s) as i32;
                        if x >= sx1 && x <= sx2 && y >= sy1 && y <= sy2 {
                            let clear_x = (sr.right - 18.0) * s;
                            if !fence.search_text.is_empty() && x as f32 >= clear_x {
                                fence.search_text.clear();
                                fence.scroll = 0;
                            } else {
                                fence.search_focused = true;
                                let _ = SetForegroundWindow(hwnd);
                                let _ = SetFocus(hwnd);
                            }
                            let _ = app.render(index);
                            return LRESULT(0);
                        }
                    }
                }

                // Clic sobre el candado de la cabecera: anclar / desanclar.
                {
                    let fence = &mut app.fences[index];
                    let w_dip = fence.layout.width as f32 / fence.scale;
                    let item_cnt = fence.content.items.len();
                    let lr = lock_rect(w_dip, &app.theme, app.theme.show_counter, item_cnt);
                    let in_lock = y <= theme_header
                        && x >= (lr.left * fence.scale) as i32
                        && x <= (lr.right * fence.scale) as i32;
                    if in_lock {
                        fence.layout.locked = !fence.layout.locked;
                        fence.drag = DragMode::None;
                        let _ = app.render(index);
                        SetTimer(app.controller, TIMER_PERSIST, 800, None);
                        return LRESULT(0);
                    }

                    if fence.layout.locked {
                        fence.drag = DragMode::None;
                        return LRESULT(0);
                    }
                }

                // 1. Redimensionado de la caja (esquina inferior derecha - GRIP): prioridad absoluta.
                {
                    let fence = &mut app.fences[index];
                    let in_grip = x > fence.layout.width - GRIP
                        && y > fence.visible_height(&app.theme) - GRIP
                        && !fence.layout.collapsed;
                    if in_grip {
                        fence.anchor = screen_cursor();
                        fence.origin = fence.layout.clone();
                        fence.drag = DragMode::Resize;
                        SetCapture(hwnd);
                        return LRESULT(0);
                    }
                }

                // 2. Arrastrar cabecera para mover la caja.
                if y <= theme_header {
                    let fence = &mut app.fences[index];
                    fence.anchor = screen_cursor();
                    fence.origin = fence.layout.clone();
                    fence.drag = DragMode::Move;
                    SetCapture(hwnd);
                    return LRESULT(0);
                }

                // 3. Seleccion de elementos y Rubberband.
                let item_idx = app.fences[index].item_at(&app.theme, x, y);
                let ctrl_down = (GetKeyState(VK_CONTROL.0 as i32) as u32) & 0x8000 != 0;
                let shift_down = (GetKeyState(VK_SHIFT.0 as i32) as u32) & 0x8000 != 0;

                if let Some(idx) = item_idx {
                    let fence = &mut app.fences[index];
                    fence.search_focused = false;
                    if ctrl_down {
                        if fence.selected.contains(&idx) {
                            fence.selected.remove(&idx);
                        } else {
                            fence.selected.insert(idx);
                        }
                    } else if shift_down {
                        if let Some(&first_sel) = fence.selected.iter().next() {
                            let min_i = first_sel.min(idx);
                            let max_i = first_sel.max(idx);
                            for i in min_i..=max_i {
                                fence.selected.insert(i);
                            }
                        } else {
                            fence.selected.insert(idx);
                        }
                    } else {
                        if !fence.selected.contains(&idx) {
                            fence.selected.clear();
                            fence.selected.insert(idx);
                        }
                    }
                    fence.drag = DragMode::ItemDrag { item_idx: idx, start_x: x, start_y: y };
                    let _ = app.render(index);
                } else {
                    // Clic en zona vacia dentro del cuerpo: iniciar seleccion por caja (Rubberband).
                    let fence = &mut app.fences[index];
                    fence.search_focused = false;
                    if !ctrl_down && !shift_down {
                        fence.selected.clear();
                    }
                    let fx = x as f32 / fence.scale;
                    let fy = y as f32 / fence.scale;
                    fence.drag = DragMode::Select { start_x: fx, start_y: fy, curr_x: fx, curr_y: fy };
                    fence.rubberband = Some((fx, fy, fx, fy));
                    SetCapture(hwnd);
                    let _ = app.render(index);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_of(lparam);
                let mode = app.fences[index].drag.clone();

                if let DragMode::ItemDrag { item_idx: _, start_x, start_y } = mode {
                    if (wparam.0 & 0x0001) != 0 {
                        let dx = (x - start_x).abs();
                        let dy = (y - start_y).abs();
                        if dx > 4 || dy > 4 {
                            let paths: Vec<PathBuf> = app.fences[index].selected.iter().filter_map(|&i| app.fences[index].content.items.get(i).map(|item| item.path.clone())).collect();
                            let first_path = paths.first().cloned();
                            app.fences[index].drag = DragMode::ManualDrag { paths, source_fence: index };
                            SetCapture(hwnd);
                            SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                            if let Some(ref p) = first_path {
                                app.show_drag_image(p);
                            }
                            return LRESULT(0);
                        }
                    } else {
                        app.fences[index].drag = DragMode::None;
                    }
                }

                // Comprobar hover sobre el candado
                let w_dip = app.fences[index].layout.width as f32 / app.fences[index].scale;
                let theme_header = (app.theme.header * app.fences[index].scale) as i32;
                let item_cnt = app.fences[index].content.items.len();
                let lr = lock_rect(w_dip, &app.theme, app.theme.show_counter, item_cnt);
                let in_lock = y <= theme_header
                    && x >= (lr.left * app.fences[index].scale) as i32
                    && x <= (lr.right * app.fences[index].scale) as i32;
                if in_lock != app.fences[index].hover_lock {
                    app.fences[index].hover_lock = in_lock;
                    let _ = app.render(index);
                }

                // Durante drag manual: actualizar miniatura + highlight de subcarpeta destino.
                if let DragMode::ManualDrag { source_fence, .. } = mode {
                    app.update_drag_image_position();
                    // Detectar si el cursor esta sobre una subcarpeta de otra caja.
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    let mut new_target: Option<(usize, i32)> = None;
                    for (fi, f) in app.fences.iter().enumerate() {
                        if fi == source_fence { continue; }
                        let fx = f.layout.x;
                        let fy = f.layout.y;
                        let fw = f.layout.width;
                        let fh = f.visible_height(&app.theme);
                        if cursor.x >= fx && cursor.x < fx + fw && cursor.y >= fy && cursor.y < fy + fh {
                            let local_x = cursor.x - fx;
                            let local_y = cursor.y - fy;
                            if let Some(idx) = f.item_at(&app.theme, local_x, local_y) {
                                if let Some(item) = f.content.items.get(idx) {
                                    if item.is_dir {
                                        new_target = Some((fi, idx as i32));
                                    }
                                }
                            }
                            break;
                        }
                    }
                    // Aplicar o limpiar el highlight.
                    if new_target != app.drag_hover_target {
                        // Limpiar anterior.
                        if let Some((prev_fi, _)) = app.drag_hover_target {
                            app.fences[prev_fi].hover = -1;
                            let _ = app.render(prev_fi);
                        }
                        // Aplicar nuevo.
                        if let Some((fi, item_idx)) = new_target {
                            app.fences[fi].hover = item_idx;
                            let _ = app.render(fi);
                        }
                        app.drag_hover_target = new_target;
                    }
                }

                if mode == DragMode::None {
                    let hover_idx = app.fences[index]
                        .item_at(&app.theme, x, y);
                    let hover = hover_idx.map(|i| i as i32).unwrap_or(-1);
                    if hover != app.fences[index].hover {
                        app.fences[index].hover = hover;
                        // Miniatura emergente para imagenes.
                        if let Some(idx) = hover_idx {
                            let is_img = {
                                let item = &app.fences[index].content.items[idx];
                                rules::is_image(&item.ext) && item.path.is_file()
                            };
                            if is_img {
                                let path = app.fences[index].content.items[idx].path.clone();
                                app.show_thumb(&path);
                            } else {
                                app.hide_thumb();
                            }
                        } else {
                            app.hide_thumb();
                        }
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut track);
                        let _ = app.render(index);
                    }
                    return LRESULT(0);
                }

                if let DragMode::Select { start_x, start_y, .. } = mode {
                    let fx = x as f32 / app.fences[index].scale;
                    let fy = y as f32 / app.fences[index].scale;
                    app.fences[index].drag = DragMode::Select { start_x, start_y, curr_x: fx, curr_y: fy };
                    app.fences[index].rubberband = Some((start_x, start_y, fx, fy));
                    
                    let (x1, x2) = (start_x.min(fx), start_x.max(fx));
                    let (y1, y2) = (start_y.min(fy), start_y.max(fy));
                    
                    let scale = app.fences[index].scale;
                    let theme = &app.theme;
                    let fence = &mut app.fences[index];
                    let grid_mode = theme.grid_mode;
                    let pad = theme.padding;
                    
                    if grid_mode {
                        let cell = theme.grid_item_size.max(48.0);
                        let w_dip = fence.layout.width as f32 / scale;
                        let grid_cols = ((w_dip - pad) / cell).floor().max(1.0) as usize;
                        let scroll_off = fence.scroll as f32 * cell;
                        for (idx, _) in fence.content.items.iter().enumerate() {
                            let row = idx / grid_cols;
                            let col = idx % grid_cols;
                            let cx = pad * 0.5 + col as f32 * cell;
                            let cy_val = theme.header + (row as f32 * cell) - scroll_off;
                            if cx + cell >= x1 && cx <= x2 && cy_val + cell >= y1 && cy_val <= y2 {
                                fence.selected.insert(idx);
                            }
                        }
                    } else {
                        for (idx, _) in fence.content.items.iter().enumerate() {
                            let top = theme.header + (idx as f32 - fence.scroll as f32) * theme.row;
                            let bottom = top + theme.row;
                            if top <= y2 && bottom >= y1 {
                                fence.selected.insert(idx);
                            }
                        }
                    }
                    let _ = app.render(index);
                    return LRESULT(0);
                }

                let cursor = screen_cursor();
                let dx = cursor.x - app.fences[index].anchor.x;
                let dy = cursor.y - app.fences[index].anchor.y;
                let origin = app.fences[index].origin.clone();
                if mode == DragMode::Move {
                    let mut candidate_x = origin.x + dx;
                    let mut candidate_y = origin.y + dy;

                    let snap_dist = 10;
                    let work = work_area();

                    // Snap a los bordes del monitor
                    if (candidate_x - work.left).abs() < snap_dist { candidate_x = work.left; }
                    if (candidate_y - work.top).abs() < snap_dist { candidate_y = work.top; }
                    if ((candidate_x + origin.width) - work.right).abs() < snap_dist { candidate_x = work.right - origin.width; }
                    if ((candidate_y + origin.height) - work.bottom).abs() < snap_dist { candidate_y = work.bottom - origin.height; }

                    // Snap imantado con respecto a otras cajas adyacentes
                    for (i, f) in app.fences.iter().enumerate() {
                        if i == index { continue; }
                        let fx = f.layout.x;
                        let fy = f.layout.y;
                        let fw = f.layout.width;
                        let fh = f.layout.height;

                        if (candidate_x - (fx + fw)).abs() < snap_dist { candidate_x = fx + fw; }
                        if ((candidate_x + origin.width) - fx).abs() < snap_dist { candidate_x = fx - origin.width; }
                        if (candidate_x - fx).abs() < snap_dist { candidate_x = fx; }

                        if (candidate_y - (fy + fh)).abs() < snap_dist { candidate_y = fy + fh; }
                        if ((candidate_y + origin.height) - fy).abs() < snap_dist { candidate_y = fy - origin.height; }
                        if (candidate_y - fy).abs() < snap_dist { candidate_y = fy; }
                    }

                    app.fences[index].layout.x = candidate_x;
                    app.fences[index].layout.y = candidate_y;
                } else if mode == DragMode::Resize {
                    app.fences[index].layout.width = (origin.width + dx).max(160);
                    app.fences[index].layout.height = (origin.height + dy).max(96);
                } else {
                    return LRESULT(0);
                }
                let layout = app.fences[index].layout.clone();
                let height = app.fences[index].visible_height(&app.theme);
                let _ = SetWindowPos(
                    hwnd,
                    HWND_BOTTOM,
                    layout.x,
                    layout.y,
                    layout.width,
                    height,
                    SWP_NOACTIVATE,
                );
                let _ = app.render(index);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let drag_state = app.fences[index].drag.clone();
                if let DragMode::ItemDrag { .. } = drag_state {
                    app.fences[index].drag = DragMode::None;
                    return LRESULT(0);
                }
                if let DragMode::ManualDrag { paths, source_fence } = drag_state {
                    let _ = ReleaseCapture();
                    // Limpiar highlight de subcarpeta destino.
                    app.clear_drag_hover();
                    SetCursor(LoadCursorW(None, IDC_ARROW).unwrap_or_default());
                    app.hide_drag_image();
                    app.fences[index].drag = DragMode::None;
                    let mut pt = POINT::default();
                    GetCursorPos(&mut pt).expect("GetCursorPos");
                    // Comprobar si el cursor esta sobre OTRA caja (no la de origen).
                    // Recolectamos (fence_idx, optional_subfolder) dentro del
                    // bucle y actuamos fuera para no chocar con el borrow checker.
                    let mut dropped_on_other = false;
                    let mut drop_target: Option<(usize, Option<(PathBuf, String)>)> = None;
                    for (fi, f) in app.fences.iter().enumerate() {
                        if fi == source_fence { continue; }
                        let fx = f.layout.x;
                        let fy = f.layout.y;
                        let fw = f.layout.width;
                        let fh = f.visible_height(&app.theme);
                        if pt.x >= fx && pt.x < fx + fw && pt.y >= fy && pt.y < fy + fh {
                            let local_x = pt.x - fx;
                            let local_y = pt.y - fy;
                            let subfolder = f.item_at(&app.theme, local_x, local_y)
                                .and_then(|idx| f.content.items.get(idx))
                                .filter(|item| item.is_dir)
                                .map(|item| (item.path.clone(), item.name.clone()));
                            drop_target = Some((fi, subfolder));
                            break;
                        }
                    }
                    if let Some((fi, subfolder)) = drop_target {
                        dropped_on_other = true;
                        if let Some((dest_path, dest_name)) = subfolder {
                            let _ = app.move_paths_to(&dest_path, &dest_name, paths.clone());
                        } else {
                            let _ = app.accept_drop(fi, paths.clone());
                        }
                    }
                    if !dropped_on_other {
                        app.drop_to_desktop(paths);
                    }
                    let _ = app.render(index);
                    let _ = SetTimer(app.controller, TIMER_PERSIST, 800, None);
                    return LRESULT(0);
                }
                if let DragMode::Select { .. } = drag_state {
                    let _ = ReleaseCapture();
                    app.fences[index].drag = DragMode::None;
                    app.fences[index].rubberband = None;
                    let _ = app.render(index);
                    return LRESULT(0);
                }
 if app.fences[index].drag != DragMode::None {
                    let _ = ReleaseCapture();
                    app.fences[index].drag = DragMode::None;
                    let snap = app.theme.snap;
                    if snap > 1 {
                        let layout = &mut app.fences[index].layout;
                        layout.x = (layout.x as f32 / snap as f32).round() as i32 * snap;
                        layout.y = (layout.y as f32 / snap as f32).round() as i32 * snap;
                    }
                    let layout = app.fences[index].layout.clone();
                    let height = app.fences[index].visible_height(&app.theme);
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_BOTTOM,
                        layout.x,
                        layout.y,
                        layout.width,
                        height,
                        SWP_NOACTIVATE,
                    );
                    let _ = app.render(index);
                    SetTimer(app.controller, TIMER_PERSIST, 800, None);
                }
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = point_of(lparam);
                let header = (app.theme.header * app.fences[index].scale) as i32;
                let w_dip = app.fences[index].layout.width as f32 / app.fences[index].scale;
                let s = app.fences[index].scale;
                let item_cnt = app.fences[index].content.items.len();
                let lr = lock_rect(w_dip, &app.theme, app.theme.show_counter, item_cnt);
                let in_lock = y <= header
                    && x >= (lr.left * s) as i32
                    && x <= (lr.right * s) as i32;
                let in_search = app.cfg.appearance.show_search && y <= header && search_rect(w_dip, &app.theme, app.theme.show_counter, item_cnt).map_or(false, |sr| {
                    x >= (sr.left * s) as i32 && x <= (sr.right * s) as i32
                });
                if in_lock || in_search {
                    return LRESULT(0);
                }
                if y <= header {
                    app.start_collapse_anim(index);
                } else if let Some(item) = app.fences[index].item_at(&app.theme, x, y) {
                    let path = app.fences[index].content.items[item].path.clone();
                    app.open_item(&path);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) as i16) as i32 / WHEEL_DELTA as i32;
                let max = if app.cfg.appearance.grid_mode {
                    let cell = (app.cfg.appearance.grid_item_size * app.fences[index].scale).max(48.0);
                    let w_dip = app.fences[index].layout.width as f32 / app.fences[index].scale;
                    let pad = app.theme.padding;
                    let grid_cols = ((w_dip * app.fences[index].scale - pad * 2.0) / cell).floor().max(1.0) as usize;
                    let grid_rows = (app.fences[index].content.items.len() + grid_cols - 1) / grid_cols;
                    let h_dip = app.fences[index].visible_height(&app.theme) as f32 / app.fences[index].scale;
                    let visible_rows = ((h_dip * app.fences[index].scale - app.theme.header * app.fences[index].scale) / cell).floor().max(1.0) as usize;
                    grid_rows.saturating_sub(visible_rows) as i32
                } else {
                    app.fences[index].max_scroll(&app.theme)
                };
                let next = (app.fences[index].scroll - delta * 3).clamp(0, max);
                if next != app.fences[index].scroll {
                    app.fences[index].scroll = next;
                    let _ = app.render(index);
                }
                LRESULT(0)
            }
            WM_CHAR => {
                let ch_val = wparam.0 as u32;
                if ch_val == 0x08 { // Backspace
                    if !app.fences[index].search_text.is_empty() {
                        app.fences[index].search_text.pop();
                        app.fences[index].scroll = 0;
                        let _ = app.render(index);
                    }
                } else if ch_val == 0x1B { // Escape
                    app.fences[index].search_text.clear();
                    app.fences[index].search_focused = false;
                    app.fences[index].scroll = 0;
                    let _ = app.render(index);
                } else if ch_val >= 32 && ch_val != 127 {
                    if let Some(ch) = char::from_u32(ch_val) {
                        app.fences[index].search_text.push(ch);
                        app.fences[index].scroll = 0;
                        let _ = app.render(index);
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let vk = wparam.0 as u32;
                let ctrl = (GetKeyState(VK_CONTROL.0 as i32) as u32) & 0x8000 != 0;
                match vk {
                    0x41 if ctrl => { // Ctrl + A (Seleccionar todo)
                        let fence = &mut app.fences[index];
                        fence.selected = (0..fence.content.items.len()).collect();
                        let _ = app.render(index);
                    }
                    0x46 if ctrl => { // Ctrl + F (Foco en el buscador)
                        let fence = &mut app.fences[index];
                        fence.search_focused = true;
                        let _ = SetForegroundWindow(hwnd);
                        let _ = SetFocus(hwnd);
                        let _ = app.render(index);
                    }
                    0x2E => { // VK_DELETE (Enviar a la papelera de reciclaje)
                        let paths: Vec<PathBuf> = app.fences[index].selected.iter()
                            .filter_map(|&idx| app.fences[index].content.items.get(idx).map(|it| it.path.clone()))
                            .collect();
                        if !paths.is_empty() {
                            app.delete_to_recycle_bin(hwnd, &paths);
                            app.fences[index].selected.clear();
                            app.refresh_contents();
                        }
                    }
                    0x0D => { // VK_RETURN (Abrir elementos seleccionados)
                        let fence = &app.fences[index];
                        let paths: Vec<PathBuf> = fence.selected.iter()
                            .filter_map(|&idx| fence.content.items.get(idx).map(|it| it.path.clone()))
                            .collect();
                        for path in paths {
                            app.open_item(&path);
                        }
                    }
                    0x08 => { // VK_BACK
                        if !app.fences[index].search_text.is_empty() {
                            app.fences[index].search_text.pop();
                            let _ = app.render(index);
                        }
                    }
                    0x1B => { // VK_ESCAPE
                        let fence = &mut app.fences[index];
                        fence.search_text.clear();
                        fence.search_focused = false;
                        fence.selected.clear();
                        fence.scroll = 0;
                        let _ = app.render(index);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let (x, y) = point_of(lparam);
                if let Some(item) = app.fences[index].item_at(&app.theme, x, y) {
                    let fence = &mut app.fences[index];
                    if !fence.selected.contains(&item) {
                        let ctrl = (GetKeyState(VK_CONTROL.0 as i32) as u32) & 0x8000 != 0;
                        if !ctrl {
                            fence.selected.clear();
                        }
                        fence.selected.insert(item);
                    }
                    let paths: Vec<PathBuf> = fence.selected.iter()
                        .filter_map(|&idx| fence.content.items.get(idx).map(|it| it.path.clone()))
                        .collect();
                    if !paths.is_empty() {
                        app.show_shell_menu_for_paths(hwnd, &paths);
                    }
                } else {
                    app.show_menu(hwnd, Some(index));
                }
                LRESULT(0)
            }

            // Submenus del menu nativo ("Abrir con"...): se reenvian al
            // IContextMenu3 para que el shell dibuje y gestione sus items.
            WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM => {
                if let Some(menu) = &app.shell_menu {
                    if let Ok(menu3) = menu.cast::<IContextMenu3>() {
                        let _ = menu3.HandleMenuMsg(message, wparam, lparam);
                    }
                }
                LRESULT(0)
            }
            // Arrastrar ficheros desde el escritorio/Explorer sobre una caja
            // (via clasica WM_DROPFILES; la via OLE llega por IDropTarget).
            WM_DROPFILES => {
                let hdrop = HDROP(wparam.0 as *mut c_void);
                app.handle_hdrop(index, hdrop);
                DragFinish(hdrop);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = (wparam.0 & 0xFFFF) as f32;
                app.fences[index].scale = if dpi > 0.0 { dpi / 96.0 } else { 1.0 };
                app.fences[index].surface = None;
                let _ = app.render(index);
                LRESULT(0)
            }
            WM_PAINT => {
                let _ = ValidateRect(hwnd, None);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

// ---------------------------------------------------------------------------
// Hook de raton de bajo nivel (Modo Zen)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct HookState {
    controller: Option<HWND>,
    last_time: u32,
    last_x: i32,
    last_y: i32,
}

thread_local! {
    static HOOK_STATE: Cell<HookState> = Cell::new(HookState::default());
}

/// Los hooks `WH_MOUSE_LL` reciben pulsaciones sueltas, nunca `WM_LBUTTONDBLCLK`:
/// el doble clic se sintetiza aqui comparando tiempo y distancia con los
/// umbrales del sistema. El cuerpo se mantiene deliberadamente minimo (unas
/// pocas comparaciones enteras) porque se ejecuta en la ruta de entrada global.
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wparam.0 as u32 == WM_LBUTTONDOWN {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        HOOK_STATE.with(|cell| {
            let mut state = cell.get();
            let dt = info.time.wrapping_sub(state.last_time);
            let dx = (info.pt.x - state.last_x).abs();
            let dy = (info.pt.y - state.last_y).abs();
            let max_x = GetSystemMetrics(SM_CXDOUBLECLK).max(2) / 2;
            let max_y = GetSystemMetrics(SM_CYDOUBLECLK).max(2) / 2;

            if dt <= GetDoubleClickTime() && dx <= max_x && dy <= max_y {
                if let Some(controller) = state.controller {
                    // Trabajo fuera del hook: solo se publica el punto.
                    let _ = PostMessageW(
                        controller,
                        WM_ZEN_DBLCLICK,
                        WPARAM(info.pt.x as usize),
                        LPARAM(info.pt.y as isize),
                    );
                }
                state.last_time = 0;
            } else {
                state.last_time = info.time;
                state.last_x = info.pt.x;
                state.last_y = info.pt.y;
            }
            cell.set(state);
        });
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// true si el punto cae en zona libre del escritorio (ni ventanas, ni iconos).
unsafe fn is_empty_desktop(point: POINT) -> bool {
    let hwnd = WindowFromPoint(point);
    if hwnd.is_invalid() {
        return false;
    }
    let class = class_name(hwnd);
    match class.as_str() {
        "Progman" | "WorkerW" | "SHELLDLL_DefView" => true,
        "SysListView32" => !desktop_icon_at(hwnd, point),
        _ => false,
    }
}

/// Consulta `LVM_HITTEST` en el proceso de Explorer.
///
/// El ListView del escritorio pertenece a explorer.exe, asi que la estructura
/// `LVHITTESTINFO` debe residir en SU espacio de direcciones: se reserva con
/// `VirtualAllocEx`, se escribe con `WriteProcessMemory` y se envia el mensaje
/// con `SendMessageTimeoutW` (nunca `SendMessageW`: un Explorer colgado no debe
/// bloquear nuestro hilo de interfaz).
unsafe fn desktop_icon_at(listview: HWND, screen: POINT) -> bool {
    let mut client = screen;
    let _ = ScreenToClient(listview, &mut client);

    let mut pid = 0u32;
    GetWindowThreadProcessId(listview, Some(&mut pid));
    if pid == 0 {
        return false;
    }
    let process = match OpenProcess(
        PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(_) => return false,
    };

    let size = std::mem::size_of::<LVHITTESTINFO>();
    let remote = VirtualAllocEx(process, None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if remote.is_null() {
        let _ = windows::Win32::Foundation::CloseHandle(process);
        return false;
    }

    let mut hit = LVHITTESTINFO {
        pt: client,
        ..Default::default()
    };
    let mut hit_index: isize = -1;
    if windows::Win32::System::Diagnostics::Debug::WriteProcessMemory(
        process,
        remote,
        &hit as *const _ as *const c_void,
        size,
        None,
    )
    .is_ok()
    {
        let mut result: usize = 0;
        SendMessageTimeoutW(
            listview,
            LVM_HITTEST,
            WPARAM(0),
            LPARAM(remote as isize),
            SMTO_ABORTIFHUNG,
            120,
            Some(&mut result as *mut usize),
        );
        hit_index = result as isize;
        let _ = windows::Win32::System::Diagnostics::Debug::ReadProcessMemory(
            process,
            remote,
            &mut hit as *mut _ as *mut c_void,
            size,
            None,
        );
    }

    let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
    let _ = windows::Win32::Foundation::CloseHandle(process);
    hit_index >= 0
}

/// Localiza el ListView de iconos, tanto en el arbol clasico (Progman) como
/// cuando Explorer lo ha movido a una ventana WorkerW (fondo dinamico).
pub unsafe fn desktop_listview() -> Option<HWND> {
    if let Ok(progman) = FindWindowW(w!("Progman"), None) {
        if let Ok(defview) = FindWindowExW(progman, None, w!("SHELLDLL_DefView"), None) {
            if let Ok(listview) = FindWindowExW(defview, None, w!("SysListView32"), None) {
                return Some(listview);
            }
        }
    }
    let mut found: Option<HWND> = None;
    let _ = EnumWindows(
        Some(enum_worker),
        LPARAM(&mut found as *mut Option<HWND> as isize),
    );
    found
}

unsafe extern "system" fn enum_worker(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
    if class_name(hwnd) == "WorkerW" {
        if let Ok(defview) = FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), None) {
            if let Ok(listview) = FindWindowExW(defview, None, w!("SysListView32"), None) {
                let out = &mut *(lparam.0 as *mut Option<HWND>);
                *out = Some(listview);
                return windows::Win32::Foundation::BOOL(0); // detener enumeracion
            }
        }
    }
    windows::Win32::Foundation::BOOL(1)
}

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}

fn wide_str(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    let value = lparam.0 as u32;
    ((value & 0xFFFF) as i16 as i32, (value >> 16) as i16 as i32)
}

fn screen_cursor() -> POINT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    point
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let len = GetClassNameW(hwnd, &mut buffer);
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

fn work_area() -> RECT {
    let mut area = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut area as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    if area.right <= area.left {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    } else {
        area
    }
}

/// Comprobacion en tiempo de compilacion: la geometria persistida debe seguir
/// siendo `Copy` y de tamano fijo para poder clonarse en cada frame de arrastre.
const _: () = assert!(std::mem::size_of::<FenceLayout>() <= 128);

// ---------------------------------------------------------------------------
// Probe end-to-end (cargo test --release -- --ignored fence_e2e_probe)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Rule;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        mouse_event, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    };

    /// Crea un escritorio temporal unico con unos ficheros de prueba.
    fn temp_desktop(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zendesktop_probe_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["foto.png", "informe.pdf", "juego.xyz"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        dir
    }

    fn packed(x: i32, y: i32) -> LPARAM {
        LPARAM((((y & 0xFFFF) << 16) | (x & 0xFFFF)) as isize)
    }

    fn post(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) {
        unsafe {
            let _ = PostMessageW(hwnd, msg, wparam, lparam);
        }
    }

    /// Prueba real de la app: lanza el proceso de interfaz sobre un escritorio
    /// temporal, redimensiona una caja arrastrando el grip (mensajes de raton
    /// sintetizados) y cierra la app comprobando que los ficheros organizados
    /// vuelven al escritorio. Requiere sesion interactiva.
    #[test]
    #[ignore = "probe e2e manual"]
    fn fence_e2e_probe() {
        // 1) Escritorio temporal + configuracion con cajas fisicas.
        let root = temp_desktop("e2e");
        let mut cfg = Config::default();
        cfg.general.root_folder = "ZenDesktop".into();
        cfg.general.archive_folder = "Archivo".into();
        cfg.rules = vec![
            Rule {
                title: "Media".into(),
                folder: "Media".into(),
                color: "#7DD3FC".into(),
                extensions: vec!["png".into(), "jpg".into()],
                name_patterns: vec![],
                enabled: true,
                move_files: true,
                include_folders: true,
                ..Rule::default()
            },
            Rule {
                title: "Docs".into(),
                folder: "Docs".into(),
                color: "#C4B5FD".into(),
                extensions: vec!["pdf".into(), "docx".into()],
                name_patterns: vec![],
                enabled: true,
                move_files: true,
                include_folders: false,
                ..Rule::default()
            },
            Rule {
                title: "Varios".into(),
                folder: "Varios".into(),
                color: "#6EE7B7".into(),
                extensions: vec![],
                name_patterns: vec![],
                enabled: true,
                move_files: true,
                include_folders: true,
                ..Rule::default()
            },
        ];

        // 2) Organizar como hace el arranque real.
        let report = rules::organize(&cfg, &root);
        assert!(report.errors.is_empty(), "organize errores: {:?}", report.errors);
        assert!(!root.join("foto.png").exists());
        assert!(root.join("ZenDesktop/Media/foto.png").exists());

        // 3) Lanzar la app en un hilo con su bucle de mensajes.
        let run_cfg = cfg.clone();
        let run_root = root.clone();
        let app_thread = std::thread::spawn(move || {
            let app = App::launch(run_cfg, run_root.join("zendesktop.toml"), run_root, vec![])
                .expect("launch fallo");
            let mut msg = MSG::default();
            loop {
                let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if status.0 == 0 || status.0 == -1 {
                    break; // WM_QUIT o error irrecuperable
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            app.shutdown();
        });

        // 4) Esperar a la ventana controladora y a la primera caja.
        let mut controller = HWND::default();
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            controller = unsafe { FindWindowW(CLASS_CONTROLLER, None).unwrap_or_default() };
            if !controller.is_invalid() {
                break;
            }
        }
        assert!(!controller.is_invalid(), "no aparece la ventana controladora");

        let mut fence = HWND::default();
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            fence = unsafe { FindWindowW(CLASS_FENCE, None).unwrap_or_default() };
            if !fence.is_invalid() {
                break;
            }
        }
        assert!(!fence.is_invalid(), "no aparece ninguna caja");

        // 5) Redimensionar por el grip (esquina inferior derecha).
        let mut rc = RECT::default();
        unsafe { let _ = GetWindowRect(fence, &mut rc); }
        let (w0, h0) = (rc.right - rc.left, rc.bottom - rc.top);
        assert!(w0 > 0 && h0 > 0, "rect de caja invalido");

        let ok = unsafe { SetCursorPos(rc.right - 8, rc.bottom - 8) };
        assert!(ok.is_ok(), "SetCursorPos fallo (requiere sesion interactiva)");
        post(fence, WM_LBUTTONDOWN, WPARAM(1), packed(w0 - 8, h0 - 8));
        std::thread::sleep(std::time::Duration::from_millis(200));

        let ok = unsafe { SetCursorPos(rc.right - 8 + 90, rc.bottom - 8 + 60) };
        assert!(ok.is_ok());
        post(fence, WM_MOUSEMOVE, WPARAM(0), packed(w0 - 8 + 90, h0 - 8 + 60));
        std::thread::sleep(std::time::Duration::from_millis(200));
        post(fence, WM_LBUTTONUP, WPARAM(0), packed(w0 - 8 + 90, h0 - 8 + 60));
        std::thread::sleep(std::time::Duration::from_millis(300));

        unsafe { let _ = GetWindowRect(fence, &mut rc); }
        let (w1, h1) = (rc.right - rc.left, rc.bottom - rc.top);
        assert!(
            w1 >= w0 + 80 && h1 >= h0 + 50,
            "la caja no se redimensiono: {w0}x{h0} -> {w1}x{h1}"
        );
        println!("RESIZE OK: {w0}x{h0} -> {w1}x{h1}");

        // 6) Cerrar la app: los ficheros organizados deben volver al
        // escritorio y la carpeta interna retirarse si queda vacia.
        post(controller, WM_CLOSE, WPARAM(0), LPARAM(0));
        app_thread.join().expect("hilo de la app termino mal");

        assert!(root.join("foto.png").exists(), "foto.png no volvio");
        assert!(root.join("informe.pdf").exists(), "informe.pdf no volvio");
        assert!(root.join("juego.xyz").exists(), "juego.xyz no volvio");
        assert!(
            !root.join("ZenDesktop").exists(),
            "la carpeta interna no se retiro"
        );
        println!("RESTORE OK: los ficheros volvieron al escritorio");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Verifica el cableado de la vtable OLE escrita a mano: el puntero se
    /// puede consultar como IUnknown e IDropTarget (QI) y DragLeave devuelve
    /// S_OK sin tocar la app. Detecta errores ABI de la vtable.
    #[test]
    #[ignore = "probe vtable"]
    fn drop_target_vtable_probe() {
        unsafe {
            let target = FenceDropTarget {
                vtbl: &DROP_TARGET_VTBL,
                app: std::ptr::null_mut(),
                accepts_files: std::cell::Cell::new(false),
            };
            let i: IDropTarget = IDropTarget::from_raw(&target as *const FenceDropTarget as *mut c_void);
            let _u: windows::core::IUnknown = i.cast().expect("QI IUnknown fallo");
            let _d: IDropTarget = i.cast().expect("QI IDropTarget fallo");
            assert!(i.DragLeave().is_ok(), "DragLeave no devolvio S_OK");
            println!("DROP TARGET VTABLE OK");
        }
    }

    /// Verifica el pipeline del menu nativo sin abrirlo en pantalla:
    /// SHParseDisplayName -> GetUIObjectOf -> QueryContextMenu debe traer
    /// items para un fichero real. No requiere sesion interactiva.
    #[test]
    #[ignore = "probe shell menu"]
    fn shell_menu_unit() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let file = std::env::temp_dir().join("zendesktop_menu_probe.txt");
            std::fs::write(&file, b"x").unwrap();
            let pair = build_shell_menu_for_paths(&[file.clone()]);
            match pair {
                Ok((_menu, hmenu, pidl)) => {
                    let count = GetMenuItemCount(hmenu);
                    let _ = DestroyMenu(hmenu);
                    let _ = CoTaskMemFree(Some(pidl as *const c_void));
                    assert!(count > 0, "el menu nativo debe traer items, trajo {count}");
                    println!("SHELL MENU UNIT OK: {count} items");
                }
                Err(step) => panic!("build_shell_menu_for fallo en {step} para {}", file.display()),
            }
            let _ = std::fs::remove_file(&file);
            CoUninitialize();
        }
    }

    /// Crea un HDROP valido en memoria (DROPFILES + rutas wide) listo para
    /// entregar por WM_DROPFILES; el receptor lo libera con DragFinish.
    fn make_hdrop(file: &std::path::Path) -> HGLOBAL {
        let mut buf: Vec<u8> = Vec::new();
        let mut hdr = [0u8; 20]; // DROPFILES { pFiles, pt, fNC, fWide }
        hdr[0..4].copy_from_slice(&20u32.to_ne_bytes());
        hdr[16] = 1; // fWide = TRUE
        buf.extend_from_slice(&hdr);
        for u in file.to_string_lossy().encode_utf16() {
            buf.extend_from_slice(&u.to_ne_bytes());
        }
        buf.extend_from_slice(&[0, 0, 0, 0]); // doble nulo final
        unsafe {
            let hmem = GlobalAlloc(GMEM_MOVEABLE, buf.len()).expect("GlobalAlloc");
            let ptr = GlobalLock(hmem);
            std::ptr::copy_nonoverlapping(buf.as_ptr(), ptr as *mut u8, buf.len());
            let _ = GlobalUnlock(hmem);
            hmem
        }
    }

    /// Arrastrar y soltar de verdad: se construye un HDROP con un fichero y se
    /// entrega por WM_DROPFILES a la primera caja; el fichero debe aparecer
    /// dentro de la carpeta fisica de esa caja.
    #[test]
    #[ignore = "probe e2e manual"]
    fn fence_drop_probe() {
        let root = temp_desktop("drop");
        let mut cfg = Config::default();
        cfg.general.root_folder = "ZenDesktop".into();
        cfg.general.archive_folder = "Archivo".into();
        cfg.rules = vec![
            Rule {
                title: "Media".into(),
                folder: "Media".into(),
                color: "#7DD3FC".into(),
                extensions: vec!["png".into(), "jpg".into()],
                name_patterns: vec![],
                enabled: true,
                move_files: true,
                include_folders: true,
                ..Rule::default()
            },
        ];
        let report = rules::organize(&cfg, &root);
        assert!(report.errors.is_empty(), "organize errores: {:?}", report.errors);

        let run_cfg = cfg.clone();
        let run_root = root.clone();
        let app_thread = std::thread::spawn(move || {
            // Como main.rs: OLE para que el shell (SHParseDisplayName y el
            // menu nativo) funcione dentro del hilo de interfaz.
            unsafe { let _ = OleInitialize(None); }
            let app = App::launch(run_cfg, run_root.join("zendesktop.toml"), run_root, vec![])
                .expect("launch fallo");
            let mut msg = MSG::default();
            loop {
                let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if status.0 == 0 || status.0 == -1 {
                    break;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            app.shutdown();
            unsafe { OleUninitialize(); }
        });

        let mut fence = HWND::default();
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            fence = unsafe { FindWindowW(CLASS_FENCE, None).unwrap_or_default() };
            if !fence.is_invalid() {
                break;
            }
        }
        assert!(!fence.is_invalid(), "no aparece ninguna caja");

        let file = root.join("suelto.txt");
        std::fs::write(&file, b"x").unwrap();
        let hmem = make_hdrop(&file);
        post(fence, WM_DROPFILES, WPARAM(hmem.0 as usize), LPARAM(0));

        // Esperar a que el watcher/refresh mueva el fichero.
        let mut found = false;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let zen = root.join("ZenDesktop");
            if let Ok(entries) = std::fs::read_dir(&zen) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && p.file_name().map(|n| n.to_string_lossy().ends_with("suelto.txt")).unwrap_or(false) {
                        found = true;
                    }
                    if p.is_dir() && p.join("suelto.txt").exists() {
                        found = true;
                    }
                }
            }
            if found {
                break;
            }
        }
        assert!(found, "el fichero soltado no llego a ninguna carpeta de caja");

        post(
            unsafe { FindWindowW(CLASS_CONTROLLER, None).unwrap_or_default() },
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
        let _ = app_thread.join();
        println!("DROP OK: el fichero soltado entro en la caja");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Clic derecho real sobre un elemento de una caja: debe aparecer el menu
    /// contextual (ventana de clase "#32768") y cerrarse con Escape. Requiere
    /// sesion interactiva (SetCursorPos + mouse sintetico).
    #[test]
    #[ignore = "probe e2e manual"]
    fn shell_menu_probe() {
        let root = temp_desktop("menu");
        let mut cfg = Config::default();
        cfg.general.root_folder = "ZenDesktop".into();
        cfg.general.archive_folder = "Archivo".into();
        cfg.rules = vec![
            Rule {
                title: "Media".into(),
                folder: "Media".into(),
                color: "#7DD3FC".into(),
                extensions: vec!["png".into(), "jpg".into()],
                name_patterns: vec![],
                enabled: true,
                move_files: true,
                include_folders: true,
                ..Rule::default()
            },
        ];
        let _report = rules::organize(&cfg, &root);

        let run_cfg = cfg.clone();
        let run_root = root.clone();
        let app_thread = std::thread::spawn(move || {
            // Como main.rs: OLE para que el shell (SHParseDisplayName y el
            // menu nativo) funcione dentro del hilo de interfaz.
            unsafe { let _ = OleInitialize(None); }
            let app = App::launch(run_cfg, run_root.join("zendesktop.toml"), run_root, vec![])
                .expect("launch fallo");
            let mut msg = MSG::default();
            loop {
                let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if status.0 == 0 || status.0 == -1 {
                    break;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            app.shutdown();
            unsafe { OleUninitialize(); }
        });

        let mut fence = HWND::default();
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            fence = unsafe { FindWindowW(CLASS_FENCE, None).unwrap_or_default() };
            if !fence.is_invalid() {
                break;
            }
        }
        assert!(!fence.is_invalid(), "no aparece ninguna caja");

        let mut rc = RECT::default();
        unsafe { let _ = GetWindowRect(fence, &mut rc); }
        let visible = unsafe { IsWindowVisible(fence).as_bool() };
        assert!(visible, "la caja no esta visible");
        // Elegir un punto del rect que no este tapado por otra ventana (las
        // cajas viven al fondo del orden Z y una ventana del usuario puede
        // cubrirlas): se prueba una rejilla de puntos.
        let mut fx = 0i32;
        let mut fy = 0i32;
        let mut free = false;
        for (dx, dy) in [(30, 40), (30, 64), (64, 40), (12, 12), (24, 88)] {
            let p = POINT { x: rc.left + dx, y: rc.top + dy };
            let hit = unsafe { WindowFromPoint(p) };
            if hit == fence {
                fx = p.x;
                fy = p.y;
                free = true;
                break;
            }
        }
        assert!(free, "ningun punto del rect de la caja esta descubierto: otra ventana la tapa");
        let ok = unsafe { SetCursorPos(fx, fy) };
        assert!(ok.is_ok(), "SetCursorPos fallo (requiere sesion interactiva)");
        unsafe {
            mouse_event(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0);
            mouse_event(MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0);
        }

        let mut popup = HWND::default();
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            popup = unsafe { FindWindowW(w!("#32768"), None).unwrap_or_default() };
            if !popup.is_invalid() {
                break;
            }
        }
        assert!(!popup.is_invalid(), "el clic derecho no abrio el menu contextual");

        // Cerrar el menu con WM_CLOSE directo a la ventana del popup (mas
        // fiable que sintetizar Escape, que depende de la ventana con foco)
        // y esperar a que desaparezca antes de apagar la app.
        let mut closed = false;
        for _ in 0..40 {
            let _ = unsafe { PostMessageW(popup, WM_CLOSE, WPARAM(0), LPARAM(0)) };
            std::thread::sleep(std::time::Duration::from_millis(50));
            let p = unsafe { FindWindowW(w!("#32768"), None).unwrap_or_default() };
            if p.is_invalid() {
                closed = true;
                break;
            }
        }
        assert!(closed, "el menu no se cerro");
        post(
            unsafe { FindWindowW(CLASS_CONTROLLER, None).unwrap_or_default() },
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
        let _ = app_thread.join();
        println!("SHELL MENU E2E OK: menu nativo abierto y cerrado");
        let _ = std::fs::remove_dir_all(&root);
    }
}
