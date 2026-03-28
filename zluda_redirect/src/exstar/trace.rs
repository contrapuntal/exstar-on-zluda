use std::env;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use crate::{EXSTAR_APPUI_TRACE, EXSTAR_EXE_TRACE, EXSTAR_HOST_TRACE, EXSTAR_HOST_TRACE_FILE, EXSTAR_LIGHT_TRACE};

pub(crate) fn env_flag(name: &str) -> bool {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(_) => return false,
    };
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

pub(crate) fn exstar_host_trace_enabled() -> bool {
    *EXSTAR_HOST_TRACE.get_or_init(|| env_flag("ZLUDA_EXSTAR_HOST_TRACE"))
}

pub(crate) fn exstar_light_trace_enabled() -> bool {
    *EXSTAR_LIGHT_TRACE.get_or_init(|| env_flag("ZLUDA_EXSTAR_LIGHT_TRACE"))
}

pub(crate) fn exstar_trace_logging_enabled() -> bool {
    exstar_host_trace_enabled() || exstar_light_trace_enabled()
}

pub(crate) fn exstar_appui_trace_enabled() -> bool {
    *EXSTAR_APPUI_TRACE.get_or_init(|| env_flag("ZLUDA_EXSTAR_APPUI_TRACE"))
}

pub(crate) fn exstar_exe_trace_enabled() -> bool {
    *EXSTAR_EXE_TRACE.get_or_init(|| env_flag("ZLUDA_EXSTAR_EXE_TRACE"))
}

pub(crate) fn exstar_hub_light_trace_enabled() -> bool {
    exstar_light_trace_enabled() && crate::exstar_window_trace_enabled()
}

fn exstar_host_trace_path() -> std::path::PathBuf {
    env::var_os("ZLUDA_EXSTAR_HOST_TRACE_PATH")
        .map(Into::into)
        .unwrap_or_else(|| {
            let mut path = env::temp_dir();
            path.push("zluda");
            path.push(format!("zluda-exstar-host-{}.log", std::process::id()));
            path
        })
}

pub(crate) fn exstar_host_trace_file() -> Option<&'static Mutex<File>> {
    EXSTAR_HOST_TRACE_FILE
        .get_or_init(|| {
            if !exstar_trace_logging_enabled() {
                return None;
            }
            let path = exstar_host_trace_path();
            if let Some(parent) = path.parent() {
                if let Err(err) = create_dir_all(parent) {
                    eprintln!(
                        "[ZLUDA_EXSTAR_HOST] failed to create log dir path={} error={}",
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
                        "[ZLUDA_EXSTAR_HOST] failed to open log path={} error={}",
                        path.display(),
                        err
                    );
                    None
                }
            }
        })
        .as_ref()
}

pub(crate) fn log_exstar_host(args: std::fmt::Arguments<'_>) {
    if !exstar_trace_logging_enabled() {
        return;
    }
    let line = format!("[ZLUDA_EXSTAR_HOST pid={}] {args}", unsafe {
        windows_sys::Win32::System::Threading::GetCurrentProcessId()
    });
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    if let Some(file) = exstar_host_trace_file() {
        if let Ok(mut file) = file.lock() {
            let _ = writeln!(file, "{line}");
        }
    }
}
