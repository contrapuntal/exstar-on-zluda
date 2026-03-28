use cuda_types::cuda::CUerror;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) mod r#impl;
#[cfg_attr(windows, path = "os_win.rs")]
#[cfg_attr(not(windows), path = "os_unix.rs")]
mod os;

static INITIALIZED: AtomicBool = AtomicBool::new(true);
pub(crate) fn initialized() -> bool {
    INITIALIZED.load(Ordering::SeqCst)
}

#[cfg_attr(not(windows), dtor::dtor)]
fn deinitialize() {
    INITIALIZED.store(false, Ordering::SeqCst);
}

macro_rules! unimplemented {
    ($($abi:literal fn $fn_name:ident( $($arg_id:ident : $arg_type:ty),* ) -> $ret_type:ty;)*) => {
        $(
            #[cfg_attr(not(test), no_mangle)]
            #[allow(improper_ctypes)]
            #[allow(improper_ctypes_definitions)]
            #[allow(unused_variables)]
            pub unsafe extern $abi fn $fn_name ( $( $arg_id : $arg_type),* ) -> $ret_type {
                crate::r#impl::debug::log_stream_memory(format_args!(
                    "op={} phase=unimplemented",
                    stringify!($fn_name)
                ));
                Err(r#impl::unimplemented())
            }
        )*
    };
}

macro_rules! implemented {
    ($($abi:literal fn $fn_name:ident( $($arg_id:ident : $arg_type:ty),* ) -> $ret_type:ty;)*) => {
        $(
            #[cfg_attr(not(test), no_mangle)]
            #[allow(improper_ctypes)]
            #[allow(improper_ctypes_definitions)]
            pub unsafe extern $abi fn $fn_name ( $( $arg_id : $arg_type),* ) -> $ret_type {
                if !initialized() {
                    return Err(CUerror::DEINITIALIZED);
                }
                cuda_macros::cuda_normalize_fn!( crate::r#impl::$fn_name ) ($(zluda_common::FromCuda::<_, CUerror>::from_cuda(&$arg_id)?),*)?;
                Ok(())
            }
        )*
    };
}

macro_rules! implemented_in_function {
    ($($abi:literal fn $fn_name:ident( $($arg_id:ident : $arg_type:ty),* ) -> $ret_type:ty;)*) => {
        $(
            #[cfg_attr(not(test), no_mangle)]
            #[allow(improper_ctypes)]
            #[allow(improper_ctypes_definitions)]
            pub unsafe extern $abi fn $fn_name ( $( $arg_id : $arg_type),* ) -> $ret_type {
                if !initialized() {
                    return Err(CUerror::DEINITIALIZED);
                }
                cuda_macros::cuda_normalize_fn!( crate::r#impl::function::$fn_name ) ($(zluda_common::FromCuda::<_, CUerror>::from_cuda(&$arg_id)?),*)?;
                Ok(())
            }
        )*
    };
}

macro_rules! ignored {
    ($($abi:literal fn $fn_name:ident( $($arg_id:ident : $arg_type:ty),* ) -> $ret_type:ty;)*) => {};
}

cuda_macros::cuda_function_declarations!(
    unimplemented,
    ignored
        <= [
            cuFuncSetBlockShape,
            cuFuncSetSharedSize,
            cuLaunch,
            cuLaunchGrid,
            cuLaunchGridAsync,
            cuParamSetSize,
            cuParamSeti,
            cuParamSetf,
            cuParamSetv,
        ],
    implemented
        <= [
            cuCtxCreate_v2,
            cuCtxDestroy_v2,
            cuCtxGetApiVersion,
            cuCtxGetCurrent,
            cuCtxGetDevice_v2,
            cuCtxGetDevice,
            cuCtxGetLimit,
            cuCtxGetStreamPriorityRange,
            cuCtxPopCurrent_v2,
            cuCtxPopCurrent,
            cuCtxPushCurrent_v2,
            cuCtxPushCurrent,
            cuCtxSetCurrent,
            cuCtxSetFlags,
            cuCtxSetLimit,
            cuCtxSynchronize_v2,
            cuCtxSynchronize,
            cuDeviceComputeCapability,
            cuDeviceGet,
            cuDeviceGetAttribute,
            cuDeviceGetCount,
            cuDeviceGetLuid,
            cuDeviceGetName,
            cuDeviceGetProperties,
            cuDeviceGetUuid_v2,
            cuDeviceGetUuid,
            cuDevicePrimaryCtxGetState,
            cuDevicePrimaryCtxRelease_v2,
            cuDevicePrimaryCtxRelease,
            cuDevicePrimaryCtxReset,
            cuDevicePrimaryCtxRetain,
            cuDevicePrimaryCtxSetFlags_v2,
            cuDevicePrimaryCtxSetFlags,
            cuDeviceTotalMem_v2,
            cuDriverGetVersion,
            cuEventCreate,
            cuEventDestroy_v2,
            cuEventElapsedTime_v2,
            cuEventElapsedTime,
            cuEventQuery,
            cuEventRecord,
            cuEventRecord_ptsz,
            cuEventRecordWithFlags,
            cuEventRecordWithFlags_ptsz,
            cuEventSynchronize,
            cuFuncGetAttribute,
            cuFuncSetAttribute,
            cuGetErrorString,
            cuGetExportTable,
            cuGetProcAddress_v2,
            cuGetProcAddress,
            cuGraphDestroy,
            cuGraphExecDestroy,
            cuGraphExecUpdate_v2,
            cuGraphGetNodes,
            cuGraphInstantiateWithFlags,
            cuGraphLaunch,
            cuGraphLaunch_ptsz,
            cuInit,
            cuKernelGetAttribute,
            cuKernelGetFunction,
            cuKernelSetAttribute,
            cuLaunchKernel,
            cuLaunchKernelEx,
            cuLaunchKernelEx_ptsz,
            cuLibraryGetGlobal,
            cuLibraryGetKernel,
            cuLibraryGetModule,
            cuLibraryLoadData,
            cuLibraryUnload,
            cuMemAlloc_v2,
            cuMemAllocPitch_v2,
            cuMemcpy2D_v2,
            cuMemcpy2DAsync_v2,
            cuMemcpy2DAsync_v2_ptsz,
            cuMemcpy2DUnaligned_v2,
            cuMemcpyAsync,
            cuMemcpyDtoDAsync_v2,
            cuMemcpyDtoH_v2_ptds,
            cuMemcpyDtoH_v2,
            cuMemcpyDtoHAsync_v2,
            cuMemcpyHtoD_v2_ptds,
            cuMemcpyHtoD_v2,
            cuMemcpyHtoDAsync_v2,
            cuMemFree_v2,
            cuMemFreeHost,
            cuMemGetAddressRange_v2,
            cuMemGetAllocationGranularity,
            cuMemGetInfo_v2,
            cuMemHostAlloc,
            cuMemHostGetDevicePointer_v2,
            cuMemRetainAllocationHandle,
            cuMemsetD16_v2,
            cuMemsetD2D16_v2,
            cuMemsetD2D16_v2_ptds,
            cuMemsetD2D16Async,
            cuMemsetD2D16Async_ptsz,
            cuMemsetD16Async,
            cuMemsetD2D32_v2,
            cuMemsetD2D32_v2_ptds,
            cuMemsetD2D32Async,
            cuMemsetD2D32Async_ptsz,
            cuMemsetD2D8_v2,
            cuMemsetD2D8_v2_ptds,
            cuMemsetD2D8Async,
            cuMemsetD2D8Async_ptsz,
            cuMemsetD32_v2,
            cuMemsetD32Async,
            cuMemsetD8_v2,
            cuMemsetD8Async,
            cuModuleGetFunction,
            cuModuleGetGlobal_v2,
            cuModuleGetLoadingMode,
            cuModuleLoad,
            cuModuleLoadData,
            cuModuleLoadDataEx,
            cuModuleLoadFatBinary,
            cuModuleUnload,
            cuOccupancyMaxActiveBlocksPerMultiprocessorWithFlags,
            cuOccupancyMaxPotentialBlockSize,
            cuPointerGetAttribute,
            cuPointerGetAttributes,
            cuProfilerStart,
            cuProfilerStop,
            cuStreamBeginCapture_v2,
            cuStreamCreate,
            cuStreamCreateWithPriority,
            cuStreamDestroy_v2,
            cuStreamEndCapture,
            cuStreamGetCaptureInfo_v2,
            cuStreamGetCaptureInfo_v3,
            cuStreamIsCapturing,
            cuStreamSynchronize,
            cuStreamSynchronize_ptsz,
            cuStreamWaitEvent,
            cuStreamWaitEvent_ptsz,
            cuThreadExchangeStreamCaptureMode,
        ],
    implemented_in_function <= [cuLaunchKernel, cuLaunchKernel_ptsz,]
);

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuFuncSetBlockShape(
    hfunc: cuda_types::cuda::CUfunction,
    x: ::core::ffi::c_int,
    y: ::core::ffi::c_int,
    z: ::core::ffi::c_int,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::function::set_block_shape(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&hfunc)?,
        x,
        y,
        z,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuFuncSetSharedSize(
    hfunc: cuda_types::cuda::CUfunction,
    bytes: ::core::ffi::c_uint,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::function::set_shared_size(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&hfunc)?,
        bytes,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuParamSetSize(
    hfunc: cuda_types::cuda::CUfunction,
    numbytes: ::core::ffi::c_uint,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::param_set_size(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&hfunc)?,
        numbytes,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuParamSeti(
    hfunc: cuda_types::cuda::CUfunction,
    offset: ::core::ffi::c_int,
    value: ::core::ffi::c_uint,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::param_seti(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&hfunc)?,
        offset,
        value,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuParamSetf(
    hfunc: cuda_types::cuda::CUfunction,
    offset: ::core::ffi::c_int,
    value: f32,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::param_setf(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&hfunc)?,
        offset,
        value,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuParamSetv(
    hfunc: cuda_types::cuda::CUfunction,
    offset: ::core::ffi::c_int,
    ptr: *mut ::core::ffi::c_void,
    numbytes: ::core::ffi::c_uint,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::param_setv(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&hfunc)?,
        offset,
        ptr,
        numbytes,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuLaunch(
    f: cuda_types::cuda::CUfunction,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::launch(zluda_common::FromCuda::<_, CUerror>::from_cuda(&f)?)
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuLaunchGrid(
    f: cuda_types::cuda::CUfunction,
    grid_width: ::core::ffi::c_int,
    grid_height: ::core::ffi::c_int,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::launch_grid(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&f)?,
        grid_width,
        grid_height,
    )
}

#[cfg_attr(not(test), no_mangle)]
#[allow(improper_ctypes)]
#[allow(improper_ctypes_definitions)]
pub unsafe extern "system" fn cuLaunchGridAsync(
    f: cuda_types::cuda::CUfunction,
    grid_width: ::core::ffi::c_int,
    grid_height: ::core::ffi::c_int,
    h_stream: cuda_types::cuda::CUstream,
) -> cuda_types::cuda::CUresult {
    if !initialized() {
        return Err(CUerror::DEINITIALIZED);
    }
    crate::r#impl::driver::launch_grid_async(
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&f)?,
        grid_width,
        grid_height,
        zluda_common::FromCuda::<_, CUerror>::from_cuda(&h_stream)?,
    )
}

#[cfg(test)]
mod tests;
