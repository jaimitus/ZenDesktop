//! ZenDesktop :: build.rs
//!
//! Genera un archivo `.rc` temporal (en `OUT_DIR`) con la informacion de
//! version tomada de `CARGO_PKG_VERSION`, para que las propiedades del .exe
//! (FileVersion / ProductVersion que muestra el Explorador) nunca se queden
//! desincronizadas de `Cargo.toml`. Antes la version estaba hardcodeada en
//! `assets/zendesktop.rc` y se quedaba atras en cada release.
//!
//!   * Recurso 1 -> icono de la aplicacion (Explorer, barra de tareas).
//!   * Recurso 2 -> icono de la bandeja (se carga en tiempo de ejecucion).
//!   * Recurso 1 -> version info (propiedades del archivo).
//!
//! Requiere `rc.exe` (Windows SDK / VS Build Tools), incluido con el
//! toolchain MSVC.

use std::path::PathBuf;

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icons/zendesktop.ico");
        println!("cargo:rerun-if-changed=assets/icons/zendesktop-tray.ico");
        println!("cargo:rerun-if-changed=Cargo.toml");

        let version = env!("CARGO_PKG_VERSION");
        // FILEVERSION / PRODUCTVERSION esperan 4 numeros separados por comas.
        // "1.0.22" -> "1,0,22,0"; "1.0.22.1" -> "1,0,22,1".
        let mut parts: Vec<u32> = version
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        while parts.len() < 4 {
            parts.push(0);
        }
        let comma = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);

        // Rutas absolutas a los iconos: el .rc generado vive en OUT_DIR, asi
        // que las referencias relativas al directorio `assets/` ya no valdrian.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let icon = manifest.join("assets/icons/zendesktop.ico");
        let tray = manifest.join("assets/icons/zendesktop-tray.ico");
        let icon = icon.to_string_lossy().replace('\\', "\\\\");
        let tray = tray.to_string_lossy().replace('\\', "\\\\");

        let rc = format!(
            "// Generado por build.rs desde CARGO_PKG_VERSION ({version}).\n\
             1 ICON \"{icon}\"\n\
             2 ICON \"{tray}\"\n\
             \n\
             1 VERSIONINFO\n\
             FILEVERSION    {comma}\n\
             PRODUCTVERSION {comma}\n\
             FILEOS         0x40004L          // VOS_NT_WINDOWS32\n\
             FILETYPE       0x1L              // VFT_APP\n\
             BEGIN\n\
                 BLOCK \"StringFileInfo\"\n\
                 BEGIN\n\
                     BLOCK \"040904b0\"        // en-US, Unicode\n\
                     BEGIN\n\
                         VALUE \"CompanyName\",      \"ZenDesktop Core Team\"\n\
                         VALUE \"FileDescription\",  \"Organizador dinamico de escritorio\"\n\
                         VALUE \"FileVersion\",      \"{version}\"\n\
                         VALUE \"InternalName\",     \"zendesktop\"\n\
                         VALUE \"LegalCopyright\",   \"MIT\"\n\
                         VALUE \"OriginalFilename\", \"zendesktop.exe\"\n\
                         VALUE \"ProductName\",      \"ZenDesktop\"\n\
                         VALUE \"ProductVersion\",   \"{version}\"\n\
                     END\n\
                 END\n\
                 BLOCK \"VarFileInfo\"\n\
                 BEGIN\n\
                     VALUE \"Translation\", 0x409, 1200\n\
                 END\n\
             END\n"
        );

        let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
        let rc_path = out_dir.join("zendesktop-version.rc");
        std::fs::write(&rc_path, rc).expect("failed to write generated .rc");

        embed_resource::compile(&rc_path, embed_resource::NONE);
    }
}
