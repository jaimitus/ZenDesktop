//! ZenDesktop :: build.rs
//!
//! Embebe en el .exe los recursos declarados en `assets/zendesktop.rc`:
//!
//!   * Recurso 1 -> icono de la aplicacion (Explorer, barra de tareas).
//!   * Recurso 2 -> icono de la bandeja (se carga en tiempo de ejecucion).
//!   * Recurso 1 -> version info (propiedades del archivo).
//!
//! Requiere `rc.exe` (Windows SDK / VS Build Tools), incluido con el
//! toolchain MSVC.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/zendesktop.rc");
        println!("cargo:rerun-if-changed=assets/icons/zendesktop.ico");
        println!("cargo:rerun-if-changed=assets/icons/zendesktop-tray.ico");
        embed_resource::compile("assets/zendesktop.rc", embed_resource::NONE);
    }
}
