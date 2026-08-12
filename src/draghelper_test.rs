// start_ole_drag: arrastre OLE nativo hacia Explorer y otras aplicaciones.
// Incluido desde ui.rs tras dropsource.rs (MyDropSource ya definido).
pub fn start_ole_drag(paths: &[PathBuf]) {
    if paths.is_empty() { return; }

    // SHParseDisplayName es más robusto que ILCreateFromPathW (maneja paths relativos, UNC, etc).
    let mut raws = Vec::new();
    let mut ptrs: Vec<*const ITEMIDLIST> = Vec::new();
    for p in paths {
        let w = wide(p.to_str().unwrap_or(""));
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None) }.is_ok()
            && !pidl.is_null()
        {
            ptrs.push(pidl as *const ITEMIDLIST);
            raws.push(pidl);
        }
    }
    if !ptrs.is_empty() {
        if let Ok(item_array) = unsafe { SHCreateShellItemArrayFromIDLists(&ptrs) } {
            if let Ok(data_obj) = unsafe { item_array.BindToHandler(None, &BHID_DataObject) } {
                do_drag(data_obj);
            }
            drop(item_array);
        }
    }
    for p in raws { unsafe { CoTaskMemFree(Some(p as *const std::ffi::c_void)); } }
}

fn do_drag(data_obj: IDataObject) {
    let src = Box::leak(Box::new(MyDropSource::new()));
    let drop_source: IDropSource = unsafe {
        IDropSource::from_raw(src as *mut MyDropSource as *mut std::ffi::c_void)
    };
    let mut effect = DROPEFFECT(0);
    let ok = DROPEFFECT(DROPEFFECT_MOVE.0 | DROPEFFECT_COPY.0);
    unsafe { let _ = DoDragDrop(&data_obj, &drop_source, ok, &mut effect as *mut _); }
}
