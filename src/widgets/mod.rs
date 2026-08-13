//! ZenDesktop :: widgets/mod.rs
//!
//! Framework de widgets programables en Lua. Cada caja "widget" ejecuta un
//! script `.lua` que dibuja mediante un conjunto reducido de comandos
//! (rectangulos, texto, barras de progreso) rellenados por el host con D2D.
//! El sandbox expone un `ctx` con metodos de dibujo; las APIs de datos
//! (`http`, `json`, `storage`, `app`, `crypto`) llegan en la fase de Spotify.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, UserData, UserDataMethods};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SW_SHOWNORMAL, WM_APP,
};

/// Widgets de ejemplo empaquetados en el binario. Se instalan en la carpeta de
/// config del usuario la primera vez que arranca sin scripts (aparecen en
/// Configuracion -> Widgets como si los hubiera creado el usuario).
const BUNDLED_EXAMPLES: &[(&str, &str)] = &[
    ("clock", include_str!("../../widgets/clock.lua")),
    ("notas", include_str!("../../widgets/notas.lua")),
    ("clima", include_str!("../../widgets/clima.lua")),
    ("contador", include_str!("../../widgets/contador.lua")),
];

/// true si la carpeta contiene algun script `.lua`.
fn has_any_lua(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten().any(|e| {
                e.path().extension().and_then(|x| x.to_str()) == Some("lua")
            })
        })
        .unwrap_or(false)
}

/// Instala los ejemplos empaquetados en la carpeta de widgets del usuario:
///
/// * carpeta vacia -> se instalan todos;
/// * archivos de una version retirada del bundle (marcados con `CONFIG`) ->
///   se restauran a la version actual;
/// * scripts propios del usuario (o ejemplos editados) -> no se tocan.
/// * carpeta con solo ejemplos del bundle -> tambien se instalan los ejemplos
///   nuevos que falten (sin pisar nada).
///
/// Devuelve cuantos archivos se escribieron.
pub fn install_bundled_examples(dir: &Path) -> usize {
    if std::fs::create_dir_all(dir).is_err() {
        return 0;
    }
    let empty = !has_any_lua(dir);
    // ¿Todos los scripts presentes son del bundle actual? Si es asi, la
    // carpeta es "de ejemplos" y se pueden añadir ejemplos nuevos.
    let bundled_names: Vec<String> = BUNDLED_EXAMPLES.iter().map(|(n, _)| format!("{n}.lua")).collect();
    let all_bundled = std::fs::read_dir(dir)
        .map(|it| {
            it.flatten().all(|e| {
                e.path().extension().and_then(|x| x.to_str()) != Some("lua")
                    || e.file_name().to_string_lossy().ends_with(".lua")
                        && bundled_names.iter().any(|b| &e.file_name().to_string_lossy() == b)
            })
        })
        .unwrap_or(empty);
    let mut installed = 0;
    for (name, source) in BUNDLED_EXAMPLES {
        let path = dir.join(format!("{name}.lua"));
        let current = std::fs::read_to_string(&path).ok();
        let action = match current {
            // No existe: solo se crea si la carpeta no tiene scripts ajenos.
            None => empty || all_bundled,
            // Version retirada de la fase CONFIG (config.lat ya no existe).
            Some(c) => c != *source && c.contains("CONFIG"),
        };
        if action && std::fs::write(&path, source).is_ok() {
            installed += 1;
        }
    }
    installed
}

/// Comando de dibujo emitido por un widget y rellenado por el host con D2D.
/// `color` es un ARGB empaquetado en u32 (0xAARRGGBB).
#[derive(Debug, Clone)]
pub enum DrawCmd {
    FillRect { x: f32, y: f32, w: f32, h: f32, color: u32 },
    Text { x: f32, y: f32, text: String, size: f32, color: u32 },
    Progress { x: f32, y: f32, w: f32, h: f32, value: f32, color: u32, bg: u32 },
    /// Segmento de linea con grosor en DIP.
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: u32 },
    /// Elipse rellena (cx, cy = centro, r = radio horizontal/vertical).
    Circle { cx: f32, cy: f32, r: f32, color: u32 },
    /// Borde de elipse (mismo espacio que `Circle`).
    CircleStroke { cx: f32, cy: f32, r: f32, width: f32, color: u32 },
    /// Rectangulo redondeado relleno con borde opcional (width 0 = sin borde).
    RoundRect { x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32, border_width: f32, border_color: u32 },
    /// Texto centrado horizontalmente en `x` (anchura real medida por el host).
    TextCenter { x: f32, y: f32, text: String, size: f32, color: u32 },
    /// Texto alineado a la derecha en `x` (x = borde derecho).
    TextRight { x: f32, y: f32, text: String, size: f32, color: u32 },
    /// Imagen descargada de una URL (asincrona + cache); placeholder mientras carga.
    Image { url: String, x: f32, y: f32, w: f32, h: f32 },
}

/// Contexto de dibujo expuesto al script como `ctx`. Los metodos empujan
/// comandos a un buffer compartido (`Rc<RefCell<..>>`) que el host lee al
/// terminar `render`.
struct Ctx {
    w: f32,
    h: f32,
    cmds: Rc<RefCell<Vec<DrawCmd>>>,
}

impl UserData for Ctx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("width", |_, this, (): ()| Ok(this.w as f64));
        methods.add_method("height", |_, this, (): ()| Ok(this.h as f64));
        methods.add_method("now_ms", |_, _this, (): ()| Ok(now_ms() as f64));
        methods.add_method(
            "fill_rect",
            |_, this, (x, y, w, h, color): (f64, f64, f64, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::FillRect {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "text",
            |_, this, (x, y, text, size, color): (f64, f64, String, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::Text {
                    x: x as f32,
                    y: y as f32,
                    text,
                    size: size as f32,
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "progress",
            |_, this, (x, y, w, h, value, color): (f64, f64, f64, f64, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::Progress {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    value: (value as f32).clamp(0.0, 1.0),
                    color: color as u32,
                    bg: 0x22FFFFFF,
                });
                Ok(())
            },
        );
        methods.add_method(
            "line",
            |_, this, (x1, y1, x2, y2, width, color): (f64, f64, f64, f64, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::Line {
                    x1: x1 as f32,
                    y1: y1 as f32,
                    x2: x2 as f32,
                    y2: y2 as f32,
                    width: (width as f32).max(0.5),
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "circle",
            |_, this, (cx, cy, r, color): (f64, f64, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::Circle {
                    cx: cx as f32,
                    cy: cy as f32,
                    r: (r as f32).max(0.0),
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "circle_stroke",
            |_, this, (cx, cy, r, width, color): (f64, f64, f64, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::CircleStroke {
                    cx: cx as f32,
                    cy: cy as f32,
                    r: (r as f32).max(0.0),
                    width: (width as f32).max(0.5),
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "round_rect",
            |_, this,
             (x, y, w, h, radius, color, border_width, border_color): (
                f64, f64, f64, f64, f64, f64, f64, f64,
            )| {
                this.cmds.borrow_mut().push(DrawCmd::RoundRect {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    radius: (radius as f32).max(0.0),
                    color: color as u32,
                    border_width: (border_width as f32).max(0.0),
                    border_color: border_color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "text_center",
            |_, this, (x, y, text, size, color): (f64, f64, String, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::TextCenter {
                    x: x as f32,
                    y: y as f32,
                    text,
                    size: size as f32,
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "text_right",
            |_, this, (x, y, text, size, color): (f64, f64, String, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::TextRight {
                    x: x as f32,
                    y: y as f32,
                    text,
                    size: size as f32,
                    color: color as u32,
                });
                Ok(())
            },
        );
        methods.add_method(
            "image",
            |_, this, (url, x, y, w, h): (String, f64, f64, f64, f64)| {
                this.cmds.borrow_mut().push(DrawCmd::Image {
                    url,
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                });
                Ok(())
            },
        );
    }
}

// -- API `http` (descargas cacheadas en segundo plano) ----------------------
//
// Los scripts llaman a `http:get(url)` / `http:get_json(url)`. La primera
// llamada con la cache fria devuelve `nil` y lanza la descarga en un hilo de
// trabajo; al terminar, el hilo publica `WM_ZEN_WIDGET_HTTP` a la ventana
// controladora y la app repinta los widgets (el script vera los datos al
// ejecutar `render` de nuevo). La UI nunca se bloquea.

/// Tiempo de vida de las respuestas cacheadas (5 min).
const HTTP_TTL: Duration = Duration::from_secs(300);

/// Caché de respuestas del sandbox: URL -> (cuerpo, momento de descarga).
/// `Option` para inicializacion perezosa (HashMap::new no es const).
static HTTP_CACHE: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);
/// URLs con una descarga en vuelo (evita lanzar varias veces la misma).
static HTTP_IN_FLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// HWND crudo de la ventana controladora (los handles no son `Send`).
static WIDGET_CTRL: AtomicIsize = AtomicIsize::new(0);

/// Mensaje que publica el hilo de descarga al terminar (coincide con
/// `WM_ZEN_WIDGET_HTTP` manejado en ui.rs).
pub const WM_ZEN_WIDGET_HTTP: u32 = WM_APP + 0x1A;

/// Registra la ventana controladora para poder repintar los widgets cuando
/// termina una descarga en segundo plano.
pub fn set_widget_controller(ctrl: isize) {
    WIDGET_CTRL.store(ctrl, Ordering::SeqCst);
}

/// Descarga sincrona (solo se ejecuta en el hilo de trabajo) con timeout.
fn http_fetch(url: &str) -> Option<String> {
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(8))
        .call()
        .ok()?;
    resp.into_string().ok()
}

/// Devuelve el cuerpo cacheado si es reciente; si esta frio, lanza la
/// descarga en segundo plano y devuelve `None` (el widget muestra "cargando").
fn http_get_cached(url: &str) -> Option<String> {
    let now = Instant::now();
    {
        let mut cache = HTTP_CACHE.lock().unwrap();
        let cache = cache.get_or_insert_with(HashMap::new);
        if let Some((body, at)) = cache.get(url) {
            if now.duration_since(*at) < HTTP_TTL {
                return Some(body.clone());
            }
        }
    }
    let ctrl = WIDGET_CTRL.load(Ordering::SeqCst);
    if ctrl != 0 {
        let mut in_flight = HTTP_IN_FLIGHT.lock().unwrap();
        let in_flight = in_flight.get_or_insert_with(HashSet::new);
        if !in_flight.contains(url) {
            in_flight.insert(url.to_string());
            let url = url.to_string();
            thread::spawn(move || {
                if let Some(body) = http_fetch(&url) {
                    HTTP_CACHE
                        .lock()
                        .unwrap()
                        .get_or_insert_with(HashMap::new)
                        .insert(url.clone(), (body, Instant::now()));
                }
                HTTP_IN_FLIGHT
                    .lock()
                    .unwrap()
                    .get_or_insert_with(HashSet::new)
                    .remove(&url);
                let _ = unsafe {
                    PostMessageW(
                        HWND(ctrl as *mut c_void),
                        WM_ZEN_WIDGET_HTTP,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            });
        }
    }
    None
}

/// Convierte un `serde_json::Value` en un valor Lua (tablas para objetos y
/// arrays, numeros, cadenas y booleanos; null -> nil).
fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Value {
    match v {
        serde_json::Value::Null => mlua::Value::Nil,
        serde_json::Value::Bool(b) => mlua::Value::Boolean(*b),
        serde_json::Value::Number(n) => mlua::Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => match lua.create_string(s.as_str()) {
            Ok(ss) => mlua::Value::String(ss),
            Err(_) => mlua::Value::Nil,
        },
        serde_json::Value::Array(arr) => {
            if let Ok(t) = lua.create_table() {
                for (i, item) in arr.iter().enumerate() {
                    let _ = t.set(i + 1, json_to_lua(lua, item));
                }
                mlua::Value::Table(t)
            } else {
                mlua::Value::Nil
            }
        }
        serde_json::Value::Object(obj) => {
            if let Ok(t) = lua.create_table() {
                for (k, val) in obj {
                    let _ = t.set(k.as_str(), json_to_lua(lua, val));
                }
                mlua::Value::Table(t)
            } else {
                mlua::Value::Nil
            }
        }
    }
}

/// API `http` expuesta como global `http` a todos los scripts.
struct HttpApi;

impl UserData for HttpApi {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // `http:get(url)` -> cadena cruda (o nil mientras se descarga).
        methods.add_method("get", |_, _, url: String| {
            Ok(http_get_cached(&url))
        });
        // `http:get_json(url)` -> tabla Lua (o nil mientras se descarga).
        methods.add_method("get_json", |lua, _, url: String| {
            let body = http_get_cached(&url);
            match body {
                Some(b) => {
                    let v: serde_json::Value =
                        serde_json::from_str(&b).unwrap_or(serde_json::Value::Null);
                    Ok(json_to_lua(lua, &v))
                }
                None => Ok(mlua::Value::Nil),
            }
        });
    }
}

// -- API `app` (acciones del sistema) ---------------------------------------
//
// `app:open(ruta)` abre un archivo/programa con su aplicacion por defecto;
// `app:notify(msg)` muestra un toast de la app; `app:version()` la version.
// Los scripts lo usan para widgets lanzadores o avisos.

/// Mensaje que pide a la app mostrar un toast (lo maneja ui.rs).
pub const WM_ZEN_WIDGET_TOAST: u32 = WM_APP + 0x1B;

/// Ultimo mensaje pedido por un widget (hilo UI -> UI: el script ya corre en
/// el hilo de interfaz, asi que el valor se copia sin condicion de carrera).
static WIDGET_TOAST: Mutex<Option<String>> = Mutex::new(None);

/// Deja el toast pedido por el ultimo widget listo para que lo consuma
/// `controller_proc` al recibir `WM_ZEN_WIDGET_TOAST`.
pub fn take_widget_toast() -> Option<String> {
    WIDGET_TOAST.lock().unwrap().take()
}

struct AppApi;

impl UserData for AppApi {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // `app:open(ruta)` -> abre con la aplicacion por defecto (ShellExecute).
        methods.add_method("open", |_, _, path: String| {
            let wide = crate::config::wide(&path);
            unsafe {
                let _ = ShellExecuteW(
                    None,
                    w!("open"),
                    PCWSTR(wide.as_ptr()),
                    None,
                    None,
                    SW_SHOWNORMAL,
                );
            }
            Ok(())
        });
        // `app:notify(mensaje)` -> toast de la app (no bloquea, no modal).
        methods.add_method("notify", |_, _, msg: String| {
            *WIDGET_TOAST.lock().unwrap() = Some(msg);
            let ctrl = WIDGET_CTRL.load(Ordering::SeqCst);
            if ctrl != 0 {
                let _ = unsafe {
                    PostMessageW(
                        HWND(ctrl as *mut c_void),
                        WM_ZEN_WIDGET_TOAST,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            }
            Ok(())
        });
        // `app:version()` -> version de la app (para widgets con comportamiento
        // distinto segun la version instalada).
        methods.add_method("version", |_, _, (): ()| {
            Ok(env!("CARGO_PKG_VERSION").to_string())
        });
    }
}

// -- API `image` (descargas de imagenes cacheadas) ---------------------------
//
// `ctx:image(url, x, y, w, h)` pinta una imagen descargada por URL. La
// descarga y decodificacion ocurre en un hilo de trabajo (como `http`); con la
// cache fria el host pinta un placeholder y repinta cuando llega la imagen.

/// Tamano maximo del cache de imagenes (evita crecer sin limite).
const IMAGE_CACHE_MAX: usize = 32;
/// Lado maximo en px: imagenes mayores se ignoran (acota memoria por imagen).
const IMAGE_MAX_SIDE: u32 = 1024;

/// Pixeles decodificados (BGRA premultiplicado) listos para crear un bitmap D2D.
pub struct ImagePixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Caché URL -> (imagen, momento). `Option` por inicializacion perezosa.
static IMAGE_CACHE: Mutex<Option<HashMap<String, (ImagePixels, Instant)>>> = Mutex::new(None);

/// Descarga y decodifica una imagen en el hilo de trabajo. Devuelve `None` si
/// la imagen es invalida o excede el limite de tamano.
fn image_fetch(url: &str) -> Option<ImagePixels> {
    use image::GenericImageView;
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(8))
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().read_to_end(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 || width > IMAGE_MAX_SIDE || height > IMAGE_MAX_SIDE {
        return None;
    }
    let rgba = img.to_rgba8();
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for p in rgba.pixels() {
        // Premultiplicado para poder usar D2D1_ALPHA_MODE_PREMULTIPLIED.
        let a = p[3] as u32;
        bgra.extend_from_slice(&[
            (p[2] as u32 * a / 255) as u8,
            (p[1] as u32 * a / 255) as u8,
            (p[0] as u32 * a / 255) as u8,
            p[3],
        ]);
    }
    Some(ImagePixels { width, height, bgra })
}

/// Pixeles de la imagen `url` si estan en cache (fresca). Si la cache esta
/// fria, lanza la descarga en segundo plano y devuelve `None`.
pub fn image_pixels(url: &str) -> Option<ImagePixels> {
    let now = Instant::now();
    {
        let mut cache = IMAGE_CACHE.lock().unwrap();
        let cache = cache.get_or_insert_with(HashMap::new);
        if let Some((img, at)) = cache.get(url) {
            if now.duration_since(*at) < HTTP_TTL {
                return Some(ImagePixels {
                    width: img.width,
                    height: img.height,
                    bgra: img.bgra.clone(),
                });
            }
        }
    }
    // Descarga en vuelo compartida con `http` (misma red, mismo repintado).
    let ctrl = WIDGET_CTRL.load(Ordering::SeqCst);
    if ctrl != 0 {
        let mut in_flight = HTTP_IN_FLIGHT.lock().unwrap();
        let in_flight = in_flight.get_or_insert_with(HashSet::new);
        if !in_flight.contains(url) {
            in_flight.insert(url.to_string());
            let url = url.to_string();
            thread::spawn(move || {
                if let Some(img) = image_fetch(&url) {
                    let mut cache = IMAGE_CACHE.lock().unwrap();
                    let cache = cache.get_or_insert_with(HashMap::new);
                    cache.insert(url.clone(), (img, Instant::now()));
                    // Eviccion simple por antiguedad cuando se pasa del tope.
                    if cache.len() > IMAGE_CACHE_MAX {
                        let mut entries: Vec<(Instant, String)> =
                            cache.iter().map(|(k, (_, at))| (*at, k.clone())).collect();
                        entries.sort_by_key(|(at, _)| *at);
                        for (_, k) in entries.iter().take(cache.len() - IMAGE_CACHE_MAX) {
                            cache.remove(k);
                        }
                    }
                }
                HTTP_IN_FLIGHT
                    .lock()
                    .unwrap()
                    .get_or_insert_with(HashSet::new)
                    .remove(&url);
                let _ = unsafe {
                    PostMessageW(
                        HWND(ctrl as *mut c_void),
                        WM_ZEN_WIDGET_HTTP,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            });
        }
    }
    None
}

/// Widget cargado y listo para dibujar. Guarda su `Lua` vivo mientras exista.
pub struct Widget {
    pub name: String,
    pub title: String,
    lua: Lua,
}

impl Widget {
    /// Carga un script Lua desde su codigo fuente. El script define opcionales
    /// `WIDTH`, `HEIGHT`, `TITLE`, una funcion `render(ctx)` y, opcionalmente,
    /// `click(x, y)` (llamada al pulsar dentro del cuerpo del widget).
    pub fn load(name: &str, source: &str) -> Result<Widget, String> {
        let lua = Lua::new();
        // API de datos: descargas cacheadas en segundo plano.
        lua.globals()
            .set("http", HttpApi)
            .map_err(|e| format!("{name}: {e}"))?;
        // API del sistema: abrir archivos, toasts, version.
        lua.globals()
            .set("app", AppApi)
            .map_err(|e| format!("{name}: {e}"))?;
        // Estado persistente: tabla `state` compartida entre renders y clics
        // (vive mientras el widget este cargado). Debe existir ANTES de
        // ejecutar el chunk: los scripts pueden leerla en codigo de nivel
        // superior (p.ej. inicializar el contador).
        let state = lua
            .create_table()
            .map_err(|e| format!("{name}: {e}"))?;
        lua.globals()
            .set("state", state.clone())
            .map_err(|e| format!("{name}: {e}"))?;

        lua.load(source)
            .set_name(name)
            .exec()
            .map_err(|e| format!("{name}: {e}"))?;

        let globals = lua.globals();
        let title = globals
            .get::<String>("TITLE")
            .unwrap_or_else(|_| name.to_string());

        Ok(Widget {
            name: name.to_string(),
            title,
            lua,
        })
    }

    /// Notifica un clic en el cuerpo del widget: coordenadas DIP relativas al
    /// cuerpo (0,0 = esquina superior izquierda bajo la cabecera) y tamano del
    /// cuerpo (w, h) para que el script pueda hacer hit-testing correcto
    /// aunque el usuario redimensione la caja.
    pub fn handle_click(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if let Ok(click_fn) = self.lua.globals().get::<Function>("click") {
            let _ = click_fn.call::<()>((x as f64, y as f64, w as f64, h as f64));
        }
    }

    /// Ejecuta `render(ctx)` y devuelve los comandos de dibujo emitidos.
    pub fn render(&mut self, w: f32, h: f32) -> Vec<DrawCmd> {
        let cmds = Rc::new(RefCell::new(Vec::new()));
        let ctx = Ctx {
            w,
            h,
            cmds: cmds.clone(),
        };
        let ud = match self.lua.create_userdata(ctx) {
            Ok(u) => u,
            Err(_) => return Vec::new(),
        };
        if self.lua.globals().set("ctx", ud.clone()).is_err() {
            return Vec::new();
        }
        if let Ok(render_fn) = self.lua.globals().get::<Function>("render") {
            let _ = render_fn.call::<()>(ud);
        }
        let out = cmds.borrow().clone();
        out
    }
}

/// Registro de widgets cargados desde una carpeta de scripts `.lua`.
pub struct WidgetHost {
    widgets: Vec<Widget>,
}

impl WidgetHost {
    /// Carga todos los scripts `*.lua` de `dir` (los que no compilan se omiten).
    pub fn load_dir(dir: &Path) -> Self {
        let mut widgets = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                    continue;
                };
                if name.is_empty() {
                    continue;
                }
                if let Ok(source) = std::fs::read_to_string(&path) {
                    if let Ok(widget) = Widget::load(&name, &source) {
                        widgets.push(widget);
                    }
                }
            }
        }
        widgets.sort_by(|a, b| a.name.cmp(&b.name));
        WidgetHost { widgets }
    }

    pub fn get(&self, name: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|w| w.name == name)
    }
}

/// Milisegundos desde el arranque de la app (para animaciones dentro del script).
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: &str = r#"
WIDTH = 240
HEIGHT = 120
TITLE = "Reloj"

function render(ctx)
    ctx:fill_rect(0, 0, ctx:width(), ctx:height(), 0x22000000)
    ctx:text(10, 10, "Hola", 16, 0xFFFFFFFF)
    ctx:progress(10, 40, 220, 6, 0.5, 0xFF00FF00)
end
"#;

    #[test]
    fn loads_metadata_and_renders_commands() {
        let mut w = Widget::load("clock", CLOCK).unwrap();
        assert_eq!(w.title, "Reloj");

        let cmds = w.render(240.0, 120.0);
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[0], DrawCmd::FillRect { .. }));
        assert!(matches!(cmds[1], DrawCmd::Text { .. }));
        assert!(matches!(cmds[2], DrawCmd::Progress { .. }));
    }

    #[test]
    fn ignores_invalid_scripts() {
        assert!(Widget::load("bad", "this is not valid lua !!!").is_err());
    }

    #[test]
    fn notas_example_script_renders() {
        // Carga el script real de ejemplo para validar que sigue compilando
        // y dibujando con la API actual del sandbox.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/widgets/notas.lua");
        let source = std::fs::read_to_string(path).expect("widgets/notas.lua missing");
        let mut w = Widget::load("notas", &source).unwrap();
        let cmds = w.render(260.0, 230.0);
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::Text { .. })));
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::Progress { .. })));
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::FillRect { .. })));
    }

    #[test]
    fn clima_example_script_renders_without_network() {
        // Con la cache fria y sin controladora, `http:get_json` devuelve nil
        // y el script debe dibujar su estado "cargando" sin romper.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/widgets/clima.lua");
        let source = std::fs::read_to_string(path).expect("widgets/clima.lua missing");
        let mut w = Widget::load("clima", &source).unwrap();
        let cmds = w.render(260.0, 160.0);
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::Text { .. })));
    }

    #[test]
    fn bundled_examples_are_installed_when_dir_empty() {
        let dir = std::env::temp_dir().join(format!("zdt_widgets_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let n = install_bundled_examples(&dir);
        assert_eq!(n, 4);
        assert!(dir.join("clock.lua").exists());
        assert!(dir.join("notas.lua").exists());
        assert!(dir.join("clima.lua").exists());
        assert!(dir.join("contador.lua").exists());
        // Ya instalados: no se vuelven a escribir.
        assert_eq!(install_bundled_examples(&dir), 0);
        // Un widget del usuario no impide mantener los ejemplos al dia.
        let _ = std::fs::write(
            dir.join("mio.lua"),
            "TITLE = \"Mio\"\nfunction render(ctx) end\n",
        );
        assert_eq!(install_bundled_examples(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_bundled_examples_installed_when_folder_only_has_bundled() {
        // Carpeta con solo ejemplos del bundle: los ejemplos nuevos que falten
        // se instalan (sin pisar los existentes).
        let dir = std::env::temp_dir().join(format!("zdt_widgets_test4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let clock = include_str!("../../widgets/clock.lua");
        let _ = std::fs::write(dir.join("clock.lua"), clock);
        let n = install_bundled_examples(&dir);
        assert_eq!(n, 3); // notas, clima y contador
        assert!(dir.join("contador.lua").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_persists_across_renders() {
        let src = r#"
function render(ctx)
    if state.count == nil then state.count = 0 end
    state.count = state.count + 1
    ctx:text(5, 5, tostring(state.count), 12, 0xFFFFFFFF)
end
"#;
        let mut w = Widget::load("state", src).unwrap();
        w.render(100.0, 50.0);
        w.render(100.0, 50.0);
        let cmds = w.render(100.0, 50.0);
        let DrawCmd::Text { text, .. } = &cmds[0] else {
            panic!("se esperaba un Text");
        };
        assert_eq!(text, "3");
    }

    #[test]
    fn click_calls_script_callback_and_mutates_state() {
        let src = r#"
state.pulsado = 0
function render(ctx)
    ctx:text(5, 5, tostring(state.pulsado), 12, 0xFFFFFFFF)
end
function click(x, y, w, h)
    state.pulsado = state.pulsado + 1
end
"#;
        let mut w = Widget::load("click", src).unwrap();
        w.handle_click(10.0, 20.0, 100.0, 50.0);
        w.handle_click(30.0, 40.0, 100.0, 50.0);
        let cmds = w.render(100.0, 50.0);
        let DrawCmd::Text { text, .. } = &cmds[0] else {
            panic!("se esperaba un Text");
        };
        assert_eq!(text, "2");
    }

    #[test]
    fn new_draw_commands_are_emitted() {
        let src = r#"
function render(ctx)
    ctx:line(0, 0, 10, 10, 2, 0xFFFFFFFF)
    ctx:circle(5, 5, 3, 0xFF0000FF)
    ctx:circle_stroke(5, 5, 4, 1, 0xFF00FF00)
    ctx:round_rect(1, 1, 9, 9, 2, 0xFF112233, 1, 0xFF445566)
    ctx:text_center(50, 2, "mid", 12, 0xFFFFFFFF)
    ctx:text_right(100, 2, "end", 12, 0xFFFFFFFF)
    ctx:image("https://x/y.png", 0, 0, 16, 16)
end
"#;
        let mut w = Widget::load("prims", src).unwrap();
        let cmds = w.render(100.0, 50.0);
        assert_eq!(cmds.len(), 7);
        assert!(matches!(cmds[0], DrawCmd::Line { width: 2.0, .. }));
        assert!(matches!(cmds[1], DrawCmd::Circle { r: 3.0, color: 0xFF0000FF, .. }));
        assert!(matches!(cmds[2], DrawCmd::CircleStroke { .. }));
        assert!(matches!(
            cmds[3],
            DrawCmd::RoundRect { border_width: 1.0, .. }
        ));
        assert!(matches!(cmds[4], DrawCmd::TextCenter { .. }));
        assert!(matches!(cmds[5], DrawCmd::TextRight { .. }));
        assert!(matches!(
            cmds[6],
            DrawCmd::Image { ref url, .. } if url == "https://x/y.png"
        ));
    }

    #[test]
    fn contador_example_renders_and_responds_to_clicks() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/widgets/contador.lua");
        let source = std::fs::read_to_string(path).expect("widgets/contador.lua missing");
        let mut w = Widget::load("contador", &source).unwrap();
        let cmds = w.render(240.0, 150.0);
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::RoundRect { .. })));
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::TextCenter { .. })));
        assert!(cmds.iter().any(|c| matches!(c, DrawCmd::Line { .. })));
        // Boton "+1": x en [18, 108], y en [84, 118] (caja de 240 de ancho).
        w.handle_click(40.0, 100.0, 240.0, 150.0);
        w.handle_click(40.0, 100.0, 240.0, 150.0);
        let texts = |w: &mut Widget| {
            let cmds = w.render(240.0, 150.0);
            cmds.iter()
                .filter_map(|c| match c {
                    DrawCmd::TextCenter { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<String>>()
        };
        let t = texts(&mut w);
        assert!(t.contains(&"2".to_string()), "2 clics deben mostrar 2: {t:?}");
        // Reset: x en [132, 222], y en [84, 118] -> vuelve a 0.
        w.handle_click(180.0, 100.0, 240.0, 150.0);
        let t = texts(&mut w);
        assert!(
            !t.contains(&"2".to_string()),
            "el Reset debe poner el contador a 0: {t:?}"
        );
        assert!(
            t.iter().any(|s| s.contains("Pulsa para contar")),
            "con el contador a 0 se muestra la bienvenida: {t:?}"
        );
    }

    #[test]
    fn image_cache_cold_returns_none_without_controller() {
        // Sin ventana controladora (WIDGET_CTRL = 0) no se lanza ninguna
        // descarga: devuelve None y no bloquea ni rompe.
        assert_eq!(WIDGET_CTRL.load(Ordering::SeqCst), 0);
        assert!(image_pixels("https://example.invalid/never.png").is_none());
    }

    #[test]
    fn legacy_config_examples_are_repaired() {
        let dir = std::env::temp_dir().join(format!("zdt_widgets_test2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        // Un clima viejo de la fase CONFIG (config.lat ya no existe).
        let legacy = "TITLE = \"Clima\"\nCONFIG = { { key = \"lat\", type = \"number\", default = 40.42 } }\nfunction render(ctx) ctx:text(1, 1, config.lat, 10, 0xFFFFFFFF) end\n";
        let _ = std::fs::write(dir.join("clima.lua"), legacy);
        let n = install_bundled_examples(&dir);
        // Repara el clima legacy Y instala los demas ejemplos que faltan
        // (la carpeta solo tiene nombres del bundle).
        assert_eq!(n, 4);
        let fixed = std::fs::read_to_string(dir.join("clima.lua")).unwrap();
        assert!(!fixed.contains("CONFIG"));
        assert!(fixed.contains("http:get_json"));
        // Un script de usuario (sin CONFIG) no se toca.
        let _ = std::fs::write(
            dir.join("mio.lua"),
            "TITLE = \"Mio\"\nfunction render(ctx) ctx:text(1, 1, \"hola\", 10, 0xFFFFFFFF) end\n",
        );
        assert_eq!(install_bundled_examples(&dir), 0);
        let mio = std::fs::read_to_string(dir.join("mio.lua")).unwrap();
        assert!(mio.contains("hola"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_api_returns_nil_when_cache_cold() {
        let lua = Lua::new();
        let globals = lua.globals();
        globals.set("http", HttpApi).unwrap();
        lua.load(
            r#"
            function probe()
                local data = http:get_json("https://example.invalid/never")
                return data == nil
            end
            "#,
        )
        .exec()
        .unwrap();
        let probe: mlua::Function = globals.get("probe").unwrap();
        assert!(probe.call::<bool>(()).unwrap());
    }
}
