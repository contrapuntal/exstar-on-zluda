use crate::r#impl::{self, context, debug, device, function, module};
use cuda_types::cuda::*;
use hip_runtime_sys::*;
use libloading::Library;
use std::{
    backtrace::Backtrace,
    collections::BTreeMap,
    ffi::{c_void, CStr, CString},
    mem, ptr, slice,
    sync::{Mutex, OnceLock},
    usize,
};
use zluda_common::{constants, FromCuda, LiveCheck};
#[cfg(windows)]
use zluda_windows::get_module_path_for_function;

#[cfg_attr(windows, path = "os_win.rs")]
#[cfg_attr(not(windows), path = "os_unix.rs")]
mod os;

fn log_cls_success_backtrace(
    operation: &str,
    ctx: CUcontext,
    key: *mut c_void,
    value: *mut c_void,
    storage_entries: usize,
) {
    if !debug::trace_cls_success() {
        return;
    }
    let backtrace = format!("{:?}", Backtrace::force_capture()).replace('\n', " | ");
    debug::log_launch(format_args!(
        "phase=dark_api_trace table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_{} effective_ctx={:?} key={key:p} value={value:p} storage_entries={} backtrace={}",
        operation,
        ctx,
        storage_entries,
        backtrace
    ));
}

pub(crate) struct GlobalState {
    pub devices: Vec<Device>,
    pub cache_path: Option<String>,
    pub allocations: Mutex<Allocations>,
    pub compute_capability: (i32, i32),
}

pub(crate) struct Allocations {
    pub pointers: BTreeMap<usize, AllocationInfo>,
}

impl Allocations {
    pub fn new() -> Self {
        Allocations {
            pointers: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, ptr: usize, size: usize, context: CUcontext) {
        self.pointers.insert(ptr, AllocationInfo { size, context });
    }

    pub fn get_offset_and_info(&self, ptr: usize) -> Option<(usize, AllocationInfo)> {
        // Find last pair where `start <= ptr`
        let (start, alloc) = self.pointers.range(..=ptr).rev().next()?;
        // Check if allocation contains the pointer
        if start + alloc.size > ptr {
            Some((ptr - start, *alloc))
        } else {
            None
        }
    }

    pub fn remove(&mut self, ptr: usize) {
        self.pointers.remove(&ptr);
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct AllocationInfo {
    pub size: usize,
    pub context: CUcontext,
}

pub(crate) struct Device {
    pub(crate) _comgr_isa: CString,
    primary_context: LiveCheck<context::Context>,
}

impl Device {
    pub(crate) fn primary_context<'a>(&'a self) -> (&'a context::Context, CUcontext) {
        unsafe {
            (
                self.primary_context.data.assume_init_ref(),
                self.primary_context.as_handle(),
            )
        }
    }
}

pub(crate) fn device(dev: i32) -> Result<&'static Device, CUerror> {
    global_state()?
        .devices
        .get(dev as usize)
        .ok_or(CUerror::INVALID_DEVICE)
}

pub(crate) fn global_state() -> Result<&'static GlobalState, CUerror> {
    static GLOBAL_STATE: OnceLock<Result<GlobalState, CUerror>> = OnceLock::new();
    fn cast_slice<'a>(bytes: &'a [i8]) -> &'a [u8] {
        unsafe { slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len()) }
    }
    GLOBAL_STATE
        .get_or_init(|| {
            let mut device_count = 0;
            unsafe { hipGetDeviceCount(&mut device_count) }?;
            let allocations = Mutex::new(Allocations::new());
            Ok(GlobalState {
                allocations,
                devices: (0..device_count)
                    .map(|i| {
                        let mut props = unsafe { mem::zeroed() };
                        unsafe { hipGetDevicePropertiesR0600(&mut props, i) }?;
                        Ok::<_, CUerror>(Device {
                            _comgr_isa: CStr::from_bytes_until_nul(cast_slice(
                                &props.gcnArchName[..],
                            ))
                            .map_err(|_| CUerror::UNKNOWN)?
                            .to_owned(),
                            primary_context: LiveCheck::new(context::Context::new(i)?),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                cache_path: zluda_cache::ModuleCache::create_cache_dir_and_get_path(),
                compute_capability: constants::compute_capability(),
            })
        })
        .as_ref()
        .map_err(|e| *e)
}

pub(crate) fn init(flags: ::core::ffi::c_uint) -> CUresult {
    super::debug::log_launch(format_args!(
        "op=cuInit phase=enter flags={} pid={} exe={:?}",
        flags,
        std::process::id(),
        std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
    ));
    let hip_result = unsafe { hipInit(flags) };
    super::debug::log_launch(format_args!(
        "op=cuInit phase=after_hipInit result={:?}",
        hip_result
    ));
    hip_result?;
    let gs_result = global_state();
    super::debug::log_launch(format_args!(
        "op=cuInit phase=after_global_state result={:?}",
        gs_result.as_ref().map(|_| "ok").map_err(|e| *e)
    ));
    gs_result?;
    super::debug::log_launch(format_args!("op=cuInit phase=complete"));
    Ok(())
}

struct UnknownBuffer<const S: usize> {
    buffer: std::cell::UnsafeCell<[u32; S]>,
}

impl<const S: usize> UnknownBuffer<S> {
    const fn new() -> Self {
        UnknownBuffer {
            buffer: std::cell::UnsafeCell::new([0; S]),
        }
    }
    const fn byte_len(&self) -> usize {
        S * std::mem::size_of::<u32>()
    }
}

unsafe impl<const S: usize> Sync for UnknownBuffer<S> {}

static UNKNOWN_BUFFER1: UnknownBuffer<1024> = UnknownBuffer::new();
static UNKNOWN_BUFFER2: UnknownBuffer<14> = UnknownBuffer::new();

struct DarkApi {}

unsafe fn describe_fatbinc_wrapper(
    fatbinc_wrapper: *const cuda_types::dark_api::FatbincWrapper,
) -> String {
    let Some(wrapper) = fatbinc_wrapper.as_ref() else {
        return "wrapper=null".to_string();
    };
    match wrapper.data.as_ref() {
        Some(header) => format!(
            "wrapper_magic=0x{:08x} wrapper_version={} data={:p} fatbin_magic=0x{:08x} fatbin_version={} header_size={} files_size={}",
            wrapper.magic,
            wrapper.version,
            wrapper.data,
            header.magic,
            header.version,
            header.header_size,
            header.files_size
        ),
        None => format!(
            "wrapper_magic=0x{:08x} wrapper_version={} data={:p} fatbin_header=null",
            wrapper.magic,
            wrapper.version,
            wrapper.data
        ),
    }
}

impl ::dark_api::cuda::CudaDarkApi for DarkApi {
    unsafe extern "system" fn get_module_from_cubin(
        result: *mut cuda_types::cuda::CUmodule,
        fatbinc_wrapper: *const cuda_types::dark_api::FatbincWrapper,
    ) -> cuda_types::cuda::CUresult {
        let wrapper_desc = unsafe { describe_fatbinc_wrapper(fatbinc_wrapper) };
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CUDART_INTERFACE fn=get_module_from_cubin result_ptr={result:p} fatbinc_wrapper={fatbinc_wrapper:p} {wrapper_desc}"
        ));
        let result = match result.as_mut() {
            Some(p) => p,
            None => return CUresult::ERROR_INVALID_VALUE,
        };
        let data = fatbinc_wrapper
            .cast::<c_void>()
            .as_ref()
            .ok_or(CUerror::INVALID_VALUE)?;
        let load_result = module::load_data(result, data);
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CUDART_INTERFACE fn=get_module_from_cubin module={:?} result={:?} {wrapper_desc}",
            *result, load_result
        ));
        load_result
    }

    unsafe extern "system" fn cudart_interface_fn2(
        pctx: *mut cuda_types::cuda::CUcontext,
        hip_dev: hipDevice_t,
    ) -> cuda_types::cuda::CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CUDART_INTERFACE fn=cudart_interface_fn2 pctx_ptr={pctx:p} hip_dev={}",
            hip_dev
        ));
        let pctx = match pctx.as_mut() {
            Some(p) => p,
            None => return CUresult::ERROR_INVALID_VALUE,
        };

        let (_, cu_ctx) = device::get_primary_context(hip_dev)?;
        *pctx = cu_ctx;
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CUDART_INTERFACE fn=cudart_interface_fn2 cu_ctx={:?} result={:?}",
            *pctx, CUresult::SUCCESS
        ));
        Ok(())
    }

    unsafe extern "system" fn get_module_from_cubin_ext1(
        result: *mut cuda_types::cuda::CUmodule,
        fatbinc_wrapper: *const cuda_types::dark_api::FatbincWrapper,
        arg3: *mut std::ffi::c_void,
        arg4: *mut std::ffi::c_void,
        arg5: u32,
    ) -> cuda_types::cuda::CUresult {
        let wrapper_desc = unsafe { describe_fatbinc_wrapper(fatbinc_wrapper) };
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CUDART_INTERFACE fn=get_module_from_cubin_ext1 result_ptr={result:p} fatbinc_wrapper={fatbinc_wrapper:p} arg3={arg3:p} arg4={arg4:p} arg5={arg5} {wrapper_desc}"
        ));
        if arg3 != ptr::null_mut() || arg4 != ptr::null_mut() || arg5 != 0 {
            return CUresult::ERROR_NOT_SUPPORTED;
        }
        let result = match result.as_mut() {
            Some(p) => p,
            None => return CUresult::ERROR_INVALID_VALUE,
        };
        let data = fatbinc_wrapper
            .cast::<c_void>()
            .as_ref()
            .ok_or(CUerror::INVALID_VALUE)?;
        let load_result = module::load_data(result, data);
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CUDART_INTERFACE fn=get_module_from_cubin_ext1 module={:?} result={:?} {wrapper_desc}",
            *result, load_result
        ));
        load_result
    }

    unsafe extern "system" fn cudart_interface_fn7(_arg1: usize) -> () {
        debug::log_launch(format_args!(
            "phase=dark_api table=CUDART_INTERFACE fn=cudart_interface_fn7 arg1={}",
            _arg1
        ));
        ()
    }

    unsafe extern "system" fn get_module_from_cubin_ext2(
        fatbin_header: *const cuda_types::dark_api::FatbinHeader,
        result: *mut cuda_types::cuda::CUmodule,
        arg3: *mut std::ffi::c_void,
        arg4: *mut std::ffi::c_void,
        arg5: u32,
    ) -> cuda_types::cuda::CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CUDART_INTERFACE fn=get_module_from_cubin_ext2 fatbin_header={fatbin_header:p} result_ptr={result:p} arg3={arg3:p} arg4={arg4:p} arg5={arg5}"
        ));
        if arg3 != ptr::null_mut() || arg4 != ptr::null_mut() || arg5 != 0 {
            return CUresult::ERROR_NOT_SUPPORTED;
        }
        let result = match result.as_mut() {
            Some(p) => p,
            None => return CUresult::ERROR_INVALID_VALUE,
        };
        let data = fatbin_header
            .cast::<c_void>()
            .as_ref()
            .ok_or(CUerror::INVALID_VALUE)?;
        let load_result = module::load_data(result, data);
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CUDART_INTERFACE fn=get_module_from_cubin_ext2 module={:?} result={:?}",
            *result, load_result
        ));
        load_result
    }

    unsafe extern "system" fn get_unknown_buffer1(
        ptr: *mut *mut std::ffi::c_void,
        size: *mut usize,
    ) -> () {
        *ptr = UNKNOWN_BUFFER1.buffer.get() as *mut std::ffi::c_void;
        *size = UNKNOWN_BUFFER1.byte_len();
        debug::log_launch(format_args!(
            "phase=dark_api table=TOOLS_RUNTIME_CALLBACK_HOOKS fn=get_unknown_buffer1 buffer={:p} size={}",
            *ptr, *size
        ));
    }

    unsafe extern "system" fn get_unknown_buffer2(
        ptr: *mut *mut std::ffi::c_void,
        size: *mut usize,
    ) -> () {
        *ptr = UNKNOWN_BUFFER2.buffer.get() as *mut std::ffi::c_void;
        *size = UNKNOWN_BUFFER2.byte_len();
        debug::log_launch(format_args!(
            "phase=dark_api table=TOOLS_RUNTIME_CALLBACK_HOOKS fn=get_unknown_buffer2 buffer={:p} size={}",
            *ptr, *size
        ));
    }

    unsafe extern "system" fn context_local_storage_put(
        cu_ctx: CUcontext,
        key: *mut c_void,
        value: *mut c_void,
        dtor_cb: Option<extern "system" fn(CUcontext, *mut c_void, *mut c_void)>,
    ) -> CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put cu_ctx={:?} key={key:p} value={value:p} has_dtor={}",
            cu_ctx,
            dtor_cb.is_some()
        ));
        if debug::cls_disabled() {
            debug::log_launch(format_args!(
                "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put result={:?} reason=disabled",
                CUresult::ERROR_INVALID_HANDLE
            ));
            return CUresult::ERROR_INVALID_HANDLE;
        }
        if debug::cls_put_fails() {
            debug::log_launch(format_args!(
                "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put result={:?} reason=fail_put",
                CUresult::ERROR_INVALID_HANDLE
            ));
            return CUresult::ERROR_INVALID_HANDLE;
        }
        let _ctx = if cu_ctx.0 != ptr::null_mut() {
            cu_ctx
        } else {
            let mut current_ctx: CUcontext = CUcontext(ptr::null_mut());
            context::get_current(&mut current_ctx)?;
            current_ctx
        };
        let ctx_obj: &context::Context = FromCuda::<_, CUerror>::from_cuda(&_ctx)?;
        let mut trace_success_put = false;
        let mut trace_put_storage_entries = 0usize;
        let mut trace_put_value = ptr::null_mut();
        ctx_obj.with_state_mut(|state: &mut context::ContextState| {
            let key_usize = key as usize;
            let fail_count = if debug::compat_cls_module_window() {
                let miss_count = state.storage_get_misses.get(&key_usize).copied().unwrap_or(0);
                let storage_present = state.storage.contains_key(&key_usize);
                let (module_matches, module_path_text) = dtor_cb
                    .and_then(|cb| {
                        #[cfg(windows)]
                        {
                            get_module_path_for_function(cb as usize)
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = cb;
                            None
                        }
                    })
                    .map(|path| {
                        let path_text = path.to_string_lossy().into_owned();
                        (
                            path_text
                                .to_ascii_lowercase()
                                .contains("sn3dcolorcorrect.dll"),
                            path_text,
                        )
                    })
                    .unwrap_or((false, String::from("<unknown>")));
                let null_ctx = cu_ctx.0 == ptr::null_mut();
                let should_gate = null_ctx
                    && module_matches
                    && !storage_present
                    && miss_count >= 2;
                debug::log_launch(format_args!(
                    "phase=dark_api table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put_compat_gate effective_ctx={:?} key={key:p} null_ctx={} miss_count={} storage_present={} has_dtor={} module_matches={} module_path={} should_gate={}",
                    _ctx,
                    null_ctx,
                    miss_count,
                    storage_present,
                    dtor_cb.is_some(),
                    module_matches,
                    module_path_text,
                    should_gate
                ));
                if should_gate {
                    2
                } else {
                    0
                }
            } else if debug::compat_cls_pattern_window() {
                let miss_count = state.storage_get_misses.get(&key_usize).copied().unwrap_or(0);
                let storage_present = state.storage.contains_key(&key_usize);
                let null_ctx = cu_ctx.0 == ptr::null_mut();
                let should_gate = null_ctx
                    && dtor_cb.is_some()
                    && !storage_present
                    && miss_count >= 2;
                debug::log_launch(format_args!(
                    "phase=dark_api table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put_pattern_gate effective_ctx={:?} key={key:p} null_ctx={} miss_count={} storage_present={} has_dtor={} should_gate={}",
                    _ctx,
                    null_ctx,
                    miss_count,
                    storage_present,
                    dtor_cb.is_some(),
                    should_gate
                ));
                if should_gate {
                    2
                } else {
                    0
                }
            } else if debug::compat_cls_put_window() {
                if dtor_cb.is_some() && !state.storage.contains_key(&key_usize) {
                    2
                } else {
                    0
                }
            } else {
                debug::cls_put_fail_count().unwrap_or_else(|| {
                    if debug::cls_first_put_fails() {
                        1
                    } else {
                        0
                    }
                })
            };
            let observed_failures = state.failed_storage_puts.entry(key_usize).or_insert(0);
            if fail_count > 0 && *observed_failures < fail_count {
                *observed_failures += 1;
                debug::log_launch(format_args!(
                    "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put effective_ctx={:?} result={:?} reason={} key={key:p} failure_index={} failure_target={}",
                    _ctx,
                    CUresult::ERROR_INVALID_HANDLE,
                    if debug::compat_cls_module_window() {
                        "compat_module_put_window"
                    } else if debug::compat_cls_pattern_window() {
                        "compat_pattern_put_window"
                    } else if debug::compat_cls_put_window() {
                        "compat_put_window"
                    } else {
                        "fail_put_window"
                    },
                    *observed_failures,
                    fail_count
                ));
                return CUresult::ERROR_INVALID_HANDLE;
            }
            let (stored_value, stored_dtor, shim_reason) = if debug::cls_null_value() {
                (ptr::null_mut(), None, "null_value")
            } else {
                (value, dtor_cb, "verbatim")
            };
            let replaced = state.storage.insert(
                key_usize,
                context::StorageData {
                    value: stored_value as usize,
                    reset_cb: stored_dtor,
                    handle: _ctx,
                },
            );
            state.storage_get_misses.remove(&key_usize);
            state.successful_storage_gets.remove(&key_usize);
            if debug::trace_cls_success() {
                trace_success_put = state.traced_storage_puts.insert(key_usize);
                trace_put_storage_entries = state.storage.len();
                trace_put_value = stored_value;
            }
            debug::log_launch(format_args!(
                "phase=dark_api table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put_state effective_ctx={:?} key={key:p} storage_entries={} replaced={} input_value={value:p} stored_value={stored_value:p} input_dtor={:p} stored_dtor={:p} old_value={:p} old_dtor={:p} shim={}",
                _ctx,
                state.storage.len(),
                replaced.is_some(),
                dtor_cb.map(|cb| cb as *const ()).unwrap_or(ptr::null()),
                stored_dtor.map(|cb| cb as *const ()).unwrap_or(ptr::null()),
                replaced
                    .as_ref()
                    .map(|entry| entry.value as *mut c_void)
                    .unwrap_or(ptr::null_mut()),
                replaced
                    .as_ref()
                    .and_then(|entry| entry.reset_cb.map(|cb| cb as *const ()))
                    .unwrap_or(ptr::null()),
                shim_reason,
            ));
            Ok(())
        })?;
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_put effective_ctx={:?} result={:?} shim={}",
            _ctx,
            CUresult::SUCCESS,
            if debug::cls_null_value() {
                "null_value"
            } else {
                "verbatim"
            }
        ));
        if trace_success_put {
            log_cls_success_backtrace("put", _ctx, key, trace_put_value, trace_put_storage_entries);
        }
        Ok(())
    }

    unsafe extern "system" fn context_local_storage_delete(
        cu_ctx: CUcontext,
        key: *mut c_void,
    ) -> CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_delete cu_ctx={:?} key={key:p}",
            cu_ctx
        ));
        if debug::cls_disabled() {
            debug::log_launch(format_args!(
                "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_delete cu_ctx={:?} result={:?} reason=disabled",
                cu_ctx,
                CUresult::ERROR_INVALID_HANDLE
            ));
            return CUresult::ERROR_INVALID_HANDLE;
        }
        let ctx_obj: &context::Context = FromCuda::<_, CUerror>::from_cuda(&cu_ctx)?;
        ctx_obj.with_state_mut(|state: &mut context::ContextState| {
            let key_usize = key as usize;
            let removed = state.storage.remove(&key_usize);
            state.storage_get_misses.remove(&key_usize);
            state.successful_storage_gets.remove(&key_usize);
            state.traced_storage_puts.remove(&key_usize);
            state.traced_storage_gets.remove(&key_usize);
            debug::log_launch(format_args!(
                "phase=dark_api table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_delete_state cu_ctx={:?} key={key:p} removed={} old_value={:p} old_dtor={:p} storage_entries={}",
                cu_ctx,
                removed.is_some(),
                removed
                    .as_ref()
                    .map(|entry| entry.value as *mut c_void)
                    .unwrap_or(ptr::null_mut()),
                removed
                    .as_ref()
                    .and_then(|entry| entry.reset_cb.map(|cb| cb as *const ()))
                    .unwrap_or(ptr::null()),
                state.storage.len()
            ));
            Ok(())
        })?;
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_delete cu_ctx={:?} result={:?}",
            cu_ctx,
            CUresult::SUCCESS
        ));
        Ok(())
    }

    unsafe extern "system" fn context_local_storage_get(
        value: *mut *mut c_void,
        cu_ctx: CUcontext,
        key: *mut c_void,
    ) -> CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_get value_ptr={value:p} cu_ctx={:?} key={key:p}",
            cu_ctx
        ));
        if debug::cls_disabled() {
            if let Some(value) = value.as_mut() {
                *value = ptr::null_mut();
            }
            debug::log_launch(format_args!(
                "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_get effective_ctx={:?} key={key:p} value={:p} result={:?} storage_entries=0 reason=disabled",
                cu_ctx,
                ptr::null_mut::<c_void>(),
                CUresult::ERROR_INVALID_HANDLE
            ));
            return CUresult::ERROR_INVALID_HANDLE;
        }
        if debug::cls_get_fails() {
            if let Some(value) = value.as_mut() {
                *value = ptr::null_mut();
            }
            debug::log_launch(format_args!(
                "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_get effective_ctx={:?} key={key:p} value={:p} result={:?} storage_entries=0 reason=fail_get",
                cu_ctx,
                ptr::null_mut::<c_void>(),
                CUresult::ERROR_INVALID_HANDLE
            ));
            return CUresult::ERROR_INVALID_HANDLE;
        }
        let mut _ctx: CUcontext;
        if cu_ctx.0 == ptr::null_mut() {
            _ctx = context::get_current_context()?;
        } else {
            _ctx = cu_ctx
        };
        let ctx_obj: &context::Context = FromCuda::<_, CUerror>::from_cuda(&_ctx)?;
        let mut storage_entries = 0usize;
        let key_usize = key as usize;
        let mut trace_success_get = false;
        let mut trace_hit_count_get = false;
        let mut successful_get_count = 0u32;
        ctx_obj.with_state_mut(|state: &mut context::ContextState| {
            storage_entries = state.storage.len();
            match state.storage.get(&key_usize) {
                Some(data) => {
                    *value = data.value as *mut c_void;
                    state.storage_get_misses.remove(&key_usize);
                    successful_get_count = {
                        let hit_entry = state.successful_storage_gets.entry(key_usize).or_insert(0);
                        *hit_entry += 1;
                        *hit_entry
                    };
                    if let Some(fail_after_count) = debug::cls_get_fail_after_count() {
                        if successful_get_count > fail_after_count {
                            *value = ptr::null_mut();
                            debug::log_launch(format_args!(
                                "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_get effective_ctx={:?} key={key:p} value={:p} result={:?} storage_entries={} hit_count={} fail_after_count={} reason=fail_get_after_count",
                                _ctx,
                                ptr::null_mut::<c_void>(),
                                CUresult::ERROR_INVALID_HANDLE,
                                storage_entries,
                                successful_get_count,
                                fail_after_count
                            ));
                            return CUresult::ERROR_INVALID_HANDLE;
                        }
                    }
                    if debug::trace_cls_success() {
                        trace_success_get = state.traced_storage_gets.insert(key_usize);
                    }
                    if let Some(target_hit_count) = debug::trace_cls_get_hit_count() {
                        trace_hit_count_get = successful_get_count == target_hit_count;
                    }
                }
                None => {
                    let miss_count = {
                        let miss_entry = state.storage_get_misses.entry(key_usize).or_insert(0);
                        *miss_entry += 1;
                        *miss_entry
                    };
                    debug::log_launch(format_args!(
                        "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_get effective_ctx={:?} key={key:p} value={:p} result={:?} storage_entries={} miss_count={}",
                        _ctx,
                        ptr::null_mut::<c_void>(),
                        CUresult::ERROR_INVALID_HANDLE,
                        storage_entries,
                        miss_count
                    ));
                    return CUresult::ERROR_INVALID_HANDLE;
                }
            }
            Ok(())
        })?;
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CONTEXT_LOCAL_STORAGE_INTERFACE_V0301 fn=context_local_storage_get effective_ctx={:?} key={key:p} value={:p} result={:?} storage_entries={} hit_count={}",
            _ctx,
            *value,
            CUresult::SUCCESS,
            storage_entries,
            successful_get_count
        ));
        if trace_success_get {
            log_cls_success_backtrace("get", _ctx, key, *value, storage_entries);
        }
        if trace_hit_count_get {
            let operation = format!("get_hit_{}", successful_get_count);
            log_cls_success_backtrace(&operation, _ctx, key, *value, storage_entries);
        }
        Ok(())
    }

    unsafe extern "system" fn ctx_create_v2_bypass(
        _pctx: *mut cuda_types::cuda::CUcontext,
        _flags: ::std::os::raw::c_uint,
        _dev: cuda_types::cuda::CUdevice,
    ) -> cuda_types::cuda::CUresult {
        Err(r#impl::unimplemented())
    }

    unsafe extern "system" fn heap_alloc(
        _heap_alloc_record_ptr: *mut *const std::ffi::c_void,
        _arg2: usize,
        _arg3: usize,
    ) -> cuda_types::cuda::CUresult {
        Err(r#impl::unimplemented())
    }

    unsafe extern "system" fn heap_free(
        _heap_alloc_record_ptr: *const std::ffi::c_void,
        _arg2: *mut usize,
    ) -> cuda_types::cuda::CUresult {
        Err(r#impl::unimplemented())
    }

    unsafe extern "system" fn device_get_attribute_ext(
        dev: cuda_types::cuda::CUdevice,
        attribute: std::ffi::c_uint,
        unknown: std::ffi::c_int,
        result: *mut [usize; 2],
    ) -> cuda_types::cuda::CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CUDART_INTERFACE fn=device_get_attribute_ext dev={:?} attribute={} unknown={} result_ptr={result:p}",
            dev,
            attribute,
            unknown
        ));
        Err(r#impl::unimplemented())
    }

    unsafe extern "system" fn device_get_something(
        result: *mut std::ffi::c_uchar,
        dev: cuda_types::cuda::CUdevice,
    ) -> cuda_types::cuda::CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_enter table=CUDART_INTERFACE fn=device_get_something dev={:?} result_ptr={result:p}",
            dev
        ));
        Err(r#impl::unimplemented())
    }

    unsafe extern "system" fn integrity_check(
        version: u32,
        unix_seconds: u64,
        result: *mut [u64; 2],
    ) -> cuda_types::cuda::CUresult {
        let current_process = std::process::id();
        let current_thread = os::current_thread();

        let integrity_check_table = EXPORT_TABLE.INTEGRITY_CHECK.as_ptr().cast();
        let cudart_table = EXPORT_TABLE.CUDART_INTERFACE.as_ptr().cast();
        let fn_address = EXPORT_TABLE.INTEGRITY_CHECK[1];

        let devices = get_device_hash_info()?;
        let device_count = devices.len() as u32;
        let get_device = |dev| devices[dev as usize];

        let hash = ::dark_api::integrity_check(
            version,
            unix_seconds,
            cuda_types::cuda::CUDA_VERSION,
            current_process,
            current_thread,
            integrity_check_table,
            cudart_table,
            fn_address,
            device_count,
            get_device,
        );
        *result = hash;
        Ok(())
    }

    unsafe extern "system" fn context_check(
        _ctx_in: cuda_types::cuda::CUcontext,
        result1: *mut u32,
        _result2: *mut *const std::ffi::c_void,
    ) -> cuda_types::cuda::CUresult {
        *result1 = 0;
        CUresult::SUCCESS
    }

    unsafe extern "system" fn check_fn3() -> u32 {
        0
    }

    unsafe extern "system" fn hybrid_runtime_load_get_proc_address(
        name: *const std::ffi::c_char,
        fn_ptr: *mut *const std::ffi::c_void,
        token: *mut usize,
    ) -> cuda_types::cuda::CUresult {
        let name = CStr::from_ptr(name)
            .to_str()
            .map_err(|_| CUerror::INVALID_VALUE)?;
        if name != "nvcudart_hybrid64.dll" && name != "nvcudart_hybrid64a.dll" {
            return CUresult::ERROR_INVALID_VALUE;
        }
        debug::log_launch(format_args!(
            "phase=hybrid_runtime_load dll={} fn_ptr_ptr={fn_ptr:p} token_ptr={token:p}",
            name
        ));
        let hybrid_runtime = &mut *HYBRID_RUNTIME_HANDLE.lock().map_err(|_| CUerror::UNKNOWN)?;
        let library = match hybrid_runtime {
            Some(lib) => lib,
            None => {
                let library =
                    crate::os::try_load_library(name).map_err(|_| CUerror::FILE_NOT_FOUND)?;
                *hybrid_runtime = Some(library);
                hybrid_runtime.as_ref().unwrap()
            }
        };
        let fn_ = library
            .get::<*const std::ffi::c_void>(b"__cudaGetProcAddress\0")
            .map_err(|_| CUerror::OPERATING_SYSTEM)?;

        *fn_ptr = *fn_;
        *token = 302100128;
        debug::log_launch(format_args!(
            "phase=hybrid_runtime_ready dll={} proc={:p} token={}",
            name, *fn_ptr, *token
        ));
        Ok(())
    }

    unsafe extern "system" fn hybrid_runtime_free(token: usize) -> cuda_types::cuda::CUresult {
        if token != 302100128 {
            return CUresult::ERROR_INVALID_HANDLE;
        }
        CUresult::SUCCESS
    }

    unsafe extern "system" fn load_compilers() -> cuda_types::cuda::CUresult {
        debug::log_launch(format_args!(
            "phase=dark_api_return table=CUDART_INTERFACE fn=load_compilers result={:?}",
            CUresult::SUCCESS
        ));
        CUresult::SUCCESS
    }
}

static HYBRID_RUNTIME_HANDLE: Mutex<Option<Library>> = Mutex::new(None);

fn should_log_proc_request(symbol: &str) -> bool {
    matches!(
        symbol,
        "cuLaunchKernel"
            | "cuLaunchKernelEx"
            | "cuGraphLaunch"
            | "cudaLaunch"
            | "cudaLaunchKernel"
            | "cudaGraphLaunch"
            | "cudaConfigureCall"
            | "cudaSetupArgument"
            | "cudaLaunchDevice"
            | "__cudaPushCallConfiguration"
            | "__cudaPopCallConfiguration"
    )
}

fn get_device_hash_info() -> Result<Vec<::dark_api::DeviceHashinfo>, CUerror> {
    let mut device_count = 0;
    device::get_count(&mut device_count)?;

    (0..device_count)
        .map(|dev| {
            let mut guid = unsafe { mem::zeroed() };
            device::get_uuid_v2(&mut guid, dev)?;

            let mut pci_domain = 0;
            device::get_attribute(
                &mut pci_domain,
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_PCI_DOMAIN_ID,
                dev,
            )?;

            let mut pci_bus = 0;
            device::get_attribute(
                &mut pci_bus,
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_PCI_BUS_ID,
                dev,
            )?;

            let mut pci_device = 0;
            device::get_attribute(
                &mut pci_device,
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_PCI_DEVICE_ID,
                dev,
            )?;

            Ok(::dark_api::DeviceHashinfo {
                guid: unsafe { mem::transmute(guid) },
                pci_domain,
                pci_bus,
                pci_device,
            })
        })
        .collect()
}

static EXPORT_TABLE: ::dark_api::cuda::CudaDarkApiGlobalTable =
    ::dark_api::cuda::CudaDarkApiGlobalTable::new::<DarkApi>();

fn export_table_name(p_export_table_id: &CUuuid) -> &'static str {
    if *p_export_table_id == ::dark_api::cuda::CudartInterface::GUID {
        "CUDART_INTERFACE"
    } else if *p_export_table_id == ::dark_api::cuda::ToolsTls::GUID {
        "TOOLS_TLS"
    } else if *p_export_table_id == ::dark_api::cuda::ToolsRuntimeCallbackHooks::GUID {
        "TOOLS_RUNTIME_CALLBACK_HOOKS"
    } else if *p_export_table_id == ::dark_api::cuda::HybridCudart::GUID {
        "HYBRID_CUDART"
    } else if *p_export_table_id == ::dark_api::cuda::IntegrityCheck::GUID {
        "INTEGRITY_CHECK"
    } else if *p_export_table_id == ::dark_api::cuda::ContextChecks::GUID {
        "CONTEXT_CHECKS"
    } else if *p_export_table_id == ::dark_api::cuda::ContextLocalStorageInterfaceV0301::GUID {
        "CONTEXT_LOCAL_STORAGE_INTERFACE_V0301"
    } else if *p_export_table_id == ::dark_api::cuda::CtxCreateBypass::GUID {
        "CTX_CREATE_BYPASS"
    } else {
        "UNKNOWN"
    }
}

pub(crate) fn get_export_table(
    pp_export_table: &mut *const ::core::ffi::c_void,
    p_export_table_id: &CUuuid,
) -> CUresult {
    let table_name = export_table_name(p_export_table_id);
    debug::log_launch(format_args!(
        "phase=get_export_table_enter table={} guid={:?} out_ptr={:p}",
        table_name, p_export_table_id.bytes, pp_export_table
    ));
    if let Some(table) = EXPORT_TABLE.get(p_export_table_id) {
        *pp_export_table = table.start();
        let result = cuda_types::cuda::CUresult::SUCCESS;
        debug::log_launch(format_args!(
            "phase=get_export_table_return table={} result={:?} export_table={:p}",
            table_name, result, *pp_export_table
        ));
        result
    } else {
        let result = cuda_types::cuda::CUresult::ERROR_INVALID_VALUE;
        debug::log_launch(format_args!(
            "phase=get_export_table_return table={} result={:?} export_table={:p}",
            table_name, result, *pp_export_table
        ));
        result
    }
}

pub(crate) fn get_version(version: &mut ::core::ffi::c_int) -> CUresult {
    *version = cuda_types::cuda::CUDA_VERSION as i32;
    Ok(())
}

pub(crate) unsafe fn get_proc_address(
    symbol: &CStr,
    pfn: &mut *mut ::core::ffi::c_void,
    cuda_version: ::core::ffi::c_int,
    flags: cuda_types::cuda::cuuint64_t,
) -> CUresult {
    get_proc_address_v2(symbol, pfn, cuda_version, flags, None)
}

pub(crate) unsafe fn get_proc_address_v2(
    symbol: &CStr,
    pfn: &mut *mut ::core::ffi::c_void,
    cuda_version: ::core::ffi::c_int,
    flags: cuda_types::cuda::cuuint64_t,
    symbol_status: Option<&mut cuda_types::cuda::CUdriverProcAddressQueryResult>,
) -> CUresult {
    // This implementation is mostly the same as cuGetProcAddress_v2 in zluda_trace. We may want to factor out the duplication at some point.
    let symbol_name = symbol.to_str().unwrap_or("<invalid-utf8>");
    let log_proc_request = should_log_proc_request(symbol_name);
    if log_proc_request {
        debug::log_launch(format_args!(
            "phase=get_proc_address_enter symbol={} cuda_version={} flags={}",
            symbol_name, cuda_version, flags
        ));
    }
    fn raw_match(name: &[u8], flag: u64, version: i32) -> *mut ::core::ffi::c_void {
        use crate::*;
        include!("../../../zluda_bindgen/src/process_table.rs")
    }
    let fn_ptr = raw_match(symbol.to_bytes(), flags, cuda_version);
    let result = match fn_ptr as usize {
        0 => {
            if let Some(symbol_status) = symbol_status {
                *symbol_status = cuda_types::cuda::CUdriverProcAddressQueryResult::CU_GET_PROC_ADDRESS_SYMBOL_NOT_FOUND;
            }
            *pfn = ptr::null_mut();
            CUresult::ERROR_NOT_FOUND
        }
        usize::MAX => {
            if let Some(symbol_status) = symbol_status {
                *symbol_status = cuda_types::cuda::CUdriverProcAddressQueryResult::CU_GET_PROC_ADDRESS_VERSION_NOT_SUFFICIENT;
            }
            *pfn = ptr::null_mut();
            CUresult::ERROR_NOT_FOUND
        }
        _ => {
            if let Some(symbol_status) = symbol_status {
                *symbol_status =
                    cuda_types::cuda::CUdriverProcAddressQueryResult::CU_GET_PROC_ADDRESS_SUCCESS;
            }
            *pfn = fn_ptr;
            Ok(())
        }
    };
    if log_proc_request {
        debug::log_launch(format_args!(
            "phase=get_proc_address_return symbol={} result={:?} fn_ptr={:p}",
            symbol_name, result, *pfn
        ));
    }
    result
}

pub(crate) fn profiler_start() -> CUresult {
    Ok(())
}

pub(crate) fn profiler_stop() -> CUresult {
    Ok(())
}

pub(crate) unsafe fn thread_exchange_stream_capture_mode(
    mode: *mut hipStreamCaptureMode,
) -> hipError_t {
    hipThreadExchangeStreamCaptureMode(mode)
}

pub(crate) unsafe fn occupancy_max_active_blocks_per_multiprocessor_with_flags(
    num_blocks: &mut ::core::ffi::c_int,
    func: &function::Function,
    block_size: ::core::ffi::c_int,
    dynamic_smem_size: usize,
    flags: ::core::ffi::c_uint,
) -> hipError_t {
    hipModuleOccupancyMaxActiveBlocksPerMultiprocessorWithFlags(
        num_blocks,
        func.base,
        block_size,
        dynamic_smem_size,
        flags,
    )?;
    *num_blocks = (*num_blocks).max(1);
    Ok(())
}

pub(crate) unsafe fn occupancy_max_potential_block_size(
    grid_size: &mut ::core::ffi::c_int,
    block_size: &mut ::core::ffi::c_int,
    f: &function::Function,
    _block_size_to_dynamic_smem_size: CUoccupancyB2DSize,
    dyn_shared_mem_per_blk: usize,
    block_size_limit: ::core::ffi::c_int,
) -> hipError_t {
    hipModuleOccupancyMaxPotentialBlockSize(
        grid_size,
        block_size,
        f.base,
        dyn_shared_mem_per_blk,
        block_size_limit,
    )?;
    Ok(())
}

pub(crate) unsafe fn launch_kernel_ex(
    config: &cuda_types::cuda::CUlaunchConfig,
    f: &function::Function,
    kernel_params: *mut *mut ::core::ffi::c_void,
    extra: *mut *mut ::core::ffi::c_void,
) -> CUresult {
    let attrs = std::slice::from_raw_parts(config.attrs, config.numAttrs as usize);
    if attrs.iter().any(|&attr| {
        !(attr.id == CUlaunchAttributeID::CU_LAUNCH_ATTRIBUTE_PROGRAMMATIC_STREAM_SERIALIZATION
            && attr.value.programmaticStreamSerializationAllowed == 0)
    }) {
        return CUresult::ERROR_NOT_SUPPORTED;
    }
    function::launch_kernel(
        f,
        config.gridDimX,
        config.gridDimY,
        config.gridDimZ,
        config.blockDimX,
        config.blockDimY,
        config.blockDimZ,
        config.sharedMemBytes,
        FromCuda::<_, CUerror>::from_cuda(&config.hStream)?,
        kernel_params,
        extra,
    )?;
    Ok(())
}

pub(crate) unsafe fn launch_kernel_ex_ptsz(
    config: &cuda_types::cuda::CUlaunchConfig,
    f: &function::Function,
    kernel_params: *mut *mut ::core::ffi::c_void,
    extra: *mut *mut ::core::ffi::c_void,
) -> CUresult {
    launch_kernel_ex(config, f, kernel_params, extra)
}

fn log_legacy_launch_not_supported(
    op: &str,
    f: &function::Function,
    details: std::fmt::Arguments<'_>,
) -> CUresult {
    let seq = debug::next_launch_sequence();
    debug::log_launch(format_args!(
        "seq={} phase=legacy_enter op={} kernel={} hip_func={:?} {}",
        seq,
        op,
        f.name.to_string_lossy(),
        f.base.0,
        details
    ));
    debug::log_launch(format_args!(
        "seq={} phase=legacy_return op={} kernel={} result={:?}",
        seq,
        op,
        f.name.to_string_lossy(),
        CUresult::ERROR_NOT_SUPPORTED
    ));
    CUresult::ERROR_NOT_SUPPORTED
}

pub(crate) fn func_set_block_shape(
    f: &function::Function,
    x: ::core::ffi::c_int,
    y: ::core::ffi::c_int,
    z: ::core::ffi::c_int,
) -> CUresult {
    log_legacy_launch_not_supported(
        "cuFuncSetBlockShape",
        f,
        format_args!("block={},{},{}", x, y, z),
    )
}

pub(crate) fn func_set_shared_size(f: &function::Function, bytes: ::core::ffi::c_uint) -> CUresult {
    log_legacy_launch_not_supported(
        "cuFuncSetSharedSize",
        f,
        format_args!("shared_mem={}", bytes),
    )
}

pub(crate) fn param_set_size(f: &function::Function, numbytes: ::core::ffi::c_uint) -> CUresult {
    log_legacy_launch_not_supported("cuParamSetSize", f, format_args!("numbytes={}", numbytes))
}

pub(crate) fn param_seti(
    f: &function::Function,
    offset: ::core::ffi::c_int,
    value: ::core::ffi::c_uint,
) -> CUresult {
    log_legacy_launch_not_supported(
        "cuParamSeti",
        f,
        format_args!("offset={} value={}", offset, value),
    )
}

pub(crate) fn param_setf(
    f: &function::Function,
    offset: ::core::ffi::c_int,
    value: f32,
) -> CUresult {
    log_legacy_launch_not_supported(
        "cuParamSetf",
        f,
        format_args!("offset={} value={}", offset, value),
    )
}

pub(crate) fn param_setv(
    f: &function::Function,
    offset: ::core::ffi::c_int,
    ptr: *mut ::core::ffi::c_void,
    numbytes: ::core::ffi::c_uint,
) -> CUresult {
    log_legacy_launch_not_supported(
        "cuParamSetv",
        f,
        format_args!("offset={} ptr={:p} numbytes={}", offset, ptr, numbytes),
    )
}

pub(crate) fn launch(f: &function::Function) -> CUresult {
    log_legacy_launch_not_supported("cuLaunch", f, format_args!("grid=1,1"))
}

pub(crate) fn launch_grid(
    f: &function::Function,
    grid_width: ::core::ffi::c_int,
    grid_height: ::core::ffi::c_int,
) -> CUresult {
    log_legacy_launch_not_supported(
        "cuLaunchGrid",
        f,
        format_args!("grid={},{}", grid_width, grid_height),
    )
}

pub(crate) fn launch_grid_async(
    f: &function::Function,
    grid_width: ::core::ffi::c_int,
    grid_height: ::core::ffi::c_int,
    h_stream: hipStream_t,
) -> CUresult {
    log_legacy_launch_not_supported(
        "cuLaunchGridAsync",
        f,
        format_args!(
            "grid={},{} stream={:?}",
            grid_width, grid_height, h_stream.0
        ),
    )
}

pub(crate) unsafe fn get_error_string(
    error: cuda_types::cuda::CUresult,
    error_string: &mut *const ::core::ffi::c_char,
) -> CUresult {
    *error_string = match error {
        CUresult::SUCCESS => c"no error".as_ptr(),
        CUresult::ERROR_INVALID_VALUE => c"invalid value".as_ptr(),
        CUresult::ERROR_OUT_OF_MEMORY => c"out of memory".as_ptr(),
        CUresult::ERROR_NOT_INITIALIZED => c"driver not initialized".as_ptr(),
        CUresult::ERROR_DEINITIALIZED => c"driver deinitialized".as_ptr(),
        CUresult::ERROR_NO_DEVICE => c"no CUDA-capable device is detected".as_ptr(),
        CUresult::ERROR_INVALID_DEVICE => c"invalid device".as_ptr(),
        CUresult::ERROR_INVALID_IMAGE => c"invalid kernel image".as_ptr(),
        CUresult::ERROR_INVALID_CONTEXT => c"invalid context".as_ptr(),
        CUresult::ERROR_CONTEXT_ALREADY_CURRENT => c"context already current".as_ptr(),
        CUresult::ERROR_MAP_FAILED => c"map failed".as_ptr(),
        CUresult::ERROR_UNMAP_FAILED => c"unmap failed".as_ptr(),
        CUresult::ERROR_ARRAY_IS_MAPPED => c"array is mapped".as_ptr(),
        CUresult::ERROR_ALREADY_MAPPED => c"already mapped".as_ptr(),
        CUresult::ERROR_NO_BINARY_FOR_GPU => c"no binary for GPU".as_ptr(),
        CUresult::ERROR_ALREADY_ACQUIRED => c"already acquired".as_ptr(),
        CUresult::ERROR_NOT_MAPPED => c"not mapped".as_ptr(),
        CUresult::ERROR_NOT_SUPPORTED => c"operation not supported".as_ptr(),
        CUresult::ERROR_INVALID_SOURCE => c"invalid source".as_ptr(),
        CUresult::ERROR_FILE_NOT_FOUND => c"file not found".as_ptr(),
        CUresult::ERROR_INVALID_HANDLE => c"invalid handle".as_ptr(),
        CUresult::ERROR_NOT_READY => c"not ready".as_ptr(),
        CUresult::ERROR_ILLEGAL_ADDRESS => c"illegal address".as_ptr(),
        CUresult::ERROR_LAUNCH_OUT_OF_RESOURCES => c"launch out of resources".as_ptr(),
        CUresult::ERROR_LAUNCH_TIMEOUT => c"launch timeout".as_ptr(),
        CUresult::ERROR_LAUNCH_INCOMPATIBLE_TEXTURING => c"launch incompatible texturing".as_ptr(),
        CUresult::ERROR_PEER_ACCESS_ALREADY_ENABLED => c"peer access already enabled".as_ptr(),
        CUresult::ERROR_PEER_ACCESS_NOT_ENABLED => c"peer access not enabled".as_ptr(),
        CUresult::ERROR_PRIMARY_CONTEXT_ACTIVE => c"primary context active".as_ptr(),
        CUresult::ERROR_CONTEXT_IS_DESTROYED => c"context is destroyed".as_ptr(),
        CUresult::ERROR_ASSERT => c"device-side assert triggered".as_ptr(),
        CUresult::ERROR_TOO_MANY_PEERS => c"too many peers".as_ptr(),
        CUresult::ERROR_HOST_MEMORY_ALREADY_REGISTERED => {
            c"host memory already registered".as_ptr()
        }
        CUresult::ERROR_HOST_MEMORY_NOT_REGISTERED => c"host memory not registered".as_ptr(),
        CUresult::ERROR_UNKNOWN => c"unknown error".as_ptr(),
        _ => c"error".as_ptr(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::i32;

    use crate::r#impl::driver::AllocationInfo;
    use crate::tests::CudaApi;
    use cuda_macros::test_cuda;
    use cuda_types::cuda::CUcontext;

    #[test_cuda]
    fn init(api: impl CudaApi) {
        api.cuInit(0);
    }

    #[test]
    fn get_allocation() {
        let ctx1 = CUcontext(0x1234 as _);
        let ctx2 = CUcontext(0x5678 as _);
        let mut alloc_info = super::Allocations::new();
        alloc_info.insert(0x1000, 4, ctx1);
        alloc_info.insert(0x2000, 8, ctx2);
        for i in 0..4 {
            assert_eq!(
                alloc_info.get_offset_and_info(0x1000 + i),
                Some((
                    i,
                    AllocationInfo {
                        size: 4,
                        context: ctx1
                    }
                ))
            );
        }
        assert_eq!(alloc_info.get_offset_and_info(0x1000 + 4), None);
        for i in 0..8 {
            assert_eq!(
                alloc_info.get_offset_and_info(0x2000 + i),
                Some((
                    i,
                    AllocationInfo {
                        size: 8,
                        context: ctx2
                    }
                ))
            );
        }
        assert_eq!(alloc_info.get_offset_and_info(0x2000 + 8), None);
    }

    #[test_cuda]
    fn primary_context_is_inactive_on_init(api: impl CudaApi) {
        api.cuInit(0);
        let mut flags = u32::MAX;
        let mut active = i32::MAX;
        api.cuDevicePrimaryCtxGetState(0, &mut flags, &mut active);
        assert_eq!(flags, 0);
        assert_eq!(active, 0);
    }

    #[test_cuda]
    unsafe fn cudart_interface_fn2_creates_inactive_primary_ctx(api: impl CudaApi) {
        api.cuInit(0);
        let mut table_ptr = std::ptr::null();
        api.cuGetExportTable(&mut table_ptr, &dark_api::cuda::CudartInterface::GUID);
        let cuda_rt_iface = dark_api::cuda::CudartInterface::new(table_ptr);
        let mut dark_ctx = std::mem::zeroed();
        cuda_rt_iface
            .cudart_interface_fn2(&mut dark_ctx, 0)
            .unwrap();
        let mut flags = u32::MAX;
        let mut active = i32::MAX;
        api.cuDevicePrimaryCtxGetState(0, &mut flags, &mut active);
        assert_eq!(flags, 0);
        assert_eq!(active, 0);
        let mut primary_ctx = std::mem::zeroed();
        api.cuDevicePrimaryCtxRetain(&mut primary_ctx, 0);
        assert_eq!(dark_ctx.0, primary_ctx.0);
    }
}
