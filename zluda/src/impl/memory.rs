use crate::r#impl::{
    context, debug,
    driver::{self},
};
use cuda_types::cuda::{CUerror, CUresult, CUresultConsts};
use hip_runtime_sys::*;
use std::{mem, ptr};
use zluda_common::FromCuda;

struct DropGuard<F: FnMut()>(F);

impl<F: FnMut()> Drop for DropGuard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

pub(crate) unsafe fn alloc_v2(dptr: &mut hipDeviceptr_t, bytesize: usize) -> CUresult {
    let cu_context = context::get_current_context()?;
    let context: &context::Context = FromCuda::<_, CUerror>::from_cuda(&cu_context)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemAlloc_v2 phase=enter cu_context={:?} alloc_stream={:?} bytesize={}",
        cu_context, context.alloc_stream.0, bytesize
    ));
    hipMalloc(ptr::from_mut(dptr).cast(), bytesize)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemAlloc_v2 phase=malloc_return cu_context={:?} alloc_stream={:?} dptr={:?} bytesize={}",
        cu_context,
        context.alloc_stream.0,
        dptr.0,
        bytesize
    ));
    fill_with_zero_and_register(cu_context, context, dptr, bytesize)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemAlloc_v2 phase=return cu_context={:?} alloc_stream={:?} dptr={:?} bytesize={}",
        cu_context, context.alloc_stream.0, dptr.0, bytesize
    ));
    Ok(())
}

unsafe fn fill_with_zero_and_register(
    cu_context: cuda_types::cuda::CUcontext,
    context: &context::Context,
    dptr: &mut hipDeviceptr_t,
    bytesize: usize,
) -> Result<(), CUerror> {
    let drop_guard = DropGuard(|| {
        unsafe { hipFree(dptr.0) }.ok();
    });
    let mut capturing_status = mem::zeroed();
    hipStreamIsCapturing(hipStream_t(ptr::null_mut()), &mut capturing_status)?;
    debug::log_stream_memory(format_args!(
        "op=fill_with_zero_and_register phase=enter cu_context={:?} dptr={:?} bytesize={} alloc_stream={:?} capturing_status={:?}",
        cu_context,
        dptr.0,
        bytesize,
        context.alloc_stream.0,
        capturing_status
    ));
    if capturing_status == hipStreamCaptureStatus::hipStreamCaptureStatusNone {
        debug::log_stream_memory(format_args!(
            "op=fill_with_zero_and_register phase=memset_async_enter dptr={:?} bytesize={} stream={:?}",
            dptr.0,
            bytesize,
            context.alloc_stream.0
        ));
        hipMemsetD8Async(*dptr, 0, bytesize, context.alloc_stream)?;
        debug::log_stream_memory(format_args!(
            "op=fill_with_zero_and_register phase=memset_async_return dptr={:?} bytesize={} stream={:?}",
            dptr.0,
            bytesize,
            context.alloc_stream.0
        ));
        debug::log_stream_memory(format_args!(
            "op=fill_with_zero_and_register phase=stream_sync_enter stream={:?}",
            context.alloc_stream.0
        ));
        hipStreamSynchronize(context.alloc_stream)?;
        debug::log_stream_memory(format_args!(
            "op=fill_with_zero_and_register phase=stream_sync_return stream={:?}",
            context.alloc_stream.0
        ));
    } else {
        debug::log_stream_memory(format_args!(
            "op=fill_with_zero_and_register phase=skip_memset reason=capturing stream={:?}",
            context.alloc_stream.0
        ));
    }
    add_allocation(dptr.0, bytesize, cu_context)?;
    debug::log_stream_memory(format_args!(
        "op=fill_with_zero_and_register phase=allocation_registered cu_context={:?} dptr={:?} bytesize={}",
        cu_context,
        dptr.0,
        bytesize
    ));
    mem::forget(drop_guard);
    Ok(())
}

pub(crate) unsafe fn free_v2(dptr: hipDeviceptr_t) -> CUresult {
    let hip_result = hipFree(dptr.0);
    remove_allocation(dptr.0)?;
    Ok(hip_result?)
}

pub(crate) fn copy_dto_h_v2(
    dst_host: *mut ::core::ffi::c_void,
    src_device: hipDeviceptr_t,
    byte_count: usize,
) -> hipError_t {
    unsafe { hipMemcpyDtoH(dst_host, src_device, byte_count) }
}

pub(crate) fn copy_hto_d_v2(
    dst_device: hipDeviceptr_t,
    src_host: *const ::core::ffi::c_void,
    byte_count: usize,
) -> hipError_t {
    unsafe { hipMemcpyHtoD(dst_device, src_host.cast_mut(), byte_count) }
}

pub(crate) fn copy_hto_d_v2_ptds(
    dst_device: hipDeviceptr_t,
    src_host: *const ::core::ffi::c_void,
    byte_count: usize,
) -> hipError_t {
    unsafe {
        hipMemcpy_spt(
            dst_device.0.cast(),
            src_host.cast_mut(),
            byte_count,
            hipMemcpyKind::hipMemcpyHostToDevice,
        )
    }
}

pub(crate) fn copy_dto_h_v2_ptds(
    dst_host: *mut ::core::ffi::c_void,
    src_device: hipDeviceptr_t,
    byte_count: usize,
) -> hipError_t {
    unsafe {
        hipMemcpy_spt(
            dst_host.cast(),
            src_device.0.cast(),
            byte_count,
            hipMemcpyKind::hipMemcpyDeviceToHost,
        )
    }
}

pub(crate) fn get_address_range_v2(
    pbase: *mut hipDeviceptr_t,
    psize: *mut usize,
    dptr: hipDeviceptr_t,
) -> hipError_t {
    unsafe { hipMemGetAddressRange(pbase, psize, dptr) }
}

pub(crate) fn set_d8_v2(dst: hipDeviceptr_t, value: ::core::ffi::c_uchar, n: usize) -> hipError_t {
    unsafe { hipMemsetD8(dst, value, n) }
}

pub(crate) fn set_d8_async(
    dst: hipDeviceptr_t,
    value: ::core::ffi::c_uchar,
    n: usize,
    stream: hipStream_t,
) -> hipError_t {
    unsafe { hipMemsetD8Async(dst, value, n, stream) }
}

pub(crate) fn set_d16_v2(
    dst: hipDeviceptr_t,
    value: ::core::ffi::c_ushort,
    n: usize,
) -> hipError_t {
    unsafe { hipMemsetD16(dst, value, n) }
}

pub(crate) fn set_d16_async(
    dst: hipDeviceptr_t,
    value: ::core::ffi::c_ushort,
    n: usize,
    stream: hipStream_t,
) -> hipError_t {
    unsafe { hipMemsetD16Async(dst, value, n, stream) }
}

pub(crate) fn set_d32_v2(dst: hipDeviceptr_t, value: ::core::ffi::c_uint, n: usize) -> hipError_t {
    unsafe { hipMemsetD32(dst, value as _, n) }
}

pub(crate) fn set_d32_async(
    dst: hipDeviceptr_t,
    value: ::core::ffi::c_uint,
    n: usize,
    stream: hipStream_t,
) -> hipError_t {
    unsafe { hipMemsetD32Async(dst, value as _, n, stream) }
}

pub(crate) fn get_info_v2(free: *mut usize, total: *mut usize) -> hipError_t {
    unsafe { hipMemGetInfo(free, total) }
}

pub(crate) unsafe fn free_host(ptr: *mut ::core::ffi::c_void) -> CUresult {
    let hip_result = hipFreeHost(ptr);
    remove_allocation(ptr)?;
    Ok(hip_result?)
}

pub(crate) unsafe fn host_alloc(
    pp: &mut *mut ::core::ffi::c_void,
    bytesize: usize,
    flags: ::std::os::raw::c_uint,
) -> CUresult {
    let context = context::get_current_context()?;
    hipHostMalloc(pp, bytesize, flags)?;
    add_allocation(*pp, bytesize, context)?;
    Ok(())
}

pub(crate) unsafe fn host_get_device_pointer_v2(
    pdptr: &mut hipDeviceptr_t,
    p: *mut ::core::ffi::c_void,
    flags: ::std::os::raw::c_uint,
) -> CUresult {
    if p.is_null() {
        return CUresult::ERROR_INVALID_VALUE;
    }

    // HIP equivalent of cuMemHostGetDevicePointer_v2
    debug::log_stream_memory(format_args!(
        "op=cuMemHostGetDevicePointer_v2 phase=enter host_ptr={:p} flags={}",
        p, flags
    ));
    hipHostGetDevicePointer(std::ptr::from_mut(pdptr).cast(), p, flags)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemHostGetDevicePointer_v2 phase=return host_ptr={:p} flags={} device_ptr={:?}",
        p, flags, pdptr.0
    ));
    Ok(())
}

fn add_allocation(
    dptr: *mut ::core::ffi::c_void,
    bytesize: usize,
    context: cuda_types::cuda::CUcontext,
) -> Result<(), CUerror> {
    let global_state = driver::global_state()?;
    let mut allocations = global_state
        .allocations
        .lock()
        .map_err(|_| CUerror::UNKNOWN)?;
    allocations.insert(dptr as usize, bytesize, context);
    Ok(())
}

fn remove_allocation(ptr: *mut std::ffi::c_void) -> Result<(), CUerror> {
    let global_state = driver::global_state()?;
    let mut allocations = global_state
        .allocations
        .lock()
        .map_err(|_| CUerror::UNKNOWN)?;
    allocations.remove(ptr as usize);
    Ok(())
}

pub(crate) unsafe fn retain_allocation_handle(
    _handle: *mut cuda_types::cuda::CUmemGenericAllocationHandle,
    _addr: *mut ::core::ffi::c_void,
) -> CUresult {
    CUresult::ERROR_NOT_SUPPORTED
}

pub(crate) unsafe fn copy_hto_d_async_v2(
    dst_device: hipDeviceptr_t,
    src_host: *const ::core::ffi::c_void,
    byte_count: usize,
    stream: hipStream_t,
) -> hipError_t {
    hipMemcpyHtoDAsync(dst_device, src_host.cast_mut(), byte_count, stream)
}

pub(crate) unsafe fn copy_dto_h_async_v2(
    dst_host: *mut ::core::ffi::c_void,
    src_device: hipDeviceptr_t,
    byte_count: usize,
    stream: hipStream_t,
) -> hipError_t {
    hipMemcpyDtoHAsync(dst_host, src_device, byte_count, stream)
}

pub(crate) unsafe fn copy_dto_d_async_v2(
    dst_device: hipDeviceptr_t,
    src_device: hipDeviceptr_t,
    byte_count: usize,
    stream: hipStream_t,
) -> hipError_t {
    hipMemcpyDtoDAsync(dst_device, src_device, byte_count, stream)
}

pub(crate) unsafe fn copy_async(
    dst: hipDeviceptr_t,
    src: hipDeviceptr_t,
    byte_count: usize,
    stream: hipStream_t,
) -> hipError_t {
    hipMemcpyAsync(
        dst.0,
        src.0,
        byte_count,
        hipMemcpyKind::hipMemcpyDefault,
        stream,
    )
}

pub(crate) fn get_allocation_granularity(
    _granularity: &mut usize,
    _property: &cuda_types::cuda::CUmemAllocationProp,
    _option: cuda_types::cuda::CUmemAllocationGranularity_flags,
) -> CUresult {
    CUresult::ERROR_NOT_SUPPORTED
}

pub(crate) unsafe fn alloc_pitch_v2(
    dptr: *mut hipDeviceptr_t,
    p_pitch: *mut usize,
    width_in_bytes: usize,
    height: usize,
    element_size_bytes: ::core::ffi::c_uint,
) -> CUresult {
    let cu_context = context::get_current_context()?;
    let context: &context::Context = FromCuda::<_, CUerror>::from_cuda(&cu_context)?;
    let dptr = dptr.as_mut().ok_or(CUerror::INVALID_VALUE)?;
    let p_pitch = p_pitch.as_mut().ok_or(CUerror::INVALID_VALUE)?;
    if element_size_bytes == 0 {
        return CUresult::ERROR_INVALID_VALUE;
    }
    // HIP can return an exact-width pitch here, but EXStar's color-correction path
    // appears to expect CUDA-style padded pitches before it continues to launch setup.
    let pitch_alignment = 512usize;
    let pitch = width_in_bytes
        .checked_add(pitch_alignment - 1)
        .ok_or(CUerror::INVALID_VALUE)?
        / pitch_alignment
        * pitch_alignment;
    let bytesize = pitch.checked_mul(height).ok_or(CUerror::INVALID_VALUE)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemAllocPitch_v2 phase=enter cu_context={:?} alloc_stream={:?} width_in_bytes={} height={} element_size_bytes={} pitch_alignment={} computed_pitch={} computed_bytesize={}",
        cu_context,
        context.alloc_stream.0,
        width_in_bytes,
        height,
        element_size_bytes,
        pitch_alignment,
        pitch,
        bytesize
    ));
    hipMalloc(ptr::from_mut(dptr).cast(), bytesize)?;
    *p_pitch = pitch;
    debug::log_stream_memory(format_args!(
        "op=cuMemAllocPitch_v2 phase=malloc_return cu_context={:?} alloc_stream={:?} dptr={:?} pitch={} bytesize={}",
        cu_context,
        context.alloc_stream.0,
        dptr.0,
        *p_pitch,
        bytesize
    ));
    fill_with_zero_and_register(cu_context, context, dptr, bytesize)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemAllocPitch_v2 phase=return cu_context={:?} alloc_stream={:?} dptr={:?} pitch={} bytesize={}",
        cu_context,
        context.alloc_stream.0,
        dptr.0,
        *p_pitch,
        bytesize
    ));
    Ok(())
}

pub(crate) unsafe fn copy_2d_v2(memcpy: hip_Memcpy2D) -> CUresult {
    debug::log_stream_memory(format_args!(
        "op=cuMemcpy2D_v2 phase=enter srcMemoryType={} dstMemoryType={} srcXInBytes={} srcY={} dstXInBytes={} dstY={} WidthInBytes={} Height={} srcPitch={} dstPitch={} srcDevice={:?} dstDevice={:?}",
        memcpy.srcMemoryType.0,
        memcpy.dstMemoryType.0,
        memcpy.srcXInBytes,
        memcpy.srcY,
        memcpy.dstXInBytes,
        memcpy.dstY,
        memcpy.WidthInBytes,
        memcpy.Height,
        memcpy.srcPitch,
        memcpy.dstPitch,
        memcpy.srcDevice.0,
        memcpy.dstDevice.0
    ));
    hipMemcpyParam2D(&memcpy)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemcpy2D_v2 phase=return WidthInBytes={} Height={} srcPitch={} dstPitch={}",
        memcpy.WidthInBytes, memcpy.Height, memcpy.srcPitch, memcpy.dstPitch
    ));
    Ok(())
}

pub(crate) unsafe fn copy_2d_async_v2(memcpy: hip_Memcpy2D, stream: hipStream_t) -> CUresult {
    debug::log_stream_memory(format_args!(
        "op=cuMemcpy2DAsync_v2 phase=enter srcMemoryType={} dstMemoryType={} srcXInBytes={} srcY={} dstXInBytes={} dstY={} WidthInBytes={} Height={} srcPitch={} dstPitch={} srcDevice={:?} dstDevice={:?} stream={:?}",
        memcpy.srcMemoryType.0,
        memcpy.dstMemoryType.0,
        memcpy.srcXInBytes,
        memcpy.srcY,
        memcpy.dstXInBytes,
        memcpy.dstY,
        memcpy.WidthInBytes,
        memcpy.Height,
        memcpy.srcPitch,
        memcpy.dstPitch,
        memcpy.srcDevice.0,
        memcpy.dstDevice.0,
        stream.0
    ));
    hipMemcpyParam2DAsync(ptr::from_ref(&memcpy), stream)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemcpy2DAsync_v2 phase=return WidthInBytes={} Height={} srcPitch={} dstPitch={} stream={:?}",
        memcpy.WidthInBytes,
        memcpy.Height,
        memcpy.srcPitch,
        memcpy.dstPitch,
        stream.0
    ));
    Ok(())
}

pub(crate) unsafe fn copy_2d_async_v2_ptsz(
    memcpy: hip_Memcpy2D,
    stream: hipStream_t,
) -> CUresult {
    copy_2d_async_v2(memcpy, stream)
}

pub(crate) unsafe fn copy_2d_unaligned_v2(memcpy: hip_Memcpy2D) -> CUresult {
    debug::log_stream_memory(format_args!(
        "op=cuMemcpy2DUnaligned_v2 phase=enter srcMemoryType={} dstMemoryType={} srcXInBytes={} srcY={} dstXInBytes={} dstY={} WidthInBytes={} Height={} srcPitch={} dstPitch={} srcDevice={:?} dstDevice={:?}",
        memcpy.srcMemoryType.0,
        memcpy.dstMemoryType.0,
        memcpy.srcXInBytes,
        memcpy.srcY,
        memcpy.dstXInBytes,
        memcpy.dstY,
        memcpy.WidthInBytes,
        memcpy.Height,
        memcpy.srcPitch,
        memcpy.dstPitch,
        memcpy.srcDevice.0,
        memcpy.dstDevice.0
    ));
    hipDrvMemcpy2DUnaligned(&memcpy)?;
    debug::log_stream_memory(format_args!(
        "op=cuMemcpy2DUnaligned_v2 phase=return WidthInBytes={} Height={} srcPitch={} dstPitch={}",
        memcpy.WidthInBytes, memcpy.Height, memcpy.srcPitch, memcpy.dstPitch
    ));
    Ok(())
}

pub(crate) unsafe fn set_d_2d32_v2(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uint,
    width: usize,
    height: usize,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D32_v2 phase=enter dst_device={:?} dst_pitch={} value={} width={} height={}",
        dst_device.0, dst_pitch, value, width, height
    ));
    let result = hipMemset2D(dst_device.0, dst_pitch, value as _, width, height);
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D32_v2 phase=return dst_device={:?} dst_pitch={} width={} height={} result_code={} result_name={}",
        dst_device.0,
        dst_pitch,
        width,
        height,
        debug::hip_error_code(result),
        debug::hip_error_name(result)
    ));
    result
}

pub(crate) unsafe fn set_d_2d32_v2_ptds(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uint,
    width: usize,
    height: usize,
) -> hipError_t {
    set_d_2d32_v2(dst_device, dst_pitch, value, width, height)
}

pub(crate) unsafe fn set_d_2d32_async(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uint,
    width: usize,
    height: usize,
    stream: hipStream_t,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D32Async phase=enter dst_device={:?} dst_pitch={} value={} width={} height={} stream={:?}",
        dst_device.0,
        dst_pitch,
        value,
        width,
        height,
        stream.0
    ));
    let result = hipMemset2DAsync(dst_device.0, dst_pitch, value as _, width, height, stream);
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D32Async phase=return dst_device={:?} dst_pitch={} width={} height={} stream={:?} result_code={} result_name={}",
        dst_device.0,
        dst_pitch,
        width,
        height,
        stream.0,
        debug::hip_error_code(result),
        debug::hip_error_name(result)
    ));
    result
}

pub(crate) unsafe fn set_d_2d32_async_ptsz(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uint,
    width: usize,
    height: usize,
    stream: hipStream_t,
) -> hipError_t {
    set_d_2d32_async(dst_device, dst_pitch, value, width, height, stream)
}

pub(crate) unsafe fn set_d_2d16_v2(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_ushort,
    width: usize,
    height: usize,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D16_v2 phase=enter dst_device={:?} dst_pitch={} value={} width={} height={}",
        dst_device.0, dst_pitch, value, width, height
    ));
    let result = hipMemset2D(dst_device.0, dst_pitch, value as _, width, height);
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D16_v2 phase=return dst_device={:?} dst_pitch={} width={} height={} result_code={} result_name={}",
        dst_device.0,
        dst_pitch,
        width,
        height,
        debug::hip_error_code(result),
        debug::hip_error_name(result)
    ));
    result
}

pub(crate) unsafe fn set_d_2d16_v2_ptds(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_ushort,
    width: usize,
    height: usize,
) -> hipError_t {
    set_d_2d16_v2(dst_device, dst_pitch, value, width, height)
}

pub(crate) unsafe fn set_d_2d16_async(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_ushort,
    width: usize,
    height: usize,
    stream: hipStream_t,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D16Async phase=enter dst_device={:?} dst_pitch={} value={} width={} height={} stream={:?}",
        dst_device.0,
        dst_pitch,
        value,
        width,
        height,
        stream.0
    ));
    let result = hipMemset2DAsync(dst_device.0, dst_pitch, value as _, width, height, stream);
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D16Async phase=return dst_device={:?} dst_pitch={} width={} height={} stream={:?} result_code={} result_name={}",
        dst_device.0,
        dst_pitch,
        width,
        height,
        stream.0,
        debug::hip_error_code(result),
        debug::hip_error_name(result)
    ));
    result
}

pub(crate) unsafe fn set_d_2d16_async_ptsz(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_ushort,
    width: usize,
    height: usize,
    stream: hipStream_t,
) -> hipError_t {
    set_d_2d16_async(dst_device, dst_pitch, value, width, height, stream)
}

pub(crate) unsafe fn set_d_2d8_v2(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uchar,
    width: usize,
    height: usize,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D8_v2 phase=enter dst_device={:?} dst_pitch={} value={} width={} height={}",
        dst_device.0, dst_pitch, value, width, height
    ));
    let result = hipMemset2D(dst_device.0, dst_pitch, value as _, width, height);
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D8_v2 phase=return dst_device={:?} dst_pitch={} width={} height={} result_code={} result_name={}",
        dst_device.0,
        dst_pitch,
        width,
        height,
        debug::hip_error_code(result),
        debug::hip_error_name(result)
    ));
    result
}

pub(crate) unsafe fn set_d_2d8_v2_ptds(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uchar,
    width: usize,
    height: usize,
) -> hipError_t {
    set_d_2d8_v2(dst_device, dst_pitch, value, width, height)
}

pub(crate) unsafe fn set_d_2d8_async(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uchar,
    width: usize,
    height: usize,
    stream: hipStream_t,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D8Async phase=enter dst_device={:?} dst_pitch={} value={} width={} height={} stream={:?}",
        dst_device.0,
        dst_pitch,
        value,
        width,
        height,
        stream.0
    ));
    let result = hipMemset2DAsync(dst_device.0, dst_pitch, value as _, width, height, stream);
    debug::log_stream_memory(format_args!(
        "op=cuMemsetD2D8Async phase=return dst_device={:?} dst_pitch={} width={} height={} stream={:?} result_code={} result_name={}",
        dst_device.0,
        dst_pitch,
        width,
        height,
        stream.0,
        debug::hip_error_code(result),
        debug::hip_error_name(result)
    ));
    result
}

pub(crate) unsafe fn set_d_2d8_async_ptsz(
    dst_device: hipDeviceptr_t,
    dst_pitch: usize,
    value: ::core::ffi::c_uchar,
    width: usize,
    height: usize,
    stream: hipStream_t,
) -> hipError_t {
    set_d_2d8_async(dst_device, dst_pitch, value, width, height, stream)
}
