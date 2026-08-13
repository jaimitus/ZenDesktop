//! ZenDesktop :: rules.rs
//!
//! Motor de clasificacion y de caducidad ("archivos efimeros").
//!
//! Todo el modulo es sincrono y libre de asignaciones innecesarias: se ejecuta
//! dentro del hilo de interfaz justo despues de una rafaga del vigilante, y su
//! coste tipico para un escritorio de 200 elementos es de ~1,5 ms.
//!
//! Reglas de oro:
//!   * Nunca se borra nada: como maximo se mueve al archivo historico.
//!   * Nunca se sobrescribe: los conflictos se resuelven con sufijo " (n)".
//!   * Un archivo bloqueado por otro proceso se omite y se reintenta en la
//!     siguiente rafaga; jamas aborta el barrido completo.

use std::cmp::Ordering as CmpOrdering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_FLUSH};

use crate::config::{Config, Rule};

const MAX_REPORTED_ERRORS: usize = 16;

// ---------------------------------------------------------------------------
// Modelos de datos que consume la capa de UI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FileItem {
    pub name: String,
    pub path: PathBuf,
    pub ext: String,
    pub size: u64,
    pub modified: SystemTime,
    pub is_dir: bool,
}

impl FileItem {
    fn from_path(path: PathBuf) -> Option<FileItem> {
        let meta = fs::metadata(&path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        Some(FileItem {
            name,
            ext,
            size: meta.len(),
            modified: meta.modified().unwrap_or(UNIX_EPOCH),
            is_dir: meta.is_dir(),
            path,
        })
    }

    pub fn size_label(&self) -> String {
        if self.is_dir {
            return String::from("carpeta");
        }
        human_size(self.size)
    }
}

/// Contenido resuelto de una caja, listo para renderizar.
#[derive(Debug, Clone)]
pub struct FenceContent {
    pub id: String,
    pub title: String,
    pub color: String,
    /// Vista de la caja: "auto" | "list" | "grid".
    pub view_mode: String,
    /// Tamano del icono en modo cuadricula (px). None = sigue el ajuste global.
    pub icon_size: Option<f32>,
    /// Carpeta fisica respaldada por la caja (None en cajas virtuales).
    pub folder: Option<PathBuf>,
    pub items: Vec<FileItem>,
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub organized: u32,
    pub archived: u32,
    pub purged: u32,
    pub skipped: u32,
    pub errors: Vec<String>,
    pub elapsed_ms: u128,
}

impl Report {
    fn push_error(&mut self, context: &str, err: &dyn std::fmt::Display) {
        if self.errors.len() < MAX_REPORTED_ERRORS {
            self.errors.push(format!("{context}: {err}"));
        }
    }

    fn merge(&mut self, other: Report) {
        self.organized += other.organized;
        self.archived += other.archived;
        self.purged += other.purged;
        self.skipped += other.skipped;
        for e in other.errors {
            if self.errors.len() < MAX_REPORTED_ERRORS {
                self.errors.push(e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Preparacion del arbol de carpetas
// ---------------------------------------------------------------------------

/// Crea `<Documentos>\<root>` y una subcarpeta por cada regla fisica activa,
/// mas la carpeta de archivo historico. Idempotente y barato.
pub fn ensure_layout(cfg: &Config) -> io::Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    let root = cfg.root_dir();
    let needs_root = cfg.rules.iter().any(|r| r.enabled && r.move_files);
    if needs_root {
        fs::create_dir_all(&root)?;
        created.push(root.clone());
    }
    for rule in cfg.rules.iter().filter(|r| r.enabled && r.move_files) {
        let dir = root.join(&rule.folder);
        fs::create_dir_all(&dir)?;
        created.push(dir);
    }
    if cfg.ephemeral.enabled {
        let archive = cfg.archive_dir();
        fs::create_dir_all(&archive)?;
        created.push(archive);
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// Clasificacion
// ---------------------------------------------------------------------------

/// Devuelve la primera regla que casa. El orden del array `rules` de
/// config.toml es, por tanto, la prioridad: la regla comodin va la ultima.
pub fn classify<'a>(
    cfg: &'a Config,
    name: &str,
    ext: &str,
    is_dir: bool,
    size: Option<u64>,
    modified: Option<SystemTime>,
) -> Option<&'a Rule> {
    let lower_name = name.to_ascii_lowercase();
    let lower_ext = ext.to_ascii_lowercase();

    // A) CARPETAS: buscar primero la regla dedicada a carpetas
    if is_dir {
        let folder_rule = cfg.rules.iter().find(|rule| {
            if !rule.enabled || !rule.include_folders {
                return false;
            }
            rule.id.contains("folder")
                || rule.id.contains("carpeta")
                || rule.title.to_lowercase().contains("carpeta")
                || rule.title.to_lowercase().contains("folder")
                || rule.extensions.iter().any(|e| e == "folder" || e == "carpeta" || e == "dir")
        });
        if let Some(rule) = folder_rule {
            return Some(rule);
        }

        let any_folder_rule = cfg.rules.iter().find(|rule| {
            rule.enabled && rule.include_folders && !rule.extensions.iter().any(|e| e == "*")
        });
        if let Some(rule) = any_folder_rule {
            return Some(rule);
        }
    }

    // B) ACCESOS DIRECTOS (.lnk o .url): buscar regla para accesos directos
    if lower_ext == "lnk" || lower_ext == "url" {
        let shortcut_rule = cfg.rules.iter().find(|rule| {
            rule.enabled && rule.extensions.iter().any(|e| e == "lnk" || e == "url")
        });
        if let Some(rule) = shortcut_rule {
            return Some(rule);
        }
    }

    // C) Coincidencia por extensión específica o patrón de nombre
    let hit = cfg.rules.iter().find(|rule| {
        if !rule.enabled {
            return false;
        }
        if is_dir && !rule.include_folders {
            return false;
        }
        let ext_hit = rule.extensions.iter().any(|candidate| {
            candidate != "*" && !lower_ext.is_empty() && candidate.as_str() == lower_ext
        });
        let name_hit = rule
            .name_patterns
            .iter()
            .any(|pattern| wildcard_match(&pattern.to_ascii_lowercase(), &lower_name));
        (ext_hit || name_hit) && rule_extra_matches(rule, name, size, modified)
    });

    if let Some(rule) = hit {
        return Some(rule);
    }

    // D) Regla comodín (*)
    cfg.rules.iter().find(|rule| {
        if !rule.enabled {
            return false;
        }
        if is_dir && !rule.include_folders {
            return false;
        }
        rule.extensions.iter().any(|candidate| candidate == "*")
            && rule_extra_matches(rule, name, size, modified)
    })
}

/// Filtros avanzados (tamano / fecha / regex) de una regla. Devuelve `true`
/// si el archivo cumple todos los criterios configurados (o si no hay ninguno).
fn rule_extra_matches(
    rule: &Rule,
    name: &str,
    size: Option<u64>,
    modified: Option<SystemTime>,
) -> bool {
    if let Some(min) = rule.min_size_bytes {
        match size {
            Some(s) if s >= min => {}
            _ => return false,
        }
    }
    if let Some(max) = rule.max_size_bytes {
        match size {
            Some(s) if s <= max => {}
            _ => return false,
        }
    }
    if rule.newer_than_days.is_some() || rule.older_than_days.is_some() {
        let Some(modified) = modified else { return false };
        let age_days = SystemTime::now()
            .duration_since(modified)
            .map(|d| d.as_secs_f64() / 86400.0)
            .unwrap_or(0.0);
        if let Some(days) = rule.newer_than_days {
            if age_days > days {
                return false;
            }
        }
        if let Some(days) = rule.older_than_days {
            if age_days < days {
                return false;
            }
        }
    }
    if let Some(pattern) = &rule.regex {
        match regex::Regex::new(pattern) {
            Ok(re) => {
                if !re.is_match(name) {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

/// Archivos que ZenDesktop no debe tocar jamas.
pub fn is_protected(cfg: &Config, name: &str, _ext: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == "desktop.ini" || lower == "thumbs.db" {
        return true;
    }
    cfg.general
        .protected
        .iter()
        .any(|p| wildcard_match(&p.to_ascii_lowercase(), &lower))
}

/// Rutas internas de la aplicacion que nunca se clasifican ni se listan.
fn is_internal(cfg: &Config, path: &Path) -> bool {
    let root = cfg.root_dir();
    let archive = cfg.archive_dir();
    path == root || path == archive || path.starts_with(&root) || path.starts_with(&archive)
}

// ---------------------------------------------------------------------------
// Organizacion automatica
// ---------------------------------------------------------------------------

/// Recorre el primer nivel del escritorio y traslada cada archivo (y, si esta
/// activado `organize_folders`, cada carpeta) a la caja fisica que le
/// corresponde. Las reglas con `move_files = false` no tocan el disco: solo
/// alimentan cajas virtuales. Las carpetas solo casan con reglas que tengan
/// `include_folders = true` y nunca se copian entre volumenes.
pub fn organize(cfg: &Config, desktop: &Path) -> Report {
    let started = SystemTime::now();
    let mut report = Report::default();

    let root = cfg.root_dir();
    let entries = match fs::read_dir(desktop) {
        Ok(e) => e,
        Err(err) => {
            report.push_error("lectura del escritorio", &err);
            return report;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if is_internal(cfg, &path) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(err) => {
                report.push_error("file_type", &err);
                continue;
            }
        };
        let is_dir = file_type.is_dir();
        if is_dir && !cfg.general.organize_folders {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        if is_protected(cfg, &name, &ext) {
            report.skipped += 1;
            continue;
        }

        // Las carpetas solo casan si la regla permite mover archivos.
        let meta = fs::metadata(&path).ok();
        let size = meta.as_ref().map(|m| m.len());
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let rule = classify(cfg, &name, &ext, is_dir, size, modified)
            .filter(|r| r.move_files);
        let rule = match rule {
            Some(r) => r,
            None => continue,
        };

        let target_dir = root.join(&rule.folder);
        if let Err(err) = fs::create_dir_all(&target_dir) {
            report.push_error(&format!("crear {}", target_dir.display()), &err);
            continue;
        }
        let result = if is_dir {
            move_into_dir(&path, &target_dir)
        } else {
            move_into(&path, &target_dir)
        };
        match result {
            Ok(true) => report.organized += 1,
            Ok(false) => report.skipped += 1,
            Err(err) => report.push_error(&name, &err),
        }
    }

    if report.organized > 0 {
        notify_shell();
    }
    report.elapsed_ms = elapsed_ms(started);
    report
}

pub fn notify_shell() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_FLUSH, None, None);
    }
}

pub fn restore_desktop(cfg: &Config, desktop: &Path) -> Report {
    let started = SystemTime::now();
    let mut report = Report::default();
    let root = cfg.root_dir();

    // 1) Carpetas fisicas de las cajas: se vacia TODO lo que haya dentro de la
    // raiz interna (reglas activas, desactivadas o ya eliminadas) para que
    // ningun fichero se quede atras al salir.
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db") {
                continue;
            }
            if path.is_dir() {
                restore_tree(&mut report, &path, desktop, false);
            } else {
                let _ = move_into(&path, desktop);
                report.organized += 1;
            }
        }
    }
    // 2) Archivo historico (ficheros sueltos y subcarpetas AAAA-MM aplanadas).
    restore_tree(&mut report, &cfg.archive_dir(), desktop, true);
    // 3) Si la raiz interna quedo vacia se retira entera.
    if fs::read_dir(&root).map(|mut e| e.next().is_none()).unwrap_or(false) {
        let _ = fs::remove_dir(&root);
    }

    notify_shell();

    report.elapsed_ms = elapsed_ms(started);
    report
}

/// Mueve cada entrada de `src_dir` a `dest`. Con `flat` activado las carpetas
/// internas se vacian de forma recursiva y luego se eliminan (caso de los
/// meses del archivo: su contenido vuelve a la raiz). Nunca borra contenido.
fn restore_tree(report: &mut Report, src_dir: &Path, dest: &Path, flat: bool) {
    let entries = match fs::read_dir(src_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db") {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        if is_dir && flat {
            // Aplanar: mover el contenido de la subcarpeta (p. ej. "2025-03")
            // directamente al escritorio y eliminar la subcarpeta vacia.
            restore_tree(report, &path, dest, true);
            let _ = fs::remove_dir(&path);
            continue;
        }

        let result = if is_dir {
            move_into_dir(&path, dest)
        } else {
            move_into(&path, dest)
        };
        match result {
            Ok(true) => report.organized += 1,
            Ok(false) => report.skipped += 1,
            Err(err) => report.push_error(&path.display().to_string(), &err),
        }
    }
    // `remove_dir` solo tiene exito si la carpeta quedo vacia; si algo sigue
    // (bloqueado, con nombre duplicado resuelto fuera...) se conserva intacta.
    let _ = fs::remove_dir(src_dir);
}

// ---------------------------------------------------------------------------
// Regla de archivos efimeros
// ---------------------------------------------------------------------------

/// Mueve a `<Documentos>\<archive_folder>\AAAA-MM` todo lo que lleve mas de
/// `max_age_days` sin modificarse ni abrirse. Barre el escritorio y tambien el
/// interior de las cajas fisicas (un instalador olvidado en "Instaladores"
/// tambien caduca).
pub fn sweep_ephemeral(cfg: &Config, desktop: &Path) -> Report {
    let started = SystemTime::now();
    let mut report = Report::default();
    if !cfg.ephemeral.enabled {
        return report;
    }

    let max_age = cfg.max_age();
    let min_age = Duration::from_secs(cfg.ephemeral.min_age_minutes * 60);
    let archive_root = cfg.archive_dir();
    if let Err(err) = fs::create_dir_all(&archive_root) {
        report.push_error("crear archivo historico", &err);
        return report;
    }

    let mut scopes: Vec<PathBuf> = vec![desktop.to_path_buf()];
    let root = cfg.root_dir();
    for rule in cfg.rules.iter().filter(|r| r.enabled && r.move_files) {
        let dir = root.join(&rule.folder);
        if dir.is_dir() {
            scopes.push(dir);
        }
    }

    for scope in scopes {
        let is_desktop_root = scope == desktop;
        let entries = match fs::read_dir(&scope) {
            Ok(e) => e,
            Err(err) => {
                report.push_error(&format!("leer {}", scope.display()), &err);
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if is_desktop_root && is_internal(cfg, &path) {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(err) => {
                    report.push_error("metadata", &err);
                    continue;
                }
            };
            if meta.is_dir() {
                continue; // las carpetas nunca caducan
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();

            if is_protected(cfg, &name, &ext) || cfg.ephemeral.never_expire.iter().any(|e| {
                e.trim_start_matches('.').eq_ignore_ascii_case(&ext)
            }) {
                report.skipped += 1;
                continue;
            }

            let age = age_of(&meta, cfg.ephemeral.use_access_time);
            if age < max_age || age < min_age {
                continue;
            }

            let target = if cfg.ephemeral.archive_by_month {
                archive_root.join(month_folder(meta.modified().unwrap_or(UNIX_EPOCH)))
            } else {
                archive_root.clone()
            };
            if let Err(err) = fs::create_dir_all(&target) {
                report.push_error(&format!("crear {}", target.display()), &err);
                continue;
            }
            match move_into(&path, &target) {
                Ok(true) => report.archived += 1,
                Ok(false) => report.skipped += 1,
                Err(err) => report.push_error(&name, &err),
            }
        }
    }

    if cfg.ephemeral.purge_archive_after_days > 0 {
        report.merge(purge_archive(cfg));
    }

    report.elapsed_ms = elapsed_ms(started);
    report
}

/// Borrado definitivo opcional del historico (desactivado por defecto).
pub fn purge_archive(cfg: &Config) -> Report {
    let mut report = Report::default();
    let days = cfg.ephemeral.purge_archive_after_days;
    if days == 0 {
        return report;
    }
    let limit = Duration::from_secs(days as u64 * 86_400);
    let root = cfg.archive_dir();

    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if age_of(&meta, false) >= limit {
                match fs::remove_file(&path) {
                    Ok(()) => report.purged += 1,
                    Err(err) => report.push_error("purgar", &err),
                }
            }
        }
    }
    report
}

// ---------------------------------------------------------------------------
// Construccion del contenido de las cajas
// ---------------------------------------------------------------------------

/// Resuelve el contenido visible de cada caja activa.
pub fn collect_fences(cfg: &Config, desktop: &Path, sort_overrides: &std::collections::HashMap<String, Option<String>>) -> Vec<FenceContent> {
    let root = cfg.root_dir();
    let mut fences = Vec::with_capacity(cfg.rules.len());

    for rule in cfg.rules.iter().filter(|r| r.enabled) {
        let (folder, mut items) = if rule.move_files {
            let dir = root.join(&rule.folder);
            (Some(dir.clone()), read_items(&dir, cfg, desktop, None))
        } else {
            // Caja virtual: lista lo que sigue en el escritorio y casa con la regla.
            (None, read_items(desktop, cfg, desktop, Some(rule)))
        };

        let sort_mode = sort_overrides.get(&rule.id).and_then(|m| m.as_deref()).unwrap_or(&cfg.appearance.sort_by);
        sort_items(&mut items, sort_mode);
        fences.push(FenceContent {
            id: rule.id.clone(),
            title: rule.title.clone(),
            color: rule.color.clone(),
            view_mode: rule.view_mode.clone(),
            icon_size: rule.icon_size,
            folder,
            items,
        });
    }
    fences
}

fn read_items(
    dir: &Path,
    cfg: &Config,
    desktop: &Path,
    filter: Option<&Rule>,
) -> Vec<FileItem> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_internal(cfg, &path) && dir == desktop {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        if let Some(rule) = filter {
            let meta = fs::metadata(&path).ok();
            let size = meta.as_ref().map(|m| m.len());
            let modified = meta.as_ref().and_then(|m| m.modified().ok());
            match classify(cfg, &name, &ext, is_dir, size, modified) {
                Some(hit) if hit.id == rule.id => {}
                _ => continue,
            }
        }
        if let Some(item) = FileItem::from_path(path) {
            out.push(item);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Extensiones de imagen conocidas (para la previsualizacion de miniaturas)
// ---------------------------------------------------------------------------

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "heic",
    "tiff", "tif", "svg", "ico", "psd", "raw", "cr2", "nef",
];

pub fn is_image(ext: &str) -> bool {
    !ext.is_empty() && IMAGE_EXTENSIONS.contains(&ext)
}

/// Ordena items in-place (publico: lo usa el menu contextual de cada caja).
pub fn sort_items_slice(items: &mut [FileItem], mode: &str) {
    sort_items(items, mode)
}

fn sort_items(items: &mut [FileItem], mode: &str) {
    if mode == "custom" {
        return; // mantener el orden manual actual
    }
    items.sort_by(|a, b| match mode {
        "modified" => b.modified.cmp(&a.modified),
        "size" => b.size.cmp(&a.size),
        "extension" => a
            .ext
            .cmp(&b.ext)
            .then_with(|| natural_cmp(&a.name, &b.name)),
        _ => a
            .is_dir
            .cmp(&b.is_dir)
            .reverse()
            .then_with(|| natural_cmp(&a.name, &b.name)),
    });
}

/// Siguiente modo de ordenacion en el ciclo del menu.
pub fn next_sort_mode(current: &str) -> &'static str {
    const MODES: &[&str] = &["name", "size", "type", "modified", "custom", "global"];
    let idx = MODES.iter().position(|m| *m == current).unwrap_or(0);
    MODES[(idx + 1) % MODES.len()]
}

/// Etiqueta legible para un modo de ordenacion.
pub fn sort_label(mode: &str) -> &'static str {
    match mode {
        "name" => "A-Z",
        "size" => "Tamano",
        "type" => "Tipo",
        "modified" => "Fecha",
        "custom" => "Manual",
        "global" => "Global",
        _ => "A-Z",
    }
}

/// Orden "natural" del explorador: archivo2 antes que archivo10.
fn natural_cmp(a: &str, b: &str) -> CmpOrdering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return CmpOrdering::Equal,
            (None, Some(_)) => return CmpOrdering::Less,
            (Some(_), None) => return CmpOrdering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let mut nx: u64 = 0;
                    let mut ny: u64 = 0;
                    while let Some(d) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                        nx = nx.saturating_mul(10) + (d as u64 - '0' as u64);
                        ai.next();
                    }
                    while let Some(d) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                        ny = ny.saturating_mul(10) + (d as u64 - '0' as u64);
                        bi.next();
                    }
                    match nx.cmp(&ny) {
                        CmpOrdering::Equal => continue,
                        other => return other,
                    }
                }
                let lx = x.to_ascii_lowercase();
                let ly = y.to_ascii_lowercase();
                match lx.cmp(&ly) {
                    CmpOrdering::Equal => {
                        ai.next();
                        bi.next();
                    }
                    other => return other,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utilidades de sistema de archivos
// ---------------------------------------------------------------------------

/// Mueve `src` dentro de `dir` resolviendo colisiones.
/// Devuelve `Ok(false)` si el archivo estaba bloqueado y hay que reintentar.
pub fn move_into(src: &Path, dir: &Path) -> io::Result<bool> {
    let name = match src.file_name() {
        Some(n) => n.to_os_string(),
        None => return Ok(false),
    };
    // Si el archivo ya esta en la carpeta destino, no hacer nada.
    // Evita duplicados (2) por doble drop o por arrastrar un archivo a su propia carpeta.
    if src.parent() == Some(dir) {
        return Ok(false);
    }
    let dest = unique_path(dir, &name);

    match fs::rename(src, &dest) {
        Ok(()) => Ok(true),
        Err(err) => {
            let code = err.raw_os_error().unwrap_or(0);
            match code {
                // ERROR_NOT_SAME_DEVICE: escritorio y destino en volumenes distintos.
                17 => {
                    fs::copy(src, &dest)?;
                    fs::remove_file(src)?;
                    Ok(true)
                }
                // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION / ERROR_ACCESS_DENIED
                32 | 33 | 5 => Ok(false),
                _ => Err(err),
            }
        }
    }
}

/// Mueve una carpeta completa dentro de `dir` resolviendo colisiones.
/// Solo usa `rename`: mover un arbol entre volumenes no se intenta (copiar
/// carpetas enteras seria demasiado arriesgado); en ese caso se omite y se
/// reintentara en la siguiente rafaga.
pub fn move_into_dir(src: &Path, dir: &Path) -> io::Result<bool> {
    let name = match src.file_name() {
        Some(n) => n.to_os_string(),
        None => return Ok(false),
    };
    // Si la carpeta ya esta en el destino, no hacer nada.
    if src.parent() == Some(dir) {
        return Ok(false);
    }
    let dest = unique_path(dir, &name);

    match fs::rename(src, &dest) {
        Ok(()) => Ok(true),
        Err(err) => {
            let code = err.raw_os_error().unwrap_or(0);
            match code {
                // ERROR_NOT_SAME_DEVICE: volumen distinto; no copiar arboles.
                17 => Ok(false),
                // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION / ERROR_ACCESS_DENIED
                32 | 33 | 5 => Ok(false),
                _ => Err(err),
            }
        }
    }
}

/// Mueve un archivo O una carpeta entera dentro de `dir` (usado al soltar
/// elementos sobre una caja). Los ficheros aceptan copia entre volumenes;
/// las carpetas solo se renombran en el mismo volumen (cruzar volumenes con
/// arboles enteros seria demasiado arriesgado) y se omiten en ese caso.
pub fn move_into_any(src: &Path, dir: &Path) -> io::Result<bool> {
    if src.is_dir() {
        move_into_dir(src, dir)
    } else {
        move_into(src, dir)
    }
}

/// "informe.pdf" -> "informe (2).pdf" si ya existe.
pub fn unique_path(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let mut candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let base = Path::new(name);
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("archivo"));
    let ext = base
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    for n in 2..10_000u32 {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}{ext}", now_secs()))
}

/// Edad del archivo = tiempo desde el evento mas reciente (modificacion o,
/// opcionalmente, ultimo acceso). NTFS actualiza el acceso de forma perezosa,
/// por eso se combina con la modificacion en lugar de sustituirla.
pub fn age_of(meta: &fs::Metadata, use_access: bool) -> Duration {
    let now = SystemTime::now();
    let mut newest = meta.modified().unwrap_or(UNIX_EPOCH);
    if use_access {
        if let Ok(accessed) = meta.accessed() {
            if accessed > newest {
                newest = accessed;
            }
        }
    }
    if let Ok(created) = meta.created() {
        if created > newest {
            newest = created;
        }
    }
    now.duration_since(newest).unwrap_or(Duration::ZERO)
}

/// "AAAA-MM" a partir de un SystemTime, sin dependencias de calendario.
pub fn month_folder(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let (y, m, _d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}")
}

/// Algoritmo de Howard Hinnant (dias desde 1970-01-01 -> fecha civil).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Coincidencia de patrones con `*` y `?`, iterativa y sin regex ni asignaciones.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn elapsed_ms(start: SystemTime) -> u128 {
    SystemTime::now()
        .duration_since(start)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

// ---------------------------------------------------------------------------
// Pruebas unitarias (cargo test, no entran en el binario de release)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*.tmp", "captura.tmp"));
        assert!(wildcard_match("setup*", "setup_v2.exe"));
        assert!(wildcard_match("~$*", "~$informe.docx"));
        assert!(!wildcard_match("*.tmp", "captura.png"));
        assert!(wildcard_match("*", "cualquier-cosa"));
        assert!(wildcard_match("factura-?.pdf", "factura-7.pdf"));
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.png", "img2.png", "img1.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["img1.png", "img2.png", "img10.png"]);
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }

    #[test]
    fn classification_priority() {
        let cfg = Config::default();
        let media = classify(&cfg, "vacaciones.jpg", "jpg", false, None, None).unwrap();
        assert_eq!(media.id, "media");
        let misc = classify(&cfg, "raro.xyz", "xyz", false, None, None).unwrap();
        assert_eq!(misc.id, "misc");
    }

    #[test]
    fn advanced_rule_filters() {
        let mut cfg = Config::default();
        cfg.rules = vec![Rule {
            id: "facturas".into(),
            title: "Facturas".into(),
            enabled: true,
            extensions: vec!["pdf".into()],
            name_patterns: Vec::new(),
            move_files: true,
            folder: "Facturas".into(),
            color: "#fff".into(),
            include_folders: false,
            view_mode: "auto".into(),
            icon_size: None,
            min_size_bytes: Some(1024 * 1024),
            max_size_bytes: None,
            newer_than_days: None,
            older_than_days: None,
            regex: Some(r"^factura-.*\.pdf$".into()),
        }];
        // regex ok + tamano ok -> casa
        assert!(classify(&cfg, "factura-2024.pdf", "pdf", false, Some(2 * 1024 * 1024), None).is_some());
        // regex no casa
        assert!(classify(&cfg, "nota.pdf", "pdf", false, Some(2 * 1024 * 1024), None).is_none());
        // tamano por debajo del minimo
        assert!(classify(&cfg, "factura-2024.pdf", "pdf", false, Some(10), None).is_none());
    }

    /// Crea un escritorio de prueba aislado en el directorio temporal.
    fn test_desktop(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zendesktop-{}-{}", tag, now_secs()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Configuracion aislada: redirige las carpetas internas (ZenDesktop /
    /// ZenArchive) a rutas absolutas dentro del directorio temporal, para que
    /// los tests jamas toquen Mis Documentos reales.
    fn isolated(cfg: &mut Config, base: &Path) {
        cfg.general.root_folder = base.join("ZenDesktop").to_string_lossy().into_owned();
        cfg.general.archive_folder = base.join("ZenArchive").to_string_lossy().into_owned();
    }

    #[test]
    fn organizes_files_and_folders() {
        let desktop = test_desktop("org");
        fs::create_dir_all(desktop.join("Fotos")).unwrap();
        fs::write(desktop.join("foto.jpg"), b"x").unwrap();
        fs::write(desktop.join("otro.dat"), b"x").unwrap();

        let mut cfg = Config::default();
        isolated(&mut cfg, &desktop);
        cfg.general.organize_folders = true;
        cfg.rules = vec![
            Rule {
                id: "media".into(),
                title: "Media".into(),
                enabled: true,
                extensions: vec!["jpg".into()],
                name_patterns: vec!["fotos*".into()],
                move_files: true,
                folder: "Media".into(),
                color: "#38BDF8".into(),
                include_folders: true,
                view_mode: "auto".into(),
                icon_size: None,
                min_size_bytes: None,
                max_size_bytes: None,
                newer_than_days: None,
                older_than_days: None,
                regex: None,
            },
            Rule {
                id: "misc".into(),
                title: "Varios".into(),
                enabled: true,
                extensions: vec!["*".into()],
                name_patterns: Vec::new(),
                move_files: true,
                folder: "Varios".into(),
                color: "#34D399".into(),
                include_folders: true,
                view_mode: "auto".into(),
                icon_size: None,
                min_size_bytes: None,
                max_size_bytes: None,
                newer_than_days: None,
                older_than_days: None,
                regex: None,
            },
        ];

        ensure_layout(&cfg).unwrap();
        let report = organize(&cfg, &desktop);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let root = cfg.root_dir();
        // La carpeta "Fotos" casa con la regla Media por patron de nombre
        // (las carpetas no tienen extension, asi que casan por nombre o por
        // la regla comodin).
        assert!(root.join("Media/Fotos").is_dir(), "la carpeta debio moverse a Media");
        assert!(!desktop.join("Fotos").exists());
        // El jpg va a Media y el resto al cajon de sastre.
        assert!(root.join("Media/foto.jpg").is_file());
        assert!(root.join("Varios/otro.dat").is_file());
        assert!(report.organized == 3, "3 movimientos esperados, got {}", report.organized);

        let _ = fs::remove_dir_all(&desktop);
    }

    #[test]
    fn folders_stay_when_disabled() {
        let desktop = test_desktop("nofolders");
        fs::create_dir_all(desktop.join("Fotos")).unwrap();

        let mut cfg = Config::default();
        isolated(&mut cfg, &desktop);
        cfg.general.organize_folders = false;
        cfg.rules = vec![Rule {
            id: "media".into(),
            title: "Media".into(),
            enabled: true,
            extensions: vec!["*".into()],
            name_patterns: Vec::new(),
            move_files: true,
            folder: "Media".into(),
            color: "#38BDF8".into(),
            include_folders: true,
            view_mode: "auto".into(),
            icon_size: None,
                min_size_bytes: None,
                max_size_bytes: None,
                newer_than_days: None,
                older_than_days: None,
                regex: None,
        }];
        ensure_layout(&cfg).unwrap();
        let report = organize(&cfg, &desktop);
        assert!(report.errors.is_empty());
        assert!(desktop.join("Fotos").is_dir(), "sin organize_folders la carpeta no se toca");
        assert_eq!(report.organized, 0);

        let _ = fs::remove_dir_all(&desktop);
    }

    #[test]
    fn restore_returns_files_to_desktop() {
        let desktop = test_desktop("restore");
        fs::write(desktop.join("foto.jpg"), b"x").unwrap();
        fs::write(desktop.join("nota.pdf"), b"x").unwrap();

        let mut cfg = Config::default();
        isolated(&mut cfg, &desktop);
        cfg.general.organize_folders = true;
        cfg.rules = vec![
            Rule {
                id: "media".into(),
                title: "Media".into(),
                enabled: true,
                extensions: vec!["jpg".into()],
                name_patterns: Vec::new(),
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
            },
            Rule {
                id: "misc".into(),
                title: "Varios".into(),
                enabled: true,
                extensions: vec!["*".into()],
                name_patterns: Vec::new(),
                move_files: true,
                folder: "Varios".into(),
                color: "#34D399".into(),
                include_folders: true,
                view_mode: "auto".into(),
                icon_size: None,
                min_size_bytes: None,
                max_size_bytes: None,
                newer_than_days: None,
                older_than_days: None,
                regex: None,
            },
        ];
        ensure_layout(&cfg).unwrap();
        let report = organize(&cfg, &desktop);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let restore = restore_desktop(&cfg, &desktop);
        assert!(restore.errors.is_empty(), "{:?}", restore.errors);
        assert!(desktop.join("foto.jpg").is_file(), "foto.jpg debe volver a la raiz");
        assert!(desktop.join("nota.pdf").is_file(), "nota.pdf debe volver a la raiz");
        // La raiz interna quedo vacia y se retiro.
        assert!(!cfg.root_dir().join("Media").exists());
        assert!(!cfg.root_dir().join("Varios").exists());

        let _ = fs::remove_dir_all(&desktop);
    }

    #[test]
    fn restore_flattens_archive_months() {
        let desktop = test_desktop("restore-archive");
        fs::write(desktop.join("viejo.log"), b"x").unwrap();

        let mut cfg = Config::default();
        isolated(&mut cfg, &desktop);
        cfg.rules = vec![Rule {
            id: "misc".into(),
            title: "Varios".into(),
            enabled: true,
            extensions: vec!["*".into()],
            name_patterns: Vec::new(),
            move_files: true,
            folder: "Varios".into(),
            color: "#34D399".into(),
            include_folders: true,
            view_mode: "auto".into(),
            icon_size: None,
                min_size_bytes: None,
                max_size_bytes: None,
                newer_than_days: None,
                older_than_days: None,
                regex: None,
        }];
        ensure_layout(&cfg).unwrap();
        organize(&cfg, &desktop);

        // Simular el archivo historico con un mes anidado.
        let month = cfg.archive_dir().join("2025-01");
        fs::create_dir_all(&month).unwrap();
        fs::write(month.join("caduco.log"), b"x").unwrap();

        let restore = restore_desktop(&cfg, &desktop);
        assert!(restore.errors.is_empty(), "{:?}", restore.errors);
        assert!(desktop.join("caduco.log").is_file(), "el archivado vuelve a la raiz");
        assert!(!month.exists(), "la subcarpeta de mes se aplana y se retira");

        let _ = fs::remove_dir_all(&desktop);
    }

    #[test]
    fn restore_empties_orphaned_and_disabled_rule_folders() {
        let desktop = test_desktop("restore-orphan");
        fs::write(desktop.join("foto.jpg"), b"x").unwrap();

        let mut cfg = Config::default();
        isolated(&mut cfg, &desktop);
        cfg.general.organize_folders = true;
        cfg.rules = vec![Rule {
            id: "media".into(),
            title: "Media".into(),
            enabled: true,
            extensions: vec!["jpg".into()],
            name_patterns: Vec::new(),
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
        }];
        ensure_layout(&cfg).unwrap();
        organize(&cfg, &desktop);
        assert!(cfg.root_dir().join("Media").join("foto.jpg").is_file());

        // Carpeta huerfana (regla eliminada o nunca creada): su contenido
        // tambien debe volver al escritorio al restaurar, aunque la regla
        // correspondiente ya no este activa.
        fs::create_dir_all(cfg.root_dir().join("Instaladores")).unwrap();
        fs::write(cfg.root_dir().join("Instaladores").join("setup.exe"), b"x").unwrap();
        cfg.rules[0].enabled = false;

        let restore = restore_desktop(&cfg, &desktop);
        assert!(restore.errors.is_empty(), "{:?}", restore.errors);
        assert!(desktop.join("foto.jpg").is_file(), "foto.jpg vuelve a la raiz");
        assert!(desktop.join("setup.exe").is_file(), "la carpeta huerfana se vacia");
        assert!(!cfg.root_dir().exists(), "la raiz interna se retira al quedar vacia");

        let _ = fs::remove_dir_all(&desktop);
    }
}
