// ZenDesktop - organizador dinamico de escritorio para Windows.
// Sin consola: subsistema GUI puro (no aparece ninguna ventana negra).
#![windows_subsystem = "windows"]

//! ZenDesktop :: main.rs
//!
//! Secuencia de arranque (objetivo: < 40 ms hasta el primer frame):
//!
//!   1. Mutex nombrado -> instancia unica.
//!   2. Conciencia de DPI por monitor v2 (sin manifiesto externo).
//!   3. COM en modo apartamento: lo exige el shell (SHGetKnownFolderPath,
//!      ShellExecuteW, iconos).
//!   4. Carga o creacion de config.toml portable.
//!   5. Creacion del arbol de cajas y clasificacion inicial opcional.
//!   6. Alta de la interfaz (Direct2D) y del vigilante de disco.
//!   7. Bucle de mensajes bloqueante: `GetMessageW` deja el hilo dormido en el
//!      kernel, por lo que en reposo el proceso consume 0 % de CPU.

mod ai;
mod changelog;
mod config;
mod i18n;
mod rules;
mod settings;
mod ui;
mod updater;
mod watcher;

use std::ffi::c_void;
use std::process::ExitCode;
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WPARAM,
    WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, GetMessageW, MessageBoxW, PostMessageW, TranslateMessage,
    MB_ICONERROR, MB_OK, MSG, WM_CLOSE,
};

use crate::config::Config;
use crate::i18n::{Lang, Tr};
use crate::ui::App;
use crate::watcher::{DesktopWatcher, WindowTarget};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            // Error de arranque: todavia no hay configuracion cargada, asi que
            // se muestra en el idioma por defecto (ingles).
            let tr = Tr::get(Lang::En);
            fatal(&tr.fatal_start.replace("{error}", &error.to_string()));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    // ---------------------------------------------------------------- 1. Unicidad
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\ZenDesktop.SingleInstance.v1"))? };
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_running {
        if update_restart_requested() {
            // Reinicio tras actualizacion: pedir el cierre de la instancia
            // anterior y esperar a que libere el mutex antes de continuar.
            unsafe {
                if let Ok(old) = FindWindowW(w!("ZenDesktop.Controller"), None) {
                    if !old.is_invalid() {
                        let _ = PostMessageW(old, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                let wait = WaitForSingleObject(mutex, 8000);
                // WAIT_OBJECT_0 = liberado; WAIT_ABANDONED = la instancia
                // anterior murio sin liberar el mutex: ambos nos dejan como
                // unica instancia viva.
                if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
                    let _ = CloseHandle(mutex);
                    return Ok(ExitCode::SUCCESS);
                }
            }
        } else {
            // Ya hay una instancia viva: salir en silencio, sin molestar al usuario.
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return Ok(ExitCode::SUCCESS);
        }
    }

    // Limpia el .bak que dejo una actualizacion previa (ya no esta en uso:
    // esta ejecucion es la unica instancia viva).
    cleanup_stale_backup();

    // ------------------------------------------------- 2. DPI y 3. inicializacion COM
    unsafe {
        // Ignorable: en Windows 8.1 y anteriores basta con el comportamiento clasico.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // OLE en lugar de COM puro: es lo que exige el drag & drop del shell
        // (RegisterDragDrop) y sigue dando servicio al menu contextual nativo.
        match OleInitialize(None) {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("OleInitialize failed: {e:?}");
                let body: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
                MessageBoxW(None, PCWSTR(body.as_ptr()), w!("ZenDesktop"), MB_OK | MB_ICONERROR);
            }
        }
    }

    let result = bootstrap();

    unsafe {
        OleUninitialize();
        let _ = CloseHandle(mutex);
    }
    result
}

fn bootstrap() -> Result<ExitCode, Box<dyn std::error::Error>> {
    // ------------------------------------------------------------ 4. Configuracion
    let (mut cfg, cfg_path) = Config::load_or_create()?;

    // Si esta version aun no se ha visto, se marca "What's New" pendiente: se
    // avisa con un toast no intrusivo una vez arrancada la interfaz (nada de
    // ventanas modales al iniciar).
    let current_ver = env!("CARGO_PKG_VERSION");
    let mut whats_new_pending = false;
    if cfg.general.last_seen_version != current_ver {
        if let Some((ver, _body)) = changelog::latest_release() {
            // Solo si la entrada mas reciente del changelog coincide con esta version.
            if ver == current_ver {
                whats_new_pending = true;
            }
        }
        // Marcar como vista para no volver a avisar.
        cfg.general.last_seen_version = current_ver.to_string();
        let _ = cfg.save(&cfg_path);
    }

    if cfg.general.start_with_windows {
        // Un fallo aqui (politicas de grupo) no debe impedir el arranque.
        let _ = config::apply_autostart(true);
    }

    let desktop = config::desktop_dir()?;
    let mut extra_desktops = Vec::new();
    if cfg.general.watch_public_desktop {
        if let Some(public) = config::public_desktop_dir() {
            if public != desktop {
                extra_desktops.push(public);
            }
        }
    }

    // ------------------------------------------- 5. Arbol de carpetas y primer barrido
    rules::ensure_layout(&cfg)?;
    if cfg.general.organize_on_start {
        let organize = rules::organize(&cfg, &desktop);
        let sweep = rules::sweep_ephemeral(&cfg, &desktop);
        debug_assert!(
            organize.errors.is_empty() && sweep.errors.is_empty(),
            "incidencias durante el barrido inicial"
        );
    }

    // ------------------------------------------------------ 6. Interfaz y vigilante
    let debounce = Duration::from_millis(cfg.general.debounce_ms);
    let lang = cfg.lang();
    let auto_check_updates = cfg.general.auto_check_updates;
    let handle = App::launch(cfg, cfg_path, desktop, extra_desktops)?;

    // Aviso "What's New" via toast (ya con la interfaz lista y el toast creado).
    if whats_new_pending {
        unsafe {
            let _ = PostMessageW(
                handle.controller(),
                ui::WM_ZEN_SHOW_WHATS_NEW_TOAST,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    // Chequeo de updates en segundo plano: no bloquea el arranque. El hilo
    // consulta GitHub y postea el resultado a la ventana de control.
    if auto_check_updates {
        // HWND no es Send: se cruza el limite de hilos como usize (isize crudo).
        let controller_raw = handle.controller().0 as usize;
        std::thread::spawn(move || {
            let status = crate::updater::check_update();
            if matches!(status, crate::updater::UpdateStatus::UpdateAvailable { .. }) {
                crate::updater::store_last_check(status);
                unsafe {
                    let _ = PostMessageW(
                        HWND(controller_raw as *mut c_void),
                        ui::WM_ZEN_UPDATE_CHECKED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        });
    }

    let target = WindowTarget::new(handle.controller(), ui::WM_ZEN_FS);
    match DesktopWatcher::start(handle.watch_paths(), target, debounce) {
        Ok(watcher) => handle.attach_watcher(watcher),
        Err(error) => {
            // Sin vigilante la app sigue siendo util (menu "Organizar ahora"),
            // pero conviene avisar: es una degradacion importante.
            let tr = Tr::get(lang);
            fatal(&tr.watcher_failed.replace("{error}", &error.to_string()));
        }
    }

    // ------------------------------------------------------- 7. Bucle de mensajes
    let exit_code = pump_messages();

    handle.shutdown();
    Ok(if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Bucle principal. `GetMessageW` bloquea el hilo dentro del kernel hasta que
/// llega un mensaje (evento de disco publicado por el vigilante, entrada de
/// raton, hotkey o temporizador). Cero polling, cero CPU en reposo.
fn pump_messages() -> i32 {
    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        match status.0 {
            0 => return message.wParam.0 as i32, // WM_QUIT
            -1 => return 1,                      // error irrecuperable de la cola
            _ => unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
}

/// Dialogo modal de ultimo recurso: sin consola no hay stderr donde escribir.
fn fatal(text: &str) {
    let body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            w!("ZenDesktop"),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Comprobacion en tiempo de compilacion: el objetivo del vigilante tiene que
/// poder cruzar el limite de hilos (contiene un HWND marcado como `Send`).
#[allow(dead_code)]
fn assert_thread_contract() {
    fn require_send<T: Send>() {}
    require_send::<WindowTarget>();
    let _: HANDLE = HANDLE::default();
}

/// True si esta ejecucion es el "relevo" de una actualizacion: se lanzo con
/// `--update-restart`, o acaba de aparecer un `.bak` junto al ejecutable (las
/// versiones antiguas no pasaban el argumento, pero si dejan el `.bak`).
fn update_restart_requested() -> bool {
    if std::env::args().any(|a| a == "--update-restart") {
        return true;
    }
    if let Ok(exe) = std::env::current_exe() {
        let bak = exe.with_extension("exe.bak");
        if let Ok(meta) = std::fs::metadata(&bak) {
            if let Ok(modified) = meta.modified() {
                if let Ok(age) = modified.elapsed() {
                    return age.as_secs() < 60;
                }
            }
        }
    }
    false
}

/// Borra el `.bak` que deja una actualizacion (el ejecutable antiguo). Solo se
/// llama cuando esta ejecucion ya es la unica instancia, asi que el archivo no
/// esta en uso y se puede borrar sin riesgo.
fn cleanup_stale_backup() {
    if let Ok(exe) = std::env::current_exe() {
        let bak = exe.with_extension("exe.bak");
        if bak.exists() {
            let _ = std::fs::remove_file(&bak);
        }
    }
}
