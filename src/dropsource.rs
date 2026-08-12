// IDropSource con vtable manual — mismo patron que FenceDropTarget.
// Incluido desde ui.rs. GiveFeedback usa u32 (no DROPEFFECT) para
// eliminar cualquier duda de ABI con structs pasados por valor.

#[repr(C)]
pub struct MyDropSource {
    vtbl: *const MyDropSourceVtbl,
}

#[repr(C)]
pub struct MyDropSourceVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> windows::core::HRESULT,
    pub add_ref: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    pub query_continue_drag:
        unsafe extern "system" fn(*mut std::ffi::c_void, windows::Win32::Foundation::BOOL, u32) -> windows::core::HRESULT,
    pub give_feedback:
        unsafe extern "system" fn(*mut std::ffi::c_void, u32) -> windows::core::HRESULT,
}

static DROP_SOURCE_VTBL: MyDropSourceVtbl = MyDropSourceVtbl {
    query_interface: drop_source_qi,
    add_ref: drop_source_addref,
    release: drop_source_release,
    query_continue_drag: drop_source_qcd,
    give_feedback: drop_source_gf,
};

impl MyDropSource {
    pub fn new() -> Self {
        Self { vtbl: &DROP_SOURCE_VTBL }
    }
}

unsafe extern "system" fn drop_source_qi(
    this: *mut std::ffi::c_void,
    riid: *const GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> windows::core::HRESULT {
    const IID_UNK: GUID = GUID::from_u128(0x0000_0000_0000_0000_c000_0000_0000_0046);
    const IID_DS: GUID = GUID::from_u128(0x0000_0121_0000_0000_c000_0000_0000_0046);
    if riid.is_null() || ppv.is_null() {
        return windows::core::HRESULT(0x8000_4002u32 as i32);
    }
    if *riid == IID_UNK || *riid == IID_DS {
        *ppv = this;
        return windows::core::HRESULT(0);
    }
    *ppv = std::ptr::null_mut();
    windows::core::HRESULT(0x8000_4002u32 as i32)
}

unsafe extern "system" fn drop_source_addref(_: *mut std::ffi::c_void) -> u32 { 1 }
unsafe extern "system" fn drop_source_release(_: *mut std::ffi::c_void) -> u32 { 1 }

unsafe extern "system" fn drop_source_qcd(
    _this: *mut std::ffi::c_void,
    esc: windows::Win32::Foundation::BOOL,
    ks: u32,
) -> windows::core::HRESULT {
    const MK_LBUTTON: u32 = 0x0001;
    if esc.as_bool() { return windows::core::HRESULT(0x00040101u32 as i32); }
    if (ks & MK_LBUTTON) == 0 { return windows::core::HRESULT(0x00040100u32 as i32); }
    windows::core::HRESULT(0)
}

unsafe extern "system" fn drop_source_gf(
    _this: *mut std::ffi::c_void,
    _effect: u32,
) -> windows::core::HRESULT {
    windows::core::HRESULT(0x00040102u32 as i32)
}
