use super::debug;
use hip_runtime_sys::*;

pub(crate) fn synchronize(stream: hipStream_t) -> hipError_t {
    debug::log_sync(format_args!(
        "op=hipStreamSynchronize phase=enter stream={:?}",
        stream.0,
    ));
    let result = unsafe { hipStreamSynchronize(stream) };
    debug::log_sync(format_args!(
        "op=hipStreamSynchronize phase=return stream={:?} result_code={} result_name={}",
        stream.0,
        debug::hip_error_code(result),
        debug::hip_error_name(result),
    ));
    result
}

pub(crate) fn synchronize_ptsz(stream: hipStream_t) -> hipError_t {
    synchronize(stream)
}

pub(crate) fn create(stream: *mut hipStream_t, flags: ::core::ffi::c_uint) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=hipStreamCreateWithFlags phase=enter stream_out={:p} flags={}",
        stream, flags
    ));
    let result = unsafe { hipStreamCreateWithFlags(stream, flags) };
    let created_stream = unsafe {
        stream
            .as_ref()
            .map(|stream| stream.0)
            .unwrap_or(std::ptr::null_mut())
    };
    debug::log_stream_memory(format_args!(
        "op=hipStreamCreateWithFlags phase=return stream_out={:p} created_stream={:?} flags={} result_code={} result_name={}",
        stream,
        created_stream,
        flags,
        debug::hip_error_code(result),
        debug::hip_error_name(result),
    ));
    result
}

pub(crate) fn create_with_priority(
    stream: *mut hipStream_t,
    flags: ::core::ffi::c_uint,
    priority: ::core::ffi::c_int,
) -> hipError_t {
    debug::log_stream_memory(format_args!(
        "op=hipStreamCreateWithPriority phase=enter stream_out={:p} flags={} priority={}",
        stream, flags, priority
    ));
    let result = unsafe { hipStreamCreateWithPriority(stream, flags, priority) };
    let created_stream = unsafe {
        stream
            .as_ref()
            .map(|stream| stream.0)
            .unwrap_or(std::ptr::null_mut())
    };
    debug::log_stream_memory(format_args!(
        "op=hipStreamCreateWithPriority phase=return stream_out={:p} created_stream={:?} flags={} priority={} result_code={} result_name={}",
        stream,
        created_stream,
        flags,
        priority,
        debug::hip_error_code(result),
        debug::hip_error_name(result),
    ));
    result
}

pub(crate) fn destroy_v2(stream: hipStream_t) -> hipError_t {
    unsafe { hipStreamDestroy(stream) }
}

pub(crate) fn begin_capture_v2(stream: hipStream_t, mode: hipStreamCaptureMode) -> hipError_t {
    unsafe { hipStreamBeginCapture(stream, mode) }
}

pub(crate) fn end_capture(stream: hipStream_t, graph: *mut hipGraph_t) -> hipError_t {
    unsafe { hipStreamEndCapture(stream, graph) }
}

pub(crate) fn is_capturing(
    stream: hipStream_t,
    capture_status: *mut hipStreamCaptureStatus,
) -> hipError_t {
    unsafe { hipStreamIsCapturing(stream, capture_status) }
}

pub(crate) fn get_capture_info_v2(
    stream: hipStream_t,
    capture_status: *mut hipStreamCaptureStatus,
    id: *mut ::core::ffi::c_ulonglong,
    graph_out: *mut hipGraph_t,
    dependencies_out: *mut *const hipGraphNode_t,
    num_dependencies_out: *mut usize,
) -> hipError_t {
    unsafe {
        hipStreamGetCaptureInfo_v2(
            stream,
            capture_status,
            id,
            graph_out,
            dependencies_out,
            num_dependencies_out,
        )
    }
}

pub(crate) fn get_capture_info_v3(
    stream: hipStream_t,
    capture_status_out: *mut hipStreamCaptureStatus,
    id_out: *mut ::core::ffi::c_ulonglong,
    graph_out: *mut hipGraph_t,
    dependencies_out: *mut *const hipGraphNode_t,
    edge_data_out: *mut *const cuda_types::cuda::CUgraphEdgeData,
    num_dependencies_out: *mut usize,
) -> hipError_t {
    if !edge_data_out.is_null() {
        return hipError_t::ErrorNotSupported;
    }
    unsafe {
        hipStreamGetCaptureInfo_v2(
            stream,
            capture_status_out,
            id_out,
            graph_out,
            dependencies_out,
            num_dependencies_out,
        )
    }
}

pub(crate) fn wait_event(
    stream: hipStream_t,
    event: hipEvent_t,
    flags: ::core::ffi::c_uint,
) -> hipError_t {
    debug::log_sync(format_args!(
        "op=hipStreamWaitEvent phase=enter stream={:?} event={:?} flags={}",
        stream.0, event, flags,
    ));
    let result = unsafe { hipStreamWaitEvent(stream, event, flags) };
    debug::log_sync(format_args!(
        "op=hipStreamWaitEvent phase=return stream={:?} event={:?} result_code={} result_name={}",
        stream.0,
        event,
        debug::hip_error_code(result),
        debug::hip_error_name(result),
    ));
    result
}

pub(crate) fn wait_event_ptsz(
    stream: hipStream_t,
    event: hipEvent_t,
    flags: ::core::ffi::c_uint,
) -> hipError_t {
    wait_event(stream, event, flags)
}
