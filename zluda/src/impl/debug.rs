use hip_runtime_sys::{hipError_t, hipGetErrorName};
use std::{
    env,
    ffi::CStr,
    fmt,
    fs::{create_dir_all, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

static DEBUG_LAUNCH: OnceLock<bool> = OnceLock::new();
static DEBUG_SYNC: OnceLock<bool> = OnceLock::new();
static DEBUG_SYNC_AFTER_LAUNCH: OnceLock<bool> = OnceLock::new();
static DEBUG_STREAM_MEMORY: OnceLock<bool> = OnceLock::new();
static DEBUG_BLOCK_KERNEL_SUBSTRING: OnceLock<Option<String>> = OnceLock::new();
static DEBUG_BLOCK_MODULE_SUBSTRING: OnceLock<Option<String>> = OnceLock::new();
static DEBUG_DISABLE_CLS: OnceLock<bool> = OnceLock::new();
static DEBUG_NULL_CLS_VALUE: OnceLock<bool> = OnceLock::new();
static DEBUG_FAIL_CLS_PUT: OnceLock<bool> = OnceLock::new();
static DEBUG_FAIL_CLS_GET: OnceLock<bool> = OnceLock::new();
static DEBUG_FAIL_FIRST_CLS_PUT: OnceLock<bool> = OnceLock::new();
static DEBUG_FAIL_CLS_PUT_COUNT: OnceLock<Option<u32>> = OnceLock::new();
static DEBUG_FAIL_CLS_GET_AFTER_COUNT: OnceLock<Option<u32>> = OnceLock::new();
static DEBUG_TRACE_CLS_GET_HIT_COUNT: OnceLock<Option<u32>> = OnceLock::new();
static DEBUG_COMPAT_CLS_PUT_WINDOW: OnceLock<bool> = OnceLock::new();
static DEBUG_COMPAT_CLS_PATTERN_WINDOW: OnceLock<bool> = OnceLock::new();
static DEBUG_COMPAT_CLS_MODULE_WINDOW: OnceLock<bool> = OnceLock::new();
static DEBUG_TRACE_CLS_SUCCESS: OnceLock<bool> = OnceLock::new();
static EXSTAR_COMPAT_COLOR_CORRECT_CLS: OnceLock<bool> = OnceLock::new();
static EXSTAR_COMPAT_DEVICE_QUALIFICATION: OnceLock<bool> = OnceLock::new();
static DEBUG_SYNC_AFTER_LAUNCH_ANNOUNCED: OnceLock<()> = OnceLock::new();
static DEBUG_LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static LAUNCH_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn env_flag(name: &str) -> bool {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(_) => return false,
    };
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

pub(crate) fn launch_logging_enabled() -> bool {
    *DEBUG_LAUNCH.get_or_init(|| env_flag("ZLUDA_DEBUG_LAUNCH"))
}

pub(crate) fn sync_logging_enabled() -> bool {
    *DEBUG_SYNC.get_or_init(|| env_flag("ZLUDA_DEBUG_SYNC"))
}

pub(crate) fn sync_after_launch_enabled() -> bool {
    *DEBUG_SYNC_AFTER_LAUNCH.get_or_init(|| env_flag("ZLUDA_DEBUG_SYNC_AFTER_LAUNCH"))
}

pub(crate) fn stream_memory_logging_enabled() -> bool {
    *DEBUG_STREAM_MEMORY.get_or_init(|| env_flag("ZLUDA_DEBUG_STREAM_MEMORY"))
}

pub(crate) fn blocked_kernel_substring() -> Option<&'static str> {
    DEBUG_BLOCK_KERNEL_SUBSTRING
        .get_or_init(|| {
            env::var("ZLUDA_DEBUG_BLOCK_KERNEL_SUBSTRING")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .as_deref()
}

pub(crate) fn blocked_module_substring() -> Option<&'static str> {
    DEBUG_BLOCK_MODULE_SUBSTRING
        .get_or_init(|| {
            env::var("ZLUDA_DEBUG_BLOCK_MODULE_SUBSTRING")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .as_deref()
}

pub(crate) fn cls_disabled() -> bool {
    *DEBUG_DISABLE_CLS.get_or_init(|| env_flag("ZLUDA_DEBUG_DISABLE_CLS"))
}

pub(crate) fn cls_null_value() -> bool {
    *DEBUG_NULL_CLS_VALUE.get_or_init(|| env_flag("ZLUDA_DEBUG_NULL_CLS_VALUE"))
}

pub(crate) fn cls_put_fails() -> bool {
    *DEBUG_FAIL_CLS_PUT.get_or_init(|| env_flag("ZLUDA_DEBUG_FAIL_CLS_PUT"))
}

pub(crate) fn cls_get_fails() -> bool {
    *DEBUG_FAIL_CLS_GET.get_or_init(|| env_flag("ZLUDA_DEBUG_FAIL_CLS_GET"))
}

pub(crate) fn cls_first_put_fails() -> bool {
    *DEBUG_FAIL_FIRST_CLS_PUT.get_or_init(|| env_flag("ZLUDA_DEBUG_FAIL_FIRST_CLS_PUT"))
}

pub(crate) fn cls_put_fail_count() -> Option<u32> {
    DEBUG_FAIL_CLS_PUT_COUNT
        .get_or_init(|| {
            env::var("ZLUDA_DEBUG_FAIL_CLS_PUT_COUNT")
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|count| *count > 0)
        })
        .to_owned()
}

pub(crate) fn cls_get_fail_after_count() -> Option<u32> {
    DEBUG_FAIL_CLS_GET_AFTER_COUNT
        .get_or_init(|| {
            env::var("ZLUDA_DEBUG_FAIL_CLS_GET_AFTER_COUNT")
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|count| *count > 0)
        })
        .to_owned()
}

pub(crate) fn trace_cls_get_hit_count() -> Option<u32> {
    DEBUG_TRACE_CLS_GET_HIT_COUNT
        .get_or_init(|| {
            env::var("ZLUDA_DEBUG_TRACE_CLS_GET_HIT_COUNT")
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|count| *count > 0)
        })
        .to_owned()
}

pub(crate) fn compat_cls_put_window() -> bool {
    *DEBUG_COMPAT_CLS_PUT_WINDOW.get_or_init(|| env_flag("ZLUDA_DEBUG_COMPAT_CLS_PUT_WINDOW"))
}

pub(crate) fn compat_cls_pattern_window() -> bool {
    *DEBUG_COMPAT_CLS_PATTERN_WINDOW
        .get_or_init(|| env_flag("ZLUDA_DEBUG_COMPAT_CLS_PATTERN_WINDOW"))
}

pub(crate) fn compat_cls_module_window() -> bool {
    *DEBUG_COMPAT_CLS_MODULE_WINDOW.get_or_init(|| env_flag("ZLUDA_DEBUG_COMPAT_CLS_MODULE_WINDOW"))
        || *EXSTAR_COMPAT_COLOR_CORRECT_CLS
            .get_or_init(|| env_flag("ZLUDA_EXSTAR_COLOR_CORRECT_CLS_COMPAT"))
}

pub(crate) fn trace_cls_success() -> bool {
    *DEBUG_TRACE_CLS_SUCCESS.get_or_init(|| env_flag("ZLUDA_DEBUG_TRACE_CLS_SUCCESS"))
}

pub(crate) fn exstar_device_qualification_compat() -> bool {
    // Always spoof NVIDIA device attributes for EXStar compatibility.
    // EXStar checks GPU name/capabilities and refuses to run on non-NVIDIA GPUs.
    // The env var override is kept for testing (set to "0" to disable).
    *EXSTAR_COMPAT_DEVICE_QUALIFICATION.get_or_init(|| {
        match std::env::var("ZLUDA_EXSTAR_DEVICE_QUALIFICATION_COMPAT") {
            Ok(v) if v == "0" => false,
            _ => true, // default ON
        }
    })
}

pub(crate) fn next_launch_sequence() -> u64 {
    LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn hip_error_code(error: hipError_t) -> u32 {
    match error {
        Ok(()) => 0,
        Err(code) => code.0.get(),
    }
}

pub(crate) fn hip_error_name(error: hipError_t) -> &'static str {
    let error_name = unsafe { hipGetErrorName(error) };
    if error_name.is_null() {
        return "<null>";
    }
    unsafe { CStr::from_ptr(error_name) }
        .to_str()
        .unwrap_or("<invalid-utf8>")
}

fn default_log_path() -> PathBuf {
    let mut path = env::temp_dir();
    path.push("zluda");
    path.push(format!("zluda-debug-{}.log", std::process::id()));
    path
}

fn debug_log_path() -> Option<PathBuf> {
    env::var_os("ZLUDA_DEBUG_LOG_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            if launch_logging_enabled()
                || sync_logging_enabled()
                || sync_after_launch_enabled()
                || stream_memory_logging_enabled()
            {
                Some(default_log_path())
            } else {
                None
            }
        })
}

fn debug_log_file() -> Option<&'static Mutex<File>> {
    DEBUG_LOG_FILE
        .get_or_init(|| {
            let path = debug_log_path()?;
            if let Some(parent) = path.parent() {
                if let Err(err) = create_dir_all(parent) {
                    eprintln!(
                        "[ZLUDA_DEBUG] failed to create log directory path={} error={}",
                        parent.display(),
                        err
                    );
                    return None;
                }
            }
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => Some(Mutex::new(file)),
                Err(err) => {
                    eprintln!(
                        "[ZLUDA_DEBUG] failed to open log file path={} error={}",
                        path.display(),
                        err
                    );
                    None
                }
            }
        })
        .as_ref()
}

fn emit_line(line: &str) {
    eprintln!("{line}");
    if let Some(file) = debug_log_file() {
        match file.lock() {
            Ok(mut file) => {
                if let Err(err) = writeln!(file, "{line}") {
                    eprintln!("[ZLUDA_DEBUG] failed to write log file error={}", err);
                }
            }
            Err(err) => {
                eprintln!("[ZLUDA_DEBUG] failed to lock log file error={}", err);
            }
        }
    }
}

pub(crate) fn log_launch(args: fmt::Arguments<'_>) {
    if launch_logging_enabled() {
        emit_line(&format!("[ZLUDA_LAUNCH] {args}"));
    }
}

pub(crate) fn log_sync(args: fmt::Arguments<'_>) {
    if sync_logging_enabled() {
        emit_line(&format!("[ZLUDA_SYNC] {args}"));
    }
}

pub(crate) fn log_stream_memory(args: fmt::Arguments<'_>) {
    if stream_memory_logging_enabled() {
        emit_line(&format!("[ZLUDA_STREAM_MEMORY] {args}"));
    }
}

pub(crate) fn announce_sync_after_launch() {
    if !sync_after_launch_enabled() {
        return;
    }
    DEBUG_SYNC_AFTER_LAUNCH_ANNOUNCED.get_or_init(|| {
        emit_line("[ZLUDA_LAUNCH] mode=sync_after_launch enabled=1");
    });
}
