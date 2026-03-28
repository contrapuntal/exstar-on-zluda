use std::{ffi::c_void, ptr, slice};
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE};

pub(crate) fn exstar_should_suppress_prestartcheck_timer(callback: *const c_void) -> bool {
    let _ = callback;
    false
}

pub(crate) unsafe fn exstar_patch_prestartcheck_module(handle: *mut c_void) {
    const PATCH_OFFSET: usize = 0x59f1;
    const PATCH_LEN: usize = 8;
    const ORIG_BYTES: [u8; PATCH_LEN] = [0x84, 0xDB, 0x0F, 0x84, 0xA7, 0x07, 0x00, 0x00];
    const BAD_PATCH_BYTES: [u8; PATCH_LEN] = [0x90, 0x90, 0xE9, 0x35, 0x0C, 0x00, 0x00, 0x90];
    const FIXED_PATCH_BYTES: [u8; PATCH_LEN] = [0x84, 0xDB, 0xE9, 0xA8, 0x07, 0x00, 0x00, 0x90];

    if handle.is_null() {
        return;
    }
    let patch_ptr = (handle as usize + PATCH_OFFSET) as *mut u8;
    let current = slice::from_raw_parts(patch_ptr.cast_const(), PATCH_LEN);
    if current == FIXED_PATCH_BYTES {
        return;
    }
    if current != ORIG_BYTES && current != BAD_PATCH_BYTES {
        crate::log_exstar_host(format_args!(
            "kind=compat action=patch_prestartcheck status=unexpected_bytes handle={:p} offset=0x{:x} bytes={:02x?}",
            handle,
            PATCH_OFFSET,
            current
        ));
        return;
    }

    let mut old_protect = 0u32;
    if VirtualProtect(
        patch_ptr.cast(),
        PATCH_LEN,
        PAGE_EXECUTE_READWRITE,
        &mut old_protect,
    ) == 0
    {
        crate::log_exstar_host(format_args!(
            "kind=compat action=patch_prestartcheck status=virtualprotect_failed handle={:p} offset=0x{:x}",
            handle,
            PATCH_OFFSET
        ));
        return;
    }
    ptr::copy_nonoverlapping(FIXED_PATCH_BYTES.as_ptr(), patch_ptr, PATCH_LEN);
    let mut restore_protect = 0u32;
    VirtualProtect(
        patch_ptr.cast(),
        PATCH_LEN,
        old_protect,
        &mut restore_protect,
    );
    crate::log_exstar_host(format_args!(
        "kind=compat action=patch_prestartcheck status=patched handle={:p} offset=0x{:x} from={:02x?} to={:02x?}",
        handle,
        PATCH_OFFSET,
        current,
        FIXED_PATCH_BYTES
    ));
}
