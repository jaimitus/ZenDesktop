# -*- coding: utf-8 -*-
"""Parcial: extrae build_shell_menu a fn libre y anade probes e2e."""
import io

path = 'src/ui.rs'
src = open(path, encoding='utf-8').read()

# ---------------------------------------------------------------------------
# 1) Convertir App::build_shell_menu en delegado de una funcion libre.
# ---------------------------------------------------------------------------
method_start = src.index('    /// Prepara el menu contextual nativo del shell para un elemento de una')
method_end = src.index('    unsafe fn show_shell_menu(&mut self, owner: HWND, path: &Path) {')
method = src[method_start:method_end]

# Extraer el cuerpo: desde la firma hasta su llave de cierre.
sig = '    unsafe fn build_shell_menu(&self, path: &Path) -> Result<(IContextMenu, HMENU), ()> {'
body_start = method.index(sig)
open_brace = method.index('{', body_start)
# Buscar la llave de cierre al nivel 0 (texto sin llaves dentro).
depth = 0
close = None
for i in range(open_brace, len(method)):
    if method[i] == '{':
        depth += 1
    elif method[i] == '}':
        depth -= 1
        if depth == 0:
            close = i
            break
assert close is not None, 'no se encontro el cierre de build_shell_menu'

inner = method[open_brace + 1:close]

free_fn = (
    '/// (libre para poder probarse sin una App): prepara el menu contextual\n'
    '/// nativo del shell para un elemento de una caja. Devuelve el IContextMenu\n'
    '/// y su HMENU ya poblado y validado; no bloquea.\n'
    'unsafe fn build_shell_menu_for(path: &Path) -> Result<(IContextMenu, HMENU), ()> {'
    + inner +
    '}\n\n'
)

delegate = (
    '    /// Prepara el menu contextual nativo del shell para un elemento de una\n'
    '    /// caja (Abrir, Abrir con, Copiar, Eliminar, Propiedades...).\n'
    '    unsafe fn build_shell_menu(&self, path: &Path) -> Result<(IContextMenu, HMENU), ()> {\n'
    '        build_shell_menu_for(path)\n'
    '    }\n\n'
)

# Insertar la funcion libre justo antes del primer impl App (tras los imports y helpers).
anchor = '    /// Crea la ventana controladora, las cajas y todos los recursos graficos.\n    pub fn launch('
anchor_idx = src.index(anchor)
# El impl App empieza unas lineas antes: buscar "impl App {" hacia atras desde anchor_idx.
impl_start = src.rindex('impl App {', 0, anchor_idx)
# Insertar antes del doc-comment de launch para no tocar la estructura del impl.
# Mejor: insertar la fn libre justo antes de "impl App {".
src = src[:impl_start] + free_fn + src[impl_start:]

# Reemplazar el metodo por el delegado (buscar en el texto posterior a la insercion).
src = src.replace(method, delegate, 1)

# ---------------------------------------------------------------------------
# 2) Anadir los probes e2e al final del modulo tests.
# ---------------------------------------------------------------------------
probes = '''
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
            let pair = build_shell_menu_for(&file);
            match pair {
                Ok((_menu, hmenu)) => {
                    let count = GetMenuItemCount(hmenu);
                    let _ = DestroyMenu(hmenu);
                    assert!(count > 0, "el menu nativo debe traer items, trajo {count}");
                    println!("SHELL MENU UNIT OK: {count} items");
                }
                Err(_) => panic!("build_shell_menu_for fallo para {}", file.display()),
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
            let hmem = GlobalAlloc(GMEM_MOVEABLE, buf.len() as u32);
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
        let (fx, fy) = (rc.left + 30, rc.top + 40); // sobre la primera fila de items
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

        // Cerrar el menu con Escape.
        unsafe {
            keybd_event(VK_ESCAPE, 0, KEYEVENTF_KEYUP, 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
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
'''

tail_marker = '        let _ = std::fs::remove_dir_all(&root);\n    }\n}\n'
assert src.endswith(tail_marker), 'el final del archivo no coincide con lo esperado'
src = src[: -len(tail_marker)] + '        let _ = std::fs::remove_dir_all(&root);\n    }\n' + probes

# ---------------------------------------------------------------------------
# 3) Imports adicionales para los probes (dentro del modulo tests).
# ---------------------------------------------------------------------------
old_use = 'mod tests {\n    use super::*;\n    use crate::config::Rule;'
new_use = (
    'mod tests {\n'
    '    use super::*;\n'
    '    use crate::config::Rule;\n'
    '    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};\n'
    '    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};\n'
    '    use windows::Win32::UI::Input::KeyboardAndMouse::{\n'
    '        keybd_event, mouse_event, KEYEVENTF_KEYUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, VK_ESCAPE,\n'
    '    };'
)
assert old_use in src
src = src.replace(old_use, new_use, 1)

open(path, 'w', encoding='utf-8', newline='').write(src)
print('PATCH OK: build_shell_menu_for + 3 probes anadidos')
