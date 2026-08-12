//! ZenDesktop :: watcher.rs
//!
//! Monitorizacion del escritorio 100% dirigida por eventos.
//!
//! `notify` usa en Windows `ReadDirectoryChangesW` con E/S superpuesta: el hilo
//! vigilante duerme dentro del kernel (estado *waiting*, 0 % CPU) hasta que el
//! sistema de archivos publica un cambio. No existe ningun `sleep` de sondeo ni
//! ningun temporizador de refresco.
//!
//! Cadena completa:
//!
//!   kernel (IRP) -> notify -> mpsc -> hilo coalescedor -> PostMessageW -> hilo UI
//!
//! El hilo coalescedor agrupa las rafagas (descomprimir un ZIP genera cientos de
//! eventos) y despierta al hilo de interfaz una sola vez por rafaga mediante un
//! mensaje asincrono, de modo que la UI nunca se bloquea ni repinta de mas.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Sufijos de archivos de trabajo temporales: ignorarlos evita repintar la UI
/// mientras un navegador o un instalador escriben su fichero parcial.
const NOISE_SUFFIXES: [&str; 6] = [
    ".tmp",
    ".crdownload",
    ".part",
    ".partial",
    ".~tmp",
    ".zendesktop-write-probe",
];

/// Handle de ventana marcado como `Send`.
///
/// SEGURIDAD: `PostMessageW` es explicitamente thread-safe (encola el mensaje en
/// la cola del hilo propietario y retorna de inmediato). Un `HWND` es un simple
/// identificador opaco, por lo que trasladarlo entre hilos es correcto siempre
/// que solo se use con APIs thread-safe, que es justo lo que hace este modulo.
#[derive(Clone, Copy)]
pub struct WindowTarget {
    hwnd: HWND,
    message: u32,
}

unsafe impl Send for WindowTarget {}

impl WindowTarget {
    pub fn new(hwnd: HWND, message: u32) -> Self {
        WindowTarget { hwnd, message }
    }

    fn notify(&self, batch: u32) {
        unsafe {
            // Fallo tipico: la ventana ya se destruyo durante el apagado.
            let _ = PostMessageW(self.hwnd, self.message, WPARAM(batch as usize), LPARAM(0));
        }
    }
}

/// Vigilante activo. Al soltarse (`Drop`) cierra el watcher del sistema y
/// espera ordenadamente al hilo coalescedor.
pub struct DesktopWatcher {
    watcher: Option<RecommendedWatcher>,
    worker: Option<JoinHandle<()>>,
    paths: Vec<PathBuf>,
}

impl DesktopWatcher {
    /// Arranca la vigilancia.
    ///
    /// * `paths`      - carpetas a observar (escritorio de usuario, publico y cajas).
    /// * `target`     - ventana + mensaje que se publicara tras cada rafaga.
    /// * `debounce`   - ventana de silencio necesaria para considerar cerrada una rafaga.
    pub fn start(
        paths: Vec<PathBuf>,
        target: WindowTarget,
        debounce: Duration,
    ) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                // El envio solo falla si la UI ya termino: se ignora.
                let _ = tx.send(res);
            },
            NotifyConfig::default(),
        )?;

        let mut watched = Vec::with_capacity(paths.len());
        for path in &paths {
            if !path.is_dir() {
                continue;
            }
            // NonRecursive: nos interesa el primer nivel de cada carpeta, nunca
            // el interior del archivo historico. Menos handles y menos ruido.
            match watcher.watch(path, RecursiveMode::NonRecursive) {
                Ok(()) => watched.push(path.clone()),
                Err(_) => {
                    // Una carpeta inaccesible (permisos, unidad desconectada) no
                    // debe abortar la vigilancia del resto.
                    continue;
                }
            }
        }

        let max_latency = debounce.saturating_mul(8);

        let worker = thread::Builder::new()
            .name("zen-fs-coalescer".into())
            .stack_size(64 * 1024) // hilo minimo: solo agrupa y publica mensajes
            .spawn(move || {
                loop {
                    // Bloqueo indefinido: el hilo no consume CPU mientras no
                    // ocurra nada en el disco.
                    let first = match rx.recv() {
                        Ok(ev) => ev,
                        Err(_) => break, // watcher soltado -> fin del hilo
                    };

                    let mut batch = 0u32;
                    if accept(&first) {
                        batch += 1;
                    }

                    // Fase de coalescencia: se espera un periodo de silencio de
                    // `debounce`, con un techo duro de `max_latency` para que una
                    // copia masiva continua tambien refresque la interfaz.
                    let started = Instant::now();
                    loop {
                        match rx.recv_timeout(debounce) {
                            Ok(ev) => {
                                if accept(&ev) {
                                    batch = batch.saturating_add(1);
                                }
                                if started.elapsed() >= max_latency {
                                    break;
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => {
                                if batch > 0 {
                                    target.notify(batch);
                                }
                                return;
                            }
                        }
                    }

                    if batch > 0 {
                        target.notify(batch);
                    }
                }
            })
            .map_err(notify::Error::from)?;

        Ok(DesktopWatcher {
            watcher: Some(watcher),
            worker: Some(worker),
            paths: watched,
        })
    }

    /// Anade una carpeta en caliente (por ejemplo, una caja creada al vuelo).
    pub fn watch_extra(&mut self, path: &Path) -> notify::Result<()> {
        if let Some(w) = self.watcher.as_mut() {
            if path.is_dir() && !self.paths.iter().any(|p| p == path) {
                w.watch(path, RecursiveMode::NonRecursive)?;
                self.paths.push(path.to_path_buf());
            }
        }
        Ok(())
    }

    /// Deja de vigilar una carpeta (regla desactivada o carpeta borrada).
    pub fn unwatch(&mut self, path: &Path) {
        if let Some(w) = self.watcher.as_mut() {
            let _ = w.unwatch(path);
            self.paths.retain(|p| p != path);
        }
    }

}

impl Drop for DesktopWatcher {
    fn drop(&mut self) {
        // Soltar el watcher cierra el canal y desbloquea `rx.recv()`.
        self.watcher.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Filtro barato aplicado en el hilo coalescedor: descarta metadatos, accesos y
/// archivos temporales para no despertar a la UI sin motivo.
fn accept(event: &notify::Result<Event>) -> bool {
    let event = match event {
        Ok(ev) => ev,
        Err(_) => return false,
    };

    let relevant = matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) | EventKind::Any
    );
    if !relevant {
        return false;
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }

    event.paths.iter().any(|p| !is_noise(p))
}

fn is_noise(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_ascii_lowercase(),
        None => return true,
    };
    if name.starts_with("~$") || name.starts_with(".~") {
        return true;
    }
    NOISE_SUFFIXES.iter().any(|s| name.ends_with(s))
}

