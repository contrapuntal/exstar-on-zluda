#![cfg(target_os = "windows")]

mod exstar;
use exstar::prestartcheck::{exstar_patch_prestartcheck_module, exstar_should_suppress_prestartcheck_timer};
use exstar::trace::{
    env_flag, exstar_appui_trace_enabled, exstar_exe_trace_enabled, exstar_host_trace_enabled,
    exstar_host_trace_file, exstar_hub_light_trace_enabled, exstar_light_trace_enabled,
    exstar_trace_logging_enabled, log_exstar_host,
};

use detours_sys::{
    DetourAttach, DetourCopyPayloadToProcess, DetourDetach, DetourRestoreAfterWith,
    DetourTransactionAbort, DetourTransactionBegin, DetourTransactionCommit,
    DetourUpdateProcessWithDll, DetourUpdateThread, LPCWSTR,
};
use rustc_hash::FxHashMap;
use std::collections::hash_map;
use std::env;
use std::ffi::{c_char, CStr, CString};
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::iter::Peekable;
use std::path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use std::{ffi::c_void, mem, panic, ptr, slice, usize};
use widestring::{U16CStr, U16CString};
use windows::Win32::Foundation::{
    CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, NTSTATUS,
    STATUS_INVALID_PARAMETER_3, UNICODE_STRING,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next,
    PROCESSENTRY32W, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, GetCurrentThreadId, OpenProcess,
    OpenThread, ResumeThread, SuspendThread, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};
use windows_sys::core::{BOOL, PCSTR, PCWSTR, PSTR, PWSTR};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, FALSE, FARPROC, HWND, NO_ERROR, RECT, TRUE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Diagnostics::Debug::{RtlCaptureStackBackTrace, CONTEXT};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameA, GetModuleHandleA, GetProcAddress, LoadLibraryExA, LoadLibraryExW,
    LOAD_LIBRARY_FLAGS,
};
use windows_sys::Win32::System::Memory::{
    VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY,
    PAGE_READWRITE, PAGE_WRITECOPY,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexA, CreateMutexW, CreateProcessA, CreateProcessAsUserA, CreateProcessAsUserW, CreateProcessW,
    CreateProcessWithLogonW, CreateProcessWithTokenW, ExitProcess, GetExitCodeProcess,
    GetProcessId, GetThreadId, QueryFullProcessImageNameW, TerminateProcess as WinTerminateProcess,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DestroyWindow, GetClassNameW, GetForegroundWindow, GetWindowRect,
    GetWindowTextW, IsWindowVisible, SetForegroundWindow, SetWindowPos, ShowWindow,
};
use windows_sys::Win32::{
    Foundation::HMODULE,
    System::LibraryLoader::{LoadLibraryA, LoadLibraryW},
};
use zluda_windows::{DllLookup, LIBRARIES};

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const EXSTAR_HUB_STARTUP_COMPAT_MAX: Duration = Duration::from_secs(45);

#[repr(C)]
#[derive(Clone, Copy)]
struct ExstarExceptionPointers {
    exception_record: *mut ExstarExceptionRecord,
    context_record: *mut CONTEXT,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ExstarExceptionRecord {
    exception_code: i32,
    exception_flags: u32,
    exception_record: *mut ExstarExceptionRecord,
    exception_address: *mut c_void,
    number_parameters: u32,
    exception_information: [usize; 15],
}

unsafe extern "system" {
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: Option<unsafe extern "system" fn(*mut ExstarExceptionPointers) -> i32>,
    ) -> *mut c_void;
    fn RemoveVectoredExceptionHandler(handle: *mut c_void) -> u32;
}

static mut DETOUR_DROP: Option<DetourDetachGuard> = None;
static mut DETOUR_PATHS: Option<DetourPaths> = None;
static mut SELF_PATH: Option<CString> = None;

pub(crate) static EXSTAR_HOST_TRACE: OnceLock<bool> = OnceLock::new();
pub(crate) static EXSTAR_LIGHT_TRACE: OnceLock<bool> = OnceLock::new();
/// Stores the handle of the "EinScan-Pro.exe" duplicate-instance mutex
/// created by the bootstrap Hub, so preserve_hub_exit can close it.
static EXSTAR_DUPLICATE_MUTEX_HANDLE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub(crate) static EXSTAR_HOST_TRACE_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static EXSTAR_HUB_EXIT_DELAY_MS: OnceLock<u64> = OnceLock::new();
static EXSTAR_HUB_STARTUP_COMPAT_TIMEOUT_MS: OnceLock<u64> = OnceLock::new();
static EXSTAR_EINSCAN_NET_SVR_COMPAT: OnceLock<bool> = OnceLock::new();
static EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK: OnceLock<bool> = OnceLock::new();
static EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK_DELAY_MS: OnceLock<u64> = OnceLock::new();
static EXSTAR_MANAGER_SKIP_SECOND_SWEEP: OnceLock<bool> = OnceLock::new();
static EXSTAR_MANAGER_PRESERVE_CORE_PEERS: OnceLock<bool> = OnceLock::new();
static EXSTAR_MANAGER_PRESERVE_HUB_AND_SCANHUB: OnceLock<bool> = OnceLock::new();
static EXSTAR_MANAGER_SKIP_KILL_TARGETS: OnceLock<Vec<String>> = OnceLock::new();
pub(crate) static EXSTAR_APPUI_TRACE: OnceLock<bool> = OnceLock::new();
pub(crate) static EXSTAR_EXE_TRACE: OnceLock<bool> = OnceLock::new();
static EXSTAR_FORCE_MAIN_WINDOW_VISIBLE: OnceLock<bool> = OnceLock::new();
static EXSTAR_SKIP_QTTUNNEL_CONNECT_HOOK: OnceLock<bool> = OnceLock::new();
static EXSTAR_HUB_STARTUP_COMPAT_START: OnceLock<Instant> = OnceLock::new();
static EXSTAR_EINSCAN_NET_SVR_LAUNCHED: AtomicBool = AtomicBool::new(false);
static EXSTAR_EINSCAN_NET_SVR_LAUNCH_LATCH: OnceLock<usize> = OnceLock::new();
static EXSTAR_MANAGER_WAIT_COUNTER: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_LOAD_CONFIG_COUNTER: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_KILL_ALL_COUNTER: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_KILL_ONE_COUNTER: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_SCANHUB_LAUNCH_COUNTER: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_SECOND_SWEEP_ID: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_SECOND_SWEEP_PENDING: AtomicBool = AtomicBool::new(false);
static EXSTAR_MAIN_WINDOW_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static EXSTAR_CHILD_HUB_START_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static EXSTAR_CHILD_HUB_APP_WINDOW_SHOWN: AtomicBool = AtomicBool::new(false);
static EXSTAR_CHILD_HUB_APP_WINDOW_FOREGROUNDED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_POST_HELPER_PHASE_ACTIVE: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_POST_HELPER_EXIT_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_POST_HELPER_EXCEPTION_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_WINDOW_HIDE_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_QTWIDGET_HIDE_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_QWINDOW_HIDE_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_QWINDOW_EVENT19_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_PROCESSMG_APP_BRANCH_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_PROCESSMG_APP8_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_PROCESSMG_APP10_DEMO_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_PROCESSMG_APP5_ORD_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_QTTUNNEL_CONNECT_RETURN_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_QTTUNNEL_IS_CONNECTED_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_HUB_PROCESSMG_SIGNAL_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_HUB_PROCESSMG_QT_METACALL_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_HUB_PROCESSMG_QT_STATIC_METACALL_BACKTRACE_EMITTED: AtomicBool =
    AtomicBool::new(false);
static EXSTAR_UI_TOPIC_PROCESSMG_PUBLISH_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_UI_TOPIC_PROCESSMG_SIGNAL_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_UI_TOPIC_QTTUNNEL_PUBLISHED_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_WAIT_MUTANT_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_WAIT_FAILED_PROCESS_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_EXE_6940_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_EXE_F0F8_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_CHILD_HUB_THREAD_EXIT_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_CHILD_HUB_FORCED_EXEC_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_CHILD_HUB_SINGLESHOT_6940_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_EXIT_PROCESS_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_TERMINATE_PROCESS_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_HUB_AV_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_SECOND_SWEEP_LOAD_CONFIG_BACKTRACE_EMITTED: AtomicBool =
    AtomicBool::new(false);
static EXSTAR_MANAGER_SECOND_SWEEP_KILL_ONE_BACKTRACE_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_COUNT: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_SECOND_SWEEP_ID: AtomicU32 = AtomicU32::new(0);
static EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_WAIT_BACKTRACE_EMITTED: AtomicBool =
    AtomicBool::new(false);
static EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_TERMINATE_BACKTRACE_EMITTED: AtomicBool =
    AtomicBool::new(false);
static EXSTAR_MANAGER_SKIP_KILL_DEPRECATED_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_PRESERVE_HUB_SCANHUB_DEPRECATED_EMITTED: AtomicBool = AtomicBool::new(false);
static EXSTAR_MANAGER_SKIP_SECOND_SWEEP_DEPRECATED_EMITTED: AtomicBool = AtomicBool::new(false);

struct HostLaunchInfo {
    api_name: &'static str,
    application_name: Option<String>,
    command_line: Option<String>,
    current_directory: Option<String>,
}

type NavNoArgs = unsafe extern "system" fn(*mut c_void);
type NavBoolArg = unsafe extern "system" fn(*mut c_void, u8);
type NavMapArg = unsafe extern "system" fn(*mut c_void, *const c_void);
type QtStaticNoArgs = unsafe extern "system" fn();
type QtStaticIntArg = unsafe extern "system" fn(i32);
type QtStaticIntReturn = unsafe extern "system" fn() -> i32;
type NavQtMetacall = unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32;
type NavQtStaticMetacall = unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void);
type ProcessMgPublish =
    unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, u8) -> u8;
type ProcessMgConnectAndRegister = unsafe extern "system" fn(*mut c_void, *mut c_void) -> u8;
type ProcessMgRegisterSubscribe = unsafe extern "system" fn(*mut c_void, *const c_void) -> u8;
type ProcessMgSubPubConnectToHub =
    unsafe extern "system" fn(*mut c_void, *const c_void, *mut c_void) -> u8;
type ProcessMgSignalPublished =
    unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, *const c_void) -> u8;
type QtTunnelCtor = unsafe extern "system" fn(
    *mut c_void,
    *const c_void,
    *const c_void,
    *const c_void,
    *const c_void,
    u16,
    *mut c_void,
) -> *mut c_void;
type QtTunnelDtor = unsafe extern "system" fn(*mut c_void);
type QtTunnelConnectNoArgs = unsafe extern "system" fn(*mut c_void);
type QtTunnelConnectWithInt = unsafe extern "system" fn(*mut c_void, i32) -> u8;
type QtTunnelIsConnected = unsafe extern "system" fn(*const c_void) -> u8;
type QtTunnelPublish =
    unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, u8) -> u8;
type QtTunnelPublished =
    unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, *const c_void);
type QtTunnelQtMetacall = unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32;
type QtTunnelQtStaticMetacall = unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void);
type QtObjectNoArgs = unsafe extern "system" fn(*mut c_void);
type QtObjectNoArgsReturn = unsafe extern "system" fn(*mut c_void) -> u8;
type QtObjectBoolArg = unsafe extern "system" fn(*mut c_void, u8);
type QtObjectEvent = unsafe extern "system" fn(*mut c_void, *mut c_void) -> u8;
type QtEventTypeFn = unsafe extern "system" fn(*mut c_void) -> i32;
type QtConnectionDtor = unsafe extern "system" fn(*mut c_void);
type QtThreadMsleep = unsafe extern "system" fn(u32);
type QtTimerSingleShotImpl = unsafe extern "system" fn(i32, i32, *const c_void, *mut c_void);
#[repr(C)]
#[derive(Clone, Copy)]
struct QtGenericArgument {
    name: *const c_char,
    data: *const c_void,
}
type QtInvokeMethodWithType = unsafe extern "system" fn(
    *mut c_void,
    *const c_char,
    i32,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
    QtGenericArgument,
) -> u8;
type QtHostAddressCtor = unsafe extern "system" fn(*mut c_void, i32) -> *mut c_void;
type QtHostAddressDtor = unsafe extern "system" fn(*mut c_void);
type Sn3DBoxPluginInstance = unsafe extern "system" fn() -> *mut c_void;
type Sn3DApplicationInit = unsafe extern "system" fn(*mut c_void, *mut c_void);
type Sn3DApplicationLoad = unsafe extern "system" fn(*mut c_void, *const c_void, *mut c_void);
type Sn3DUICppQmlItem = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type Sn3DUICppSetQmlItem = unsafe extern "system" fn(*mut c_void, *mut c_void);
type Sn3DUICppStartStop = unsafe extern "system" fn(*mut c_void) -> i32;
type OffsetTraceFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
) -> usize;

/// A Detours hook anchor that is validated by a byte-signature before attachment.
///
/// `rva` is the expected offset of a function prologue inside a module. `sig`
/// is a short slice of bytes we expect to find at that offset — typically 16
/// bytes of MSVC x64 prologue. If the bytes at `handle + rva` don't match
/// `sig`, `try_attach_offset` logs `offset_sig_mismatch` and refuses to patch.
///
/// `T` is the function pointer type stored in `slot`; in practice this is
/// `OffsetTraceFn` for tables of generic probes, or a more specific fn type
/// for individual hooks used via chained `try_attach_offset` calls.
struct OffsetProbe<T: 'static> {
    label: &'static str,
    rva: usize,
    sig: &'static [u8],
    slot: *mut Option<T>,
    detour: *mut c_void,
}
type ShowWindowFn = unsafe extern "system" fn(HWND, i32) -> BOOL;
type SetWindowPosFn = unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> BOOL;
type DestroyWindowFn = unsafe extern "system" fn(HWND) -> BOOL;

static mut NAV_CLICK_LOGIN: Option<NavNoArgs> = None;
static mut NAV_LOGIN: Option<NavNoArgs> = None;
static mut NAV_DEVICE_OFFLINE: Option<NavBoolArg> = None;
static mut NAV_SHOW_AUTHOR_PROMPT: Option<NavBoolArg> = None;
static mut NAV_DEVICE_INFO: Option<NavMapArg> = None;
static mut NAV_LOGIN_USER_INFO: Option<NavMapArg> = None;
static mut NAV_QT_METACALL: Option<NavQtMetacall> = None;
static mut NAV_QT_STATIC_METACALL: Option<NavQtStaticMetacall> = None;
static mut PROCESS_MG_PUBLISH: Option<ProcessMgPublish> = None;
static mut PROCESS_MG_CONNECT_AND_REGISTER: Option<ProcessMgConnectAndRegister> = None;
static mut PROCESS_MG_REGISTER_SUBSCRIBE: Option<ProcessMgRegisterSubscribe> = None;
static mut PROCESS_MG_SUBPUB_CONNECT: Option<ProcessMgSubPubConnectToHub> = None;
static mut PROCESS_MG_SIGNAL_PUBLISHED: Option<ProcessMgSignalPublished> = None;
static mut PROCESS_MG_QT_METACALL: Option<NavQtMetacall> = None;
static mut PROCESS_MG_QT_STATIC_METACALL: Option<NavQtStaticMetacall> = None;
static mut QTTUNNEL_MODULE_CTOR: Option<QtTunnelCtor> = None;
static mut QTTUNNEL_MODULE_DTOR: Option<QtTunnelDtor> = None;
static mut QTTUNNEL_MODULE_CONNECT: Option<QtTunnelConnectNoArgs> = None;
static mut QTTUNNEL_MODULE_CONNECT_WITH_INT: Option<QtTunnelConnectWithInt> = None;
static mut QTTUNNEL_MODULE_IS_CONNECTED: Option<QtTunnelIsConnected> = None;
static mut QTTUNNEL_MODULE_PUBLISH: Option<QtTunnelPublish> = None;
static mut QTTUNNEL_MODULE_PUBLISHED: Option<QtTunnelPublished> = None;
static mut QTTUNNEL_MODULE_QT_METACALL: Option<QtTunnelQtMetacall> = None;
static mut QTTUNNEL_MODULE_QT_STATIC_METACALL: Option<QtTunnelQtStaticMetacall> = None;
static mut QT_WIDGET_HIDE: Option<QtObjectNoArgs> = None;
static mut QT_WIDGET_SHOW: Option<QtObjectNoArgs> = None;
static mut QT_WIDGET_CLOSE: Option<QtObjectNoArgsReturn> = None;
static mut QT_WIDGET_SET_VISIBLE: Option<QtObjectBoolArg> = None;
static mut QT_WIDGET_EVENT: Option<QtObjectEvent> = None;
static mut QT_WINDOW_HIDE: Option<QtObjectNoArgs> = None;
static mut QT_WINDOW_SHOW: Option<QtObjectNoArgs> = None;
static mut QT_WINDOW_CLOSE: Option<QtObjectNoArgsReturn> = None;
static mut QT_WINDOW_SET_VISIBLE: Option<QtObjectBoolArg> = None;
static mut QT_WINDOW_EVENT: Option<QtObjectEvent> = None;
static mut QT_EVENT_TYPE: Option<QtEventTypeFn> = None;
static mut QT_CONNECTION_DTOR: Option<QtConnectionDtor> = None;
static mut QT_THREAD_MSLEEP: Option<QtThreadMsleep> = None;
static mut QT_TIMER_SINGLESHOT_IMPL: Option<QtTimerSingleShotImpl> = None;
static mut QT_METAOBJECT_INVOKE_METHOD_WITH_TYPE: Option<QtInvokeMethodWithType> = None;
static mut QT_CORE_APPLICATION_QUIT: Option<QtStaticNoArgs> = None;
static mut QT_CORE_APPLICATION_EXIT: Option<QtStaticIntArg> = None;
static mut QT_APPLICATION_EXEC: Option<QtStaticIntReturn> = None;
type QMessageBoxWarningFn = unsafe extern "system" fn(
    *mut c_void, // parent QWidget*
    *const c_void, // title QString&
    *const c_void, // text QString&
    u32, // buttons
    u32, // defaultButton
) -> u32;
static mut QT_MESSAGEBOX_WARNING: Option<QMessageBoxWarningFn> = None;
static mut QT_MESSAGEBOX_CRITICAL: Option<QMessageBoxWarningFn> = None;
static mut QT_MESSAGEBOX_INFORMATION: Option<QMessageBoxWarningFn> = None;
type QDialogExecFn = unsafe extern "system" fn(*mut c_void) -> i32;
static mut QT_DIALOG_EXEC: Option<QDialogExecFn> = None;
static mut QT_HOST_ADDRESS_CTOR: Option<QtHostAddressCtor> = None;
static mut QT_HOST_ADDRESS_DTOR: Option<QtHostAddressDtor> = None;
static mut SN3DBOX_PLUGIN_INSTANCE: Option<Sn3DBoxPluginInstance> = None;
static mut SN3DBOX_APP_INIT: Option<Sn3DApplicationInit> = None;
static mut SN3DBOX_APP_LOAD: Option<Sn3DApplicationLoad> = None;
static mut SN3DBOX_UI_QML_ITEM: Option<Sn3DUICppQmlItem> = None;
static mut SN3DBOX_UI_SET_QML_ITEM: Option<Sn3DUICppSetQmlItem> = None;
static mut SN3DBOX_UI_START: Option<Sn3DUICppStartStop> = None;
static mut SN3DBOX_UI_STOP: Option<Sn3DUICppStartStop> = None;
static mut APPUI_HANDLE_SHOW_PASSPORT: Option<OffsetTraceFn> = None;
static mut PASSPORT_HANDLE_SHOW_PASSPORT_CMD: Option<OffsetTraceFn> = None;
static mut PASSPORT_HANDLE_LOGIN_SUCCESS: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_6940: Option<OffsetTraceFn> = None;
static mut SCANSERVICE_EXE_ENTRY_6A40: Option<OffsetTraceFn> = None;
static mut SCANSERVICE_PRE_CONNECT: Option<OffsetTraceFn> = None;
static mut SCANSERVICE_ALT_PATH: Option<OffsetTraceFn> = None;
static mut SCANSERVICE_PRE_EXEC: Option<OffsetTraceFn> = None;
static mut SCANSERVICE_EARLY_CHECK: Option<OffsetTraceFn> = None;
static mut SCANSERVICE_CONNECT_AND_REGISTER_ORIGINAL: Option<
    unsafe extern "system" fn(*mut c_void, *mut c_void) -> u8,
> = None;
static mut EXSTAR_EXE_6DC0: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_BC30: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_F0F8: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_F9EC: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_FAC4: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_F6C0: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_10390: Option<OffsetTraceFn> = None;
static mut EXSTAR_EXE_A6E0: Option<OffsetTraceFn> = None;

// Sn3DDeviceEinStar.dll hooks — bypass device cleanup deadlock
static mut SN3D_DEVICE_STOP: Option<unsafe extern "system" fn(*mut c_void) -> u32> = None;

static mut PROCESS_MANAGER_CHECK_OPENGL: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_KILL_ALL_E1A0: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_KILL_ONE_E560: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_LOAD_CONFIG_EF30: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_F5F0: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_HANDLE_FLOW_E4B9: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_TERMINATE_EA6A: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_HANDLE_FLOW_EE82: Option<OffsetTraceFn> = None;
static mut PROCESS_MANAGER_EXE_HANDLE_FLOW_EF97: Option<OffsetTraceFn> = None;
static mut PROCESS_MG_OSTREAM_HELPER_3410: Option<OffsetTraceFn> = None;
static mut PROCESS_MG_POST_CONNECT_7996: Option<OffsetTraceFn> = None;
static mut PROCESS_MG_POST_CONNECT_CALL_79A4: Option<OffsetTraceFn> = None;
static mut PROCESS_MG_PLUGIN_CONNECT_IMPL_WRAPPER_6D60: Option<OffsetTraceFn> = None;
static mut PROCESS_MG_PLUGIN_CONNECT_TO_HUB_72C0: Option<OffsetTraceFn> = None;
static mut EXSTAR_VECTORED_EXCEPTION_HANDLER: *mut c_void = ptr::null_mut();
static mut SHOW_WINDOW: ShowWindowFn = ShowWindow;
static mut SET_WINDOW_POS: SetWindowPosFn = SetWindowPos;
static mut DESTROY_WINDOW: DestroyWindowFn = DestroyWindow;

struct DetourPaths {
    lookup: DllLookup,
    override_paths: [Option<(&'static CStr, Vec<u16>)>; zluda_windows::LIBRARIES.len()],
}

impl DetourPaths {
    fn new() -> Self {
        let lookup = DllLookup::new();
        let paths = zluda_windows::LIBRARIES.each_ref().map(|lib| {
            get_payload(unsafe { mem::transmute(&lib.guid) }).map(|payload| {
                let utf8 = unsafe { CStr::from_bytes_with_nul_unchecked(payload) };
                let utf16 =
                    unsafe { U16CString::from_str_unchecked(utf8.to_string_lossy()) }.into_vec();
                (utf8, utf16)
            })
        });
        DetourPaths {
            lookup,
            override_paths: paths,
        }
    }

    fn ascii_override(this: &Option<Self>, path: *const u8) -> *const u8 {
        Self::override_impl(
            this,
            path,
            |p| {
                let cstr = unsafe { CStr::from_ptr(p.cast()) };
                cstr.to_bytes()
            },
            |lookup, buffer| lookup.lookup_ascii(buffer),
            |(override_ascii, _)| override_ascii.as_ptr().cast(),
        )
    }

    fn utf16_override(this: &Option<Self>, path: *const u16) -> *const u16 {
        Self::override_impl(
            this,
            path,
            |p| {
                let u16cstr = unsafe { U16CStr::from_ptr_str(p) };
                u16cstr.as_slice()
            },
            |lookup, buffer| lookup.lookup_utf16(buffer),
            |(_, override_utf16)| override_utf16.as_ptr(),
        )
    }

    fn override_impl<T: 'static>(
        this: &Option<Self>,
        path: *const T,
        get_buffer: impl FnOnce(*const T) -> &'static [T],
        lookup: impl FnOnce(&DllLookup, &[T]) -> Option<usize>,
        get_pointer: impl FnOnce(&(&'static CStr, Vec<u16>)) -> *const T,
    ) -> *const T {
        this.as_ref()
            .map(|this| {
                let buffer = get_buffer(path);
                let index = lookup(&this.lookup, buffer);
                index
                    .map(|index| this.override_paths[index].as_ref().map(|p| get_pointer(p)))
                    .flatten()
            })
            .flatten()
            .unwrap_or(path)
    }
}


fn exstar_einscan_net_svr_compat_enabled() -> bool {
    *EXSTAR_EINSCAN_NET_SVR_COMPAT.get_or_init(|| env_flag("ZLUDA_EXSTAR_EINSCAN_NET_SVR_COMPAT"))
}

fn exstar_hub_exit_delay_ms() -> u64 {
    *EXSTAR_HUB_EXIT_DELAY_MS.get_or_init(|| {
        let value = match env::var("ZLUDA_EXSTAR_HUB_EXIT_DELAY_MS") {
            Ok(value) => value,
            Err(_) => return 0,
        };
        match value.trim().parse::<u64>() {
            Ok(delay_ms) => delay_ms,
            Err(err) => {
                eprintln!(
                    "[ZLUDA_EXSTAR_HOST] invalid ZLUDA_EXSTAR_HUB_EXIT_DELAY_MS value={value:?} error={err}"
                );
                0
            }
        }
    })
}

fn exstar_hub_startup_compat_timeout() -> Duration {
    Duration::from_millis(*EXSTAR_HUB_STARTUP_COMPAT_TIMEOUT_MS.get_or_init(|| {
        let value = match env::var("ZLUDA_EXSTAR_HUB_STARTUP_COMPAT_TIMEOUT_MS") {
            Ok(value) => value,
            Err(_) => return EXSTAR_HUB_STARTUP_COMPAT_MAX.as_millis() as u64,
        };
        match value.trim().parse::<u64>() {
            Ok(timeout_ms) => timeout_ms,
            Err(err) => {
                eprintln!(
                    "[ZLUDA_EXSTAR_HOST] invalid ZLUDA_EXSTAR_HUB_STARTUP_COMPAT_TIMEOUT_MS value={value:?} error={err}"
                );
                EXSTAR_HUB_STARTUP_COMPAT_MAX.as_millis() as u64
            }
        }
    }))
}

fn exstar_einscan_net_svr_publish_fallback_enabled() -> bool {
    *EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK
        .get_or_init(|| env_flag("ZLUDA_EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK"))
}

fn exstar_einscan_net_svr_publish_fallback_delay_ms() -> u64 {
    *EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK_DELAY_MS.get_or_init(|| {
        let value = match env::var("ZLUDA_EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK_DELAY_MS") {
            Ok(value) => value,
            Err(_) => return 0,
        };
        match value.trim().parse::<u64>() {
            Ok(delay_ms) => delay_ms,
            Err(err) => {
                eprintln!(
                    "[ZLUDA_EXSTAR_HOST] invalid ZLUDA_EXSTAR_EINSCAN_NET_SVR_PUBLISH_FALLBACK_DELAY_MS value={value:?} error={err}"
                );
                0
            }
        }
    })
}

// [DEPRECATED/DIAGNOSTIC ONLY] Target-specific skip lists were useful to
// isolate individual cleanup victims, but they did not identify a sufficient
// supported fix surface for EXStar startup.
fn exstar_manager_skip_kill_targets() -> &'static [String] {
    let targets = EXSTAR_MANAGER_SKIP_KILL_TARGETS
        .get_or_init(|| {
            env::var("ZLUDA_EXSTAR_MANAGER_SKIP_KILL")
                .ok()
                .map(|value| {
                    value
                        .split(|c: char| matches!(c, ',' | ';' | '|'))
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .as_slice();
    if !targets.is_empty()
        && !EXSTAR_MANAGER_SKIP_KILL_DEPRECATED_EMITTED.swap(true, Ordering::SeqCst)
    {
        log_exstar_host(format_args!(
            "kind=compat status=deprecated_diagnostic_only flag=ZLUDA_EXSTAR_MANAGER_SKIP_KILL reason=individual-target skips were useful for tracing but were not sufficient as a supported EXStar fix"
        ));
    }
    targets
}

// [DEPRECATED/DIAGNOSTIC ONLY] The two-peer preservation experiment helped
// narrow the branch, but it was less robust than the shipped core-peer default.
fn exstar_manager_preserve_hub_and_scanhub_enabled() -> bool {
    let enabled = *EXSTAR_MANAGER_PRESERVE_HUB_AND_SCANHUB
        .get_or_init(|| env_flag("ZLUDA_EXSTAR_MANAGER_PRESERVE_HUB_AND_SCANHUB"));
    if enabled
        && !EXSTAR_MANAGER_PRESERVE_HUB_SCANHUB_DEPRECATED_EMITTED.swap(true, Ordering::SeqCst)
    {
        log_exstar_host(format_args!(
            "kind=compat status=deprecated_diagnostic_only flag=ZLUDA_EXSTAR_MANAGER_PRESERVE_HUB_AND_SCANHUB reason=two-peer preservation reopened some runs but was retired in favor of the more robust default core-peer workaround"
        ));
    }
    enabled
}

// [DEPRECATED/DIAGNOSTIC ONLY] Second-sweep skipping bypassed the symptom,
// not the ruptured manager state, so it is retained only for trace experiments.
fn exstar_manager_skip_second_sweep_enabled() -> bool {
    let enabled = *EXSTAR_MANAGER_SKIP_SECOND_SWEEP
        .get_or_init(|| env_flag("ZLUDA_EXSTAR_MANAGER_SKIP_SECOND_SWEEP"));
    if enabled && !EXSTAR_MANAGER_SKIP_SECOND_SWEEP_DEPRECATED_EMITTED.swap(true, Ordering::SeqCst)
    {
        log_exstar_host(format_args!(
            "kind=compat status=deprecated_diagnostic_only flag=ZLUDA_EXSTAR_MANAGER_SKIP_SECOND_SWEEP reason=second-sweep skip bypassed the symptom, not the ruptured state, and was insufficient as a fix"
        ));
    }
    enabled
}

fn exstar_manager_is_core_peer(process_name: &str) -> bool {
    process_name.eq_ignore_ascii_case("scanhub.exe")
        || process_name.eq_ignore_ascii_case("EXStar Hub.exe")
        || process_name.eq_ignore_ascii_case("scanservice.exe")
        || process_name.eq_ignore_ascii_case("softwareUpgrade.exe")
        || process_name.eq_ignore_ascii_case("informationCollect.exe")
}

fn exstar_is_helper_connect_retry_target(process_name: &str) -> bool {
    process_name.eq_ignore_ascii_case("softwareUpgrade.exe")
        || process_name.eq_ignore_ascii_case("informationCollect.exe")
}

fn exstar_manager_preserve_core_peers_enabled() -> bool {
    let _ = EXSTAR_MANAGER_PRESERVE_CORE_PEERS
        .get_or_init(|| env_flag("ZLUDA_EXSTAR_MANAGER_PRESERVE_CORE_PEERS"));
    true
}

fn exstar_manager_compat_hooks_enabled() -> bool {
    exstar_manager_preserve_core_peers_enabled()
        || exstar_manager_preserve_hub_and_scanhub_enabled()
        || exstar_manager_skip_second_sweep_enabled()
        || !exstar_manager_skip_kill_targets().is_empty()
}

fn exstar_current_process_is_manager(trigger: &str) -> bool {
    let Some((_, current_exe_name)) = exstar_current_exe(trigger) else {
        return false;
    };
    current_exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe")
}

fn exstar_manager_should_skip_kill(process_name: &str, _second_sweep_id: u32) -> bool {
    if !exstar_current_process_is_manager("manager_skip_kill") {
        return false;
    }
    // Always prevent the manager from killing core processes.
    // Under ZLUDA, the startup takes longer and the manager's timeout expires
    // before all processes report ready. Killing scanhub cascades to all processes.
    let core_processes = [
        "scanhub.exe",
        "EXStar Hub.exe",
        "scanservice.exe",
        "softwareUpgrade.exe",
        "informationCollect.exe",
        "SnSyncService.exe",
    ];
    if core_processes.iter().any(|p| p.eq_ignore_ascii_case(process_name)) {
        return true;
    }
    // Also check legacy env-var-based skip logic
    if exstar_manager_skip_second_sweep_enabled() && _second_sweep_id != 0 {
        return true;
    }
    if exstar_manager_preserve_core_peers_enabled() && exstar_manager_is_core_peer(process_name) {
        return true;
    }
    exstar_manager_skip_kill_targets()
        .iter()
        .any(|target| target.eq_ignore_ascii_case(process_name))
}

fn exstar_manager_clear_second_sweep(reason: &str) {
    let second_sweep_id = EXSTAR_MANAGER_SECOND_SWEEP_ID.swap(0, Ordering::SeqCst);
    EXSTAR_MANAGER_SECOND_SWEEP_PENDING.store(false, Ordering::SeqCst);
    if second_sweep_id != 0 {
        log_exstar_host(format_args!(
            "kind=manager_cleanup_transition event=second_sweep_clear reason={} second_sweep={}",
            reason, second_sweep_id
        ));
    }
}

fn exstar_is_ui_state_topic(topic_text: &str) -> bool {
    matches!(
        topic_text,
        "AppUiTopic" | "ModeChoosePageTopic" | "Sn3DUserPassportTopic"
    )
}

fn exstar_processmg_branch_label(strings: &str) -> Option<&'static str> {
    let lower = strings.to_ascii_lowercase();
    if lower.contains("app10") && lower.contains("demo") {
        Some("app10_demo")
    } else if lower.contains("app1") && lower.contains("app6") {
        Some("app1_app6")
    } else if lower.contains("app5") && lower.contains("ord") {
        Some("app5_ord")
    } else if lower.contains("scanchannel") {
        Some("scanchannel")
    } else if lower.contains("sn3dprocessmanager.exe") && lower.contains("app5") {
        Some("manager_app5")
    } else if lower.contains("sn3dprocessmgtopic") && lower.contains("name") {
        Some("topic_name")
    } else {
        None
    }
}


fn exstar_force_main_window_visible_enabled() -> bool {
    *EXSTAR_FORCE_MAIN_WINDOW_VISIBLE
        .get_or_init(|| env_flag("ZLUDA_EXSTAR_FORCE_MAIN_WINDOW_VISIBLE"))
}

fn exstar_hub_quit_compat_enabled() -> bool {
    true
}

fn exstar_hub_startup_compat_active(_trigger: &str) -> bool {
    if !exstar_hub_quit_compat_enabled() {
        return false;
    }
    // The child Hub now reaches the normal QTimer -> 0x6940 startup path and
    // creates the real App_EA main window on its own. The older
    // force_main_window_visible compat was only needed when child startup fell
    // back to the duplicate-open dialog path, and it interferes with the
    // natural teardown of the transient "EXStar Hub" window.
    if env::args().any(|a| a == "@#$")
        && exstar_current_exe("hub_startup_compat_active")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false)
    {
        return false;
    }
    EXSTAR_HUB_STARTUP_COMPAT_START
        .get_or_init(Instant::now)
        .elapsed()
        <= exstar_hub_startup_compat_timeout()
}

fn exstar_child_hub_force_show_compat_active(_trigger: &str) -> bool {
    false
}

fn exstar_is_child_hub_process() -> bool {
    env::args().any(|a| a == "@#$")
        && exstar_current_exe("is_child_hub_process")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false)
}

fn exstar_child_hub_manager_pid_from_args() -> Option<u32> {
    if !exstar_is_child_hub_process() {
        return None;
    }
    env::args().nth(2)?.parse().ok()
}

fn exstar_should_preserve_child_hub_exit(
    startup_compat_active: bool,
    real_app_window_shown: bool,
) -> bool {
    startup_compat_active && !real_app_window_shown
}

fn exstar_should_force_child_hub_quit(payload_strings: &str, real_app_window_shown: bool) -> bool {
    exstar_is_child_hub_process()
        && real_app_window_shown
        && payload_strings
            .split('|')
            .any(|part| part.eq_ignore_ascii_case("quitApp"))
}

unsafe fn exstar_should_preserve_main_window(trigger: &str, hwnd: HWND) -> bool {
    if !is_exstar_main_window(hwnd) {
        return false;
    }
    if EXSTAR_CHILD_HUB_APP_WINDOW_SHOWN.load(Ordering::SeqCst)
        || exstar_child_hub_real_app_window_exists()
    {
        log_exstar_host(format_args!(
            "kind=compat action=release_startup_shell trigger={} hwnd={:p} title=EXStar Hub app_window_shown=true",
            trigger,
            hwnd as *mut c_void
        ));
        return false;
    }
    true
}

fn exstar_hub_main_window_compat_active(trigger: &str) -> bool {
    exstar_hub_startup_compat_active(trigger) && exstar_on_main_window_thread()
}


fn exstar_skip_qttunnel_connect_hook_enabled() -> bool {
    *EXSTAR_SKIP_QTTUNNEL_CONNECT_HOOK
        .get_or_init(|| env_flag("ZLUDA_EXSTAR_SKIP_QTTUNNEL_CONNECT_HOOK"))
}


fn launch_targets_einscan_net_svr(launch: &HostLaunchInfo) -> bool {
    launch
        .application_name
        .as_deref()
        .into_iter()
        .chain(launch.command_line.as_deref())
        .any(|text| text.to_ascii_lowercase().contains("einscan_net_svr.exe"))
}

fn launch_targets_process_name(launch: &HostLaunchInfo, process_name: &str) -> bool {
    launch
        .application_name
        .as_deref()
        .into_iter()
        .chain(launch.command_line.as_deref())
        .any(|text| text.to_ascii_lowercase().contains(process_name))
}

fn decode_pcstr(ptr: *const u8) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(ptr.cast()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn decode_pcwstr(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { U16CStr::from_ptr_str(ptr) }.to_string_lossy())
}

fn log_exstar_child_launch(
    launch: &HostLaunchInfo,
    creation_flags: u32,
    proc_info: *const windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
    result: BOOL,
) {
    if !exstar_host_trace_enabled() {
        return;
    }
    let (process_id, thread_id, process_handle, thread_handle) = unsafe {
        proc_info
            .as_ref()
            .map(|info| {
                (
                    info.dwProcessId,
                    info.dwThreadId,
                    info.hProcess as *mut c_void,
                    info.hThread as *mut c_void,
                )
            })
            .unwrap_or((0, 0, ptr::null_mut(), ptr::null_mut()))
    };
    let application_name = launch.application_name.as_deref().unwrap_or("<null>");
    let command_line = launch.command_line.as_deref().unwrap_or("<null>");
    let current_directory = launch.current_directory.as_deref().unwrap_or("<null>");
    let launches_einscan_net_svr = launch_targets_einscan_net_svr(launch);
    if result != 0 && launches_einscan_net_svr {
        EXSTAR_EINSCAN_NET_SVR_LAUNCHED.store(true, Ordering::SeqCst);
        let _ = acquire_einscan_launch_latch_silent();
    }
    if result != 0
        && exstar_manager_process_trace_enabled("CreateProcessW")
        && launch_targets_process_name(launch, "scanhub.exe")
    {
        let launch_count = EXSTAR_MANAGER_SCANHUB_LAUNCH_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
        EXSTAR_MANAGER_SECOND_SWEEP_PENDING.store(true, Ordering::SeqCst);
        log_exstar_host(format_args!(
            "kind=manager_cleanup_transition event=scanhub_launch count={} pid={} process_handle={:p} thread_handle={:p} application={} command_line={}",
            launch_count,
            process_id,
            process_handle,
            thread_handle,
            application_name,
            command_line
        ));
    }
    log_exstar_host(format_args!(
        "kind=launch api={} success={} pid={} tid={} process_handle={:p} thread_handle={:p} creation_flags=0x{:x} application={} command_line={} cwd={}",
        launch.api_name,
        result != 0,
        process_id,
        thread_id,
        process_handle,
        thread_handle,
        creation_flags,
        application_name,
        command_line,
        current_directory
    ));
}

fn exstar_current_exe(trigger: &str) -> Option<(std::path::PathBuf, String)> {
    let current_exe = match env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            log_exstar_host(format_args!(
                "kind=compat action=launch_einscan_net_svr trigger={} status=current-exe-error error={}",
                trigger,
                err
            ));
            return None;
        }
    };
    let current_exe_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>")
        .to_string();
    Some((current_exe, current_exe_name))
}

fn exstar_window_trace_enabled() -> bool {
    let Some((_, current_exe_name)) = exstar_current_exe("window_trace") else {
        return false;
    };
    current_exe_name.eq_ignore_ascii_case("EXStar Hub.exe")
}

fn exstar_hub_process_trace_enabled(trigger: &str) -> bool {
    if !exstar_host_trace_enabled() {
        return false;
    }
    let Some((_, current_exe_name)) = exstar_current_exe(trigger) else {
        return false;
    };
    current_exe_name.eq_ignore_ascii_case("EXStar Hub.exe")
}

fn exstar_manager_process_trace_enabled(trigger: &str) -> bool {
    if !exstar_trace_logging_enabled() {
        return false;
    }
    let Some((_, current_exe_name)) = exstar_current_exe(trigger) else {
        return false;
    };
    current_exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe")
}

fn exstar_manager_or_hub_process_trace_enabled(trigger: &str) -> bool {
    if !exstar_trace_logging_enabled() {
        return false;
    }
    let Some((_, current_exe_name)) = exstar_current_exe(trigger) else {
        return false;
    };
    current_exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe")
        || current_exe_name.eq_ignore_ascii_case("EXStar Hub.exe")
}

fn exstar_manager_named_mutexes() -> &'static Mutex<FxHashMap<usize, String>> {
    static MUTEXES: OnceLock<Mutex<FxHashMap<usize, String>>> = OnceLock::new();
    MUTEXES.get_or_init(|| Mutex::new(FxHashMap::default()))
}

unsafe fn read_window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 260];
    let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

unsafe fn read_window_class(hwnd: HWND) -> String {
    let mut buffer = [0u16; 260];
    let len = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

unsafe fn read_window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        None
    } else {
        Some(rect)
    }
}

unsafe extern "system" fn exstar_enum_windows_find_app_window(hwnd: HWND, lparam: isize) -> i32 {
    let state = &mut *(lparam as *mut (u32, bool));
    let mut owner_pid = 0u32;
    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, &mut owner_pid);
    if owner_pid == state.0 {
        let title = read_window_text(hwnd);
        if title.contains("App_EA.xml") {
            state.1 = true;
            return 0;
        }
    }
    1
}

unsafe extern "system" fn exstar_enum_windows_capture_app_window(hwnd: HWND, lparam: isize) -> i32 {
    let state = &mut *(lparam as *mut (u32, HWND));
    let mut owner_pid = 0u32;
    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, &mut owner_pid);
    if owner_pid == state.0 {
        let title = read_window_text(hwnd);
        if title.contains("App_EA.xml") {
            state.1 = hwnd;
            return 0;
        }
    }
    1
}

unsafe fn exstar_child_hub_real_app_window_exists() -> bool {
    if !env::args().any(|a| a == "@#$")
        || !exstar_current_exe("child_hub_real_app_window_exists")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false)
    {
        return false;
    }
    let mut state = (GetCurrentProcessId(), false);
    windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(
        Some(exstar_enum_windows_find_app_window),
        &mut state as *mut _ as isize,
    );
    state.1
}

unsafe fn exstar_child_hub_real_app_window() -> HWND {
    if !env::args().any(|a| a == "@#$")
        || !exstar_current_exe("child_hub_real_app_window")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false)
    {
        return ptr::null_mut();
    }
    let mut state = (GetCurrentProcessId(), ptr::null_mut());
    windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(
        Some(exstar_enum_windows_capture_app_window),
        &mut state as *mut _ as isize,
    );
    state.1
}

unsafe fn ensure_exstar_qt_visibility_hooks() {
    if !exstar_window_trace_enabled() {
        return;
    }
    let qt_widgets = GetModuleHandleA(c"Qt5Widgets.dll".as_ptr().cast());
    if !qt_widgets.is_null() {
        let _ = detour_exstar_qt_widgets(qt_widgets.cast());
    }
    let qt_gui = GetModuleHandleA(c"Qt5Gui.dll".as_ptr().cast());
    if !qt_gui.is_null() {
        let _ = detour_exstar_qt_gui(qt_gui.cast());
    }
}

unsafe fn is_exstar_main_window(hwnd: HWND) -> bool {
    !hwnd.is_null() && exstar_window_trace_enabled() && read_window_text(hwnd) == "EXStar Hub"
}

unsafe fn log_window_transition(
    method: &str,
    hwnd: HWND,
    detail: std::fmt::Arguments<'_>,
    result: BOOL,
) {
    ensure_exstar_qt_visibility_hooks();
    if !exstar_host_trace_enabled() || !exstar_window_trace_enabled() || hwnd.is_null() {
        return;
    }
    let title = read_window_text(hwnd);
    if title == "EXStar Hub" {
        let _ = EXSTAR_MAIN_WINDOW_THREAD_ID.compare_exchange(
            0,
            GetCurrentThreadId(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    let class_name = read_window_class(hwnd);
    let rect = read_window_rect(hwnd)
        .map(|rect| format!("{},{},{},{}", rect.left, rect.top, rect.right, rect.bottom))
        .unwrap_or_else(|| "<unavailable>".to_string());
    let visible = IsWindowVisible(hwnd) != 0;
    if title.contains("App_EA.xml") && visible {
        let was_shown = EXSTAR_CHILD_HUB_APP_WINDOW_SHOWN.swap(true, Ordering::SeqCst);
        if !was_shown {
            log_exstar_host(format_args!(
                "kind=compat action=app_window_shown title=\"{}\" hwnd={:p}",
                title,
                hwnd as *mut c_void
            ));
        }
        if !EXSTAR_CHILD_HUB_APP_WINDOW_FOREGROUNDED.swap(true, Ordering::SeqCst) {
            exstar_promote_child_app_window(hwnd, &title);
        }
    }
    log_exstar_host(format_args!(
        "kind=window method={} hwnd={:p} title=\"{}\" class=\"{}\" visible={} rect={} result={} detail={}",
        method,
        hwnd as *mut c_void,
        title,
        class_name,
        visible,
        rect,
        result,
        detail
    ));
}

unsafe fn exstar_promote_child_app_window(hwnd: HWND, title: &str) {
    let show_result = SHOW_WINDOW(hwnd, 5);
    let top_result = BringWindowToTop(hwnd);
    let foreground_result = SetForegroundWindow(hwnd);
    let foreground_hwnd = GetForegroundWindow();
    let foreground_title = if foreground_hwnd.is_null() {
        String::new()
    } else {
        read_window_text(foreground_hwnd)
    };
    log_exstar_host(format_args!(
        "kind=compat action=foreground_child_app_window hwnd={:p} title=\"{}\" show_result={} top_result={} foreground_result={} foreground_hwnd={:p} foreground_title=\"{}\"",
        hwnd as *mut c_void,
        title,
        show_result,
        top_result,
        foreground_result,
        foreground_hwnd as *mut c_void,
        foreground_title
    ));
}

fn acquire_einscan_launch_latch(trigger: &str) -> Result<(), u32> {
    acquire_einscan_launch_latch_impl(Some(trigger))
}

fn acquire_einscan_launch_latch_silent() -> Result<(), u32> {
    acquire_einscan_launch_latch_impl(None)
}

fn acquire_einscan_launch_latch_impl(trigger: Option<&str>) -> Result<(), u32> {
    if EXSTAR_EINSCAN_NET_SVR_LAUNCH_LATCH.get().is_some() {
        return Ok(());
    }
    let latch_name = U16CString::from_str("Local\\ZLUDA_EXSTAR_EINSCAN_NET_SVR_LAUNCH")
        .expect("static mutex name should be valid UTF-16");
    let handle = unsafe {
        CreateMutexW(
            ptr::null::<SECURITY_ATTRIBUTES>(),
            FALSE,
            latch_name.as_ptr(),
        )
    };
    if handle.is_null() {
        return Err(unsafe { GetLastError() });
    }
    let last_error = unsafe { GetLastError() };
    if last_error == ERROR_ALREADY_EXISTS {
        if let Some(trigger) = trigger {
            log_exstar_host(format_args!(
                "kind=compat action=launch_einscan_net_svr trigger={} status=skipped reason=latch-exists",
                trigger
            ));
        }
        return Err(ERROR_ALREADY_EXISTS);
    }
    let _ = EXSTAR_EINSCAN_NET_SVR_LAUNCH_LATCH.set(handle as usize);
    Ok(())
}

fn spawn_einscan_net_svr(trigger: &str, current_exe: &std::path::Path, payload_strings: &str) {
    if EXSTAR_EINSCAN_NET_SVR_LAUNCHED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    match acquire_einscan_launch_latch(trigger) {
        Ok(()) => {}
        Err(ERROR_ALREADY_EXISTS) => return,
        Err(err) => {
            EXSTAR_EINSCAN_NET_SVR_LAUNCHED.store(false, Ordering::SeqCst);
            log_exstar_host(format_args!(
                "kind=compat action=launch_einscan_net_svr trigger={} status=latch-error error={}",
                trigger, err
            ));
            return;
        }
    }
    let Some(parent_dir) = current_exe.parent() else {
        EXSTAR_EINSCAN_NET_SVR_LAUNCHED.store(false, Ordering::SeqCst);
        log_exstar_host(format_args!(
            "kind=compat action=launch_einscan_net_svr trigger={} status=no-parent-dir",
            trigger
        ));
        return;
    };
    let target = parent_dir.join("einscan_net_svr.exe");
    if !target.is_file() {
        EXSTAR_EINSCAN_NET_SVR_LAUNCHED.store(false, Ordering::SeqCst);
        log_exstar_host(format_args!(
            "kind=compat action=launch_einscan_net_svr trigger={} status=missing target={}",
            trigger,
            target.display()
        ));
        return;
    }
    let owner_pid = std::process::id();
    let mut command = Command::new(".\\einscan_net_svr.exe");
    command
        .current_dir(parent_dir)
        .arg(owner_pid.to_string())
        .arg("@#$")
        .arg("einscan_net_svr.exe")
        .arg("")
        .arg("./")
        .arg("-1")
        .arg("")
        .arg("0")
        .arg("0")
        .arg("0")
        .arg("1")
        .arg("0")
        .arg("@#$");
    match command.spawn() {
        Ok(child) => {
            log_exstar_host(format_args!(
                "kind=compat action=launch_einscan_net_svr trigger={} status=spawned mode=manager-style pid={} owner_pid={} target={} payload_strings=\"{}\"",
                trigger,
                child.id(),
                owner_pid,
                target.display(),
                payload_strings
            ));
        }
        Err(err) => {
            EXSTAR_EINSCAN_NET_SVR_LAUNCHED.store(false, Ordering::SeqCst);
            log_exstar_host(format_args!(
                "kind=compat action=launch_einscan_net_svr trigger={} status=spawn-failed mode=manager-style target={} error={}",
                trigger,
                target.display(),
                err
            ));
        }
    }
}

fn maybe_launch_einscan_net_svr_publish(trigger: &str, payload_strings: &str) {
    if !exstar_einscan_net_svr_compat_enabled() || !payload_strings.contains("einscan_net_svr") {
        return;
    }
    let Some((current_exe, current_exe_name)) = exstar_current_exe(trigger) else {
        return;
    };
    if current_exe_name.eq_ignore_ascii_case("einscan_net_svr.exe") {
        return;
    }
    if !current_exe_name.eq_ignore_ascii_case("informationCollect.exe") {
        return;
    }
    let payload_segments = payload_strings
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let use_publish_fallback = exstar_einscan_net_svr_publish_fallback_enabled();
    let payload_reason = if payload_segments.len() == 3
        && payload_segments[0].eq_ignore_ascii_case("informationCollect.exe")
        && payload_segments[1].eq_ignore_ascii_case("einscan_net_svr")
        && payload_segments[2].eq_ignore_ascii_case("xml")
    {
        None
    } else if use_publish_fallback {
        None
    } else if payload_segments.len() == 1
        && payload_segments[0].eq_ignore_ascii_case("einscan_net_svr")
    {
        Some("bare-publish-observational")
    } else if payload_segments
        .iter()
        .any(|segment| segment.eq_ignore_ascii_case("channel"))
    {
        Some("channel-observational")
    } else if payload_segments
        .iter()
        .any(|segment| segment.eq_ignore_ascii_case("app8"))
    {
        Some("app8-observational")
    } else {
        Some("payload-mismatch")
    };
    if let Some(reason) = payload_reason {
        log_exstar_host(format_args!(
            "kind=compat action=launch_einscan_net_svr trigger={} status=skipped reason={} current_exe={} payload_strings=\"{}\"",
            trigger,
            reason,
            current_exe_name,
            payload_strings
        ));
        return;
    }
    let using_publish_fallback = use_publish_fallback
        && !(payload_segments.len() == 3
            && payload_segments[0].eq_ignore_ascii_case("informationCollect.exe")
            && payload_segments[1].eq_ignore_ascii_case("einscan_net_svr")
            && payload_segments[2].eq_ignore_ascii_case("xml"));
    if using_publish_fallback {
        log_exstar_host(format_args!(
            "kind=compat action=launch_einscan_net_svr trigger={} status=using-publish-fallback current_exe={} payload_strings=\"{}\"",
            trigger,
            current_exe_name,
            payload_strings
        ));
    }
    let publish_fallback_delay_ms = if using_publish_fallback {
        exstar_einscan_net_svr_publish_fallback_delay_ms()
    } else {
        0
    };
    if publish_fallback_delay_ms != 0 {
        log_exstar_host(format_args!(
            "kind=compat action=launch_einscan_net_svr trigger={} status=delaying-publish-fallback current_exe={} delay_ms={} payload_strings=\"{}\"",
            trigger,
            current_exe_name,
            publish_fallback_delay_ms,
            payload_strings
        ));
        thread::sleep(Duration::from_millis(publish_fallback_delay_ms));
    }
    spawn_einscan_net_svr(trigger, &current_exe, payload_strings);
}

fn maybe_launch_einscan_net_svr_delivery(
    trigger: &str,
    arg1_text: &str,
    arg2_text: &str,
    payload_strings: &str,
) {
    if !exstar_einscan_net_svr_compat_enabled() {
        return;
    }
    let Some((current_exe, current_exe_name)) = exstar_current_exe(trigger) else {
        return;
    };
    if current_exe_name.eq_ignore_ascii_case("einscan_net_svr.exe") {
        return;
    }
    if !current_exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe") {
        return;
    }
    if !arg1_text.eq_ignore_ascii_case("app5")
        || !arg2_text.eq_ignore_ascii_case("Sn3dProcessMgTopic")
    {
        return;
    }
    spawn_einscan_net_svr(trigger, &current_exe, payload_strings);
}

fn memory_readable(ptr: *const c_void, min_len: usize) -> bool {
    if ptr.is_null() || min_len == 0 {
        return false;
    }
    let mut mbi = unsafe { mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
    let queried =
        unsafe { VirtualQuery(ptr, &mut mbi, mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
    if queried < mem::size_of::<MEMORY_BASIC_INFORMATION>() {
        return false;
    }
    if mbi.State != MEM_COMMIT {
        return false;
    }
    let protect = mbi.Protect;
    if protect == PAGE_NOACCESS || (protect & PAGE_GUARD) != 0 {
        return false;
    }
    let readable = matches!(
        protect,
        PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY
    );
    if !readable {
        return false;
    }
    let base = mbi.BaseAddress as usize;
    let end = base.saturating_add(mbi.RegionSize);
    let start = ptr as usize;
    start >= base && start.saturating_add(min_len) <= end
}

fn read_usize(ptr: *const c_void) -> Option<usize> {
    if !memory_readable(ptr, mem::size_of::<usize>()) {
        return None;
    }
    Some(unsafe { ptr::read_unaligned(ptr as *const usize) })
}

fn decode_utf16_candidate(ptr: *const u16, max_units: usize) -> Option<String> {
    let byte_len = max_units.checked_mul(mem::size_of::<u16>())?;
    if ptr.is_null()
        || max_units == 0
        || !(ptr as usize).is_multiple_of(mem::align_of::<u16>())
        || !memory_readable(ptr.cast(), byte_len)
    {
        return None;
    }
    let slice = unsafe { slice::from_raw_parts(ptr, max_units) };
    let end = slice.iter().position(|ch| *ch == 0)?;
    if end < 3 || end > 96 {
        return None;
    }
    let text = String::from_utf16_lossy(&slice[..end]);
    let printable = text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.' | '/' | ' '));
    let has_alpha = text.chars().any(|ch| ch.is_ascii_alphabetic());
    if printable && has_alpha {
        Some(text)
    } else {
        None
    }
}

fn decode_qstring_ref(ptr: *const c_void) -> Option<String> {
    panic::catch_unwind(|| {
        if ptr.is_null() || !memory_readable(ptr, mem::size_of::<usize>()) {
            return None;
        }
        if let Some(text) = decode_utf16_candidate(ptr.cast(), 96) {
            return Some(text);
        }
        let data_ptr = read_usize(ptr)? as *const u8;
        if data_ptr.is_null() {
            return None;
        }
        for offset in [0usize, 8, 16, 24, 32, 40] {
            let candidate = unsafe { data_ptr.add(offset) as *const u16 };
            if let Some(text) = decode_utf16_candidate(candidate, 96) {
                return Some(text);
            }
        }
        None
    })
    .ok()
    .flatten()
}

fn push_unique_string(strings: &mut Vec<String>, text: String) {
    if !strings.iter().any(|existing| existing == &text) {
        strings.push(text);
    }
}

fn prune_redundant_strings(strings: &mut Vec<String>) {
    strings.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    let mut filtered = Vec::<String>::new();
    for text in strings.drain(..) {
        if text.starts_with("0x")
            && text.len() > 4
            && text[2..].chars().all(|ch| ch.is_ascii_hexdigit())
        {
            continue;
        }
        if filtered.iter().any(|existing| existing.contains(&text)) {
            continue;
        }
        filtered.push(text);
        if filtered.len() >= 8 {
            break;
        }
    }
    *strings = filtered;
}

fn collect_utf16_strings(ptr: *const c_void, byte_len: usize, strings: &mut Vec<String>) {
    if ptr.is_null() || byte_len < 8 || !memory_readable(ptr, byte_len) {
        return;
    }
    for offset in (0..byte_len.saturating_sub(8)).step_by(2) {
        let candidate = unsafe { (ptr as *const u8).add(offset) as *const u16 };
        if let Some(text) = decode_utf16_candidate(candidate, 64) {
            push_unique_string(strings, text);
            if strings.len() >= 24 {
                break;
            }
        }
    }
}

fn payload_primary_text(payload: *const c_void) -> Option<String> {
    panic::catch_unwind(|| {
        if let Some(text) = decode_qstring_ref(payload) {
            return Some(text);
        }
        let mut strings = Vec::<String>::new();
        collect_utf16_strings(payload, 128, &mut strings);
        prune_redundant_strings(&mut strings);
        if let Some(text) = strings.into_iter().next() {
            return Some(text);
        }
        for offset in (0..96usize).step_by(mem::size_of::<usize>()) {
            let field_ptr = unsafe { (payload as *const u8).add(offset) as *const c_void };
            let candidate = match read_usize(field_ptr) {
                Some(value) if value != 0 => value as *const c_void,
                _ => continue,
            };
            if let Some(text) = decode_qstring_ref(candidate) {
                return Some(text);
            }
            let mut nested_strings = Vec::<String>::new();
            collect_utf16_strings(candidate, 128, &mut nested_strings);
            prune_redundant_strings(&mut nested_strings);
            if let Some(text) = nested_strings.into_iter().next() {
                return Some(text);
            }
        }
        None
    })
    .ok()
    .flatten()
}

fn qt_metacall_args_summary(args: *mut *mut c_void, limit: usize) -> String {
    if args.is_null()
        || !memory_readable(
            args.cast_const().cast(),
            limit * mem::size_of::<*mut c_void>(),
        )
    {
        return "<unavailable>".to_string();
    }
    let mut parts = Vec::new();
    for index in 0..limit {
        let slot = unsafe { args.add(index) };
        let value = unsafe { *slot };
        let mut part = format!(
            "slot{}={:p}[{}]",
            index,
            value,
            describe_optional_address(value.cast_const())
        );
        if let Some(deref) = read_usize(slot.cast()) {
            let deref_ptr = deref as *const c_void;
            part.push_str(&format!(
                " deref={:p}[{}]",
                deref_ptr,
                describe_optional_address(deref_ptr)
            ));
        }
        parts.push(part);
    }
    parts.join(" | ")
}

fn module_name_from_address(ptr: *const c_void) -> Option<String> {
    module_info_from_address(ptr).map(|(name, _)| name)
}

fn module_info_from_address(ptr: *const c_void) -> Option<(String, usize)> {
    if ptr.is_null() || !memory_readable(ptr, 1) {
        return None;
    }
    let mut mbi = unsafe { mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
    let queried =
        unsafe { VirtualQuery(ptr, &mut mbi, mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
    if queried < mem::size_of::<MEMORY_BASIC_INFORMATION>() || mbi.AllocationBase.is_null() {
        return None;
    }
    let mut buffer = [0u8; 512];
    let len = unsafe {
        GetModuleFileNameA(
            mbi.AllocationBase as HMODULE,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    };
    if len == 0 || len as usize > buffer.len() {
        return None;
    }
    let path = String::from_utf8_lossy(&buffer[..len as usize]).into_owned();
    let name = path
        .rsplit(path::MAIN_SEPARATOR)
        .next()
        .map(|name| name.to_string())?;
    Some((name, mbi.AllocationBase as usize))
}

fn describe_address(ptr: *const c_void) -> String {
    if let Some((module, base)) = module_info_from_address(ptr) {
        format!("{}+0x{:x}", module, (ptr as usize).saturating_sub(base))
    } else {
        format!("0x{:x}", ptr as usize)
    }
}

fn describe_optional_address(ptr: *const c_void) -> String {
    if ptr.is_null() {
        "null".to_string()
    } else {
        describe_address(ptr)
    }
}

fn exstar_plugin_tail_window_offset(ptr: *const c_void) -> Option<usize> {
    let (module, base) = module_info_from_address(ptr)?;
    let offset = (ptr as usize).saturating_sub(base);
    if module.eq_ignore_ascii_case("Sn3DProcessPlugin.dll") && (0x7996..=0x79BE).contains(&offset) {
        Some(offset)
    } else {
        None
    }
}

unsafe fn log_offset_probe(
    kind: &str,
    label: &str,
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) {
    log_exstar_host(format_args!(
        "kind={} method={} this={:p} arg1={:p} arg2={:p} arg3={:p} arg4={:p} arg5={:p}",
        kind,
        label,
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    ));
}

unsafe extern "system" fn exstar_vectored_exception_handler(
    info: *mut ExstarExceptionPointers,
) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let record = (*info).exception_record;
    let context = (*info).context_record;
    if record.is_null() || context.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let exception_address = (*record).exception_address.cast_const();
    #[cfg(target_arch = "x86_64")]
    let rip = (*context).Rip as *const c_void;
    #[cfg(not(target_arch = "x86_64"))]
    let rip = ptr::null();
    let exception_offset = exstar_plugin_tail_window_offset(exception_address);
    let rip_offset = exstar_plugin_tail_window_offset(rip);
    let exception_code = (*record).exception_code;
    let is_access_violation = exception_code == 0xC0000005u32 as i32;
    let is_stack_buffer_overrun = exception_code == 0xC0000409u32 as i32;
    let should_log_manager_hub_failure = (is_access_violation || is_stack_buffer_overrun)
        && exstar_manager_or_hub_process_trace_enabled("vectored_exception")
        && !EXSTAR_MANAGER_HUB_AV_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst);
    if exception_offset.is_none() && rip_offset.is_none() && !should_log_manager_hub_failure {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    log_exstar_host(format_args!(
        "kind=exception code=0x{:x} flags=0x{:x} address={:p}[{}] rip={:p}[{}] info0=0x{:x} info1=0x{:x}",
        (*record).exception_code,
        (*record).exception_flags,
        exception_address,
        describe_optional_address(exception_address),
        rip,
        describe_optional_address(rip),
        if (*record).number_parameters > 0 {
            (*record).exception_information[0]
        } else {
            0
        },
        if (*record).number_parameters > 1 {
            (*record).exception_information[1]
        } else {
            0
        },
    ));
    if should_log_manager_hub_failure {
        log_exstar_host_backtrace(
            "kind=exception_backtrace",
            format_args!(
                "code=0x{:x} address={:p}[{}] rip={:p}[{}] info0=0x{:x} info1=0x{:x}",
                (*record).exception_code,
                exception_address,
                describe_optional_address(exception_address),
                rip,
                describe_optional_address(rip),
                if (*record).number_parameters > 0 {
                    (*record).exception_information[0]
                } else {
                    0
                },
                if (*record).number_parameters > 1 {
                    (*record).exception_information[1]
                } else {
                    0
                },
            ),
            2,
        );
    }
    EXCEPTION_CONTINUE_SEARCH
}

unsafe fn ensure_exstar_vectored_exception_handler() {
    if !exstar_trace_logging_enabled() || !EXSTAR_VECTORED_EXCEPTION_HANDLER.is_null() {
        return;
    }
    EXSTAR_VECTORED_EXCEPTION_HANDLER =
        AddVectoredExceptionHandler(1, Some(exstar_vectored_exception_handler)).cast();
    if EXSTAR_VECTORED_EXCEPTION_HANDLER.is_null() {
        log_exstar_host(format_args!("kind=exception hook=veh_register_failed"));
    } else {
        log_exstar_host(format_args!(
            "kind=exception hook=veh_registered handle={:p}",
            EXSTAR_VECTORED_EXCEPTION_HANDLER
        ));
    }
}

fn exstar_on_main_window_thread() -> bool {
    let tracked = EXSTAR_MAIN_WINDOW_THREAD_ID.load(Ordering::SeqCst);
    tracked != 0 && tracked == unsafe { GetCurrentThreadId() }
}

fn exstar_manager_sn3d_community_kill_context() -> Option<(u32, u32)> {
    let tracked = EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_THREAD_ID.load(Ordering::SeqCst);
    let current = unsafe { GetCurrentThreadId() };
    if tracked == 0 || tracked != current {
        return None;
    }
    Some((
        EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_COUNT.load(Ordering::SeqCst),
        EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_SECOND_SWEEP_ID.load(Ordering::SeqCst),
    ))
}

fn exstar_manager_begin_sn3d_community_kill_context(kill_one_count: u32, second_sweep_id: u32) {
    let current = unsafe { GetCurrentThreadId() };
    EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_COUNT.store(kill_one_count, Ordering::SeqCst);
    EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_SECOND_SWEEP_ID.store(second_sweep_id, Ordering::SeqCst);
    EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_THREAD_ID.store(current, Ordering::SeqCst);
}

fn exstar_manager_end_sn3d_community_kill_context() {
    EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_THREAD_ID.store(0, Ordering::SeqCst);
    EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_COUNT.store(0, Ordering::SeqCst);
    EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_SECOND_SWEEP_ID.store(0, Ordering::SeqCst);
}

unsafe fn maybe_log_exe_entry_backtrace(
    emitted: &AtomicBool,
    label: &str,
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    require_main_window_thread: bool,
) {
    let on_main_window_thread = exstar_on_main_window_thread();
    if (require_main_window_thread && !on_main_window_thread)
        || emitted.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let thread_id = GetCurrentThreadId();
    log_exstar_host(format_args!(
        "kind=exe method={} this={:p} thread_id={} on_main_window_thread={} arg1={:p}[{}] arg2={:p}[{}] arg3={:p}[{}]",
        label,
        this,
        thread_id,
        on_main_window_thread,
        arg1,
        describe_optional_address(arg1.cast_const()),
        arg2,
        describe_optional_address(arg2.cast_const()),
        arg3,
        describe_optional_address(arg3.cast_const()),
    ));
    log_exstar_backtrace(
        "kind=exe_entry_backtrace",
        format_args!(
            "method={} this={:p} thread_id={} on_main_window_thread={} arg1={:p}[{}] arg2={:p}[{}] arg3={:p}[{}]",
            label,
            this,
            thread_id,
            on_main_window_thread,
            arg1,
            describe_optional_address(arg1.cast_const()),
            arg2,
            describe_optional_address(arg2.cast_const()),
            arg3,
            describe_optional_address(arg3.cast_const()),
        ),
        2,
    );
}

unsafe fn maybe_log_window_hide_backtrace(hwnd: HWND, n_cmd_show: i32) {
    if n_cmd_show != 0
        || !exstar_host_trace_enabled()
        || !exstar_window_trace_enabled()
        || hwnd.is_null()
    {
        return;
    }
    let title = read_window_text(hwnd);
    if !title.eq_ignore_ascii_case("EXStar Hub")
        || EXSTAR_WINDOW_HIDE_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let mut frames = [ptr::null_mut::<c_void>(); 24];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    log_exstar_host(format_args!(
        "kind=window_hide_backtrace title=\"{}\" frames={}",
        title, frame_count
    ));
    for (index, frame) in frames[..frame_count].iter().enumerate() {
        log_exstar_host(format_args!(
            "kind=window_hide_backtrace frame={} addr={}",
            index,
            describe_address((*frame).cast_const())
        ));
    }
}

unsafe fn log_exstar_backtrace(kind: &str, detail: std::fmt::Arguments<'_>, skip_frames: u32) {
    if !exstar_host_trace_enabled() || !exstar_window_trace_enabled() {
        return;
    }
    let mut frames = [ptr::null_mut::<c_void>(); 24];
    let frame_count = RtlCaptureStackBackTrace(
        skip_frames,
        frames.len() as u32,
        frames.as_mut_ptr(),
        ptr::null_mut(),
    ) as usize;
    log_exstar_host(format_args!(
        "{kind} frames={} detail={detail}",
        frame_count
    ));
    for (index, frame) in frames[..frame_count].iter().enumerate() {
        log_exstar_host(format_args!(
            "{kind} frame={} addr={}",
            index,
            describe_address((*frame).cast_const())
        ));
    }
}

unsafe fn log_exstar_host_backtrace(kind: &str, detail: std::fmt::Arguments<'_>, skip_frames: u32) {
    if !exstar_trace_logging_enabled() {
        return;
    }
    let mut frames = [ptr::null_mut::<c_void>(); 24];
    let frame_count = RtlCaptureStackBackTrace(
        skip_frames,
        frames.len() as u32,
        frames.as_mut_ptr(),
        ptr::null_mut(),
    ) as usize;
    log_exstar_host(format_args!(
        "{kind} frames={} detail={detail}",
        frame_count
    ));
    for (index, frame) in frames[..frame_count].iter().enumerate() {
        log_exstar_host(format_args!(
            "{kind} frame={} addr={}",
            index,
            describe_address((*frame).cast_const())
        ));
    }
}

unsafe fn maybe_log_qwidget_hide_backtrace(method: &str, this: *mut c_void) {
    if !exstar_window_trace_enabled()
        || this.is_null()
        || EXSTAR_QTWIDGET_HIDE_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
    {
        return;
    }
    log_exstar_backtrace(
        "kind=qtwidget_hide_backtrace",
        format_args!("method={} this={:p}", method, this),
        2,
    );
}

unsafe fn maybe_log_qwindow_hide_backtrace(method: &str, this: *mut c_void) {
    if !exstar_window_trace_enabled()
        || this.is_null()
        || EXSTAR_QWINDOW_HIDE_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
    {
        return;
    }
    log_exstar_backtrace(
        "kind=qwindow_hide_backtrace",
        format_args!("method={} this={:p}", method, this),
        2,
    );
}

unsafe fn maybe_log_qwindow_event19_backtrace(this: *mut c_void) {
    if !exstar_window_trace_enabled()
        || this.is_null()
        || EXSTAR_QWINDOW_EVENT19_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
    {
        return;
    }
    log_exstar_backtrace(
        "kind=qwindow_event19_backtrace",
        format_args!("method=event this={:p} type=19", this),
        2,
    );
}

unsafe fn qt_event_type(event: *mut c_void) -> Option<i32> {
    if event.is_null() {
        return None;
    }
    if QT_EVENT_TYPE.is_none() {
        let qt_core = GetModuleHandleA(c"Qt5Core.dll".as_ptr().cast());
        if !qt_core.is_null() {
            QT_EVENT_TYPE =
                GetProcAddress(qt_core, c"?type@QEvent@@QEBA?AW4Type@1@XZ".as_ptr().cast())
                    .map(|proc| mem::transmute_copy(&proc));
        }
    }
    QT_EVENT_TYPE.map(|f| f(event))
}

fn is_interesting_qt_event_type(event_type: i32) -> bool {
    matches!(event_type, 17 | 18 | 19 | 24 | 25 | 99 | 105)
}

fn payload_identity(payload: *const c_void) -> String {
    if payload.is_null() {
        return "null".to_string();
    }
    let allocation = module_name_from_address(payload).unwrap_or_else(|| "<heap>".to_string());
    let first_word = read_usize(payload).unwrap_or(0);
    let first_word_ptr = first_word as *const c_void;
    let first_word_module =
        module_name_from_address(first_word_ptr).unwrap_or_else(|| "<none>".to_string());
    format!(
        "alloc_module={} first_word=0x{:x} first_word_module={}",
        allocation, first_word, first_word_module
    )
}

fn payload_strings(payload: *const c_void) -> String {
    panic::catch_unwind(|| {
        if payload.is_null() || !memory_readable(payload, mem::size_of::<usize>()) {
            return "<none>".to_string();
        }
        let mut strings = Vec::<String>::new();
        if let Some(text) = decode_qstring_ref(payload) {
            push_unique_string(&mut strings, text);
        }
        collect_utf16_strings(payload, 128, &mut strings);
        for offset in (0..64usize).step_by(mem::size_of::<usize>()) {
            let field_ptr = unsafe { (payload as *const u8).add(offset) as *const c_void };
            let candidate = match read_usize(field_ptr) {
                Some(value) if value != 0 => value as *const c_void,
                _ => continue,
            };
            if let Some(text) = decode_qstring_ref(candidate) {
                push_unique_string(&mut strings, text);
            }
            collect_utf16_strings(candidate, 128, &mut strings);
            if strings.len() >= 24 {
                break;
            }
        }
        prune_redundant_strings(&mut strings);
        if strings.is_empty() {
            "<none>".to_string()
        } else {
            strings.join("|")
        }
    })
    .unwrap_or_else(|_| "<panic>".to_string())
}


#[link(name = "ntdll.dll", kind = "raw-dylib")]
unsafe extern "system" {
    fn LdrLoadDll(
        dll_path: LPCWSTR,
        dll_characteristics: *mut u32,
        dll_name: *const UNICODE_STRING,
        dll_handle: *mut detours_sys::PVOID,
    ) -> NTSTATUS;
    fn NtQueryObject(
        handle: HANDLE,
        object_information_class: u32,
        object_information: *mut c_void,
        object_information_length: u32,
        return_length: *mut u32,
    ) -> NTSTATUS;
    fn NtTerminateProcess(
        process_handle: HANDLE,
        exit_status: i32,
    ) -> NTSTATUS;
}

const OBJECT_NAME_INFORMATION_CLASS: u32 = 1;
const OBJECT_TYPE_INFORMATION_CLASS: u32 = 2;

static mut NT_TERMINATE_PROCESS: unsafe extern "system" fn(
    HANDLE,
    i32,
) -> NTSTATUS = NtTerminateProcess;

static mut LDR_LOAD_DLL: unsafe extern "system" fn(
    LPCWSTR,
    *mut u32,
    *const UNICODE_STRING,
    *mut detours_sys::PVOID,
) -> NTSTATUS = LdrLoadDll;

static mut LOAD_LIBRARY_A: unsafe extern "system" fn(lp_lib_file_name: PCSTR) -> HMODULE =
    LoadLibraryA;

static mut LOAD_LIBRARY_W: unsafe extern "system" fn(lp_lib_file_name: PCWSTR) -> HMODULE =
    LoadLibraryW;

static mut LOAD_LIBRARY_EX_A: unsafe extern "system" fn(
    lp_lib_file_name: PCSTR,
    file: windows_sys::Win32::Foundation::HANDLE,
    flags: LOAD_LIBRARY_FLAGS,
) -> HMODULE = LoadLibraryExA;

static mut LOAD_LIBRARY_EX_W: unsafe extern "system" fn(
    lp_lib_file_name: LPCWSTR,
    file: windows_sys::Win32::Foundation::HANDLE,
    flags: LOAD_LIBRARY_FLAGS,
) -> HMODULE = LoadLibraryExW;

static mut SLEEP_EX: unsafe extern "system" fn(u32, BOOL) -> u32 =
    windows_sys::Win32::System::Threading::SleepEx;

// Hook GetUserDefaultLocaleName to prevent QSystemLocale::query hang in scanservice.exe
extern "system" {
    fn GetUserDefaultLocaleName(lplocalename: *mut u16, cchlocalename: i32) -> i32;
}
static mut GET_USER_DEFAULT_LOCALE_NAME: unsafe extern "system" fn(*mut u16, i32) -> i32 =
    GetUserDefaultLocaleName;

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaGetUserDefaultLocaleName(
    lplocalename: *mut u16,
    cchlocalename: i32,
) -> i32 {
    static IS_SCANSERVICE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    let v = IS_SCANSERVICE.load(Ordering::Relaxed);
    let is_scan = if v == 0 {
        let yes = exstar_current_exe("GetUserDefaultLocaleName")
            .map(|(_, n)| {
                n.eq_ignore_ascii_case("scanservice.exe")
                    || n.eq_ignore_ascii_case("scanhub.exe")
                    || n.eq_ignore_ascii_case("TestOpenglHelper.exe")
            })
            .unwrap_or(false);
        IS_SCANSERVICE.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
        yes
    } else {
        v == 2
    };

    if is_scan && cchlocalename >= 6 {
        // Return "en-US" directly, bypassing the Windows API that hangs
        let locale: [u16; 6] = [b'e' as u16, b'n' as u16, b'-' as u16, b'U' as u16, b'S' as u16, 0];
        std::ptr::copy_nonoverlapping(locale.as_ptr(), lplocalename, 6);
        if exstar_trace_logging_enabled() {
            log_exstar_host(format_args!(
                "kind=compat action=locale_spoof method=GetUserDefaultLocaleName result=en-US"
            ));
        }
        return 6;
    }

    GET_USER_DEFAULT_LOCALE_NAME(lplocalename, cchlocalename)
}

// DXGI adapter spoofing — intercept IDXGIAdapter::GetDesc to report NVIDIA vendor
// We hook CreateDXGIFactory/CreateDXGIFactory1 and patch the vtable of returned adapters.
static DXGI_GETDESC_HOOKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static mut DXGI_GETDESC_ORIGINAL: Option<
    unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc) -> i32,
> = None;
static mut DXGI_GETDESC1_ORIGINAL: Option<
    unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32,
> = None;

#[repr(C)]
struct DxgiAdapterDesc {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: [u32; 2],
}

#[repr(C)]
struct DxgiAdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: [u32; 2],
    flags: u32,
}

const NVIDIA_VENDOR_ID: u32 = 0x10DE;
// RTX 3060 device ID
const NVIDIA_DEVICE_ID: u32 = 0x2503;

unsafe fn dxgi_write_nvidia_description(desc: *mut [u16; 128]) {
    let name = "NVIDIA GeForce RTX 3060";
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let len = wide.len().min(128);
    std::ptr::copy_nonoverlapping(wide.as_ptr(), (*desc).as_mut_ptr(), len);
}

unsafe extern "system" fn ZludaDxgiGetDesc(
    this: *mut c_void,
    desc: *mut DxgiAdapterDesc,
) -> i32 {
    let result = if let Some(original) = DXGI_GETDESC_ORIGINAL {
        original(this, desc)
    } else {
        -1 // E_FAIL
    };
    if result >= 0 && !desc.is_null() {
        (*desc).vendor_id = NVIDIA_VENDOR_ID;
        (*desc).device_id = NVIDIA_DEVICE_ID;
        dxgi_write_nvidia_description(&mut (*desc).description);
        if exstar_trace_logging_enabled() {
            log_exstar_host(format_args!(
                "kind=dxgi_spoof method=GetDesc vendor_id=0x{:04X} device_id=0x{:04X}",
                NVIDIA_VENDOR_ID, NVIDIA_DEVICE_ID
            ));
        }
    }
    result
}

unsafe extern "system" fn ZludaDxgiGetDesc1(
    this: *mut c_void,
    desc: *mut DxgiAdapterDesc1,
) -> i32 {
    let result = if let Some(original) = DXGI_GETDESC1_ORIGINAL {
        original(this, desc)
    } else {
        -1
    };
    if result >= 0 && !desc.is_null() {
        (*desc).vendor_id = NVIDIA_VENDOR_ID;
        (*desc).device_id = NVIDIA_DEVICE_ID;
        dxgi_write_nvidia_description(&mut (*desc).description);
        if exstar_trace_logging_enabled() {
            log_exstar_host(format_args!(
                "kind=dxgi_spoof method=GetDesc1 vendor_id=0x{:04X} device_id=0x{:04X}",
                NVIDIA_VENDOR_ID, NVIDIA_DEVICE_ID
            ));
        }
    }
    result
}

/// After CreateDXGIFactory succeeds, enumerate adapter 0 and hook its GetDesc vtable entry.
unsafe fn dxgi_hook_adapter_vtable(factory: *mut c_void) {
    if DXGI_GETDESC_HOOKED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    // IDXGIFactory vtable: index 7 = EnumAdapters
    let factory_vtable = *(factory as *const *const *const c_void);
    let enum_adapters: unsafe extern "system" fn(
        *mut c_void, u32, *mut *mut c_void,
    ) -> i32 = std::mem::transmute(*factory_vtable.add(7));

    let mut adapter: *mut c_void = ptr::null_mut();
    let hr = enum_adapters(factory, 0, &mut adapter);
    if hr < 0 || adapter.is_null() {
        return;
    }

    // IDXGIAdapter vtable: index 8 = GetDesc, index 9 = CheckInterfaceSupport
    let adapter_vtable = *(adapter as *const *mut *mut c_void);
    let getdesc_slot = adapter_vtable.add(8);

    // Save original
    DXGI_GETDESC_ORIGINAL = Some(std::mem::transmute(*getdesc_slot));

    // Make vtable writable and patch
    let mut old_protect: u32 = 0;
    let slot_size = std::mem::size_of::<*mut c_void>();
    windows_sys::Win32::System::Memory::VirtualProtect(
        getdesc_slot as *const c_void,
        slot_size,
        0x04, // PAGE_READWRITE
        &mut old_protect,
    );
    *getdesc_slot = ZludaDxgiGetDesc as *mut c_void;
    windows_sys::Win32::System::Memory::VirtualProtect(
        getdesc_slot as *const c_void,
        slot_size,
        old_protect,
        &mut old_protect,
    );

    // Also try IDXGIAdapter1::GetDesc1 at vtable index 10 (if available)
    // IDXGIAdapter1 extends IDXGIAdapter with GetDesc1 at the end
    let getdesc1_slot = adapter_vtable.add(10);
    DXGI_GETDESC1_ORIGINAL = Some(std::mem::transmute(*getdesc1_slot));
    windows_sys::Win32::System::Memory::VirtualProtect(
        getdesc1_slot as *const c_void,
        slot_size,
        0x04,
        &mut old_protect,
    );
    *getdesc1_slot = ZludaDxgiGetDesc1 as *mut c_void;
    windows_sys::Win32::System::Memory::VirtualProtect(
        getdesc1_slot as *const c_void,
        slot_size,
        old_protect,
        &mut old_protect,
    );

    // Release the adapter (AddRef was implicit in EnumAdapters)
    let release: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*(*(adapter as *const *const *const c_void)).add(2));
    release(adapter);

    DXGI_GETDESC_HOOKED.store(true, std::sync::atomic::Ordering::SeqCst);

    if exstar_trace_logging_enabled() {
        log_exstar_host(format_args!(
            "kind=dxgi_spoof action=vtable_hooked adapter={:p}",
            adapter
        ));
    }
}

type LockFileExFn = unsafe extern "system" fn(
    hfile: windows_sys::Win32::Foundation::HANDLE,
    dwflags: u32,
    dwreserved: u32,
    nnumberofbytestolocklow: u32,
    nnumberofbytestolockhigh: u32,
    lpoverlapped: *mut c_void,
) -> BOOL;

static mut LOCK_FILE_EX: LockFileExFn = lock_file_ex_stub;

unsafe extern "system" fn lock_file_ex_stub(
    _hfile: windows_sys::Win32::Foundation::HANDLE,
    _dwflags: u32, _dwreserved: u32,
    _nnumberofbytestolocklow: u32, _nnumberofbytestolockhigh: u32,
    _lpoverlapped: *mut c_void,
) -> BOOL { 1 }

static mut CREATE_MUTEX_A: unsafe extern "system" fn(
    lpmutexattributes: *const SECURITY_ATTRIBUTES,
    binitialowner: BOOL,
    lpname: PCSTR,
) -> windows_sys::Win32::Foundation::HANDLE = CreateMutexA;

static mut CREATE_MUTEX_W: unsafe extern "system" fn(
    lpmutexattributes: *const SECURITY_ATTRIBUTES,
    binitialowner: BOOL,
    lpname: PCWSTR,
) -> windows_sys::Win32::Foundation::HANDLE = CreateMutexW;

static mut WAIT_FOR_SINGLE_OBJECT: unsafe extern "system" fn(
    windows_sys::Win32::Foundation::HANDLE,
    u32,
) -> u32 = WaitForSingleObject;

static mut CREATE_PROCESS_A: unsafe extern "system" fn(
    lpapplicationname: PCSTR,
    lpcommandline: PSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOA,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL = CreateProcessA;

static mut CREATE_PROCESS_W: unsafe extern "system" fn(
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL = CreateProcessW;

static mut CREATE_PROCESS_AS_USER_A: unsafe extern "system" fn(
    htoken: windows_sys::Win32::Foundation::HANDLE,
    lpapplicationname: PCSTR,
    lpcommandline: PSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOA,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL = CreateProcessAsUserA;

static mut CREATE_PROCESS_AS_USER_W: unsafe extern "system" fn(
    htoken: windows_sys::Win32::Foundation::HANDLE,
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL = CreateProcessAsUserW;

static mut CREATE_PROCESS_WITH_TOKEN_W: unsafe extern "system" fn(
    htoken: windows_sys::Win32::Foundation::HANDLE,
    dwlogonflags: windows_sys::Win32::System::Threading::CREATE_PROCESS_LOGON_FLAGS,
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL = CreateProcessWithTokenW;

static mut CREATE_PROCESS_WITH_LOGON_W: unsafe extern "system" fn(
    lpusername: PCWSTR,
    lpdomain: PCWSTR,
    lppassword: PCWSTR,
    dwlogonflags: windows_sys::Win32::System::Threading::CREATE_PROCESS_LOGON_FLAGS,
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL = CreateProcessWithLogonW;

static mut EXIT_PROCESS_FN: unsafe extern "system" fn(u32) -> ! = ExitProcess;

static mut EXIT_THREAD_FN: unsafe extern "system" fn(u32) -> ! =
    windows_sys::Win32::System::Threading::ExitThread;

static mut TERMINATE_PROCESS_FN: unsafe extern "system" fn(
    windows_sys::Win32::Foundation::HANDLE,
    u32,
) -> BOOL = WinTerminateProcess;

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "system" fn ZludaGetProcAddress_NoRedirect(
    hmodule: HMODULE,
    lpprocname: PCSTR,
) -> FARPROC {
    GetProcAddress(hmodule, lpprocname)
}

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLoadLibraryW_NoRedirect(lpLibFileName: LPCWSTR) -> HMODULE {
    let trace_exstar_hub = exstar_host_trace_enabled()
        && exstar_current_exe("LoadLibraryW_NoRedirect")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false);
    let dll_name = if lpLibFileName.is_null() {
        "<null>".to_string()
    } else {
        decode_pcwstr(lpLibFileName as PCWSTR).unwrap_or_else(|| "<decode-failed>".to_string())
    };
    let caller = exstar_capture_caller(2);
    if trace_exstar_hub {
        log_exstar_host(format_args!(
            "kind=loadlibrary method=LoadLibraryW_NoRedirect dll=\"{}\" thread_id={} caller={}",
            dll_name,
            GetCurrentThreadId(),
            describe_address(caller)
        ));
    }
    let result = (LOAD_LIBRARY_W)(lpLibFileName);
    if trace_exstar_hub {
        log_exstar_host(format_args!(
            "kind=loadlibrary method=LoadLibraryW_NoRedirect_return dll=\"{}\" result={:p} thread_id={}",
            dll_name,
            result,
            GetCurrentThreadId()
        ));
    }
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLoadLibraryA(file_name: PCSTR) -> HMODULE {
    let result = LOAD_LIBRARY_A(DetourPaths::ascii_override(
        &*&raw const DETOUR_PATHS,
        file_name,
    ));
    if !result.is_null() && !file_name.is_null() {
        post_load_library_hook_ascii(file_name, result);
    }
    result
}

unsafe fn post_load_library_hook_ascii(file_name: PCSTR, module: HMODULE) {
    let name = std::ffi::CStr::from_ptr(file_name.cast()).to_string_lossy();
    let name_lower = name.to_ascii_lowercase();
    if name_lower.contains("sn3ddeviceeinstar") {
        detour_exstar_device_einstar(module as *mut c_void);
    }
    if name_lower.contains("dxgi") {
        dxgi_try_hook_create_factory(module);
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLoadLibraryW(file_name: LPCWSTR) -> HMODULE {
    // Trace LoadLibraryW for scanservice.exe to debug Qt locale init hang
    let is_scanservice = {
        static IS_SCAN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0); // 0=unchecked, 1=no, 2=yes
        let v = IS_SCAN.load(Ordering::Relaxed);
        if v == 0 {
            let yes = exstar_current_exe("LoadLibraryW").map(|(_, n)| n.eq_ignore_ascii_case("scanservice.exe")).unwrap_or(false);
            IS_SCAN.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
            yes
        } else { v == 2 }
    };
    if is_scanservice && exstar_host_trace_enabled() {
        let dll_name = if file_name.is_null() { "<null>".to_string() } else { decode_pcwstr(file_name as PCWSTR).unwrap_or_default() };
        log_exstar_host(format_args!("kind=scanservice_loadlib method=LoadLibraryW dll={}", dll_name));
    }
    let result = LOAD_LIBRARY_W(DetourPaths::utf16_override(
        &*&raw const DETOUR_PATHS,
        file_name,
    ));
    if is_scanservice && exstar_host_trace_enabled() {
        let dll_name = if file_name.is_null() { "<null>".to_string() } else { decode_pcwstr(file_name as PCWSTR).unwrap_or_default() };
        log_exstar_host(format_args!("kind=scanservice_loadlib method=LoadLibraryW_return dll={} result={:p}", dll_name, result as *const c_void));
    }
    // After dxgi.dll is loaded, hook CreateDXGIFactory to spoof GPU vendor
    if !result.is_null() && !file_name.is_null() {
        let name = decode_pcwstr(file_name as PCWSTR).unwrap_or_default();
        let name_lower = name.to_ascii_lowercase();
        if name_lower.contains("dxgi") {
            dxgi_try_hook_create_factory(result);
        }
        // After Sn3DDeviceEinStar.dll is loaded, hook stop() to bypass device cleanup deadlock
        if name_lower.contains("sn3ddeviceeinstar") {
            detour_exstar_device_einstar(result as *mut c_void);
        }
    }
    result
}

/// Hook CreateDXGIFactory from dxgi.dll to intercept adapter enumeration.
/// Uses a direct function pointer (not Option) so Detours can modify it correctly.
static DXGI_CREATE_FACTORY_HOOKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn dxgi_create_factory_stub(
    _riid: *const [u8; 16],
    _pp_factory: *mut *mut c_void,
) -> i32 {
    -1 // E_FAIL — should never be called; Detours replaces this
}
static mut DXGI_CREATE_FACTORY_ORIGINAL: unsafe extern "system" fn(
    *const [u8; 16],
    *mut *mut c_void,
) -> i32 = dxgi_create_factory_stub;

unsafe extern "system" fn ZludaCreateDXGIFactory(
    riid: *const [u8; 16],
    pp_factory: *mut *mut c_void,
) -> i32 {
    let result = DXGI_CREATE_FACTORY_ORIGINAL(riid, pp_factory);
    if result >= 0 && !pp_factory.is_null() && !(*pp_factory).is_null() {
        dxgi_hook_adapter_vtable(*pp_factory);
    }
    result
}

unsafe fn dxgi_try_hook_create_factory(dxgi_module: HMODULE) {
    if DXGI_CREATE_FACTORY_HOOKED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let proc_name = c"CreateDXGIFactory";
    let create_fn = GetProcAddress(dxgi_module as _, proc_name.as_ptr().cast());
    if create_fn.is_none() {
        return;
    }
    // Set the original to the real function, then detour it
    DXGI_CREATE_FACTORY_ORIGINAL = std::mem::transmute(create_fn.unwrap());

    if DetourTransactionBegin() == NO_ERROR as i32 {
        DetourAttach(
            &raw mut DXGI_CREATE_FACTORY_ORIGINAL as *mut _ as *mut *mut c_void,
            ZludaCreateDXGIFactory as *mut c_void,
        );
        DetourTransactionCommit();
    }

    if exstar_trace_logging_enabled() {
        log_exstar_host(format_args!(
            "kind=dxgi_spoof action=create_factory_hooked_late module={:p}",
            dxgi_module as *const c_void
        ));
    }
}

unsafe fn exstar_capture_caller(skip_frames: u32) -> *const c_void {
    let mut frames = [ptr::null_mut::<c_void>(); 1];
    let frame_count = RtlCaptureStackBackTrace(
        skip_frames,
        frames.len() as u32,
        frames.as_mut_ptr(),
        ptr::null_mut(),
    ) as usize;
    if frame_count == 0 {
        ptr::null()
    } else {
        frames[0].cast_const()
    }
}

unsafe fn exstar_process_snapshot() -> Option<Vec<(u32, u32, String)>> {
    let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if Process32FirstW(snapshot, &mut entry).is_err() {
        CloseHandle(snapshot).ok();
        return None;
    }
    let mut processes = Vec::new();
    loop {
        let exe_name_len = entry
            .szExeFile
            .iter()
            .position(|&ch| ch == 0)
            .unwrap_or(entry.szExeFile.len());
        let exe_name = String::from_utf16_lossy(&entry.szExeFile[..exe_name_len]);
        processes.push((entry.th32ProcessID, entry.th32ParentProcessID, exe_name));
        if Process32NextW(snapshot, &mut entry).is_err() {
            break;
        }
    }
    CloseHandle(snapshot).ok();
    Some(processes)
}

unsafe fn exstar_hub_related_manager() -> Option<(&'static str, u32)> {
    let current_pid = GetCurrentProcessId();
    let Some(processes) = exstar_process_snapshot() else {
        return None;
    };
    let parent_pid = processes
        .iter()
        .find(|(pid, _, _)| *pid == current_pid)
        .map(|(_, parent_pid, _)| *parent_pid);
    if let Some(manager_pid) = parent_pid.filter(|pid| {
        processes
            .iter()
            .find(|(process_id, _, _)| *process_id == *pid)
            .is_some_and(|(_, _, exe_name)| exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe"))
    }) {
        return Some(("parent_manager_alive", manager_pid));
    }
    processes
        .iter()
        .find(|(_, parent_pid, exe_name)| {
            *parent_pid == current_pid && exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe")
        })
        .map(|(pid, _, _)| ("child_manager_alive", *pid))
}

unsafe fn exstar_manager_has_child_hub(manager_pid: u32) -> bool {
    let Some(processes) = exstar_process_snapshot() else {
        return false;
    };
    processes.iter().any(|(_, parent_pid, exe_name)| {
        *parent_pid == manager_pid && exe_name.eq_ignore_ascii_case("EXStar Hub.exe")
    })
}

unsafe fn exstar_preserve_hub_until_related_manager_exit() {
    if !exstar_hub_startup_compat_active("preserve_hub_exit") {
        return;
    }
    // Retry finding the manager — the bootstrap Hub calls ExitProcess quickly
    // after launching the manager, so it might not appear in the process snapshot
    // on the first try.
    let mut found = None;
    for attempt in 0..20 {
        if let Some(result) = exstar_hub_related_manager() {
            found = Some(result);
            break;
        }
        if attempt == 0 {
            log_exstar_host(format_args!(
                "kind=compat action=preserve_hub_exit status=waiting_for_manager"
            ));
        }
        thread::sleep(Duration::from_millis(250));
    }
    let Some((reason, manager_pid)) = found else {
        log_exstar_host(format_args!(
            "kind=compat action=preserve_hub_exit status=no_manager_found"
        ));
        return;
    };
    let start = Instant::now();
    // Close file handles that might block child processes (e.g. Qt settings files).
    // Qt's QSettings::sync() opens .ini files exclusively; if we hold them open,
    // child processes (scanservice.exe) hang trying to open the same file during
    // their QFactoryLoader → QSettings → QFile::open init chain.
    // Use NtQueryInformationProcess/NtQueryObject to find and close file handles.
    close_non_essential_file_handles(reason, manager_pid);
    // Release the "EinScan-Pro.exe" duplicate-instance mutex before entering
    // the preserve loop. The child Hub (launched by the manager) checks this
    // mutex and shows "Software is unable to repeat opening" if it exists.
    let mutex_handle = EXSTAR_DUPLICATE_MUTEX_HANDLE
        .swap(0, std::sync::atomic::Ordering::Relaxed);
    if mutex_handle != 0 {
        windows_sys::Win32::System::Threading::ReleaseMutex(mutex_handle as _);
        CloseHandle(HANDLE(mutex_handle as *mut c_void));
        log_exstar_host(format_args!(
            "kind=compat action=release_duplicate_mutex handle=0x{:x} name=EinScan-Pro.exe",
            mutex_handle
        ));
    }
    log_exstar_host(format_args!(
        "kind=compat action=preserve_hub_exit status=begin reason={} manager_pid={} strategy=message_pump",
        reason, manager_pid
    ));
    // Keep the bootstrap Hub alive while the manager is running.
    // Use alertable waits to allow oplock break callbacks to fire, which helps
    // child processes (scanservice) that need to access the same files.
    while exstar_hub_startup_compat_active("preserve_hub_exit")
        && exstar_hub_related_manager().is_some_and(|(_, pid)| pid == manager_pid)
    {
        // Use alertable wait to process oplock break callbacks
        windows_sys::Win32::UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx(
            0,
            ptr::null(),
            50, // 50ms timeout
            0x04FF, // QS_ALLINPUT
            0x0002 | 0x0004, // MWMO_ALERTABLE | MWMO_INPUTAVAILABLE
        );
        windows_sys::Win32::System::Threading::SleepEx(0, 1); // process pending APCs
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
        while windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW(
            &mut msg,
            std::ptr::null_mut() as _,
            0,
            0,
            1, // PM_REMOVE
        ) != 0
        {
            windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
    log_exstar_host(format_args!(
        "kind=compat action=preserve_hub_exit status=end reason={} manager_pid={} elapsed_ms={}",
        reason, manager_pid, start.elapsed().as_millis()
    ));
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaSleepEx(milliseconds: u32, alertable: BOOL) -> u32 {
    // For scanservice.exe: reduce QLockFile-related sleeps to 1ms (not 0 — 0 causes
    // other timing issues). Only applies to short sleeps during the first 30 seconds
    // of process startup.
    static SCANSERVICE_DETECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    if !CHECKED.load(Ordering::Relaxed) {
        CHECKED.store(true, Ordering::Relaxed);
        let exe_name = exstar_current_exe("SleepEx").map(|(_, n)| n).unwrap_or_default();
        if exe_name.eq_ignore_ascii_case("scanservice.exe") {
            SCANSERVICE_DETECTED.store(true, Ordering::Relaxed);
            let _ = START_TIME.set(Instant::now());
        }
    }
    // Only modify short sleeps (QLockFile uses ~100ms) in scanservice.exe
    // during the first 30 seconds of startup
    if SCANSERVICE_DETECTED.load(Ordering::Relaxed)
        && milliseconds >= 50 && milliseconds <= 200
        && START_TIME.get().map_or(false, |t| t.elapsed().as_secs() < 30)
    {
        // Reduce to 1ms — lets the QLockFile timeout expire in ~150ms instead of ~15s
        return SLEEP_EX(1, alertable);
    }
    SLEEP_EX(milliseconds, alertable)
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLockFileEx(
    hfile: windows_sys::Win32::Foundation::HANDLE,
    dwflags: u32,
    dwreserved: u32,
    nnumberofbytestolocklow: u32,
    nnumberofbytestolockhigh: u32,
    lpoverlapped: *mut c_void,
) -> BOOL {
    // Add LOCKFILE_FAIL_IMMEDIATELY to prevent blocking when another process
    // holds the lock. This fixes the hang where scanservice.exe blocks on
    // QSettings file lock held by the bootstrap Hub during Qt init.
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x00000001;
    let modified_flags = dwflags | LOCKFILE_FAIL_IMMEDIATELY;
    let result = LOCK_FILE_EX(hfile, modified_flags, dwreserved, nnumberofbytestolocklow, nnumberofbytestolockhigh, lpoverlapped);
    if result == 0 && (dwflags & LOCKFILE_FAIL_IMMEDIATELY) == 0 {
        // The original call wanted to block, but we made it fail immediately.
        // Return success anyway — the file write might be slightly unsafe but
        // won't deadlock. Qt will handle concurrent access gracefully enough.
        if exstar_host_trace_enabled() {
            log_exstar_host(format_args!(
                "kind=compat action=lockfileex_nonblocking flags=0x{:x} modified=0x{:x} result=force_success",
                dwflags, modified_flags
            ));
        }
        return 1; // Pretend lock succeeded
    }
    result
}

unsafe fn close_non_essential_file_handles(reason: &str, manager_pid: u32) {
    // NtQuerySystemInformation with SystemHandleInformation is complex.
    // Simpler approach: iterate handle values 4..1024 (typical range) and
    // close file-type handles that aren't stdin/stdout/stderr or our log file.
    let process = windows_sys::Win32::System::Threading::GetCurrentProcess();
    let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());

    // NtQueryObject to get handle type
    type NtQueryObjectFn = unsafe extern "system" fn(
        handle: *mut c_void, info_class: u32, buffer: *mut c_void,
        length: u32, result_length: *mut u32,
    ) -> i32;
    let nt_query_object: Option<NtQueryObjectFn> = std::mem::transmute(
        GetProcAddress(ntdll as _, c"NtQueryObject".as_ptr().cast())
    );
    let Some(query_object) = nt_query_object else {
        log_exstar_host(format_args!(
            "kind=compat action=close_file_handles status=skip reason=no_NtQueryObject"
        ));
        return;
    };

    let mut closed_count = 0u32;
    let stdin_handle = windows_sys::Win32::System::Console::GetStdHandle(
        windows_sys::Win32::System::Console::STD_INPUT_HANDLE
    );
    let stdout_handle = windows_sys::Win32::System::Console::GetStdHandle(
        windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE
    );
    let stderr_handle = windows_sys::Win32::System::Console::GetStdHandle(
        windows_sys::Win32::System::Console::STD_ERROR_HANDLE
    );

    // Try handle values 4 to 4096 (step 4 since handles are multiples of 4)
    for handle_val in (4u64..4096).step_by(4) {
        let handle = handle_val as *mut c_void;
        // Skip stdin/stdout/stderr
        if handle == stdin_handle as *mut c_void
            || handle == stdout_handle as *mut c_void
            || handle == stderr_handle as *mut c_void
        {
            continue;
        }
        // Query object type (ObjectTypeInformation = 2)
        let mut buf = [0u8; 1024];
        let mut result_len: u32 = 0;
        let status = query_object(
            handle, 2, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut result_len
        );
        if status != 0 { continue; } // Invalid handle or error

        // OBJECT_TYPE_INFORMATION starts with UNICODE_STRING (Length: u16, MaxLength: u16, Buffer: *wchar)
        if result_len < 8 { continue; }
        let type_name_len = u16::from_le_bytes([buf[0], buf[1]]) as usize / 2;
        let type_name_ptr = u64::from_le_bytes([buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15]]) as *const u16;
        if type_name_ptr.is_null() || type_name_len == 0 { continue; }
        let type_name = String::from_utf16_lossy(std::slice::from_raw_parts(type_name_ptr, type_name_len));

        if type_name == "File" {
            // This is a file handle — close it to release locks
            CloseHandle(HANDLE(handle as _));
            closed_count += 1;
        }
    }

    log_exstar_host(format_args!(
        "kind=compat action=close_file_handles status=done closed={} reason={} manager_pid={}",
        closed_count, reason, manager_pid
    ));
}

/// Keep the child Hub alive after the "repeat opening" dialog by pumping
/// Windows messages. The child Hub's window exists (force_main_window_visible
/// suppressed the DestroyWindow call), and the Qt event loop can still process
/// signals from the manager via QtTunnel.
unsafe fn exstar_preserve_child_hub_after_dialog() {
    let start = Instant::now();
    log_exstar_host(format_args!(
        "kind=compat action=preserve_child_hub status=begin strategy=message_pump"
    ));
    // Pump messages indefinitely to keep the child Hub alive and responsive.
    // The Hub window stays visible because force_main_window_visible suppresses
    // destruction. The manager can still communicate via QtTunnel.
    loop {
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
        while windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW(
            &mut msg,
            std::ptr::null_mut() as _,
            0,
            0,
            1, // PM_REMOVE
        ) != 0
        {
            windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
        thread::sleep(Duration::from_millis(50));
        // Log a heartbeat every 60 seconds
        if start.elapsed().as_secs() % 60 == 0
            && start.elapsed().as_millis() % 60000 < 100
        {
            log_exstar_host(format_args!(
                "kind=compat action=preserve_child_hub status=alive elapsed_ms={}",
                start.elapsed().as_millis()
            ));
        }
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaExitProcess(exit_code: u32) -> ! {
    let caller = exstar_capture_caller(2);
    if exstar_trace_logging_enabled() {
        let exe_name = exstar_current_exe("ExitProcess")
            .map(|(_, n)| n)
            .unwrap_or_default();
        log_exstar_host(format_args!(
            "kind=process_exit method=ExitProcess exit_code={} exe={} caller={}",
            exit_code,
            exe_name,
            describe_address(caller)
        ));
        if !EXSTAR_MANAGER_EXIT_PROCESS_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=process_exit_backtrace",
                format_args!(
                    "method=ExitProcess exit_code={} exe={} caller={}",
                    exit_code,
                    exe_name,
                    describe_address(caller)
                ),
                2,
            );
        }
    }
    if exit_code == 0 && exstar_hub_exit_delay_ms() != 0 {
        if let Some((_, current_exe_name)) = exstar_current_exe("ExitProcessDelay") {
            if current_exe_name.eq_ignore_ascii_case("EXStar Hub.exe") {
                let delay_ms = exstar_hub_exit_delay_ms();
                log_exstar_host(format_args!(
                    "kind=compat action=delay_hub_exit exit_code={} delay_ms={}",
                    exit_code, delay_ms
                ));
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
        }
    }
    if exit_code == 0 {
        if let Some((_, current_exe_name)) = exstar_current_exe("ExitProcessPreserveHub") {
            if current_exe_name.eq_ignore_ascii_case("EXStar Hub.exe") {
                // Check if this is the child Hub (launched by manager with @#$ args).
                // The child Hub exits after the "repeat opening" dialog, but we want
                // to keep it alive because its window and Qt infrastructure exist.
                let is_child_hub = env::args().any(|a| a == "@#$");
                if is_child_hub {
                    let startup_compat_active =
                        exstar_hub_startup_compat_active("ExitProcessPreserveChildHub");
                    let real_app_window_shown =
                        EXSTAR_CHILD_HUB_APP_WINDOW_SHOWN.load(Ordering::SeqCst)
                            || exstar_child_hub_real_app_window_exists();
                    if exstar_should_preserve_child_hub_exit(
                        startup_compat_active,
                        real_app_window_shown,
                    ) {
                        exstar_preserve_child_hub_after_dialog();
                    } else {
                        log_exstar_host(format_args!(
                            "kind=compat action=skip_preserve_child_hub_exit startup_compat_active={} real_app_window_shown={}",
                            startup_compat_active,
                            real_app_window_shown
                        ));
                    }
                } else {
                    exstar_preserve_hub_until_related_manager_exit();
                }
            }
        }
    }
    EXIT_PROCESS_FN(exit_code)
}

unsafe fn exstar_process_id_exists(target_pid: u32) -> bool {
    exstar_process_snapshot()
        .map(|processes| processes.iter().any(|(pid, _, _)| *pid == target_pid))
        .unwrap_or(false)
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaExitThread(exit_code: u32) -> ! {
    let current_thread_id = GetCurrentThreadId();
    let tracked_thread_id = EXSTAR_CHILD_HUB_START_THREAD_ID.load(Ordering::SeqCst);
    let is_child_hub_start_thread = tracked_thread_id != 0
        && current_thread_id == tracked_thread_id
        && exstar_current_exe("ExitThread")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false);
    if is_child_hub_start_thread {
        let caller = exstar_capture_caller(2);
        log_exstar_host(format_args!(
            "kind=thread_exit method=ExitThread exit_code={} thread_id={} caller={} is_child_hub=true",
            exit_code,
            current_thread_id,
            describe_address(caller)
        ));
        if !EXSTAR_CHILD_HUB_THREAD_EXIT_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=thread_exit_backtrace",
                format_args!(
                    "method=ExitThread exit_code={} thread_id={} caller={} is_child_hub=true",
                    exit_code,
                    current_thread_id,
                    describe_address(caller)
                ),
                2,
            );
        }
    }
    if is_child_hub_start_thread
        && exit_code == 0
        && !EXSTAR_CHILD_HUB_FORCED_EXEC_ATTEMPTED.swap(true, Ordering::SeqCst)
    {
        log_exstar_host(format_args!(
            "kind=compat action=force_child_qapplication_exec status=begin thread_id={}",
            current_thread_id
        ));
        let exec_result = QT_APPLICATION_EXEC.map(|original| original()).unwrap_or(-1);
        log_exstar_host(format_args!(
            "kind=compat action=force_child_qapplication_exec status=return result={} thread_id={}",
            exec_result,
            current_thread_id
        ));
    }
    EXIT_THREAD_FN(exit_code)
}


#[allow(non_snake_case)]
unsafe extern "system" fn ZludaTerminateProcess(
    process: windows_sys::Win32::Foundation::HANDLE,
    exit_code: u32,
) -> BOOL {
    if exstar_trace_logging_enabled() {
        let suspicious_context = exstar_manager_sn3d_community_kill_context();
        let caller = exstar_capture_caller(2);
        let process_id = if process.is_null() {
            0
        } else {
            GetProcessId(process)
        };
        let object_type = query_object_unicode_string(process, OBJECT_TYPE_INFORMATION_CLASS)
            .ok()
            .flatten()
            .unwrap_or_else(|| "<unknown>".to_string());
        let object_name = query_object_unicode_string(process, OBJECT_NAME_INFORMATION_CLASS)
            .ok()
            .flatten()
            .unwrap_or_else(|| "<unnamed>".to_string());
        let process_path =
            query_process_image_path(process).unwrap_or_else(|| "<unknown>".to_string());
        let target_exit_code = if process.is_null() {
            None
        } else {
            let mut code = 0u32;
            if GetExitCodeProcess(process, &mut code) != 0 {
                Some(code)
            } else {
                None
            }
        };
        log_exstar_host(format_args!(
            "kind=manager_exit method=TerminateProcess process={:p} process_id={} exit_code={} target_exit_code={} object_type={} object_name={} process_path={} caller={} sn3dcommunity_kill_active={} sn3dcommunity_kill_count={} sn3dcommunity_second_sweep={}",
            process,
            process_id,
            exit_code,
            target_exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "<na>".to_string()),
            object_type,
            object_name,
            process_path,
            describe_address(caller),
            suspicious_context.is_some(),
            suspicious_context
                .map(|(kill_count, _)| kill_count.to_string())
                .unwrap_or_else(|| "0".to_string()),
            suspicious_context
                .map(|(_, sweep_id)| sweep_id.to_string())
                .unwrap_or_else(|| "0".to_string())
        ));
        if suspicious_context.is_some()
            && !EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_TERMINATE_BACKTRACE_EMITTED
                .swap(true, Ordering::SeqCst)
        {
            log_exstar_host_backtrace(
                "kind=manager_exit_backtrace",
                format_args!(
                    "method=TerminateProcess label=sn3dcommunity_kill process={:p} process_id={} exit_code={} object_type={} object_name={} process_path={} caller={}",
                    process,
                    process_id,
                    exit_code,
                    object_type,
                    object_name,
                    process_path,
                    describe_address(caller)
                ),
                2,
            );
        }
        if !EXSTAR_MANAGER_TERMINATE_PROCESS_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=manager_exit_backtrace",
                format_args!(
                    "method=TerminateProcess process={:p} process_id={} exit_code={} object_type={} object_name={} process_path={} caller={}",
                    process,
                    process_id,
                    exit_code,
                    object_type,
                    object_name,
                    process_path,
                    describe_address(caller)
                ),
                2,
            );
        }
    }
    // TestOpenglHelper.exe self-terminates with exit_code=1 when the OpenGL check fails.
    // Under ZLUDA the GPU is AMD but usable via HIP translation, so force success.
    // This prevents the manager from aborting the startup sequence.
    let is_self_terminate = process.is_null()
        || GetProcessId(process) == windows_sys::Win32::System::Threading::GetCurrentProcessId();
    if is_self_terminate && exit_code == 1 {
        if let Some((_, exe_name)) = exstar_current_exe("TerminateProcess_opengl_fix") {
            if exe_name.eq_ignore_ascii_case("TestOpenglHelper.exe") {
                log_exstar_host(format_args!(
                    "kind=compat action=opengl_helper_exit_override original_code=1 new_code=0"
                ));
                return TERMINATE_PROCESS_FN(process, 0);
            }
        }
    }
    TERMINATE_PROCESS_FN(process, exit_code)
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaNtTerminateProcess(
    process_handle: HANDLE,
    exit_status: i32,
) -> NTSTATUS {
    let is_self = process_handle == HANDLE(ptr::null_mut()) || process_handle == HANDLE(-1isize as _);
    let target_pid = if is_self {
        GetCurrentProcessId()
    } else {
        GetProcessId(process_handle.0 as _)
    };
    let exe_name = exstar_current_exe("NtTerminateProcess")
        .map(|(_, n)| n)
        .unwrap_or_default();
    let caller = exstar_capture_caller(2);
    log_exstar_host(format_args!(
        "kind=nt_terminate method=NtTerminateProcess is_self={} target_pid={} exit_status={} exe={} caller={}",
        is_self,
        target_pid,
        exit_status,
        exe_name,
        describe_address(caller)
    ));
    if is_self {
        log_exstar_host_backtrace(
            "kind=nt_terminate_backtrace",
            format_args!(
                "is_self={} target_pid={} exit_status={} exe={}",
                is_self, target_pid, exit_status, exe_name
            ),
            2,
        );
    }
    // TestOpenglHelper.exe exit_status=1 means SUCCESS (frameSwapped fired = OpenGL works).
    // The manager checks exitCode==1 to confirm GPU capability. Do NOT override this.
    NT_TERMINATE_PROCESS(process_handle, exit_status)
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLoadLibraryExA(
    file_name: PCSTR,
    hfile: windows_sys::Win32::Foundation::HANDLE,
    dwflags: LOAD_LIBRARY_FLAGS,
) -> HMODULE {
    let result = LOAD_LIBRARY_EX_A(
        DetourPaths::ascii_override(&*&raw const DETOUR_PATHS, file_name),
        hfile,
        dwflags,
    );
    if !result.is_null() && !file_name.is_null() && dwflags & 0x1 == 0 {
        post_load_library_hook_ascii(file_name, result);
    }
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLoadLibraryExW(
    file_name: PCWSTR,
    hfile: windows_sys::Win32::Foundation::HANDLE,
    dwflags: LOAD_LIBRARY_FLAGS,
) -> HMODULE {
    let result = LOAD_LIBRARY_EX_W(
        DetourPaths::utf16_override(&*&raw const DETOUR_PATHS, file_name),
        hfile,
        dwflags,
    );
    // Hook dynamically loaded modules
    if !result.is_null() && !file_name.is_null() && dwflags & 0x1 == 0 {
        // Skip LOAD_LIBRARY_AS_DATAFILE (flag 0x2) etc
        let name = decode_pcwstr(file_name).unwrap_or_default();
        let name_lower = name.to_ascii_lowercase();
        if name_lower.contains("sn3ddeviceeinstar") {
            detour_exstar_device_einstar(result as *mut c_void);
        }
        if name_lower.contains("dxgi") {
            dxgi_try_hook_create_factory(result);
        }
    }
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateMutexA(
    lpmutexattributes: *const SECURITY_ATTRIBUTES,
    binitialowner: BOOL,
    lpname: PCSTR,
) -> windows_sys::Win32::Foundation::HANDLE {
    let result = CREATE_MUTEX_A(lpmutexattributes, binitialowner, lpname);
    let is_child_hub = env::args().any(|a| a == "@#$")
        && exstar_current_exe("CreateMutexA")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false);
    // Capture the "EinScan-Pro.exe" duplicate-instance mutex handle so we can
    // release it in preserve_hub_exit before the child Hub starts.
    if !lpname.is_null() && !result.is_null() {
        let name = std::ffi::CStr::from_ptr(lpname.cast());
        if let Ok(s) = name.to_str() {
            if s == "EinScan-Pro.exe" {
                if is_child_hub && GetLastError() == ERROR_ALREADY_EXISTS {
                    windows_sys::Win32::Foundation::SetLastError(0);
                    log_exstar_host(format_args!(
                        "kind=compat action=clear_duplicate_mutex_last_error method=CreateMutexA name={} is_child_hub=true",
                        s
                    ));
                }
                EXSTAR_DUPLICATE_MUTEX_HANDLE
                    .store(result as usize, std::sync::atomic::Ordering::Relaxed);
                log_exstar_host(format_args!(
                    "kind=compat action=capture_duplicate_mutex handle={:p} name={}",
                    result as *mut c_void, s
                ));
            }
        }
    }
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateMutexW(
    lpmutexattributes: *const SECURITY_ATTRIBUTES,
    binitialowner: BOOL,
    lpname: PCWSTR,
) -> windows_sys::Win32::Foundation::HANDLE {
    let mutex_name = decode_pcwstr(lpname).unwrap_or_else(|| "<null>".to_string());
    let result = CREATE_MUTEX_W(lpmutexattributes, binitialowner, lpname);
    let last_error = GetLastError();
    let is_child_hub = env::args().any(|a| a == "@#$")
        && exstar_current_exe("CreateMutexW")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false);
    if is_child_hub && mutex_name == "EinScan-Pro.exe" && last_error == ERROR_ALREADY_EXISTS {
        windows_sys::Win32::Foundation::SetLastError(0);
        log_exstar_host(format_args!(
            "kind=compat action=clear_duplicate_mutex_last_error method=CreateMutexW name={} is_child_hub=true",
            mutex_name
        ));
    }
    if exstar_manager_process_trace_enabled("create_mutex")
        && !result.is_null()
        && mutex_name != "<null>"
    {
        if let Ok(mut mutexes) = exstar_manager_named_mutexes().lock() {
            mutexes.insert(result as usize, mutex_name.clone());
        }
        log_exstar_host(format_args!(
            "kind=manager_wait method=CreateMutexW handle={:p} initial_owner={} name={} last_error={}",
            result as *mut c_void,
            binitialowner != 0,
            mutex_name,
            last_error
        ));
    }
    result
}

fn decode_unicode_string_ptr(unicode: *const UNICODE_STRING) -> Option<String> {
    let unicode = unsafe { unicode.as_ref() }?;
    if unicode.Length == 0 || unicode.Buffer.is_null() {
        return None;
    }
    let chars = unsafe { slice::from_raw_parts(unicode.Buffer.0, (unicode.Length as usize) / 2) };
    Some(String::from_utf16_lossy(chars))
}

fn query_object_unicode_string(
    handle: windows_sys::Win32::Foundation::HANDLE,
    object_information_class: u32,
) -> Result<Option<String>, NTSTATUS> {
    let mut return_length = 0u32;
    let status = unsafe {
        NtQueryObject(
            HANDLE(handle as *mut c_void),
            object_information_class,
            ptr::null_mut(),
            0,
            &mut return_length,
        )
    };
    if return_length == 0 {
        return if status.is_ok() {
            Ok(None)
        } else {
            Err(status)
        };
    }
    let mut buffer = vec![0u8; return_length as usize];
    let status = unsafe {
        NtQueryObject(
            HANDLE(handle as *mut c_void),
            object_information_class,
            buffer.as_mut_ptr().cast(),
            return_length,
            &mut return_length,
        )
    };
    if !status.is_ok() {
        return Err(status);
    }
    Ok(decode_unicode_string_ptr(buffer.as_ptr().cast()))
}

fn query_process_image_path(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    if handle.is_null() {
        return None;
    }
    let mut size = 260u32;
    let mut buffer = vec![0u16; size as usize];
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    if ok == 0 || size == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

fn summarize_handle_candidate(raw: *mut c_void) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    let handle = raw as windows_sys::Win32::Foundation::HANDLE;
    let object_type = query_object_unicode_string(handle, OBJECT_TYPE_INFORMATION_CLASS)
        .ok()
        .flatten()?;
    let object_name = query_object_unicode_string(handle, OBJECT_NAME_INFORMATION_CLASS)
        .ok()
        .flatten()
        .unwrap_or_else(|| "<unnamed>".to_string());
    let process_id = if object_type == "Process" {
        unsafe { GetProcessId(handle) }
    } else {
        0
    };
    let process_path = if object_type == "Process" {
        query_process_image_path(handle).unwrap_or_else(|| "<unknown>".to_string())
    } else {
        "<na>".to_string()
    };
    let target_exit_code = if object_type == "Process" {
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(handle, &mut code) } != 0 {
            code.to_string()
        } else {
            "<na>".to_string()
        }
    } else {
        "<na>".to_string()
    };
    Some(format!(
        "type={} name={} process_id={} process_path={} exit_code={}",
        object_type, object_name, process_id, process_path, target_exit_code
    ))
}

unsafe fn log_manager_handle_flow_probe(
    label: &str,
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) {
    log_offset_probe("manager_exe", label, this, arg1, arg2, arg3, arg4, arg5);
    let args = [
        ("arg1", arg1),
        ("arg2", arg2),
        ("arg3", arg3),
        ("arg4", arg4),
        ("arg5", arg5),
    ];
    for (name, value) in args {
        if let Some(summary) = summarize_handle_candidate(value) {
            log_exstar_host(format_args!(
                "kind=manager_exe_handle_flow method={} slot={} value={:p} summary={}",
                label, name, value, summary
            ));
        }
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaWaitForSingleObject(
    handle: windows_sys::Win32::Foundation::HANDLE,
    milliseconds: u32,
) -> u32 {
    let trace_manager = exstar_manager_process_trace_enabled("wait_for_single_object");
    let suspicious_context = if trace_manager {
        exstar_manager_sn3d_community_kill_context()
    } else {
        None
    };
    let tracked_mutex_name = if trace_manager {
        exstar_manager_named_mutexes()
            .lock()
            .ok()
            .and_then(|mutexes| mutexes.get(&(handle as usize)).cloned())
    } else {
        None
    };
    let result = WAIT_FOR_SINGLE_OBJECT(handle, milliseconds);
    if trace_manager {
        let caller = exstar_capture_caller(2);
        let count = EXSTAR_MANAGER_WAIT_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
        if tracked_mutex_name.is_some() || count <= 32 || suspicious_context.is_some() {
            let mutex_name = tracked_mutex_name.unwrap_or_else(|| "<untracked>".to_string());
            let handle_desc = describe_optional_address((handle as usize as *const c_void).cast());
            let process_id = GetProcessId(handle);
            let thread_id = GetThreadId(handle);
            let object_type = query_object_unicode_string(handle, OBJECT_TYPE_INFORMATION_CLASS)
                .ok()
                .flatten()
                .unwrap_or_else(|| "<unknown>".to_string());
            let object_name = query_object_unicode_string(handle, OBJECT_NAME_INFORMATION_CLASS)
                .ok()
                .flatten()
                .unwrap_or_else(|| "<unnamed>".to_string());
            let mut exit_code = 0u32;
            let has_exit_code = if process_id != 0 {
                GetExitCodeProcess(handle, &mut exit_code) != 0
            } else {
                false
            };
            let process_path = if object_type == "Process" {
                query_process_image_path(handle)
            } else {
                None
            };
            if process_path
                .as_deref()
                .is_some_and(|path| path.ends_with("TestOpenglHelper.exe"))
                && has_exit_code
                && exit_code == 1
            {
                EXSTAR_MANAGER_POST_HELPER_PHASE_ACTIVE.store(true, Ordering::SeqCst);
                if !EXSTAR_MANAGER_POST_HELPER_EXIT_EMITTED.swap(true, Ordering::SeqCst) {
                    log_exstar_host(format_args!(
                        "kind=manager_helper_exit phase=post_helper active=true handle={:p}[{}] milliseconds={} result={} process_id={} process_path={}",
                        handle as *mut c_void,
                        handle_desc,
                        milliseconds,
                        result,
                        process_id,
                        process_path.as_deref().unwrap_or("<unknown>")
                    ));
                }
            }
            log_exstar_host(format_args!(
                "kind=manager_wait method=WaitForSingleObject count={} handle={:p}[{}] milliseconds={} result={} mutex_name={} process_id={} thread_id={} exit_code={} object_type={} object_name={} caller={} sn3dcommunity_kill_active={} sn3dcommunity_kill_count={} sn3dcommunity_second_sweep={}",
                count,
                handle as *mut c_void,
                handle_desc,
                milliseconds,
                result,
                mutex_name,
                process_id,
                thread_id,
                if has_exit_code {
                    exit_code.to_string()
                } else {
                    "<na>".to_string()
                },
                object_type,
                object_name,
                describe_address(caller),
                suspicious_context.is_some(),
                suspicious_context
                    .map(|(kill_count, _)| kill_count.to_string())
                    .unwrap_or_else(|| "0".to_string()),
                suspicious_context
                    .map(|(_, sweep_id)| sweep_id.to_string())
                    .unwrap_or_else(|| "0".to_string())
            ));
            if suspicious_context.is_some()
                && (!EXSTAR_MANAGER_SN3D_COMMUNITY_KILL_WAIT_BACKTRACE_EMITTED
                    .swap(true, Ordering::SeqCst)
                    || result == u32::MAX
                    || result == WAIT_TIMEOUT)
            {
                log_exstar_host_backtrace(
                    "kind=manager_wait_backtrace",
                    format_args!(
                        "method=WaitForSingleObject label=sn3dcommunity_kill handle={:p}[{}] milliseconds={} result={} process_id={} object_type={} object_name={}",
                        handle as *mut c_void,
                        handle_desc,
                        milliseconds,
                        result,
                        process_id,
                        object_type,
                        object_name
                    ),
                    2,
                );
            }
            if object_type == "Mutant"
                && object_name == "<unnamed>"
                && !EXSTAR_MANAGER_WAIT_MUTANT_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
            {
                log_exstar_host_backtrace(
                    "kind=manager_wait_backtrace",
                    format_args!(
                        "method=WaitForSingleObject label=unnamed_mutant handle={:p}[{}] milliseconds={} result={}",
                        handle as *mut c_void,
                        handle_desc,
                        milliseconds,
                        result
                    ),
                    2,
                );
            }
            if object_type == "Process"
                && result == u32::MAX
                && !EXSTAR_MANAGER_WAIT_FAILED_PROCESS_BACKTRACE_EMITTED
                    .swap(true, Ordering::SeqCst)
            {
                log_exstar_host_backtrace(
                    "kind=manager_wait_backtrace",
                    format_args!(
                        "method=WaitForSingleObject label=failed_process_wait handle={:p}[{}] milliseconds={} process_id={} exit_code={}",
                        handle as *mut c_void,
                        handle_desc,
                        milliseconds,
                        process_id,
                        if has_exit_code {
                            exit_code.to_string()
                        } else {
                            "<na>".to_string()
                        }
                    ),
                    2,
                );
            }
        }
    }
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaShowWindow(hwnd: HWND, n_cmd_show: i32) -> BOOL {
    let result = SHOW_WINDOW(hwnd, n_cmd_show);
    log_window_transition(
        "ShowWindow",
        hwnd,
        format_args!("nCmdShow={} applied={}", n_cmd_show, n_cmd_show),
        result,
    );
    maybe_log_window_hide_backtrace(hwnd, n_cmd_show);
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaSetWindowPos(
    hwnd: HWND,
    hwnd_insert_after: HWND,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    u_flags: u32,
) -> BOOL {
    let result = SET_WINDOW_POS(hwnd, hwnd_insert_after, x, y, cx, cy, u_flags);
    log_window_transition(
        "SetWindowPos",
        hwnd,
        format_args!(
            "insert_after={:p} x={} y={} cx={} cy={} flags=0x{:x} applied_flags=0x{:x}",
            hwnd_insert_after as *mut c_void, x, y, cx, cy, u_flags, u_flags
        ),
        result,
    );
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaDestroyWindow(hwnd: HWND) -> BOOL {
    let result = DESTROY_WINDOW(hwnd);
    log_window_transition(
        "DestroyWindow",
        hwnd,
        format_args!("requested=true"),
        result,
    );
    result
}

unsafe extern "system" fn zluda_qwidget_hide(this: *mut c_void) {
    log_exstar_host(format_args!("kind=qtwidget method=hide this={:p}", this));
    maybe_log_qwidget_hide_backtrace("hide", this);
    if exstar_hub_main_window_compat_active("QWidget::hide") {
        log_exstar_host(format_args!(
            "kind=compat action=force_main_window_visible trigger=QWidget::hide suppressed=true this={:p} on_main_window_thread=true",
            this,
        ));
        return;
    }
    if let Some(original) = QT_WIDGET_HIDE {
        original(this);
    }
}

unsafe extern "system" fn zluda_qwidget_show(this: *mut c_void) {
    log_exstar_host(format_args!("kind=qtwidget method=show this={:p}", this));
    if let Some(original) = QT_WIDGET_SHOW {
        original(this);
    }
}

unsafe extern "system" fn zluda_qwidget_close(this: *mut c_void) -> u8 {
    log_exstar_host(format_args!("kind=qtwidget method=close this={:p}", this));
    maybe_log_qwidget_hide_backtrace("close", this);
    if exstar_hub_main_window_compat_active("QWidget::close") {
        log_exstar_host(format_args!(
            "kind=compat action=force_main_window_visible trigger=QWidget::close suppressed=true this={:p} on_main_window_thread=true",
            this,
        ));
        return 1;
    }
    QT_WIDGET_CLOSE.map(|original| original(this)).unwrap_or(0)
}

unsafe extern "system" fn zluda_qwidget_set_visible(this: *mut c_void, visible: u8) {
    log_exstar_host(format_args!(
        "kind=qtwidget method=setVisible this={:p} visible={}",
        this,
        visible != 0
    ));
    if visible == 0 {
        maybe_log_qwidget_hide_backtrace("setVisible(false)", this);
        if exstar_hub_main_window_compat_active("QWidget::setVisible") {
            log_exstar_host(format_args!(
                "kind=compat action=force_main_window_visible trigger=QWidget::setVisible suppressed=true this={:p} visible={} on_main_window_thread=true",
                this,
                visible != 0
            ));
            return;
        }
    }
    if let Some(original) = QT_WIDGET_SET_VISIBLE {
        original(this, visible);
    }
}

unsafe extern "system" fn zluda_qwidget_event(this: *mut c_void, event: *mut c_void) -> u8 {
    let event_type = qt_event_type(event).unwrap_or(-1);
    if is_interesting_qt_event_type(event_type) {
        log_exstar_host(format_args!(
            "kind=qtwidget method=event this={:p} event={:p} type={}",
            this, event, event_type
        ));
        if event_type == 18 || event_type == 105 {
            maybe_log_qwidget_hide_backtrace("event", this);
        }
    }
    QT_WIDGET_EVENT
        .map(|original| original(this, event))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_qwindow_show(this: *mut c_void) {
    log_exstar_host(format_args!("kind=qwindow method=show this={:p}", this));
    if let Some(original) = QT_WINDOW_SHOW {
        original(this);
    }
}

unsafe extern "system" fn zluda_qwindow_close(this: *mut c_void) -> u8 {
    log_exstar_host(format_args!("kind=qwindow method=close this={:p}", this));
    maybe_log_qwindow_hide_backtrace("close", this);
    // Don't suppress QWindow::close — the splash screen close is NORMAL behavior.
    // After Sn3DBox.dll loads and inits, the Hub unconditionally closes the splash
    // window and should then show the main application window.
    QT_WINDOW_CLOSE.map(|original| original(this)).unwrap_or(0)
}

unsafe extern "system" fn zluda_qwindow_hide(this: *mut c_void) {
    log_exstar_host(format_args!("kind=qwindow method=hide this={:p}", this));
    maybe_log_qwindow_hide_backtrace("hide", this);
    if exstar_hub_main_window_compat_active("QWindow::hide") {
        log_exstar_host(format_args!(
            "kind=compat action=force_main_window_visible trigger=QWindow::hide suppressed=true this={:p} on_main_window_thread=true",
            this,
        ));
        return;
    }
    if let Some(original) = QT_WINDOW_HIDE {
        original(this);
    }
}

unsafe extern "system" fn zluda_qwindow_set_visible(this: *mut c_void, visible: u8) {
    log_exstar_host(format_args!(
        "kind=qwindow method=setVisible this={:p} visible={}",
        this,
        visible != 0
    ));
    if visible == 0 {
        maybe_log_qwindow_hide_backtrace("setVisible(false)", this);
        if exstar_hub_main_window_compat_active("QWindow::setVisible") {
            log_exstar_host(format_args!(
                "kind=compat action=force_main_window_visible trigger=QWindow::setVisible suppressed=true this={:p} visible={} on_main_window_thread=true",
                this,
                visible != 0
            ));
            return;
        }
    }
    if let Some(original) = QT_WINDOW_SET_VISIBLE {
        original(this, visible);
    }
}

unsafe extern "system" fn zluda_qwindow_event(this: *mut c_void, event: *mut c_void) -> u8 {
    let event_type = qt_event_type(event).unwrap_or(-1);
    if is_interesting_qt_event_type(event_type) {
        log_exstar_host(format_args!(
            "kind=qwindow method=event this={:p} event={:p} type={}",
            this, event, event_type
        ));
        if event_type == 19 {
            maybe_log_qwindow_event19_backtrace(this);
        }
        if event_type == 18 || event_type == 105 {
            maybe_log_qwindow_hide_backtrace("event", this);
        }
    }
    QT_WINDOW_EVENT
        .map(|original| original(this, event))
        .unwrap_or(0)
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateProcessA(
    lpapplicationname: PCSTR,
    lpcommandline: PSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOA,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL {
    let launch = HostLaunchInfo {
        api_name: "CreateProcessA",
        application_name: decode_pcstr(lpapplicationname),
        command_line: decode_pcstr(lpcommandline.cast_const()),
        current_directory: decode_pcstr(lpcurrentdirectory),
    };
    create_process(
        &launch,
        dwcreationflags,
        lpprocessinformation,
        |creation_flags, proc_info| {
            CREATE_PROCESS_A(
                lpapplicationname,
                lpcommandline,
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                creation_flags,
                lpenvironment,
                lpcurrentdirectory,
                lpstartupinfo,
                proc_info,
            )
        },
    )
}

unsafe extern "system" fn zluda_nav_click_login(this: *mut c_void) {
    log_exstar_host(format_args!("kind=nav method=clickLogin this={:p}", this));
    if let Some(original) = NAV_CLICK_LOGIN {
        original(this);
    }
}

unsafe extern "system" fn zluda_nav_login(this: *mut c_void) {
    log_exstar_host(format_args!("kind=nav method=login this={:p}", this));
    if let Some(original) = NAV_LOGIN {
        original(this);
    }
}

unsafe extern "system" fn zluda_nav_device_offline(this: *mut c_void, offline: u8) {
    log_exstar_host(format_args!(
        "kind=nav method=deviceOffline this={:p} offline={}",
        this,
        offline != 0
    ));
    if let Some(original) = NAV_DEVICE_OFFLINE {
        original(this, offline);
    }
}

unsafe extern "system" fn zluda_nav_show_author_prompt(this: *mut c_void, show: u8) {
    log_exstar_host(format_args!(
        "kind=nav method=showAuthorPrompt this={:p} show={}",
        this,
        show != 0
    ));
    if let Some(original) = NAV_SHOW_AUTHOR_PROMPT {
        original(this, show);
    }
}

unsafe extern "system" fn zluda_nav_device_info(this: *mut c_void, map: *const c_void) {
    log_exstar_host(format_args!(
        "kind=nav method=deviceInfo this={:p} map={:p}",
        this, map
    ));
    if let Some(original) = NAV_DEVICE_INFO {
        original(this, map);
    }
}

unsafe extern "system" fn zluda_nav_log_in_user_info(this: *mut c_void, map: *const c_void) {
    log_exstar_host(format_args!(
        "kind=nav method=logInUserInfo this={:p} map={:p}",
        this, map
    ));
    if let Some(original) = NAV_LOGIN_USER_INFO {
        original(this, map);
    }
}

unsafe extern "system" fn zluda_nav_qt_metacall(
    this: *mut c_void,
    call: i32,
    id: i32,
    args: *mut *mut c_void,
) -> i32 {
    log_exstar_host(format_args!(
        "kind=nav method=qt_metacall this={:p} call={} id={} args={:p}",
        this, call, id, args
    ));
    NAV_QT_METACALL
        .map(|original| original(this, call, id, args))
        .unwrap_or(id)
}

unsafe extern "system" fn zluda_nav_qt_static_metacall(
    object: *mut c_void,
    call: i32,
    id: i32,
    args: *mut *mut c_void,
) {
    log_exstar_host(format_args!(
        "kind=nav method=qt_static_metacall object={:p} call={} id={} args={:p}",
        object, call, id, args
    ));
    if let Some(original) = NAV_QT_STATIC_METACALL {
        original(object, call, id, args);
    }
}

unsafe extern "system" fn zluda_qmetaobject_connection_dtor(this: *mut c_void) {
    let mut frames = [ptr::null_mut::<c_void>(); 12];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    for frame in frames[..frame_count].iter() {
        let frame = (*frame).cast_const();
        if let Some((module, base)) = module_info_from_address(frame) {
            let offset = (frame as usize).saturating_sub(base);
            if module.eq_ignore_ascii_case("Sn3DProcessPlugin.dll")
                && (0x72C0..0x7990).contains(&offset)
            {
                log_exstar_host(format_args!(
                    "kind=qtcore method=connection_dtor this={:p} caller=Sn3DProcessPlugin.dll+0x{:x}",
                    this,
                    offset
                ));
                break;
            }
        }
    }
    if let Some(original) = QT_CONNECTION_DTOR {
        original(this);
    }
}

unsafe extern "system" fn zluda_qthread_msleep(msecs: u32) {
    let mut matched_offset = None;
    let mut frames = [ptr::null_mut::<c_void>(); 12];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    for frame in frames[..frame_count].iter() {
        let frame = (*frame).cast_const();
        if let Some((module, base)) = module_info_from_address(frame) {
            let offset = (frame as usize).saturating_sub(base);
            if module.eq_ignore_ascii_case("Sn3DProcessPlugin.dll")
                && (0x72C0..=0x7A00).contains(&offset)
            {
                matched_offset = Some(offset);
                break;
            }
        }
    }
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=qtcore method=QThread::msleep msecs={} caller=Sn3DProcessPlugin.dll+0x{:x}",
            msecs, offset
        ));
    }
    if let Some(original) = QT_THREAD_MSLEEP {
        original(msecs);
    }
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=qtcore method=QThread::msleep_return msecs={} caller=Sn3DProcessPlugin.dll+0x{:x}",
            msecs, offset
        ));
    }
}

unsafe extern "system" fn zluda_qtimer_singleshot_impl(
    msec: i32,
    timer_type: i32,
    receiver: *const c_void,
    slot_object: *mut c_void,
) {
    let callback = if slot_object.is_null() {
        ptr::null()
    } else {
        *(slot_object.cast::<usize>().add(1)) as *const c_void
    };
    let context = if slot_object.is_null() {
        ptr::null_mut()
    } else {
        *(slot_object.cast::<usize>().add(2)) as *mut c_void
    };
    let is_child_hub = env::args().any(|a| a == "@#$")
        && exstar_current_exe("QTimer::singleShotImpl")
            .map(|(_, name)| name.eq_ignore_ascii_case("EXStar Hub.exe"))
            .unwrap_or(false);
    if is_child_hub {
        log_exstar_host(format_args!(
            "kind=qtcore method=QTimer::singleShotImpl msec={} timer_type={} receiver={:p} slot_object={:p} callback={} context={:p}",
            msec,
            timer_type,
            receiver,
            slot_object,
            describe_address(callback),
            context
        ));
        if exstar_should_suppress_prestartcheck_timer(callback) {
            log_exstar_host(format_args!(
                "kind=compat action=suppress_prestartcheck_timer callback={} msec={} receiver={:p} slot_object={:p}",
                describe_address(callback),
                msec,
                receiver,
                slot_object
            ));
            return;
        }
    }
    if let Some(original) = QT_TIMER_SINGLESHOT_IMPL {
        original(msec, timer_type, receiver, slot_object);
    }
}

fn decode_cstr_ptr(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn describe_qt_generic_argument(arg: QtGenericArgument) -> String {
    let name = decode_cstr_ptr(arg.name).unwrap_or_else(|| "<null>".to_string());
    format!("{}@{:p}", name, arg.data)
}

unsafe extern "system" fn zluda_qmetaobject_invoke_method_with_type(
    object: *mut c_void,
    member: *const c_char,
    connection_type: i32,
    arg0: QtGenericArgument,
    arg1: QtGenericArgument,
    arg2: QtGenericArgument,
    arg3: QtGenericArgument,
    arg4: QtGenericArgument,
    arg5: QtGenericArgument,
    arg6: QtGenericArgument,
    arg7: QtGenericArgument,
    arg8: QtGenericArgument,
    arg9: QtGenericArgument,
) -> u8 {
    let mut matched_offset = None;
    let mut frames = [ptr::null_mut::<c_void>(); 12];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    for frame in frames[..frame_count].iter() {
        let frame = (*frame).cast_const();
        if let Some((module, base)) = module_info_from_address(frame) {
            let offset = (frame as usize).saturating_sub(base);
            if module.eq_ignore_ascii_case("qttunnel.3.2.7.dll")
                && (0x2260..=0x24eb).contains(&offset)
            {
                matched_offset = Some(offset);
                break;
            }
        }
    }
    let member_text = decode_cstr_ptr(member).unwrap_or_else(|| "<null>".to_string());
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=qtcore method=invokeMethod_with_type object={:p} member={} connection_type={} arg0={} arg1={} arg2={} arg3={} arg4={} caller=qttunnel.3.2.7.dll+0x{:x}",
            object,
            member_text,
            connection_type,
            describe_qt_generic_argument(arg0),
            describe_qt_generic_argument(arg1),
            describe_qt_generic_argument(arg2),
            describe_qt_generic_argument(arg3),
            describe_qt_generic_argument(arg4),
            offset
        ));
    }
    let result = QT_METAOBJECT_INVOKE_METHOD_WITH_TYPE
        .map(|original| {
            original(
                object,
                member,
                connection_type,
                arg0,
                arg1,
                arg2,
                arg3,
                arg4,
                arg5,
                arg6,
                arg7,
                arg8,
                arg9,
            )
        })
        .unwrap_or(0);
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=qtcore method=invokeMethod_with_type_result object={:p} member={} result={} caller=qttunnel.3.2.7.dll+0x{:x}",
            object,
            member_text,
            result != 0,
            offset
        ));
    }
    result
}

unsafe extern "system" fn zluda_qcoreapplication_quit() {
    let should_trace = exstar_hub_process_trace_enabled("QCoreApplication::quit");
    let suppress = exstar_hub_main_window_compat_active("QCoreApplication::quit");
    if should_trace {
        log_exstar_host(format_args!(
            "kind=qtcore method=QCoreApplication::quit suppress={} on_main_window_thread={}",
            suppress,
            exstar_on_main_window_thread()
        ));
    }
    if suppress {
        if should_trace {
            log_exstar_host(format_args!(
                "kind=compat action=force_main_window_visible trigger=QCoreApplication::quit suppressed=true on_main_window_thread=true"
            ));
        }
        return;
    }
    if let Some(original) = QT_CORE_APPLICATION_QUIT {
        original();
    }
}

unsafe extern "system" fn zluda_qcoreapplication_exit(exit_code: i32) {
    let should_trace = exstar_hub_process_trace_enabled("QCoreApplication::exit");
    let suppress = exstar_hub_main_window_compat_active("QCoreApplication::exit") && exit_code == 0;
    if should_trace {
        log_exstar_host(format_args!(
            "kind=qtcore method=QCoreApplication::exit exit_code={} suppress={} on_main_window_thread={}",
            exit_code,
            suppress,
            exstar_on_main_window_thread()
        ));
    }
    if suppress {
        if should_trace {
            log_exstar_host(format_args!(
                "kind=compat action=force_main_window_visible trigger=QCoreApplication::exit suppressed=true exit_code={} on_main_window_thread=true",
                exit_code,
            ));
        }
        return;
    }
    if let Some(original) = QT_CORE_APPLICATION_EXIT {
        original(exit_code);
    }
}

unsafe extern "system" fn zluda_qapplication_exec() -> i32 {
    log_exstar_host(format_args!("kind=qtwidget method=QApplication::exec"));
    let result = QT_APPLICATION_EXEC.map(|original| original()).unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=qtwidget method=QApplication::exec_return result={}",
        result
    ));
    result
}

unsafe extern "system" fn zluda_qmessagebox_warning(
    parent: *mut c_void,
    title: *const c_void,
    text: *const c_void,
    buttons: u32,
    default_button: u32,
) -> u32 {
    let title_text = decode_qstring_ref(title).unwrap_or_default();
    let text_text = decode_qstring_ref(text).unwrap_or_default();
    // Suppress EXStar error/warning dialogs during ZLUDA operation.
    let is_exstar_warning = title_text.contains("Warning")
        || title_text.contains("warning")
        || text_text.contains("error code")
        || text_text.contains("repeat opening")
        || text_text.contains("something went wrong");
    if is_exstar_warning {
        log_exstar_host(format_args!(
            "kind=compat action=suppress_warning_dialog title=\"{}\" text=\"{}\"",
            title_text, text_text
        ));
        return 0x00000400; // QMessageBox::Ok
    }
    log_exstar_host(format_args!(
        "kind=qtwidget method=QMessageBox::warning title=\"{}\" text=\"{}\"",
        title_text, text_text
    ));
    QT_MESSAGEBOX_WARNING
        .map(|original| original(parent, title, text, buttons, default_button))
        .unwrap_or(0x00000400)
}

unsafe extern "system" fn zluda_qdialog_exec(this: *mut c_void) -> i32 {
    // PrestartCheck.dll is binary-patched to skip its GPU check, so we don't
    // need to suppress QDialog::exec anymore. Just pass through to the original.
    // Suppressing ALL QDialog::exec caused stack corruption (0xc0000409 crashes)
    // because other code paths (file dialogs, settings) depend on exec() working.
    if exstar_trace_logging_enabled() {
        log_exstar_host(format_args!(
            "kind=qtwidget method=QDialog::exec this={:p}",
            this
        ));
    }
    QT_DIALOG_EXEC
        .map(|original| original(this))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_qmessagebox_critical(
    parent: *mut c_void,
    title: *const c_void,
    text: *const c_void,
    buttons: u32,
    default_button: u32,
) -> u32 {
    let title_text = decode_qstring_ref(title).unwrap_or_default();
    let text_text = decode_qstring_ref(text).unwrap_or_default();
    let is_exstar_error = title_text.contains("Warning")
        || title_text.contains("warning")
        || text_text.contains("error code")
        || text_text.contains("repeat opening")
        || text_text.contains("something went wrong");
    if is_exstar_error {
        log_exstar_host(format_args!(
            "kind=compat action=suppress_critical_dialog title=\"{}\" text=\"{}\"",
            title_text, text_text
        ));
        return 0x00000400;
    }
    log_exstar_host(format_args!(
        "kind=qtwidget method=QMessageBox::critical title=\"{}\" text=\"{}\"",
        title_text, text_text
    ));
    QT_MESSAGEBOX_CRITICAL
        .map(|original| original(parent, title, text, buttons, default_button))
        .unwrap_or(0x00000400)
}

unsafe extern "system" fn zluda_qmessagebox_information(
    parent: *mut c_void,
    title: *const c_void,
    text: *const c_void,
    buttons: u32,
    default_button: u32,
) -> u32 {
    let title_text = decode_qstring_ref(title).unwrap_or_default();
    let text_text = decode_qstring_ref(text).unwrap_or_default();
    let is_exstar_error = title_text.contains("Warning")
        || title_text.contains("warning")
        || text_text.contains("error code")
        || text_text.contains("repeat opening")
        || text_text.contains("something went wrong");
    if is_exstar_error {
        log_exstar_host(format_args!(
            "kind=compat action=suppress_info_dialog title=\"{}\" text=\"{}\"",
            title_text, text_text
        ));
        return 0x00000400;
    }
    log_exstar_host(format_args!(
        "kind=qtwidget method=QMessageBox::information title=\"{}\" text=\"{}\"",
        title_text, text_text
    ));
    QT_MESSAGEBOX_INFORMATION
        .map(|original| original(parent, title, text, buttons, default_button))
        .unwrap_or(0x00000400)
}

unsafe extern "system" fn zluda_qhostaddress_dtor(this: *mut c_void) {
    let mut frames = [ptr::null_mut::<c_void>(); 12];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    for frame in frames[..frame_count].iter() {
        let frame = (*frame).cast_const();
        if let Some((module, base)) = module_info_from_address(frame) {
            let offset = (frame as usize).saturating_sub(base);
            if module.eq_ignore_ascii_case("Sn3DProcessPlugin.dll")
                && (0x72C0..0x7990).contains(&offset)
            {
                log_exstar_host(format_args!(
                    "kind=qtnetwork method=qhostaddress_dtor this={:p} caller=Sn3DProcessPlugin.dll+0x{:x}",
                    this,
                    offset
                ));
                break;
            }
        }
    }
    if let Some(original) = QT_HOST_ADDRESS_DTOR {
        original(this);
    }
}

unsafe extern "system" fn zluda_qhostaddress_ctor(
    this: *mut c_void,
    special_address: i32,
) -> *mut c_void {
    let mut matched_offset = None;
    let mut frames = [ptr::null_mut::<c_void>(); 12];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    for frame in frames[..frame_count].iter() {
        let frame = (*frame).cast_const();
        if let Some((module, base)) = module_info_from_address(frame) {
            let offset = (frame as usize).saturating_sub(base);
            if module.eq_ignore_ascii_case("Sn3DProcessPlugin.dll")
                && (0x72C0..0x7990).contains(&offset)
            {
                matched_offset = Some(offset);
                break;
            }
        }
    }
    let result = QT_HOST_ADDRESS_CTOR
        .map(|original| original(this, special_address))
        .unwrap_or(this);
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=qtnetwork method=qhostaddress_ctor this={:p} special_address={} result={:p} caller=Sn3DProcessPlugin.dll+0x{:x}",
            this,
            special_address,
            result,
            offset
        ));
    }
    result
}

unsafe extern "system" fn zluda_sn3dbox_plugin_instance() -> *mut c_void {
    let result = SN3DBOX_PLUGIN_INSTANCE
        .map(|original| original())
        .unwrap_or(ptr::null_mut());
    log_exstar_host(format_args!(
        "kind=sn3dbox method=qt_plugin_instance result={:p} result_desc={}",
        result,
        describe_optional_address(result.cast_const())
    ));
    result
}

unsafe extern "system" fn zluda_sn3dbox_application_init(
    this: *mut c_void,
    parent: *mut c_void,
) {
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DApplication::init this={:p} parent={:p}[{}]",
        this,
        parent,
        describe_optional_address(parent.cast_const())
    ));
    if let Some(original) = SN3DBOX_APP_INIT {
        original(this, parent);
    }
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DApplication::init_return this={:p}",
        this
    ));
}

unsafe extern "system" fn zluda_sn3dbox_application_load(
    this: *mut c_void,
    path: *const c_void,
    parent: *mut c_void,
) {
    let path_text = decode_qstring_ref(path).unwrap_or_else(|| "<unknown>".to_string());
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DApplication::load this={:p} path={:p} path_text=\"{}\" parent={:p}[{}]",
        this,
        path,
        path_text,
        parent,
        describe_optional_address(parent.cast_const())
    ));
    if let Some(original) = SN3DBOX_APP_LOAD {
        original(this, path, parent);
    }
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DApplication::load_return this={:p} path_text=\"{}\"",
        this,
        path_text
    ));
}

unsafe extern "system" fn zluda_sn3dbox_ui_qml_item(this: *mut c_void) -> *mut c_void {
    let result = SN3DBOX_UI_QML_ITEM
        .map(|original| original(this))
        .unwrap_or(ptr::null_mut());
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::qmlItem this={:p} result={:p}[{}]",
        this,
        result,
        describe_optional_address(result.cast_const())
    ));
    result
}

unsafe extern "system" fn zluda_sn3dbox_ui_set_qml_item(this: *mut c_void, item: *mut c_void) {
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::setQmlItem this={:p} item={:p}[{}]",
        this,
        item,
        describe_optional_address(item.cast_const())
    ));
    if let Some(original) = SN3DBOX_UI_SET_QML_ITEM {
        original(this, item);
    }
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::setQmlItem_return this={:p} item={:p}",
        this,
        item
    ));
}

unsafe extern "system" fn zluda_sn3dbox_ui_start(this: *mut c_void) -> i32 {
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::start this={:p}",
        this
    ));
    let result = SN3DBOX_UI_START.map(|original| original(this)).unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::start_return this={:p} result={}",
        this,
        result
    ));
    result
}

unsafe extern "system" fn zluda_sn3dbox_ui_stop(this: *mut c_void) -> i32 {
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::stop this={:p}",
        this
    ));
    let result = SN3DBOX_UI_STOP.map(|original| original(this)).unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=sn3dbox method=Sn3DUICpp::stop_return this={:p} result={}",
        this,
        result
    ));
    result
}

unsafe extern "system" fn zluda_process_mg_publish(
    this: *mut c_void,
    topic: *const c_void,
    payload: *const c_void,
    retained: u8,
) -> u8 {
    let topic_text = decode_qstring_ref(topic).unwrap_or_else(|| "<unknown>".to_string());
    let payload_info = payload_identity(payload);
    let payload_strings = payload_strings(payload);
    if topic_text == "Sn3dProcessMgTopic" {
        maybe_launch_einscan_net_svr_publish("processmg.publish", &payload_strings);
    }
    if exstar_is_ui_state_topic(&topic_text) {
        log_exstar_host(format_args!(
            "kind=ui_state method=processmg.publish this={:p} topic={} payload_info=\"{}\" payload_strings=\"{}\" retained={}",
            this,
            topic_text,
            payload_info,
            payload_strings,
            retained != 0
        ));
        if !EXSTAR_UI_TOPIC_PROCESSMG_PUBLISH_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=ui_state_backtrace",
                format_args!(
                    "method=processmg.publish this={:p} topic={} payload_info=\"{}\" payload_strings=\"{}\" retained={}",
                    this,
                    topic_text,
                    payload_info,
                    payload_strings,
                    retained != 0
                ),
                2,
            );
        }
    }
    log_exstar_host(format_args!(
        "kind=processmg method=publish this={:p} topic={:p} topic_text={} payload={:p} payload_info=\"{}\" payload_strings=\"{}\" retained={}",
        this,
        topic,
        topic_text,
        payload,
        payload_info,
        payload_strings,
        retained != 0
    ));
    PROCESS_MG_PUBLISH
        .map(|original| original(this, topic, payload, retained))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_mg_connect_and_register(
    arg1: *mut c_void,
    arg2: *mut c_void,
) -> u8 {
    exstar_manager_clear_second_sweep("connectAndRegister");
    // Log entry first — BEFORE any payload inspection that might crash.
    // The payload_primary_text function dereferences the args as QStringList,
    // which can crash if the layout doesn't match expectations (e.g. in scanservice).
    log_exstar_host(format_args!(
        "kind=processmg method=connectAndRegister arg1={:p} arg2={:p}",
        arg1, arg2
    ));
    // Only inspect payloads if we're in the manager (where it's known to work).
    // For other processes, skip diagnostics to avoid crashes.
    let is_manager = exstar_current_exe("connectAndRegister_diag")
        .map(|(_, n)| n.eq_ignore_ascii_case("Sn3DprocessManager.exe"))
        .unwrap_or(false);
    if is_manager {
        let arg1_text =
            payload_primary_text(arg1.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
        let arg1_strings = payload_strings(arg1.cast_const());
        let arg2_text =
            payload_primary_text(arg2.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
        let arg2_strings = payload_strings(arg2.cast_const());
        if let Some(branch) = exstar_processmg_branch_label(&arg1_strings)
            .or_else(|| exstar_processmg_branch_label(&arg2_strings))
        {
            log_exstar_host(format_args!(
                "kind=processmg_branch method=connectAndRegister branch={} arg1_strings=\"{}\" arg2_strings=\"{}\"",
                branch, arg1_strings, arg2_strings
            ));
        }
        log_exstar_host(format_args!(
            "kind=processmg method=connectAndRegister_detail arg1={:p} arg1_text={} arg1_strings=\"{}\" arg2={:p} arg2_text={} arg2_strings=\"{}\"",
            arg1,
            arg1_text,
            arg1_strings,
            arg2,
            arg2_text,
            arg2_strings
        ));
    }
    let result = PROCESS_MG_CONNECT_AND_REGISTER
        .map(|original| original(arg1, arg2))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=processmg method=connectAndRegister_result success={} arg1={:p} arg2={:p}",
        result != 0,
        arg1,
        arg2
    ));
    result
}

unsafe extern "system" fn zluda_process_mg_register_subscribe(
    this: *mut c_void,
    arg: *const c_void,
) -> u8 {
    let arg_text = payload_primary_text(arg).unwrap_or_else(|| "<unknown>".to_string());
    let arg_info = payload_identity(arg);
    let arg_strings = payload_strings(arg);
    log_exstar_host(format_args!(
        "kind=processmg method=registerSubscribe this={:p} arg={:p} arg_text={} arg_info=\"{}\" arg_strings=\"{}\"",
        this,
        arg,
        arg_text,
        arg_info,
        arg_strings
    ));
    let result = PROCESS_MG_REGISTER_SUBSCRIBE
        .map(|original| original(this, arg))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=processmg method=registerSubscribe_result success={} this={:p} arg={:p}",
        result != 0,
        this,
        arg
    ));
    result
}

unsafe extern "system" fn zluda_process_mg_subpub_connect_to_hub(
    this: *mut c_void,
    arg: *const c_void,
    object: *mut c_void,
) -> u8 {
    let arg_text = payload_primary_text(arg).unwrap_or_else(|| "<unknown>".to_string());
    let arg_info = payload_identity(arg);
    let arg_strings = payload_strings(arg);
    let module_object = read_usize((this as *const u8).wrapping_add(0x10).cast())
        .map(|value| value as *const c_void)
        .unwrap_or(ptr::null());
    let module_vtable = read_usize(module_object)
        .map(|value| value as *const c_void)
        .unwrap_or(ptr::null());
    let slot_58 = read_usize((module_vtable as *const u8).wrapping_add(0x58).cast())
        .map(|value| value as *const c_void)
        .unwrap_or(ptr::null());
    log_exstar_host(format_args!(
        "kind=processmg method=subPubConnectToHub this={:p} arg={:p} arg_text={} arg_info=\"{}\" arg_strings=\"{}\" object={:p} module_object={:p}[{}] module_vtable={:p}[{}] slot58={:p}[{}]",
        this,
        arg,
        arg_text,
        arg_info,
        arg_strings,
        object,
        module_object,
        describe_optional_address(module_object),
        module_vtable,
        describe_optional_address(module_vtable),
        slot_58,
        describe_optional_address(slot_58)
    ));
    if let Some(branch) = exstar_processmg_branch_label(&arg_strings) {
        log_exstar_host(format_args!(
            "kind=processmg_branch method=subPubConnectToHub branch={} this={:p} arg_text={} arg_strings=\"{}\" object={:p}",
            branch, this, arg_text, arg_strings, object
        ));
        if branch == "app10_demo"
            && !EXSTAR_PROCESSMG_APP10_DEMO_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
        {
            log_exstar_host_backtrace(
                "kind=processmg_branch_backtrace",
                format_args!(
                    "method=subPubConnectToHub branch={} this={:p} arg={:p} arg_text={} arg_strings=\"{}\" object={:p}",
                    branch, this, arg, arg_text, arg_strings, object
                ),
                2,
            );
        } else if branch == "app5_ord"
            && !EXSTAR_PROCESSMG_APP5_ORD_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
        {
            log_exstar_host_backtrace(
                "kind=processmg_branch_backtrace",
                format_args!(
                    "method=subPubConnectToHub branch={} this={:p} arg={:p} arg_text={} arg_strings=\"{}\" object={:p}",
                    branch, this, arg, arg_text, arg_strings, object
                ),
                2,
            );
        }
    }
    let has_app_branch = arg_strings.split('|').any(|part| {
        let part = part.trim();
        part.len() > 3
            && part[..3].eq_ignore_ascii_case("app")
            && part[3..].chars().all(|ch| ch.is_ascii_digit())
    });
    if has_app_branch && !EXSTAR_PROCESSMG_APP_BRANCH_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
    {
        log_exstar_host_backtrace(
            "kind=processmg_app_branch_backtrace",
            format_args!(
                "method=subPubConnectToHub this={:p} arg={:p} arg_text={} arg_strings=\"{}\" object={:p}",
                this,
                arg,
                arg_text,
                arg_strings,
                object
            ),
            2,
        );
    }
    if arg_strings
        .split('|')
        .any(|part| part.eq_ignore_ascii_case("app8"))
        && !EXSTAR_PROCESSMG_APP8_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst)
    {
        log_exstar_host_backtrace(
            "kind=processmg_app8_backtrace",
            format_args!(
                "method=subPubConnectToHub this={:p} arg={:p} arg_text={} arg_strings=\"{}\" object={:p}",
                this,
                arg,
                arg_text,
                arg_strings,
                object
            ),
            2,
        );
    }
    let result = PROCESS_MG_SUBPUB_CONNECT
        .map(|original| original(this, arg, object))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=processmg method=subPubConnectToHub_result success={} this={:p} arg={:p} object={:p}",
        result != 0,
        this,
        arg,
        object
    ));
    result
}

unsafe extern "system" fn zluda_process_mg_signal_published(
    this: *mut c_void,
    arg1: *const c_void,
    arg2: *const c_void,
    payload: *const c_void,
) -> u8 {
    let arg1_text = decode_qstring_ref(arg1).unwrap_or_else(|| "<unknown>".to_string());
    let arg2_text = decode_qstring_ref(arg2).unwrap_or_else(|| "<unknown>".to_string());
    let payload_info = payload_identity(payload);
    let payload_strings = payload_strings(payload);
    if exstar_hub_process_trace_enabled("processmg.signal_published") {
        log_exstar_host(format_args!(
            "kind=hub_ingress method=processmg.signal_published this={:p} arg1_text={} arg2_text={} payload_info=\"{}\" payload_strings=\"{}\"",
            this,
            arg1_text,
            arg2_text,
            payload_info,
            payload_strings
        ));
        if !EXSTAR_HUB_PROCESSMG_SIGNAL_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=hub_ingress_backtrace",
                format_args!(
                    "method=processmg.signal_published this={:p} arg1_text={} arg2_text={} payload_info=\"{}\" payload_strings=\"{}\"",
                    this,
                    arg1_text,
                    arg2_text,
                    payload_info,
                    payload_strings
                ),
                2,
            );
        }
    }
    if exstar_is_ui_state_topic(&arg2_text) || exstar_is_ui_state_topic(&arg1_text) {
        log_exstar_host(format_args!(
            "kind=ui_state method=processmg.signal_published this={:p} arg1_text={} arg2_text={} payload_info=\"{}\" payload_strings=\"{}\"",
            this,
            arg1_text,
            arg2_text,
            payload_info,
            payload_strings
        ));
        if !EXSTAR_UI_TOPIC_PROCESSMG_SIGNAL_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=ui_state_backtrace",
                format_args!(
                    "method=processmg.signal_published this={:p} arg1_text={} arg2_text={} payload_info=\"{}\" payload_strings=\"{}\"",
                    this,
                    arg1_text,
                    arg2_text,
                    payload_info,
                    payload_strings
                ),
                2,
            );
        }
    }
    log_exstar_host(format_args!(
        "kind=processmg method=signal_published this={:p} arg1={:p} arg1_text={} arg2={:p} arg2_text={} payload={:p} payload_info=\"{}\" payload_strings=\"{}\"",
        this,
        arg1,
        arg1_text,
        arg2,
        arg2_text,
        payload,
        payload_info,
        payload_strings
    ));
    let result = PROCESS_MG_SIGNAL_PUBLISHED
        .map(|original| original(this, arg1, arg2, payload))
        .unwrap_or(0);
    let real_app_window_shown = EXSTAR_CHILD_HUB_APP_WINDOW_SHOWN.load(Ordering::SeqCst)
        || exstar_child_hub_real_app_window_exists();
    if exstar_should_force_child_hub_quit(&payload_strings, real_app_window_shown) {
        log_exstar_host(format_args!(
            "kind=compat action=force_child_hub_qt_exit trigger=processmg.signal_published real_app_window_shown={} payload_strings=\"{}\"",
            real_app_window_shown,
            payload_strings
        ));
        if let Some(original_exit) = QT_CORE_APPLICATION_EXIT {
            original_exit(0);
        } else if let Some(original_quit) = QT_CORE_APPLICATION_QUIT {
            original_quit();
        }
    }
    maybe_launch_einscan_net_svr_delivery(
        "processmg.signal_published",
        &arg1_text,
        &arg2_text,
        &payload_strings,
    );
    result
}

unsafe extern "system" fn zluda_process_mg_qt_metacall(
    this: *mut c_void,
    call: i32,
    id: i32,
    args: *mut *mut c_void,
) -> i32 {
    if exstar_hub_process_trace_enabled("processmg.qt_metacall") {
        log_exstar_host(format_args!(
            "kind=hub_ingress method=processmg.qt_metacall this={:p} call={} id={} args={:p}",
            this, call, id, args
        ));
        if !EXSTAR_HUB_PROCESSMG_QT_METACALL_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=hub_ingress_backtrace",
                format_args!(
                    "method=processmg.qt_metacall this={:p} call={} id={} args={:p}",
                    this, call, id, args
                ),
                2,
            );
        }
    }
    log_exstar_host(format_args!(
        "kind=processmg method=qt_metacall this={:p} call={} id={} args={:p}",
        this, call, id, args
    ));
    PROCESS_MG_QT_METACALL
        .map(|original| original(this, call, id, args))
        .unwrap_or(id)
}

unsafe extern "system" fn zluda_process_mg_qt_static_metacall(
    object: *mut c_void,
    call: i32,
    id: i32,
    args: *mut *mut c_void,
) {
    let args_summary = if call == 10 && id == 0 {
        qt_metacall_args_summary(args, 4)
    } else {
        String::new()
    };
    if exstar_hub_process_trace_enabled("processmg.qt_static_metacall") {
        if args_summary.is_empty() {
            log_exstar_host(format_args!(
                "kind=hub_ingress method=processmg.qt_static_metacall object={:p} call={} id={} args={:p}",
                object,
                call,
                id,
                args
            ));
            if !EXSTAR_HUB_PROCESSMG_QT_STATIC_METACALL_BACKTRACE_EMITTED
                .swap(true, Ordering::SeqCst)
            {
                log_exstar_host_backtrace(
                    "kind=hub_ingress_backtrace",
                    format_args!(
                        "method=processmg.qt_static_metacall object={:p} call={} id={} args={:p}",
                        object, call, id, args
                    ),
                    2,
                );
            }
        } else {
            log_exstar_host(format_args!(
                "kind=hub_ingress method=processmg.qt_static_metacall object={:p} call={} id={} args={:p} args_summary=\"{}\"",
                object,
                call,
                id,
                args,
                args_summary
            ));
            if !EXSTAR_HUB_PROCESSMG_QT_STATIC_METACALL_BACKTRACE_EMITTED
                .swap(true, Ordering::SeqCst)
            {
                log_exstar_host_backtrace(
                    "kind=hub_ingress_backtrace",
                    format_args!(
                        "method=processmg.qt_static_metacall object={:p} call={} id={} args={:p} args_summary=\"{}\"",
                        object,
                        call,
                        id,
                        args,
                        args_summary
                    ),
                    2,
                );
            }
        }
    }
    if args_summary.is_empty() {
        log_exstar_host(format_args!(
            "kind=processmg method=qt_static_metacall object={:p} call={} id={} args={:p}",
            object, call, id, args
        ));
    } else {
        log_exstar_host(format_args!(
            "kind=processmg method=qt_static_metacall object={:p} call={} id={} args={:p} args_summary=\"{}\"",
            object,
            call,
            id,
            args,
            args_summary
        ));
    }
    if let Some(original) = PROCESS_MG_QT_STATIC_METACALL {
        original(object, call, id, args);
    }
}

unsafe extern "system" fn zluda_qttunnel_module_connect(this: *mut c_void) {
    let current_exe_name = exstar_current_exe("qttunnel_connect")
        .map(|(_, exe_name)| exe_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    log_exstar_host(format_args!(
        "kind=qttunnel method=connectToHub this={:p} current_exe={}",
        this, current_exe_name
    ));
    if let Some(original) = QTTUNNEL_MODULE_CONNECT {
        original(this);
    }
    log_exstar_host(format_args!(
        "kind=qttunnel method=connectToHub_return this={:p} current_exe={}",
        this, current_exe_name
    ));
}

unsafe extern "system" fn zluda_qttunnel_module_ctor(
    this: *mut c_void,
    name: *const c_void,
    channel: *const c_void,
    password: *const c_void,
    host_address: *const c_void,
    port: u16,
    parent: *mut c_void,
) -> *mut c_void {
    let name_text = decode_qstring_ref(name).unwrap_or_else(|| "<unknown>".to_string());
    let channel_text = decode_qstring_ref(channel).unwrap_or_else(|| "<unknown>".to_string());
    let password_text = describe_optional_address(password);
    let current_exe_name = exstar_current_exe("qttunnel_ctor")
        .map(|(_, exe_name)| exe_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    log_exstar_host(format_args!(
        "kind=qttunnel method=ctor this={:p} current_exe={} name={} channel={} password={} host_address={:p}[{}] port={} parent={:p}",
        this,
        current_exe_name,
        name_text,
        channel_text,
        password_text,
        host_address,
        describe_optional_address(host_address),
        port,
        parent
    ));
    let result = QTTUNNEL_MODULE_CTOR
        .map(|original| original(this, name, channel, password, host_address, port, parent))
        .unwrap_or(this);
    log_exstar_host(format_args!(
        "kind=qttunnel method=ctor_return this={:p} result={:p}",
        this, result
    ));
    result
}

unsafe extern "system" fn zluda_qttunnel_module_dtor(this: *mut c_void) {
    log_exstar_host(format_args!("kind=qttunnel method=dtor this={:p}", this));
    if let Some(original) = QTTUNNEL_MODULE_DTOR {
        original(this);
    }
    log_exstar_host(format_args!(
        "kind=qttunnel method=dtor_return this={:p}",
        this
    ));
}

unsafe extern "system" fn zluda_qttunnel_module_connect_with_int(
    this: *mut c_void,
    arg: i32,
) -> u8 {
    let current_exe_name = exstar_current_exe("qttunnel_connect_int")
        .map(|(_, exe_name)| exe_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    log_exstar_host(format_args!(
        "kind=qttunnel method=connectToHub_int this={:p} current_exe={} arg={}",
        this, current_exe_name, arg
    ));
    QTTUNNEL_MODULE_CONNECT_WITH_INT
        .map(|original| original(this, arg))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_qttunnel_module_is_connected(this: *const c_void) -> u8 {
    let current_exe_name = exstar_current_exe("qttunnel_is_connected")
        .map(|(_, exe_name)| exe_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    log_exstar_host(format_args!(
        "kind=qttunnel method=isConnected this={:p} current_exe={}",
        this, current_exe_name
    ));
    let mut result = QTTUNNEL_MODULE_IS_CONNECTED
        .map(|original| original(this))
        .unwrap_or(0);
    if result == 0
        && exstar_trace_logging_enabled()
        && exstar_is_helper_connect_retry_target(&current_exe_name)
    {
        const HELPER_CONNECT_RETRY_DELAY_MS: u64 = 150;
        const HELPER_CONNECT_RETRY_ATTEMPTS: u32 = 4;
        for attempt in 1..=HELPER_CONNECT_RETRY_ATTEMPTS {
            log_exstar_host(format_args!(
                "kind=qttunnel method=isConnected_retry this={:p} current_exe={} attempt={} delay_ms={}",
                this,
                current_exe_name,
                attempt,
                HELPER_CONNECT_RETRY_DELAY_MS
            ));
            thread::sleep(Duration::from_millis(HELPER_CONNECT_RETRY_DELAY_MS));
            result = QTTUNNEL_MODULE_IS_CONNECTED
                .map(|original| original(this))
                .unwrap_or(0);
            log_exstar_host(format_args!(
                "kind=qttunnel method=isConnected_retry_result this={:p} current_exe={} attempt={} result={}",
                this,
                current_exe_name,
                attempt,
                result != 0
            ));
            if result != 0 {
                break;
            }
        }
    }
    log_exstar_host(format_args!(
        "kind=qttunnel method=isConnected_return this={:p} current_exe={} result={}",
        this,
        current_exe_name,
        result != 0
    ));
    result
}

unsafe extern "system" fn zluda_qttunnel_module_publish(
    this: *mut c_void,
    topic: *const c_void,
    payload: *const c_void,
    retained: u8,
) -> u8 {
    let topic_text = decode_qstring_ref(topic).unwrap_or_else(|| "<unknown>".to_string());
    let payload_info = payload_identity(payload);
    let payload_strings = payload_strings(payload);
    if topic_text == "Sn3dProcessMgTopic" {
        maybe_launch_einscan_net_svr_publish("qttunnel.publish", &payload_strings);
    }
    log_exstar_host(format_args!(
        "kind=qttunnel method=publish this={:p} topic={:p} topic_text={} payload={:p} payload_info=\"{}\" payload_strings=\"{}\" retained={}",
        this,
        topic,
        topic_text,
        payload,
        payload_info,
        payload_strings,
        retained != 0
    ));
    QTTUNNEL_MODULE_PUBLISH
        .map(|original| original(this, topic, payload, retained))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_qttunnel_module_published(
    this: *mut c_void,
    arg1: *const c_void,
    arg2: *const c_void,
    payload: *const c_void,
) {
    let arg1_text = decode_qstring_ref(arg1).unwrap_or_else(|| "<unknown>".to_string());
    let arg2_text = decode_qstring_ref(arg2).unwrap_or_else(|| "<unknown>".to_string());
    let payload_info = payload_identity(payload);
    let payload_strings = payload_strings(payload);
    if exstar_is_ui_state_topic(&arg2_text) || exstar_is_ui_state_topic(&arg1_text) {
        log_exstar_host(format_args!(
            "kind=ui_state method=qttunnel.published this={:p} arg1_text={} arg2_text={} payload_info=\"{}\" payload_strings=\"{}\"",
            this,
            arg1_text,
            arg2_text,
            payload_info,
            payload_strings
        ));
        if !EXSTAR_UI_TOPIC_QTTUNNEL_PUBLISHED_BACKTRACE_EMITTED.swap(true, Ordering::SeqCst) {
            log_exstar_host_backtrace(
                "kind=ui_state_backtrace",
                format_args!(
                    "method=qttunnel.published this={:p} arg1_text={} arg2_text={} payload_info=\"{}\" payload_strings=\"{}\"",
                    this,
                    arg1_text,
                    arg2_text,
                    payload_info,
                    payload_strings
                ),
                2,
            );
        }
    }
    log_exstar_host(format_args!(
        "kind=qttunnel method=published this={:p} arg1={:p} arg1_text={} arg2={:p} arg2_text={} payload={:p} payload_info=\"{}\" payload_strings=\"{}\"",
        this,
        arg1,
        arg1_text,
        arg2,
        arg2_text,
        payload,
        payload_info,
        payload_strings
    ));
    if let Some(original) = QTTUNNEL_MODULE_PUBLISHED {
        original(this, arg1, arg2, payload);
    }
}

unsafe extern "system" fn zluda_qttunnel_module_qt_metacall(
    this: *mut c_void,
    call: i32,
    id: i32,
    args: *mut *mut c_void,
) -> i32 {
    log_exstar_host(format_args!(
        "kind=qttunnel method=qt_metacall this={:p} call={} id={} args={:p}",
        this, call, id, args
    ));
    QTTUNNEL_MODULE_QT_METACALL
        .map(|original| original(this, call, id, args))
        .unwrap_or(id)
}

unsafe extern "system" fn zluda_qttunnel_module_qt_static_metacall(
    object: *mut c_void,
    call: i32,
    id: i32,
    args: *mut *mut c_void,
) {
    let args_summary = if call == 10 && id == 0 {
        qt_metacall_args_summary(args, 4)
    } else {
        String::new()
    };
    if args_summary.is_empty() {
        log_exstar_host(format_args!(
            "kind=qttunnel method=qt_static_metacall object={:p} call={} id={} args={:p}",
            object, call, id, args
        ));
    } else {
        log_exstar_host(format_args!(
            "kind=qttunnel method=qt_static_metacall object={:p} call={} id={} args={:p} args_summary=\"{}\"",
            object,
            call,
            id,
            args,
            args_summary
        ));
    }
    if let Some(original) = QTTUNNEL_MODULE_QT_STATIC_METACALL {
        original(object, call, id, args);
    }
}

unsafe extern "system" fn zluda_appui_handle_show_passport(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe(
        "appui",
        "handleShowPassport@0x4dd96",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    APPUI_HANDLE_SHOW_PASSPORT
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_passport_handle_show_passport_cmd(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe(
        "passport",
        "handleShowPassportCmd@0x3bfc1",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    PASSPORT_HANDLE_SHOW_PASSPORT_CMD
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_passport_handle_login_success(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe(
        "passport",
        "handleLoginSuccess@0x39ed0",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    PASSPORT_HANDLE_LOGIN_SUCCESS
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_exstar_exe_f9ec(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe("exe", "wrapper@0xf9ec", this, arg1, arg2, arg3, arg4, arg5);
    EXSTAR_EXE_F9EC
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

/// IAT-based hook for connectAndRegister in scanservice.exe.
/// The Detours-based hook doesn't fire for unknown reasons.
/// Hook for manager's checkOpenGL function at +0x5580.
/// Always returns true (1) to skip the TestOpenglHelper + Media Player checks.
/// Under ZLUDA, we know the GPU works (CUDA fully operational), but the checks
/// fail intermittently due to QProcess timing and lack of NVIDIA OpenGL driver.
unsafe extern "system" fn zluda_process_manager_check_opengl(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_exstar_host(format_args!(
        "kind=compat action=check_opengl_bypass result=true (skipping TestOpenglHelper+MediaPlayer checks)"
    ));
    1 // return true — GPU is capable
}

unsafe extern "system" fn zluda_scanservice_connect_and_register_iat(
    arg1: *mut c_void,
    arg2: *mut c_void,
) -> u8 {
    log_exstar_host(format_args!(
        "kind=scanservice method=connectAndRegister_IAT arg1={:p} arg2={:p}",
        arg1, arg2
    ));
    let result = if let Some(original) = SCANSERVICE_CONNECT_AND_REGISTER_ORIGINAL {
        original(arg1, arg2)
    } else {
        0
    };
    log_exstar_host(format_args!(
        "kind=scanservice method=connectAndRegister_IAT_result success={}",
        result != 0
    ));
    result
}

unsafe extern "system" fn zluda_scanservice_pre_exec(
    this: *mut c_void, arg1: *mut c_void, arg2: *mut c_void,
    arg3: *mut c_void, arg4: *mut c_void, arg5: *mut c_void,
) -> usize {
    log_exstar_host(format_args!(
        "kind=scanservice hook=pre_exec_4e4b_reached (about to call QCoreApplication::exec)"
    ));
    SCANSERVICE_PRE_EXEC
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_scanservice_early_check(
    this: *mut c_void, arg1: *mut c_void, arg2: *mut c_void,
    arg3: *mut c_void, arg4: *mut c_void, arg5: *mut c_void,
) -> usize {
    log_exstar_host(format_args!(
        "kind=scanservice hook=early_check_44ec_reached this={:p} (cmp [rsp+38h], 3)",
        this
    ));
    SCANSERVICE_EARLY_CHECK
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_scanservice_alt_path(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_exstar_host(format_args!(
        "kind=scanservice hook=alt_path_47e4_reached this={:p} (branch [rsp+38h]!=3 taken)",
        this
    ));
    SCANSERVICE_ALT_PATH
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_scanservice_pre_connect(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_exstar_host(format_args!(
        "kind=scanservice hook=pre_connectAndRegister_reached this={:p} arg1={:p} arg2={:p}",
        this, arg1, arg2
    ));
    SCANSERVICE_PRE_CONNECT
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_scanservice_exe_entry_6a40(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_exstar_host(format_args!(
        "kind=scanservice hook=execute_entry_6a40 method=WinMain_or_main"
    ));
    // IAT-patch connectAndRegister in scanservice.exe.
    // The Detours hook on Sn3DProcessPlugin+0x4c50 is attached but never fires for scanservice.
    // Direct IAT patching bypasses Detours entirely.
    {
        let exe_base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(ptr::null()) as usize;
        let iat_entry = (exe_base + 0x9420) as *mut *mut c_void;
        if memory_readable(iat_entry as *const c_void, 8) {
            let original_fn = *iat_entry;
            // Save original for calling from our hook
            SCANSERVICE_CONNECT_AND_REGISTER_ORIGINAL = Some(std::mem::transmute(original_fn));
            // Patch IAT to point to our hook
            let mut old_protect: u32 = 0;
            windows_sys::Win32::System::Memory::VirtualProtect(
                iat_entry as *const c_void,
                8,
                0x04, // PAGE_READWRITE
                &mut old_protect,
            );
            *iat_entry = zluda_scanservice_connect_and_register_iat as *mut c_void;
            windows_sys::Win32::System::Memory::VirtualProtect(
                iat_entry as *const c_void,
                8,
                old_protect,
                &mut old_protect,
            );
            log_exstar_host(format_args!(
                "kind=scanservice hook=iat_patched_connectAndRegister original={:p} new={:p}",
                original_fn, zluda_scanservice_connect_and_register_iat as *mut c_void
            ));
        }
    }
    // Sn3DDeviceEinStar.dll is loaded late (during Qt init, not at WinMain entry).
    // Spawn a background thread that polls for it and hooks stop() when it appears.
    unsafe extern "system" fn device_hook_poller(_: *mut c_void) -> u32 {
        for _ in 0..60 { // poll for 30 seconds
            let device_dll = GetModuleHandleA(c"Sn3DDeviceEinStar.dll".as_ptr().cast());
            if !device_dll.is_null() {
                detour_exstar_device_einstar(device_dll as *mut c_void);
                log_exstar_host(format_args!(
                    "kind=scanservice hook=device_einstar_hooked_by_poller"
                ));
                return 0;
            }
            thread::sleep(Duration::from_millis(500));
        }
        log_exstar_host(format_args!(
            "kind=scanservice hook=device_einstar_poller_timeout"
        ));
        0
    }
    let mut poller_tid: u32 = 0;
    windows_sys::Win32::System::Threading::CreateThread(
        ptr::null(),
        0,
        Some(device_hook_poller),
        ptr::null_mut(),
        0,
        &mut poller_tid,
    );
    // Spawn watchdog thread to capture stack trace if WinMain hangs
    // Use raw CreateThread instead of thread::spawn (more reliable in DLL context)
    let main_thread_id = windows_sys::Win32::System::Threading::GetCurrentThreadId();
    static mut WATCHDOG_TID: u32 = 0;
    unsafe extern "system" fn watchdog_thread_proc(param: *mut c_void) -> u32 {
        scanservice_watchdog(param as u32);
        0
    }
    windows_sys::Win32::System::Threading::CreateThread(
        ptr::null(),
        0,
        Some(watchdog_thread_proc),
        main_thread_id as *mut c_void,
        0,
        &mut WATCHDOG_TID,
    );
    let result = SCANSERVICE_EXE_ENTRY_6A40
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=scanservice hook=execute_entry_6a40_return result={}",
        result
    ));
    result
}

unsafe fn scanservice_watchdog(main_thread_id: u32) {
    // Write to our own file — the shared host log might not be thread-safe
    let pid = GetCurrentProcessId();
    let current_thread_id = GetCurrentThreadId();
    let watchdog_path = format!(
        "<runtime-repo>\\target\\debug\\debug\\launcher\\scanservice_watchdog_{}.log",
        pid
    );
    let _ = std::fs::write(&watchdog_path, format!("watchdog_start pid={} thread_id={}\n", pid, main_thread_id));
    // No sleep — process dies within milliseconds
    let mut log_lines = Vec::new();
    log_lines.push(format!(
        "kind=scanservice_watchdog phase=start main_thread_id={} pid={}",
        main_thread_id, pid
    ));
    let thread_ids = collect_process_thread_ids();
    let ordered_thread_ids = watchdog_thread_order(main_thread_id, current_thread_id, &thread_ids);
    log_lines.push(format!(
        "phase=thread_list current_thread_id={} total_threads={} captured_threads={}",
        current_thread_id,
        thread_ids.len(),
        ordered_thread_ids.len()
    ));
    for thread_id in ordered_thread_ids {
        log_lines.push(format!("phase=thread_start thread_id={}", thread_id));
        append_watchdog_thread_dump(&mut log_lines, thread_id);
        log_lines.push(format!("phase=thread_end thread_id={}", thread_id));
    }
    log_lines.push("phase=complete".to_string());
    let _ = std::fs::write(&watchdog_path, log_lines.join("\n"));
}

fn watchdog_thread_order(main_thread_id: u32, current_thread_id: u32, thread_ids: &[u32]) -> Vec<u32> {
    let mut ordered = Vec::new();
    if main_thread_id != 0
        && main_thread_id != current_thread_id
        && thread_ids.iter().any(|&thread_id| thread_id == main_thread_id)
    {
        ordered.push(main_thread_id);
    }
    for &thread_id in thread_ids {
        if thread_id == 0 || thread_id == current_thread_id || ordered.contains(&thread_id) {
            continue;
        }
        ordered.push(thread_id);
    }
    ordered
}

unsafe fn collect_process_thread_ids() -> Vec<u32> {
    let mut thread_ids = Vec::new();
    let thread_snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
        Ok(snapshot) => snapshot,
        Err(_) => return thread_ids,
    };
    let current_process = GetCurrentProcessId();
    let mut thread = THREADENTRY32::default();
    thread.dwSize = mem::size_of::<THREADENTRY32>() as u32;
    if Thread32First(thread_snapshot, &mut thread).is_ok() {
        loop {
            if thread.th32OwnerProcessID == current_process {
                thread_ids.push(thread.th32ThreadID);
            }
            if Thread32Next(thread_snapshot, &mut thread).is_err() {
                break;
            }
        }
    }
    let _ = CloseHandle(thread_snapshot);
    thread_ids
}

unsafe fn append_watchdog_thread_dump(log_lines: &mut Vec<String>, thread_id: u32) {
    let thread_handle = windows_sys::Win32::System::Threading::OpenThread(
        0x0002 | 0x0008 | 0x0040,
        0,
        thread_id,
    );
    if thread_handle.is_null() {
        log_lines.push(format!(
            "phase=thread_error thread_id={} reason=open_thread_failed last_error={}",
            thread_id,
            GetLastError()
        ));
        return;
    }
    let prev_count = windows_sys::Win32::System::Threading::SuspendThread(thread_handle);
    if prev_count == u32::MAX {
        log_lines.push(format!(
            "phase=thread_error thread_id={} reason=suspend_failed last_error={}",
            thread_id,
            GetLastError()
        ));
        let _ = CloseHandle(HANDLE(thread_handle as _));
        return;
    }
    let mut ctx: CONTEXT = std::mem::zeroed();
    ctx.ContextFlags = 0x00100000 | 0x0000000F;
    let kernel32 = GetModuleHandleA(c"kernel32.dll".as_ptr().cast());
    let get_thread_ctx: unsafe extern "system" fn(*mut c_void, *mut CONTEXT) -> i32 =
        std::mem::transmute(GetProcAddress(kernel32 as _, c"GetThreadContext".as_ptr().cast()));
    let success = get_thread_ctx(thread_handle, &mut ctx);
    if success != 0 {
        log_lines.push(format!(
            "phase=context thread_id={} rip=0x{:x} rsp=0x{:x} rbp=0x{:x}",
            thread_id, ctx.Rip, ctx.Rsp, ctx.Rbp
        ));
        log_lines.push(format!(
            "phase=regs thread_id={} rcx=0x{:x} rdx=0x{:x} r8=0x{:x} r9=0x{:x} rax=0x{:x} rbx=0x{:x} rdi=0x{:x} rsi=0x{:x}",
            thread_id, ctx.Rcx, ctx.Rdx, ctx.R8, ctx.R9, ctx.Rax, ctx.Rbx, ctx.Rdi, ctx.Rsi
        ));
        log_lines.push(format!(
            "phase=rip thread_id={} addr=0x{:x} module={}",
            thread_id,
            ctx.Rip,
            identify_module(ctx.Rip as *const c_void)
        ));
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        type RtlLookupFunctionEntryFn =
            unsafe extern "system" fn(u64, *mut u64, *mut c_void) -> *mut c_void;
        type RtlVirtualUnwindFn = unsafe extern "system" fn(
            u32,
            u64,
            u64,
            *mut c_void,
            *mut CONTEXT,
            *mut *mut c_void,
            *mut u64,
            *mut c_void,
        ) -> *mut c_void;
        let lookup_fn: Option<RtlLookupFunctionEntryFn> =
            std::mem::transmute(GetProcAddress(ntdll as _, c"RtlLookupFunctionEntry".as_ptr().cast()));
        let unwind_fn: Option<RtlVirtualUnwindFn> =
            std::mem::transmute(GetProcAddress(ntdll as _, c"RtlVirtualUnwind".as_ptr().cast()));
        if let (Some(lookup), Some(unwind)) = (lookup_fn, unwind_fn) {
            let mut unwind_ctx = ctx;
            for i in 0..20 {
                let pc = unwind_ctx.Rip;
                if pc == 0 {
                    break;
                }
                let module = identify_module(pc as *const c_void);
                log_lines.push(format!(
                    "phase=frame thread_id={} idx={} pc=0x{:x} module={}",
                    thread_id, i, pc, module
                ));
                let mut image_base: u64 = 0;
                let runtime_fn = lookup(pc, &mut image_base, ptr::null_mut());
                if runtime_fn.is_null() {
                    log_lines.push(format!(
                        "phase=unwind_end thread_id={} idx={} reason=no_function_entry",
                        thread_id, i
                    ));
                    break;
                }
                let mut handler_data: *mut c_void = ptr::null_mut();
                let mut establisher_frame: u64 = 0;
                unwind(
                    0,
                    image_base,
                    pc,
                    runtime_fn,
                    &mut unwind_ctx,
                    &mut handler_data,
                    &mut establisher_frame,
                    ptr::null_mut(),
                );
            }
        } else {
            log_lines.push(format!(
                "phase=fallback thread_id={} reason=rtl_functions_not_found",
                thread_id
            ));
            let rsp = ctx.Rsp as *const u64;
            for i in 0..32 {
                let stack_addr = rsp.add(i);
                let mut info: windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION =
                    std::mem::zeroed();
                let result = windows_sys::Win32::System::Memory::VirtualQuery(
                    stack_addr as *const c_void,
                    &mut info,
                    std::mem::size_of::<windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION>(),
                );
                if result == 0 {
                    break;
                }
                if info.Protect & (0x02 | 0x04 | 0x20 | 0x40) == 0 {
                    continue;
                }
                let val = *stack_addr;
                let module = identify_module(val as *const c_void);
                if !module.is_empty() {
                    log_lines.push(format!(
                        "phase=stack thread_id={} frame={} rsp_offset=+0x{:x} addr=0x{:x} module={}",
                        thread_id,
                        i,
                        i * 8,
                        val,
                        module
                    ));
                }
            }
        }
    } else {
        log_lines.push(format!(
            "phase=thread_error thread_id={} reason=get_context_failed last_error={}",
            thread_id,
            GetLastError()
        ));
    }
    let _ = windows_sys::Win32::System::Threading::ResumeThread(thread_handle);
    let _ = CloseHandle(HANDLE(thread_handle as _));
}

unsafe fn identify_module(addr: *const c_void) -> String {
    let addr_val = addr as u64;
    // Check against known modules using GetModuleHandleW + GetModuleFileNameW
    let mut module_name = [0u16; 260];
    let mut h_module: windows_sys::Win32::Foundation::HINSTANCE = std::ptr::null_mut();
    // Use GetModuleHandleEx with GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
    let success = windows_sys::Win32::System::LibraryLoader::GetModuleHandleExW(
        0x00000004, // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
        addr as *const u16,
        &mut h_module,
    );
    if success == 0 || h_module.is_null() {
        return String::new();
    }
    let len = windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(h_module, module_name.as_mut_ptr(), 260);
    if len == 0 {
        return format!("0x{:x}", h_module as u64);
    }
    let name = String::from_utf16_lossy(&module_name[..len as usize]);
    let base = h_module as u64;
    let offset = addr_val.wrapping_sub(base);
    // Extract just the filename
    let short = name.rsplit('\\').next().unwrap_or(&name);
    format!("{}+0x{:x}", short, offset)
}

unsafe extern "system" fn zluda_exstar_exe_6940(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    maybe_log_exe_entry_backtrace(
        &EXSTAR_EXE_6940_BACKTRACE_EMITTED,
        "entry@0x6940",
        this,
        arg1,
        arg2,
        arg3,
        true,
    );
    // Hook DXGI CreateDXGIFactory at this point — all DLLs including dxgi.dll are loaded.
    let dxgi = GetModuleHandleA(c"dxgi.dll".as_ptr().cast());
    if !dxgi.is_null() {
        dxgi_try_hook_create_factory(dxgi as HMODULE);
    }
    EXSTAR_EXE_6940
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_exstar_exe_6dc0(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let arg1_text =
        decode_qstring_ref(arg1.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    let arg2_text =
        decode_qstring_ref(arg2.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    let payload_info = payload_identity(arg3.cast_const());
    let payload_strings = payload_strings(arg3.cast_const());
    log_exstar_host(format_args!(
        "kind=exe method=lambda@0x6dc0 this={:p} arg1={:p} arg1_text={} arg2={:p} arg2_text={} payload={:p} payload_info=\"{}\" payload_strings=\"{}\" arg4={:p}[{}] arg5={:p}[{}]",
        this,
        arg1,
        arg1_text,
        arg2,
        arg2_text,
        arg3,
        payload_info,
        payload_strings,
        arg4,
        describe_optional_address(arg4.cast_const()),
        arg5,
        describe_optional_address(arg5.cast_const()),
    ));
    EXSTAR_EXE_6DC0
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_exstar_exe_bc30(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let arg1_text =
        decode_qstring_ref(arg1.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    let arg2_text =
        decode_qstring_ref(arg2.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    let payload_info = payload_identity(arg3.cast_const());
    let payload_strings = payload_strings(arg3.cast_const());
    log_exstar_host(format_args!(
        "kind=exe method=signal_slot@0xbc30 this={:p} arg1={:p} arg1_text={} arg2={:p} arg2_text={} payload={:p} payload_info=\"{}\" payload_strings=\"{}\" arg4={:p}[{}] arg5={:p}[{}]",
        this,
        arg1,
        arg1_text,
        arg2,
        arg2_text,
        arg3,
        payload_info,
        payload_strings,
        arg4,
        describe_optional_address(arg4.cast_const()),
        arg5,
        describe_optional_address(arg5.cast_const()),
    ));
    EXSTAR_EXE_BC30
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_exstar_exe_f0f8(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    if env::args().any(|a| a == "@#$") {
        let _ = EXSTAR_CHILD_HUB_START_THREAD_ID.compare_exchange(
            0,
            GetCurrentThreadId(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    maybe_log_exe_entry_backtrace(
        &EXSTAR_EXE_F0F8_BACKTRACE_EMITTED,
        "entry@0xf0f8",
        this,
        arg1,
        arg2,
        arg3,
        false,
    );
    EXSTAR_EXE_F0F8
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_exstar_exe_fac4(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe("exe", "wrapper@0xfac4", this, arg1, arg2, arg3, arg4, arg5);
    EXSTAR_EXE_FAC4
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_exstar_exe_f6c0(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let result = EXSTAR_EXE_F6C0
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    let is_child_hub = env::args().any(|a| a == "@#$");
    log_exstar_host(format_args!(
        "kind=exe method=init_check@0xf6c0 result={} is_child_hub={} this={:p}",
        result, is_child_hub, this
    ));
    result
}

unsafe extern "system" fn zluda_exstar_exe_10390(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let result = EXSTAR_EXE_10390
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    let is_child_hub = env::args().any(|a| a == "@#$");
    log_exstar_host(format_args!(
        "kind=exe method=post_d070_check@0x10390 result={} is_child_hub={} this={:p}",
        result, is_child_hub, this
    ));
    // Returns 512 (0x200) — low byte is 0, so `test al,al` → false → exit.
    // Force low byte to 1 for child Hub so it proceeds to QApplication::exec.
    if is_child_hub && (result & 0xFF) == 0 {
        log_exstar_host(format_args!(
            "kind=compat action=force_post_d070_check result={}->1 method=post_d070_check@0x10390",
            result
        ));
        return 1;
    }
    result
}

unsafe extern "system" fn zluda_exstar_exe_a6e0(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let result = EXSTAR_EXE_A6E0
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    let is_child_hub = env::args().any(|a| a == "@#$");
    log_exstar_host(format_args!(
        "kind=exe method=guard@0xa6e0 result={} is_child_hub={} this={:p} arg1={:p}",
        result,
        is_child_hub,
        this,
        arg1
    ));
    if is_child_hub && (result & 0xFF) == 0 {
        log_exstar_host(format_args!(
            "kind=compat action=force_guard_a6e0 result={}->1 method=guard@0xa6e0",
            result
        ));
        return 1;
    }
    result
}

unsafe extern "system" fn zluda_process_manager_exe_f5f0(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let process_name =
        decode_qstring_ref(arg1.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    let cmdparam_summary = if arg2.is_null() {
        "<null>".to_string()
    } else {
        format!(
            "{:p}[{}]",
            arg2,
            describe_optional_address(arg2.cast_const())
        )
    };
    log_exstar_host(format_args!(
        "kind=manager_exe method=launch@0xf5f0 this={:p} process_name={} cmdparam={} arg3={:p}[{}] arg4={:p}[{}] arg5={:p}[{}]",
        this,
        process_name,
        cmdparam_summary,
        arg3,
        describe_optional_address(arg3.cast_const()),
        arg4,
        describe_optional_address(arg4.cast_const()),
        arg5,
        describe_optional_address(arg5.cast_const())
    ));
    log_exstar_host_backtrace(
        "kind=manager_exe_launch_backtrace",
        format_args!(
            "method=launch@0xf5f0 this={:p} process_name={} cmdparam={}",
            this, process_name, cmdparam_summary
        ),
        2,
    );
    PROCESS_MANAGER_EXE_F5F0
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_manager_exe_kill_all_e1a0(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let second_sweep_id = EXSTAR_MANAGER_SECOND_SWEEP_ID.load(Ordering::SeqCst);
    if !exstar_trace_logging_enabled() {
        if exstar_manager_skip_second_sweep_enabled() && second_sweep_id != 0 {
            return 1;
        }
        return PROCESS_MANAGER_EXE_KILL_ALL_E1A0
            .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
            .unwrap_or(0);
    }
    let kill_all_count = EXSTAR_MANAGER_KILL_ALL_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    log_exstar_host(format_args!(
        "kind=manager_exe method=killAll@0xe1a0 this={:p}[{}] kill_manager={} count={} second_sweep={} arg2={:p}[{}] arg3={:p}[{}] arg4={:p}[{}] arg5={:p}[{}]",
        this,
        describe_optional_address(this.cast_const()),
        (arg1 as usize & 0xff) != 0,
        kill_all_count,
        second_sweep_id,
        arg2,
        describe_optional_address(arg2.cast_const()),
        arg3,
        describe_optional_address(arg3.cast_const()),
        arg4,
        describe_optional_address(arg4.cast_const()),
        arg5,
        describe_optional_address(arg5.cast_const())
    ));
    if exstar_manager_skip_second_sweep_enabled() && second_sweep_id != 0 {
        log_exstar_host(format_args!(
            "kind=compat action=skip_manager_kill_all status=skipped_second_sweep count={} second_sweep={} result=1",
            kill_all_count, second_sweep_id
        ));
        return 1;
    }
    let result = PROCESS_MANAGER_EXE_KILL_ALL_E1A0
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=manager_exe method=killAll@0xe1a0_result this={:p} count={} second_sweep={} result=0x{:x}",
        this,
        kill_all_count,
        second_sweep_id,
        result
    ));
    result
}

unsafe extern "system" fn zluda_process_manager_exe_kill_one_e560(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let process_name =
        decode_qstring_ref(arg1.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    let timeout = arg2 as usize as i32;
    let second_sweep_id = EXSTAR_MANAGER_SECOND_SWEEP_ID.load(Ordering::SeqCst);
    if !exstar_trace_logging_enabled() {
        if exstar_manager_should_skip_kill(&process_name, second_sweep_id) {
            return 1;
        }
        return PROCESS_MANAGER_EXE_KILL_ONE_E560
            .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
            .unwrap_or(0);
    }
    let kill_one_count = EXSTAR_MANAGER_KILL_ONE_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    let suspicious_context = second_sweep_id != 0
        && timeout == 1
        && process_name.eq_ignore_ascii_case("sn3DCommunity.exe");
    log_exstar_host(format_args!(
        "kind=manager_exe method=killOneProcess@0xe560 this={:p}[{}] process_name={} timeout={} count={} second_sweep={} arg3={:p}[{}] arg4={:p}[{}] arg5={:p}[{}]",
        this,
        describe_optional_address(this.cast_const()),
        process_name,
        timeout,
        kill_one_count,
        second_sweep_id,
        arg3,
        describe_optional_address(arg3.cast_const()),
        arg4,
        describe_optional_address(arg4.cast_const()),
        arg5,
        describe_optional_address(arg5.cast_const())
    ));
    let should_skip_kill = exstar_manager_should_skip_kill(&process_name, second_sweep_id);
    if should_skip_kill {
        let skip_status = if exstar_manager_skip_second_sweep_enabled() && second_sweep_id != 0 {
            "skipped_second_sweep"
        } else {
            "skipped"
        };
        log_exstar_host(format_args!(
            "kind=compat action=skip_manager_kill_one_process status={} process_name={} timeout={} second_sweep={} result=1",
            skip_status,
            process_name,
            timeout,
            second_sweep_id
        ));
        return 1;
    }
    if suspicious_context {
        exstar_manager_begin_sn3d_community_kill_context(kill_one_count, second_sweep_id);
    }
    let result = PROCESS_MANAGER_EXE_KILL_ONE_E560
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    if suspicious_context {
        exstar_manager_end_sn3d_community_kill_context();
    }
    log_exstar_host(format_args!(
        "kind=manager_exe method=killOneProcess@0xe560_result this={:p} process_name={} timeout={} count={} second_sweep={} result=0x{:x}",
        this,
        process_name,
        timeout,
        kill_one_count,
        second_sweep_id,
        result
    ));
    result
}

unsafe extern "system" fn zluda_process_manager_exe_load_config_ef30(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let load_config_count = EXSTAR_MANAGER_LOAD_CONFIG_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    let second_sweep = EXSTAR_MANAGER_SECOND_SWEEP_PENDING.swap(false, Ordering::SeqCst);
    let second_sweep_id = if second_sweep {
        EXSTAR_MANAGER_SECOND_SWEEP_ID.store(load_config_count, Ordering::SeqCst);
        load_config_count
    } else {
        let current = EXSTAR_MANAGER_SECOND_SWEEP_ID.load(Ordering::SeqCst);
        if current != 0 {
            current
        } else {
            0
        }
    };
    if !exstar_trace_logging_enabled() {
        return PROCESS_MANAGER_EXE_LOAD_CONFIG_EF30
            .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
            .unwrap_or(0);
    }
    let config_path =
        decode_qstring_ref(arg1.cast_const()).unwrap_or_else(|| "<unknown>".to_string());
    log_exstar_host(format_args!(
        "kind=manager_exe method=loadConfig@0xef30 this={:p}[{}] config_path={} count={} second_sweep={} arg2={:p}[{}] arg3={:p}[{}] arg4={:p}[{}] arg5={:p}[{}]",
        this,
        describe_optional_address(this.cast_const()),
        config_path,
        load_config_count,
        second_sweep_id,
        arg2,
        describe_optional_address(arg2.cast_const()),
        arg3,
        describe_optional_address(arg3.cast_const()),
        arg4,
        describe_optional_address(arg4.cast_const()),
        arg5,
        describe_optional_address(arg5.cast_const())
    ));
    let result = PROCESS_MANAGER_EXE_LOAD_CONFIG_EF30
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=manager_exe method=loadConfig@0xef30_result this={:p} config_path={} count={} second_sweep={} result=0x{:x}",
        this,
        config_path,
        load_config_count,
        second_sweep_id,
        result
    ));
    result
}

unsafe extern "system" fn zluda_process_manager_exe_handle_flow_e4b9(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_manager_handle_flow_probe("handle_flow@0xe4b9", this, arg1, arg2, arg3, arg4, arg5);
    PROCESS_MANAGER_EXE_HANDLE_FLOW_E4B9
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_manager_exe_terminate_ea6a(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_manager_handle_flow_probe("terminate@0xea6a", this, arg1, arg2, arg3, arg4, arg5);
    PROCESS_MANAGER_EXE_TERMINATE_EA6A
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_manager_exe_handle_flow_ee82(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_manager_handle_flow_probe("handle_flow@0xee82", this, arg1, arg2, arg3, arg4, arg5);
    PROCESS_MANAGER_EXE_HANDLE_FLOW_EE82
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_manager_exe_handle_flow_ef97(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_manager_handle_flow_probe("handle_flow@0xef97", this, arg1, arg2, arg3, arg4, arg5);
    PROCESS_MANAGER_EXE_HANDLE_FLOW_EF97
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_mg_plugin_connect_to_hub_72c0(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let old_module = if this.is_null() {
        ptr::null()
    } else {
        *(this.cast::<*const c_void>().add(2))
    };
    let old_control = if this.is_null() {
        ptr::null()
    } else {
        *(this.cast::<*const c_void>().add(3))
    };
    let old_control_vtable = if old_control.is_null() {
        ptr::null()
    } else {
        *(old_control.cast::<*const c_void>())
    };
    let old_release0 = if old_control_vtable.is_null() {
        ptr::null()
    } else {
        *(old_control_vtable.cast::<*const c_void>())
    };
    let old_release1 = if old_control_vtable.is_null() {
        ptr::null()
    } else {
        *(old_control_vtable.cast::<*const c_void>().add(1))
    };
    log_offset_probe(
        "processmg_connect",
        "plugin_connectToHub@0x72c0",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    log_exstar_host(format_args!(
        "kind=processmg_connect method=plugin_connectToHub@0x72c0_state this={:p} old_module={:p}[{}] old_control={:p}[{}] old_control_vtable={:p}[{}] old_release0={:p}[{}] old_release1={:p}[{}]",
        this,
        old_module,
        describe_optional_address(old_module),
        old_control,
        describe_optional_address(old_control),
        old_control_vtable,
        describe_optional_address(old_control_vtable),
        old_release0,
        describe_optional_address(old_release0),
        old_release1,
        describe_optional_address(old_release1),
    ));
    let result = PROCESS_MG_PLUGIN_CONNECT_TO_HUB_72C0
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=processmg_connect method=plugin_connectToHub@0x72c0_result this={:p} result=0x{:x}",
        this, result
    ));
    result
}

unsafe extern "system" fn zluda_process_mg_ostream_helper_3410(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    let mut matched_offset = None;
    let mut frames = [ptr::null_mut::<c_void>(); 12];
    let frame_count =
        RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), ptr::null_mut())
            as usize;
    for frame in frames[..frame_count].iter() {
        let frame = (*frame).cast_const();
        if let Some((module, base)) = module_info_from_address(frame) {
            let offset = (frame as usize).saturating_sub(base);
            if module.eq_ignore_ascii_case("Sn3DProcessPlugin.dll")
                && (0x7996..=0x7A10).contains(&offset)
            {
                matched_offset = Some(offset);
                break;
            }
        }
    }
    let text =
        decode_cstr_ptr(arg1.cast_const().cast()).unwrap_or_else(|| "<non-cstr>".to_string());
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=processmg_connect method=ostream_helper@0x3410 this={:p} text={} caller=Sn3DProcessPlugin.dll+0x{:x} arg2={:p}[{}] arg3={:p}[{}] arg4={:p}[{}] arg5={:p}[{}]",
            this,
            text,
            offset,
            arg2,
            describe_optional_address(arg2.cast_const()),
            arg3,
            describe_optional_address(arg3.cast_const()),
            arg4,
            describe_optional_address(arg4.cast_const()),
            arg5,
            describe_optional_address(arg5.cast_const()),
        ));
    }
    let result = PROCESS_MG_OSTREAM_HELPER_3410
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    if let Some(offset) = matched_offset {
        log_exstar_host(format_args!(
            "kind=processmg_connect method=ostream_helper@0x3410_result result=0x{:x} caller=Sn3DProcessPlugin.dll+0x{:x}",
            result, offset
        ));
    }
    result
}

unsafe extern "system" fn zluda_process_mg_post_connect_7996(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe(
        "processmg_connect",
        "post_connect_return_site@0x7996",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    PROCESS_MG_POST_CONNECT_7996
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_mg_post_connect_call_79a4(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe(
        "processmg_connect",
        "post_connect_helper_call@0x79a4",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    PROCESS_MG_POST_CONNECT_CALL_79A4
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0)
}

unsafe extern "system" fn zluda_process_mg_plugin_connect_impl_wrapper_6d60(
    this: *mut c_void,
    arg1: *mut c_void,
    arg2: *mut c_void,
    arg3: *mut c_void,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> usize {
    log_offset_probe(
        "processmg_connect",
        "connectImpl_wrapper@0x6d60",
        this,
        arg1,
        arg2,
        arg3,
        arg4,
        arg5,
    );
    let result = PROCESS_MG_PLUGIN_CONNECT_IMPL_WRAPPER_6D60
        .map(|original| original(this, arg1, arg2, arg3, arg4, arg5))
        .unwrap_or(0);
    log_exstar_host(format_args!(
        "kind=processmg_connect method=connectImpl_wrapper@0x6d60_result this={:p} result=0x{:x}",
        this, result
    ));
    result
}

fn exstar_navigation_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_process_mg_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_qttunnel_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_qt_widgets_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_qt_gui_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_qt_network_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_sn3dbox_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_appui_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_passport_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn exstar_exe_detours() -> &'static Mutex<FxHashMap<usize, ()>> {
    static DETOURS: OnceLock<Mutex<FxHashMap<usize, ()>>> = OnceLock::new();
    DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

unsafe fn try_attach_export<T>(
    handle: *mut c_void,
    name: &'static CStr,
    slot: *mut Option<T>,
    detour: *mut c_void,
) -> bool {
    let export = match GetProcAddress(handle, name.as_ptr().cast()) {
        Some(export) => export,
        None => {
            log_exstar_host(format_args!(
                "kind=nav hook=missing export={}",
                name.to_string_lossy()
            ));
            return true;
        }
    };
    let export_ptr = export as *const c_void;
    *slot = Some(mem::transmute_copy(&export));
    if DetourAttach(slot.cast(), detour) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=nav hook=attach_failed export={} address={:p}",
            name.to_string_lossy(),
            export_ptr
        ));
        return false;
    }
    log_exstar_host(format_args!(
        "kind=nav hook=attached export={} address={:p}",
        name.to_string_lossy(),
        export_ptr
    ));
    true
}

unsafe fn try_attach_offset<T>(
    handle: *mut c_void,
    kind: &str,
    label: &str,
    rva: usize,
    sig: &[u8],
    slot: *mut Option<T>,
    detour: *mut c_void,
) -> bool {
    let address = (handle as usize).saturating_add(rva) as *const c_void;
    let check_len = sig.len().max(16);
    if !memory_readable(address, check_len) {
        log_exstar_host(format_args!(
            "kind={} hook=offset_unreadable label={} address={}",
            kind,
            label,
            describe_address(address)
        ));
        return false;
    }
    if !sig.is_empty() {
        let actual = slice::from_raw_parts(address as *const u8, sig.len());
        if actual != sig {
            log_exstar_host(format_args!(
                "kind={} hook=offset_sig_mismatch label={} address={} expected={:02x?} actual={:02x?}",
                kind,
                label,
                describe_address(address),
                sig,
                actual
            ));
            return false;
        }
    }
    *slot = Some(mem::transmute_copy(&address));
    if DetourAttach(slot.cast(), detour) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind={} hook=offset_attach_failed label={} address={}",
            kind,
            label,
            describe_address(address)
        ));
        return false;
    }
    log_exstar_host(format_args!(
        "kind={} hook=offset_attached label={} address={}",
        kind,
        label,
        describe_address(address)
    ));
    true
}

unsafe fn detour_exstar_navigation(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_navigation_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=nav hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=nav hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"?clickLogin@Sn3DNavigationController@@QEAAXXZ",
        &raw mut NAV_CLICK_LOGIN,
        zluda_nav_click_login as _,
    ) && try_attach_export(
        handle,
        c"?login@Sn3DNavigationController@@QEAAXXZ",
        &raw mut NAV_LOGIN,
        zluda_nav_login as _,
    ) && try_attach_export(
        handle,
        c"?deviceOffline@Sn3DNavigationController@@QEAAX_N@Z",
        &raw mut NAV_DEVICE_OFFLINE,
        zluda_nav_device_offline as _,
    ) && try_attach_export(
        handle,
        c"?showAuthorPrompt@Sn3DNavigationController@@QEAAX_N@Z",
        &raw mut NAV_SHOW_AUTHOR_PROMPT,
        zluda_nav_show_author_prompt as _,
    ) && try_attach_export(
        handle,
        c"?deviceInfo@Sn3DNavigationController@@QEAAXV?$QMap@VQString@@VQVariant@@@@@Z",
        &raw mut NAV_DEVICE_INFO,
        zluda_nav_device_info as _,
    ) && try_attach_export(
        handle,
        c"?logInUserInfo@Sn3DNavigationController@@QEAAXV?$QMap@VQString@@VQVariant@@@@@Z",
        &raw mut NAV_LOGIN_USER_INFO,
        zluda_nav_log_in_user_info as _,
    ) && try_attach_export(
        handle,
        c"?qt_metacall@Sn3DNavigationController@@UEAAHW4Call@QMetaObject@@HPEAPEAX@Z",
        &raw mut NAV_QT_METACALL,
        zluda_nav_qt_metacall as _,
    ) && try_attach_export(
        handle,
        c"?qt_static_metacall@Sn3DNavigationController@@CAXPEAVQObject@@W4Call@QMetaObject@@HPEAPEAX@Z",
        &raw mut NAV_QT_STATIC_METACALL,
        zluda_nav_qt_static_metacall as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=nav hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!("kind=nav hook=complete handle={:p}", handle));
    Some(())
}

unsafe fn detour_exstar_process_mg(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_process_mg_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=processmg hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=processmg hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"?publish@processMg@Sn3DProcessMgPluginSP@@QEAA_NAEBVQString@@AEBV?$QMap@VQString@@VQVariant@@@@_N@Z",
        &raw mut PROCESS_MG_PUBLISH,
        zluda_process_mg_publish as _,
    ) && try_attach_offset(
        handle,
        "processmg_connect",
        "connectImpl_wrapper@0x6d60",
        0x6D60usize,
        &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x20, 0x4c, 0x89, 0x44, 0x24, 0x18, 0x56],
        &raw mut PROCESS_MG_PLUGIN_CONNECT_IMPL_WRAPPER_6D60,
        zluda_process_mg_plugin_connect_impl_wrapper_6d60 as _,
    ) && try_attach_offset(
        handle,
        "processmg_connect",
        "plugin_connectToHub@0x72c0",
        0x72C0usize,
        &[0x48, 0x89, 0x54, 0x24, 0x10, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57],
        &raw mut PROCESS_MG_PLUGIN_CONNECT_TO_HUB_72C0,
        zluda_process_mg_plugin_connect_to_hub_72c0 as _,
    ) && try_attach_offset(
        handle,
        "processmg",
        "connectAndRegister@0x4c50",
        0x4C50usize,
        &[0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8d, 0xac, 0x24],
        &raw mut PROCESS_MG_CONNECT_AND_REGISTER,
        zluda_process_mg_connect_and_register as _,
    ) && try_attach_export(
        handle,
        c"?registerSubscribe@processMg@Sn3DProcessMgPluginSP@@AEAA_NV?$QMap@VQString@@VQVariant@@@@@Z",
        &raw mut PROCESS_MG_REGISTER_SUBSCRIBE,
        zluda_process_mg_register_subscribe as _,
    ) && try_attach_export(
        handle,
        c"?subPubConnectToHub@processMg@Sn3DProcessMgPluginSP@@AEAA_NV?$QMap@VQString@@VQVariant@@@@PEAVQObject@@@Z",
        &raw mut PROCESS_MG_SUBPUB_CONNECT,
        zluda_process_mg_subpub_connect_to_hub as _,
    ) && try_attach_export(
        handle,
        c"?signal_published@processMg@Sn3DProcessMgPluginSP@@QEAA_NVQString@@0V?$QMap@VQString@@VQVariant@@@@@Z",
        &raw mut PROCESS_MG_SIGNAL_PUBLISHED,
        zluda_process_mg_signal_published as _,
    ) && try_attach_export(
        handle,
        c"?qt_metacall@processMg@Sn3DProcessMgPluginSP@@UEAAHW4Call@QMetaObject@@HPEAPEAX@Z",
        &raw mut PROCESS_MG_QT_METACALL,
        zluda_process_mg_qt_metacall as _,
    ) && try_attach_export(
        handle,
        c"?qt_static_metacall@processMg@Sn3DProcessMgPluginSP@@CAXPEAVQObject@@W4Call@QMetaObject@@HPEAPEAX@Z",
        &raw mut PROCESS_MG_QT_STATIC_METACALL,
        zluda_process_mg_qt_static_metacall as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=processmg hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=processmg hook=complete handle={:p}",
        handle
    ));
    Some(())
}

/// Hook Sn3DDeviceEinStar.dll to bypass the device cleanup deadlock.
/// The stop() function waits on a condition variable that never gets signaled
/// when no physical EinStar scanner is connected, blocking scanservice startup.
unsafe fn detour_exstar_device_einstar(handle: *mut c_void) -> Option<()> {
    log_exstar_host(format_args!(
        "kind=module_present module=Sn3DDeviceEinStar.dll handle={:p}",
        handle
    ));
    // Hook Sn3DDeviceBase::stop to return immediately
    let stop_export = c"?stop@Sn3DDeviceBase@@UEAA?AW4Sn3DErrorCode@CommonLib@@XZ";
    if let Some(stop_fn) = GetProcAddress(handle as _, stop_export.as_ptr().cast()) {
        SN3D_DEVICE_STOP = Some(std::mem::transmute(stop_fn));
        if DetourTransactionBegin() == NO_ERROR as i32 {
            DetourAttach(
                &raw mut SN3D_DEVICE_STOP as *mut _ as *mut *mut c_void,
                zluda_sn3d_device_stop as *mut c_void,
            );
            DetourTransactionCommit();
        }
        log_exstar_host(format_args!(
            "kind=compat action=device_stop_hooked address={:p}",
            stop_fn as *const c_void
        ));
    }
    Some(())
}

#[allow(non_snake_case)]
unsafe extern "system" fn zluda_sn3d_device_stop(this: *mut c_void) -> u32 {
    // Return 0 (success) immediately instead of blocking on condvar.
    // This prevents the deadlock in Sn3DDeviceBase::~Sn3DDeviceBase → stop → _Cnd_wait
    // that occurs when no physical EinStar scanner is connected.
    log_exstar_host(format_args!(
        "kind=compat action=device_stop_bypass this={:p}",
        this
    ));
    0 // Sn3DErrorCode::Success
}

unsafe fn detour_exstar_qttunnel(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_qttunnel_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qttunnel hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qttunnel hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attach_connect = if exstar_skip_qttunnel_connect_hook_enabled() {
        log_exstar_host(format_args!(
            "kind=qttunnel hook=skipped export=connectToHub reason=env-flag"
        ));
        true
    } else {
        try_attach_export(
            handle,
            c"?connectToHub@Module@QtTunnel@@QEAAXXZ",
            &raw mut QTTUNNEL_MODULE_CONNECT,
            zluda_qttunnel_module_connect as _,
        )
    };
    let attached = try_attach_export(
        handle,
        c"??0Module@QtTunnel@@QEAA@AEBVQString@@0AEBVQByteArray@@AEBVQHostAddress@@GPEAVQObject@@@Z",
        &raw mut QTTUNNEL_MODULE_CTOR,
        zluda_qttunnel_module_ctor as _,
    ) && try_attach_export(
        handle,
        c"??1Module@QtTunnel@@UEAA@XZ",
        &raw mut QTTUNNEL_MODULE_DTOR,
        zluda_qttunnel_module_dtor as _,
    ) && attach_connect
        && try_attach_export(
        handle,
        c"?connectToHub@Module@QtTunnel@@QEAA_NH@Z",
        &raw mut QTTUNNEL_MODULE_CONNECT_WITH_INT,
        zluda_qttunnel_module_connect_with_int as _,
    ) && try_attach_export(
        handle,
        c"?isConnected@Module@QtTunnel@@QEBA_NXZ",
        &raw mut QTTUNNEL_MODULE_IS_CONNECTED,
        zluda_qttunnel_module_is_connected as _,
    ) && try_attach_export(
        handle,
        c"?publish@Module@QtTunnel@@QEAA_NAEBVQString@@AEBV?$QMap@VQString@@VQVariant@@@@_N@Z",
        &raw mut QTTUNNEL_MODULE_PUBLISH,
        zluda_qttunnel_module_publish as _,
    ) && try_attach_export(
        handle,
        c"?published@Module@QtTunnel@@QEAAXVQString@@0V?$QMap@VQString@@VQVariant@@@@@Z",
        &raw mut QTTUNNEL_MODULE_PUBLISHED,
        zluda_qttunnel_module_published as _,
    ) && try_attach_export(
        handle,
        c"?qt_metacall@Module@QtTunnel@@UEAAHW4Call@QMetaObject@@HPEAPEAX@Z",
        &raw mut QTTUNNEL_MODULE_QT_METACALL,
        zluda_qttunnel_module_qt_metacall as _,
    ) && try_attach_export(
        handle,
        c"?qt_static_metacall@Module@QtTunnel@@CAXPEAVQObject@@W4Call@QMetaObject@@HPEAPEAX@Z",
        &raw mut QTTUNNEL_MODULE_QT_STATIC_METACALL,
        zluda_qttunnel_module_qt_static_metacall as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qttunnel hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=qttunnel hook=complete handle={:p}",
        handle
    ));
    Some(())
}

unsafe fn detour_exstar_qt_widgets(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_qt_widgets_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtwidget hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtwidget hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"?exec@QApplication@@SAHXZ",
        &raw mut QT_APPLICATION_EXEC,
        zluda_qapplication_exec as _,
    ) && try_attach_export(
        handle,
        c"?hide@QWidget@@QEAAXXZ",
        &raw mut QT_WIDGET_HIDE,
        zluda_qwidget_hide as _,
    ) && try_attach_export(
        handle,
        c"?show@QWidget@@QEAAXXZ",
        &raw mut QT_WIDGET_SHOW,
        zluda_qwidget_show as _,
    ) && try_attach_export(
        handle,
        c"?close@QWidget@@QEAA_NXZ",
        &raw mut QT_WIDGET_CLOSE,
        zluda_qwidget_close as _,
    ) && try_attach_export(
        handle,
        c"?setVisible@QWidget@@UEAAX_N@Z",
        &raw mut QT_WIDGET_SET_VISIBLE,
        zluda_qwidget_set_visible as _,
    ) && try_attach_export(
        handle,
        c"?event@QWidget@@MEAA_NPEAVQEvent@@@Z",
        &raw mut QT_WIDGET_EVENT,
        zluda_qwidget_event as _,
    );
    // QDialog::exec hook removed — PrestartCheck.dll is binary-patched to skip
    // its GPU check, so we don't need to intercept QDialog::exec. The hook was
    // causing stack corruption (0xc0000409 crashes).
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtwidget hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=qtwidget hook=complete handle={:p}",
        handle
    ));
    Some(())
}

unsafe fn detour_exstar_qt_gui(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_qt_gui_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qwindow hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qwindow hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"?hide@QWindow@@QEAAXXZ",
        &raw mut QT_WINDOW_HIDE,
        zluda_qwindow_hide as _,
    ) && try_attach_export(
        handle,
        c"?show@QWindow@@QEAAXXZ",
        &raw mut QT_WINDOW_SHOW,
        zluda_qwindow_show as _,
    ) && try_attach_export(
        handle,
        c"?close@QWindow@@QEAA_NXZ",
        &raw mut QT_WINDOW_CLOSE,
        zluda_qwindow_close as _,
    ) && try_attach_export(
        handle,
        c"?setVisible@QWindow@@QEAAX_N@Z",
        &raw mut QT_WINDOW_SET_VISIBLE,
        zluda_qwindow_set_visible as _,
    ) && try_attach_export(
        handle,
        c"?event@QWindow@@MEAA_NPEAVQEvent@@@Z",
        &raw mut QT_WINDOW_EVENT,
        zluda_qwindow_event as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qwindow hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=qwindow hook=complete handle={:p}",
        handle
    ));
    Some(())
}

unsafe fn detour_exstar_qt_core(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_qt_gui_detours().lock().ok()?;
    let handle_key = (handle as usize) ^ 0x5154434Fusize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtcore hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtcore hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"??1Connection@QMetaObject@@QEAA@XZ",
        &raw mut QT_CONNECTION_DTOR,
        zluda_qmetaobject_connection_dtor as _,
    ) && try_attach_export(
        handle,
        c"?msleep@QThread@@SAXK@Z",
        &raw mut QT_THREAD_MSLEEP,
        zluda_qthread_msleep as _,
    ) && try_attach_export(
        handle,
        c"?singleShotImpl@QTimer@@CAXHW4TimerType@Qt@@PEBVQObject@@PEAVQSlotObjectBase@QtPrivate@@@Z",
        &raw mut QT_TIMER_SINGLESHOT_IMPL,
        zluda_qtimer_singleshot_impl as _,
    ) && try_attach_export(
        handle,
        c"?invokeMethod@QMetaObject@@SA_NPEAVQObject@@PEBDW4ConnectionType@Qt@@VQGenericArgument@@333333333@Z",
        &raw mut QT_METAOBJECT_INVOKE_METHOD_WITH_TYPE,
        zluda_qmetaobject_invoke_method_with_type as _,
    ) && try_attach_export(
        handle,
        c"?quit@QCoreApplication@@SAXXZ",
        &raw mut QT_CORE_APPLICATION_QUIT,
        zluda_qcoreapplication_quit as _,
    ) && try_attach_export(
        handle,
        c"?exit@QCoreApplication@@SAXH@Z",
        &raw mut QT_CORE_APPLICATION_EXIT,
        zluda_qcoreapplication_exit as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtcore hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=qtcore hook=complete handle={:p}",
        handle
    ));
    Some(())
}

unsafe fn detour_exstar_qt_network(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_qt_network_detours().lock().ok()?;
    let handle_key = (handle as usize) ^ 0x51544E45usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtnetwork hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtnetwork hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"??0QHostAddress@@QEAA@W4SpecialAddress@0@@Z",
        &raw mut QT_HOST_ADDRESS_CTOR,
        zluda_qhostaddress_ctor as _,
    ) && try_attach_export(
        handle,
        c"??1QHostAddress@@QEAA@XZ",
        &raw mut QT_HOST_ADDRESS_DTOR,
        zluda_qhostaddress_dtor as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=qtnetwork hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=qtnetwork hook=complete handle={:p}",
        handle
    ));
    Some(())
}

unsafe fn detour_exstar_sn3dbox(handle: *mut c_void) -> Option<()> {
    let mut detours = exstar_sn3dbox_detours().lock().ok()?;
    let handle_key = (handle as usize) ^ 0x53334258usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    if DetourTransactionBegin() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=sn3dbox hook=begin_failed handle={:p}",
            handle
        ));
        return None;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=sn3dbox hook=update_thread_failed handle={:p}",
            handle
        ));
        DetourTransactionAbort();
        return None;
    }
    let attached = try_attach_export(
        handle,
        c"qt_plugin_instance",
        &raw mut SN3DBOX_PLUGIN_INSTANCE,
        zluda_sn3dbox_plugin_instance as _,
    ) && try_attach_export(
        handle,
        c"?init@Sn3DApplication@@QEAAXPEAVQObject@@@Z",
        &raw mut SN3DBOX_APP_INIT,
        zluda_sn3dbox_application_init as _,
    ) && try_attach_export(
        handle,
        c"?load@Sn3DApplication@@UEAAXAEBVQString@@PEAVQObject@@@Z",
        &raw mut SN3DBOX_APP_LOAD,
        zluda_sn3dbox_application_load as _,
    ) && try_attach_export(
        handle,
        c"?qmlItem@Sn3DUICpp@CommonLib@@QEAAPEAVQObject@@XZ",
        &raw mut SN3DBOX_UI_QML_ITEM,
        zluda_sn3dbox_ui_qml_item as _,
    ) && try_attach_export(
        handle,
        c"?setQmlItem@Sn3DUICpp@CommonLib@@QEAAXPEAVQObject@@@Z",
        &raw mut SN3DBOX_UI_SET_QML_ITEM,
        zluda_sn3dbox_ui_set_qml_item as _,
    ) && try_attach_export(
        handle,
        c"?start@Sn3DUICpp@CommonLib@@UEAA?AW4Sn3DErrorCode@2@XZ",
        &raw mut SN3DBOX_UI_START,
        zluda_sn3dbox_ui_start as _,
    ) && try_attach_export(
        handle,
        c"?stop@Sn3DUICpp@CommonLib@@UEAA?AW4Sn3DErrorCode@2@XZ",
        &raw mut SN3DBOX_UI_STOP,
        zluda_sn3dbox_ui_stop as _,
    );
    if !attached {
        DetourTransactionAbort();
        return None;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        log_exstar_host(format_args!(
            "kind=sn3dbox hook=commit_failed handle={:p}",
            handle
        ));
        return None;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=sn3dbox hook=complete handle={:p}",
        handle
    ));
    Some(())
}

unsafe fn detour_exstar_appui(handle: *mut c_void) -> Option<()> {
    if !exstar_appui_trace_enabled() {
        return Some(());
    }
    let mut detours = exstar_appui_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    let probes = [
        (
            "handleShowPassport_4dd96",
            0x4DD96usize,
            &[
                0x41u8, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8b, 0xec, 0x48, 0x83, 0xec, 0x70, 0x48, 0xc7, 0x45,
                0xc0, 0xfe, 0xff, 0xff, 0xff, 0x48, 0x89, 0x58, 0x08, 0x48, 0x89, 0x70, 0x18, 0x48, 0x89, 0x78,
            ] as &[u8],
            &raw mut APPUI_HANDLE_SHOW_PASSPORT,
            zluda_appui_handle_show_passport as *mut c_void,
        ),
    ];
    let mut attached_any = false;
    for (label, rva, sig, slot, detour) in probes {
        if DetourTransactionBegin() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=appui hook=begin_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=appui hook=update_thread_failed label={} handle={:p}",
                label, handle
            ));
            DetourTransactionAbort();
            continue;
        }
        if !try_attach_offset(handle, "appui", label, rva, sig, slot, detour) {
            DetourTransactionAbort();
            continue;
        }
        if DetourTransactionCommit() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=appui hook=commit_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        attached_any = true;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=appui hook=complete handle={:p} attached_any={}",
        handle, attached_any
    ));
    Some(())
}

unsafe fn detour_exstar_passport(handle: *mut c_void) -> Option<()> {
    if !exstar_appui_trace_enabled() {
        return Some(());
    }
    let mut detours = exstar_passport_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    let probes = [
        (
            "handleShowPassportCmd",
            0x3BFC1usize,
            &[0x48u8, 0x8d, 0x8d, 0xa8, 0x00, 0x00, 0x00, 0xff, 0x15, 0xea, 0xb2, 0x01, 0x00, 0x48, 0x8b, 0xd8][..],
            &raw mut PASSPORT_HANDLE_SHOW_PASSPORT_CMD,
            zluda_passport_handle_show_passport_cmd as *mut c_void,
        ),
        (
            "handleLoginSuccess",
            0x39ED0usize,
            &[0x40u8, 0x53, 0x48, 0x83, 0xec, 0x50, 0x48, 0xc7, 0x44, 0x24, 0x20, 0xfe, 0xff, 0xff, 0xff, 0x48][..],
            &raw mut PASSPORT_HANDLE_LOGIN_SUCCESS,
            zluda_passport_handle_login_success as *mut c_void,
        ),
    ];
    let mut attached_any = false;
    for (label, rva, sig, slot, detour) in probes {
        if DetourTransactionBegin() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=passport hook=begin_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=passport hook=update_thread_failed label={} handle={:p}",
                label, handle
            ));
            DetourTransactionAbort();
            continue;
        }
        if !try_attach_offset(handle, "passport", label, rva, sig, slot, detour) {
            DetourTransactionAbort();
            continue;
        }
        if DetourTransactionCommit() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=passport hook=commit_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        attached_any = true;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=passport hook=complete handle={:p} attached_any={}",
        handle, attached_any
    ));
    Some(())
}

unsafe fn detour_exstar_exe(handle: *mut c_void) -> Option<()> {
    if !exstar_exe_trace_enabled() || !exstar_window_trace_enabled() {
        return Some(());
    }
    let mut detours = exstar_exe_detours().lock().ok()?;
    let handle_key = handle as usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    let probes = [
        (
            "entry_6940",
            0x6940usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_6940,
            zluda_exstar_exe_6940 as *mut c_void,
        ),
        (
            "lambda_6dc0",
            0x6DC0usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_6DC0,
            zluda_exstar_exe_6dc0 as *mut c_void,
        ),
        (
            "signal_slot_bc30",
            0xBC30usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_BC30,
            zluda_exstar_exe_bc30 as *mut c_void,
        ),
        (
            "entry_f0f8",
            0xF0F8usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_F0F8,
            zluda_exstar_exe_f0f8 as *mut c_void,
        ),
        (
            "wrapper_f9ec",
            0xF9ECusize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_F9EC,
            zluda_exstar_exe_f9ec as *mut c_void,
        ),
        (
            "wrapper_fac4",
            0xFAC4usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_FAC4,
            zluda_exstar_exe_fac4 as *mut c_void,
        ),
        (
            "init_check_f6c0",
            0xF6C0usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_F6C0,
            zluda_exstar_exe_f6c0 as *mut c_void,
        ),
        (
            "post_d070_check_10390",
            0x10390usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_10390,
            zluda_exstar_exe_10390 as *mut c_void,
        ),
        (
            "guard_a6e0",
            0xA6E0usize,
            &[] as &[u8],
            &raw mut EXSTAR_EXE_A6E0,
            zluda_exstar_exe_a6e0 as *mut c_void,
        ),
    ];
    let mut attached_any = false;
    for (label, rva, sig, slot, detour) in probes {
        if DetourTransactionBegin() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=exe hook=begin_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=exe hook=update_thread_failed label={} handle={:p}",
                label, handle
            ));
            DetourTransactionAbort();
            continue;
        }
        if !try_attach_offset(handle, "exe", label, rva, sig, slot, detour) {
            DetourTransactionAbort();
            continue;
        }
        if DetourTransactionCommit() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=exe hook=commit_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        attached_any = true;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=exe hook=complete handle={:p} attached_any={}",
        handle, attached_any
    ));
    Some(())
}

unsafe fn detour_process_manager_exe(handle: *mut c_void) -> Option<()> {
    let Some((_, current_exe_name)) = exstar_current_exe("manager_exe_trace") else {
        return Some(());
    };
    let manager_trace_enabled = exstar_exe_trace_enabled();
    let manager_compat_enabled = exstar_manager_compat_hooks_enabled();
    if !current_exe_name.eq_ignore_ascii_case("Sn3DprocessManager.exe")
        || (!manager_trace_enabled && !manager_compat_enabled)
    {
        return Some(());
    }
    let mut detours = exstar_exe_detours().lock().ok()?;
    let handle_key = (handle as usize) ^ 0x53504D47usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        return Some(());
    }
    let host_trace_enabled = exstar_host_trace_enabled();
    let light_trace_enabled = exstar_light_trace_enabled();
    let keep_second_sweep_hooks = exstar_manager_skip_second_sweep_enabled() || light_trace_enabled;
    let mut probes: Vec<(&'static str, usize, &'static [u8], *mut Option<OffsetTraceFn>, *mut c_void)> =
        Vec::new();
    if host_trace_enabled || keep_second_sweep_hooks {
        probes.extend([
            (
                "kill_all_e1a0",
                0xE1A0usize,
                &[0x48u8, 0x8b, 0xc4, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8d, 0x68, 0xa8][..],
                &raw mut PROCESS_MANAGER_EXE_KILL_ALL_E1A0,
                zluda_process_manager_exe_kill_all_e1a0 as *mut c_void,
            ),
            (
                "load_config_ef30",
                0xEF30usize,
                &[0x40u8, 0x55, 0x56, 0x57, 0x48, 0x81, 0xec, 0xa0, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x44, 0x24, 0x28][..],
                &raw mut PROCESS_MANAGER_EXE_LOAD_CONFIG_EF30,
                zluda_process_manager_exe_load_config_ef30 as *mut c_void,
            ),
        ]);
    }
    probes.push((
        "kill_one_e560",
        0xE560usize,
        &[0x40u8, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8d, 0xac, 0x24][..],
        &raw mut PROCESS_MANAGER_EXE_KILL_ONE_E560,
        zluda_process_manager_exe_kill_one_e560 as *mut c_void,
    ));
    // Hook the MASTER environment detection function at +0x5e30.
    // This function contains BOTH the OpenGL check (TestOpenglHelper) AND the
    // Windows Media Player check (QMediaPlayer::error()==5). Both fail under ZLUDA.
    // Hooking at +0x5e30 (returns bool) bypasses ALL environment checks at once.
    probes.push((
        "env_detect_5e30",
        0x5E30usize,
        &[0x48u8, 0x8b, 0xc4, 0x55, 0x48, 0x8d, 0x68, 0xd8, 0x48, 0x81, 0xec, 0x20, 0x01, 0x00, 0x00, 0x48][..],
        &raw mut PROCESS_MANAGER_CHECK_OPENGL,
        zluda_process_manager_check_opengl as *mut c_void,
    ));
    if host_trace_enabled || light_trace_enabled {
        probes.push((
            "launch_f5f0",
            0xF5F0usize,
            &[0x40u8, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8d, 0xac, 0x24][..],
            &raw mut PROCESS_MANAGER_EXE_F5F0,
            zluda_process_manager_exe_f5f0 as *mut c_void,
        ));
    }
    let mut attached_any = false;
    for (label, rva, sig, slot, detour) in probes {
        if DetourTransactionBegin() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=manager_exe hook=begin_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=manager_exe hook=update_thread_failed label={} handle={:p}",
                label, handle
            ));
            DetourTransactionAbort();
            continue;
        }
        if !try_attach_offset(handle, "manager_exe", label, rva, sig, slot, detour) {
            DetourTransactionAbort();
            continue;
        }
        if DetourTransactionCommit() != NO_ERROR as i32 {
            log_exstar_host(format_args!(
                "kind=manager_exe hook=commit_failed label={} handle={:p}",
                label, handle
            ));
            continue;
        }
        attached_any = true;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=manager_exe hook=complete handle={:p} attached_any={}",
        handle, attached_any
    ));
    Some(())
}

unsafe fn detour_scanservice_exe(handle: *mut c_void) -> Option<()> {
    log_exstar_host(format_args!("kind=scanservice hook=start handle={:p}", handle));
    let Some((_, current_exe_name)) = exstar_current_exe("scanservice_trace") else {
        log_exstar_host(format_args!("kind=scanservice hook=abort reason=current_exe_failed"));
        return Some(());
    };
    if !current_exe_name.eq_ignore_ascii_case("scanservice.exe") {
        log_exstar_host(format_args!("kind=scanservice hook=abort reason=wrong_exe name={}", current_exe_name));
        return Some(());
    }
    let Some(mut detours) = exstar_exe_detours().lock().ok() else {
        log_exstar_host(format_args!("kind=scanservice hook=abort reason=lock_poisoned"));
        return None;
    };
    let handle_key = (handle as usize) ^ 0x6A40usize;
    if let hash_map::Entry::Occupied(_) = detours.entry(handle_key) {
        log_exstar_host(format_args!("kind=scanservice hook=abort reason=already_occupied handle_key={:x}", handle_key));
        return Some(());
    }
    let probes = [
        (
            "entry_6a40",
            0x6A40usize,
            &[0x48u8, 0x83, 0xec, 0x28, 0xe8, 0xa3, 0x0a, 0x00, 0x00, 0x48, 0x83, 0xc4, 0x28, 0xe9, 0xfe, 0xfd][..],
            &raw mut SCANSERVICE_EXE_ENTRY_6A40,
            zluda_scanservice_exe_entry_6a40 as *mut c_void,
        ),
        // REMOVED: Mid-function probes at 0x44ec, 0x47e4, 0x4a24, 0x4e4b
        // These CORRUPT the instruction stream — Detours can only hook function entries.
        // The corrupted code prevented connectAndRegister from ever being reached.
    ];
    let mut attached_any = false;
    for (label, rva, sig, slot, detour) in probes {
        if DetourTransactionBegin() != NO_ERROR as i32 { continue; }
        if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
            DetourTransactionAbort();
            continue;
        }
        if !try_attach_offset(handle, "scanservice", label, rva, sig, slot, detour) {
            DetourTransactionAbort();
            continue;
        }
        if DetourTransactionCommit() != NO_ERROR as i32 { continue; }
        attached_any = true;
    }
    detours.insert(handle_key, ());
    log_exstar_host(format_args!(
        "kind=scanservice hook=complete handle={:p} attached_any={}",
        handle, attached_any
    ));
    Some(())
}


fn dll_file_name(dll_name_arg: *const UNICODE_STRING) -> Result<Vec<u16>, NTSTATUS> {
    let dll_name = unsafe { dll_name_arg.as_ref() }.ok_or(STATUS_INVALID_PARAMETER_3)?;
    let dll_name =
        unsafe { slice::from_raw_parts(dll_name.Buffer.0, (dll_name.Length as usize) / 2) };
    let file_name_length = dll_name
        .iter()
        .copied()
        .rev()
        .position(|c| path::is_separator(char::from_u32(c as u32).unwrap_or(char::MIN)));
    let dll_name = match file_name_length {
        Some(file_name_length) => dll_name.split_at(dll_name.len() - file_name_length).1,
        None => dll_name,
    };
    Ok(dll_name.to_vec())
}


unsafe fn exstar_host_trace_on_load(
    dll_name_arg: *const UNICODE_STRING,
    handle: *mut c_void,
    result: NTSTATUS,
) {
    let host_trace_enabled = exstar_host_trace_enabled();
    let hub_quit_compat_enabled = exstar_hub_quit_compat_enabled();
    let hub_light_trace_enabled = exstar_hub_light_trace_enabled();
    if !result.is_ok()
        || (!host_trace_enabled && !hub_quit_compat_enabled && !hub_light_trace_enabled)
    {
        return;
    }
    let dll_name = match dll_file_name(dll_name_arg) {
        Ok(name) => String::from_utf16_lossy(&name),
        Err(_) => return,
    };
    let is_target = matches!(
        dll_name.as_str(),
        name if name.eq_ignore_ascii_case("AppUi.dll")
            || name.eq_ignore_ascii_case("Sn3DUserPassport.dll")
            || name.eq_ignore_ascii_case("libSn3DNavigation.dll")
            || name.eq_ignore_ascii_case("PrestartCheck.dll")
            || name.eq_ignore_ascii_case("Sn3DProcessPlugin.dll")
            || name.eq_ignore_ascii_case("Sn3DBox.dll")
            || name.eq_ignore_ascii_case("Qt5Core.dll")
            || name.eq_ignore_ascii_case("Qt5Network.dll")
            || name.eq_ignore_ascii_case("Qt5Widgets.dll")
            || name.eq_ignore_ascii_case("Qt5Gui.dll")
            || name.eq_ignore_ascii_case("qttunnel.3.2.7.dll")
    );
    if !is_target {
        return;
    }
    if host_trace_enabled {
        log_exstar_host(format_args!(
            "kind=module_load module={} handle={:p}",
            dll_name, handle
        ));
    }
    if dll_name.eq_ignore_ascii_case("PrestartCheck.dll") {
        exstar_patch_prestartcheck_module(handle);
    }
    if dll_name.eq_ignore_ascii_case("libSn3DNavigation.dll") {
        let _ = detour_exstar_navigation(handle);
    } else if dll_name.eq_ignore_ascii_case("AppUi.dll") {
        let _ = detour_exstar_appui(handle);
    } else if dll_name.eq_ignore_ascii_case("Sn3DUserPassport.dll") {
        let _ = detour_exstar_passport(handle);
    } else if dll_name.eq_ignore_ascii_case("Sn3DProcessPlugin.dll") {
        let _ = detour_exstar_process_mg(handle);
    } else if dll_name.eq_ignore_ascii_case("Sn3DBox.dll") {
        let _ = detour_exstar_sn3dbox(handle);
    } else if dll_name.eq_ignore_ascii_case("Qt5Core.dll") {
        let _ = detour_exstar_qt_core(handle);
    } else if dll_name.eq_ignore_ascii_case("Qt5Network.dll") {
        let _ = detour_exstar_qt_network(handle);
    } else if dll_name.eq_ignore_ascii_case("Qt5Widgets.dll") {
        let _ = detour_exstar_qt_widgets(handle);
    } else if dll_name.eq_ignore_ascii_case("Qt5Gui.dll") {
        let _ = detour_exstar_qt_gui(handle);
    } else if dll_name.eq_ignore_ascii_case("qttunnel.3.2.7.dll") {
        let _ = detour_exstar_qttunnel(handle);
    }
}

fn exstar_spawn_warning_dialog_closer() {
    use std::sync::atomic::AtomicBool;
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return; // already running
    }
    // Run in EXStar Hub and manager processes
    if let Some((_, name)) = exstar_current_exe("dialog_closer") {
        if !name.eq_ignore_ascii_case("Sn3DprocessManager.exe")
            && !name.eq_ignore_ascii_case("EXStar Hub.exe")
        {
            return;
        }
    } else {
        return;
    }
    thread::spawn(|| {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GetClassNameW, GetWindowTextW,
            GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
        };
        let my_pid = unsafe { GetCurrentProcessId() };
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(500));
            unsafe {
                unsafe extern "system" fn check_window(hwnd: HWND, lparam: isize) -> BOOL {
                    let my_pid = lparam as u32;
                    let mut window_pid = 0u32;
                    GetWindowThreadProcessId(hwnd, &mut window_pid);
                    if window_pid != my_pid || IsWindowVisible(hwnd) == 0 {
                        return 1;
                    }
                    // Check the window class — Qt dialogs use class names like
                    // "Qt5QWindowIcon" or similar
                    let mut class_buf = [0u16; 256];
                    let class_len =
                        GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
                    let class_name = if class_len > 0 {
                        String::from_utf16_lossy(&class_buf[..class_len as usize])
                    } else {
                        String::new()
                    };
                    let mut title_buf = [0u16; 256];
                    let title_len =
                        GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
                    let title = if title_len > 0 {
                        String::from_utf16_lossy(&title_buf[..title_len as usize])
                    } else {
                        String::new()
                    };
                    let mut rect = RECT::default();
                    let has_rect = GetWindowRect(hwnd, &mut rect) != 0;
                    let width = if has_rect {
                        rect.right.saturating_sub(rect.left)
                    } else {
                        0
                    };
                    let height = if has_rect {
                        rect.bottom.saturating_sub(rect.top)
                    } else {
                        0
                    };
                    // Look for visible windows with empty or "Sn3DprocessManager" title
                    // that have Qt class names — these are likely error dialogs
                    // Close any Qt dialog that contains error text — including
                    // PrestartCheck dialogs that show "Software error code" behind
                    // the splash screen, blocking the entire startup.
                    if class_name.contains("Qt") {
                        let title_lower = title.to_ascii_lowercase();
                        let title_matches = title_lower.contains("warning")
                            || title_lower.contains("confirm")
                            || title_lower.contains("error");
                        // Check child windows for "Warning" text or error content
                        // by looking at child static/label widgets
                        static FOUND_ERROR_CHILD: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(false);
                        FOUND_ERROR_CHILD.store(false, Ordering::SeqCst);
                        unsafe extern "system" fn check_child(
                            child: HWND,
                            _lparam: isize,
                        ) -> BOOL {
                            let mut buf = [0u16; 512];
                            let len =
                                GetWindowTextW(child, buf.as_mut_ptr(), buf.len() as i32);
                            if len > 0 {
                                let text = String::from_utf16_lossy(&buf[..len as usize]);
                                if text.contains("Warning")
                                    || text.contains("error code")
                                    || text.contains("something went wrong")
                                    || text.contains("repeat opening")
                                    || text.contains("graphics card")
                                    || text.contains("NVIDIA")
                                    || text.contains("dedicated GPU")
                                    || text.contains("Confirm")
                                {
                                    FOUND_ERROR_CHILD.store(true, Ordering::SeqCst);
                                    return 0; // stop enumeration
                                }
                            }
                            1
                        }
                        EnumChildWindows(hwnd, Some(check_child), 0);
                        let child_matches = FOUND_ERROR_CHILD.load(Ordering::SeqCst);
                        if title_matches || child_matches {
                            log_exstar_host(format_args!(
                                "kind=compat action=auto_close_error_dialog hwnd={:p} class=\"{}\" title=\"{}\" pid={} width={} height={} title_matches={} child_matches={}",
                                hwnd as *mut c_void,
                                class_name,
                                title,
                                my_pid,
                                width,
                                height,
                                title_matches,
                                child_matches
                            ));
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                        }
                    }
                    1
                }
                EnumWindows(Some(check_window), my_pid as isize);
            }
        }
    });
}

fn exstar_spawn_child_hub_shutdown_bridge() {
    use std::sync::atomic::AtomicBool;
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !exstar_is_child_hub_process() {
        return;
    }
    thread::spawn(|| unsafe {
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut observed_manager_pid = exstar_child_hub_manager_pid_from_args();
        let mut close_posted = false;
        while Instant::now() < deadline && !close_posted {
            thread::sleep(Duration::from_millis(250));
            let app_window_shown = EXSTAR_CHILD_HUB_APP_WINDOW_SHOWN.load(Ordering::SeqCst)
                || exstar_child_hub_real_app_window_exists();
            if !app_window_shown {
                continue;
            }
            match exstar_hub_related_manager() {
                Some((_, manager_pid)) => {
                    observed_manager_pid = Some(manager_pid);
                }
                None => {
                    if let Some(manager_pid) = observed_manager_pid {
                        if exstar_process_id_exists(manager_pid) {
                            continue;
                        }
                        let hwnd = exstar_child_hub_real_app_window();
                        if hwnd.is_null() {
                            continue;
                        }
                        let title = read_window_text(hwnd);
                        let post_result = windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                            hwnd,
                            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                            0,
                            0,
                        );
                        log_exstar_host(format_args!(
                            "kind=compat action=child_hub_shutdown_bridge manager_pid={} hwnd={:p} title=\"{}\" post_result={}",
                            manager_pid,
                            hwnd as *mut c_void,
                            title,
                            post_result
                        ));
                        if let Some(original_exit) = QT_CORE_APPLICATION_EXIT {
                            log_exstar_host(format_args!(
                                "kind=compat action=child_hub_shutdown_bridge_qt_exit manager_pid={} method=QCoreApplication::exit",
                                manager_pid
                            ));
                            original_exit(0);
                        } else if let Some(original_quit) = QT_CORE_APPLICATION_QUIT {
                            log_exstar_host(format_args!(
                                "kind=compat action=child_hub_shutdown_bridge_qt_exit manager_pid={} method=QCoreApplication::quit",
                                manager_pid
                            ));
                            original_quit();
                        }
                        thread::sleep(Duration::from_millis(500));
                        log_exstar_host(format_args!(
                            "kind=compat action=child_hub_shutdown_bridge_force_exit manager_pid={} exit_code=0",
                            manager_pid
                        ));
                        EXIT_PROCESS_FN(0);
                        close_posted = true;
                    }
                }
            }
        }
    });
}

unsafe fn exstar_host_trace_existing_modules() {
    exstar_spawn_child_hub_shutdown_bridge();
    let host_trace_enabled = exstar_host_trace_enabled();
    let hub_quit_compat_enabled = exstar_hub_quit_compat_enabled();
    let hub_light_trace_enabled = exstar_hub_light_trace_enabled();
    let manager_compat_enabled = exstar_manager_compat_hooks_enabled();
    if !host_trace_enabled
        && !hub_quit_compat_enabled
        && !hub_light_trace_enabled
        && !manager_compat_enabled
    {
        return;
    }
    
    let mut modules: Vec<(&CStr, unsafe fn(*mut c_void) -> Option<()>)> = Vec::new();
    if host_trace_enabled {
        modules.extend([
            (
                c"AppUi.dll",
                detour_exstar_appui as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Sn3DUserPassport.dll",
                detour_exstar_passport as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Sn3DProcessPlugin.dll",
                detour_exstar_process_mg as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Qt5Core.dll",
                detour_exstar_qt_core as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Qt5Network.dll",
                detour_exstar_qt_network as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Sn3DBox.dll",
                detour_exstar_sn3dbox as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Qt5Widgets.dll",
                detour_exstar_qt_widgets as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"Qt5Gui.dll",
                detour_exstar_qt_gui as unsafe fn(*mut c_void) -> Option<()>,
            ),
            (
                c"qttunnel.3.2.7.dll",
                detour_exstar_qttunnel as unsafe fn(*mut c_void) -> Option<()>,
            ),
        ]);
    } else {
        if hub_quit_compat_enabled {
            modules.push((
                c"Qt5Core.dll",
                detour_exstar_qt_core as unsafe fn(*mut c_void) -> Option<()>,
            ));
            modules.push((
                c"Qt5Widgets.dll",
                detour_exstar_qt_widgets as unsafe fn(*mut c_void) -> Option<()>,
            ));
            modules.push((
                c"Qt5Gui.dll",
                detour_exstar_qt_gui as unsafe fn(*mut c_void) -> Option<()>,
            ));
        }
        if hub_light_trace_enabled {
            if !hub_quit_compat_enabled {
                modules.push((
                    c"Sn3DBox.dll",
                    detour_exstar_sn3dbox as unsafe fn(*mut c_void) -> Option<()>,
                ));
                modules.push((
                    c"Qt5Widgets.dll",
                    detour_exstar_qt_widgets as unsafe fn(*mut c_void) -> Option<()>,
                ));
                modules.push((
                    c"Qt5Gui.dll",
                    detour_exstar_qt_gui as unsafe fn(*mut c_void) -> Option<()>,
                ));
            }
        }
    }
    
    // For scanservice.exe, hook Sn3DDeviceEinStar.dll to unblock device cleanup deadlock
    if let Some((_, exe_name)) = exstar_current_exe("module_hook_device") {
        if exe_name.eq_ignore_ascii_case("scanservice.exe") {
            modules.push((
                c"Sn3DDeviceEinStar.dll",
                detour_exstar_device_einstar as unsafe fn(*mut c_void) -> Option<()>,
            ));
        }
    }

    let exe_handle = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
    if host_trace_enabled {
        let exe_name = exstar_current_exe("init_modules")
            .map(|(_, n)| n)
            .unwrap_or_else(|| "<unknown>".to_string());
        log_exstar_host(format_args!(
            "kind=init phase=exe_detours exe={} exe_handle={:p} exe_handle_null={}",
            exe_name, exe_handle, exe_handle.is_null()
        ));
    }
    if !exe_handle.is_null() {
        if host_trace_enabled {
            let _ = detour_exstar_exe(exe_handle.cast());
            let _ = detour_process_manager_exe(exe_handle.cast());
            let _ = detour_scanservice_exe(exe_handle.cast());
        } else if manager_compat_enabled {
            let _ = detour_process_manager_exe(exe_handle.cast());
        }
    }
    if host_trace_enabled {
        log_exstar_host(format_args!(
            "kind=init phase=exe_detours_complete exe_handle={:p}",
            exe_handle
        ));
    }

    for (name, detour) in modules {
        let handle = GetModuleHandleA(name.as_ptr().cast());
        if handle.is_null() {
            continue;
        }
        if host_trace_enabled {
            log_exstar_host(format_args!(
                "kind=module_present module={} handle={:p}",
                name.to_string_lossy(),
                handle
            ));
        }
        let _ = detour(handle.cast());
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaLdrLoadDll(
    dll_path_arg: LPCWSTR,
    dll_characteristics: *mut u32,
    dll_name: *const UNICODE_STRING,
    dll_handle: *mut detours_sys::PVOID,
) -> NTSTATUS {
    let is_nvrtc = is_nvrtc(dll_name).unwrap_or(false);
    let result = (LDR_LOAD_DLL)(dll_path_arg, dll_characteristics, dll_name, dll_handle);
    if is_nvrtc && result.is_ok() {
        detour_nvrtc(*dll_handle);
    }
    exstar_host_trace_on_load(dll_name, *dll_handle, result);
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn Zluda_nvrtcCompileProgram(
    prog: *const c_void,
    num_options: std::ffi::c_int,
    options: *const *const std::ffi::c_char,
) -> u32 {
    // TODO:
    // Right now we pick any one of the detoured nvrtcCompileProgram
    // functions, but if there are multiple nvrtc instances loaded this will be
    // incorrect. We should create a thunk for each library, but I don't have
    // the time right now.
    let original_fn = {
        nvrtc_detours()
            .lock()
            .ok()
            .and_then(|nvrtc_detours| nvrtc_detours.values().next().copied())
    };
    let nvrtcCompileProgram = match original_fn {
        Some(original_fn) => original_fn,
        None => return 11, // NVRTC_ERROR_INTERNAL_ERROR
    };
    let old_options = std::slice::from_raw_parts(options, num_options as usize);
    let mut options = vec![c"-arch=compute_86".as_ptr()];
    for &option in old_options {
        if nvrtc::is_arch_option(CStr::from_ptr(option)) {
            continue;
        }
        options.push(option);
    }
    (mem::transmute::<
        _,
        unsafe extern "system" fn(
            *const c_void,
            std::ffi::c_int,
            *const *const std::ffi::c_char,
        ) -> u32,
    >(nvrtcCompileProgram))(prog, options.len() as _, options.as_ptr())
}

// There might be multiple nvrtc instances loaded,
// so we need to keep track of which ones we've detoured
fn nvrtc_detours() -> &'static Mutex<FxHashMap<usize, usize>> {
    static NVRTC_DETOURS: OnceLock<Mutex<FxHashMap<usize, usize>>> = OnceLock::new();
    NVRTC_DETOURS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

unsafe fn detour_nvrtc(handle: *mut c_void) -> Option<()> {
    let mut nvrtc_detours = nvrtc_detours().lock().ok()?;
    let nvrtc_entry = match nvrtc_detours.entry(handle as usize) {
        hash_map::Entry::Occupied(_) => return Some(()),
        hash_map::Entry::Vacant(entry) => entry,
    };
    let get_cubin = GetProcAddress(handle, c"nvrtcGetCUBIN".as_ptr().cast())?;
    let get_cubin_size = GetProcAddress(handle, c"nvrtcGetCUBINSize".as_ptr().cast())?;
    let get_ptx = GetProcAddress(handle, c"nvrtcGetPTX".as_ptr().cast())?;
    let get_ptx_size = GetProcAddress(handle, c"nvrtcGetPTXSize".as_ptr().cast())?;
    let nvrtc_compile_program = GetProcAddress(handle, c"nvrtcCompileProgram".as_ptr().cast())?;
    if !apply_detours(
        nvrtc_entry,
        get_cubin,
        get_cubin_size,
        get_ptx,
        get_ptx_size,
        nvrtc_compile_program,
    ) {
        DetourTransactionAbort();
    }
    Some(())
}

unsafe fn apply_detours(
    nvrtc_entry: hash_map::VacantEntry<usize, usize>,
    mut get_cubin: unsafe extern "system" fn() -> isize,
    mut get_cubin_size: unsafe extern "system" fn() -> isize,
    get_ptx: unsafe extern "system" fn() -> isize,
    get_ptx_size: unsafe extern "system" fn() -> isize,
    mut nvrtc_compile_program: unsafe extern "system" fn() -> isize,
) -> bool {
    if DetourTransactionBegin() != NO_ERROR as i32 {
        return false;
    }
    if DetourUpdateThread(GetCurrentThread().0) != NO_ERROR as i32 {
        return false;
    }
    if DetourAttach(std::ptr::from_mut(&mut get_cubin).cast(), get_ptx as _) != NO_ERROR as i32 {
        return false;
    }
    if DetourAttach(
        std::ptr::from_mut(&mut get_cubin_size).cast(),
        get_ptx_size as _,
    ) != NO_ERROR as i32
    {
        return false;
    }
    if DetourAttach(
        std::ptr::from_mut(&mut get_cubin_size).cast(),
        get_ptx_size as _,
    ) != NO_ERROR as i32
    {
        return false;
    }
    if DetourAttach(
        std::ptr::from_mut(&mut nvrtc_compile_program).cast(),
        Zluda_nvrtcCompileProgram as _,
    ) != NO_ERROR as i32
    {
        return false;
    }
    if DetourTransactionCommit() != NO_ERROR as i32 {
        return false;
    }
    nvrtc_entry.insert(nvrtc_compile_program as usize);
    true
}

unsafe fn is_nvrtc(dll_name_arg: *const UNICODE_STRING) -> Result<bool, NTSTATUS> {
    fn version_segment(iter: &mut Peekable<impl Iterator<Item = u16>>) -> bool {
        fn is_digit(c: u16) -> bool {
            char::from_u32(c as u32)
                .unwrap_or(char::MIN)
                .is_ascii_digit()
        }
        fn parse_digits(iter: &mut Peekable<impl Iterator<Item = u16>>) -> bool {
            match iter.next() {
                Some(c) if is_digit(c) => (),
                _ => return false,
            }
            // Remaining digits
            loop {
                match iter.peek() {
                    Some(c) if is_digit(*c) => {
                        iter.next();
                    }
                    _ => break,
                }
            }
            true
        }
        if !parse_digits(iter) {
            return false;
        }
        match iter.peek() {
            Some(c) if *c == '_' as u16 => {
                iter.next();
            }
            _ => return true,
        }
        parse_digits(iter)
    }
    fn next_str(iter: &mut impl Iterator<Item = u16>, expected: &str) -> bool {
        for e in expected.chars() {
            match iter.next() {
                Some(c) => {
                    let c = char::from_u32(c as u32).unwrap_or(char::MIN);
                    if !c.eq_ignore_ascii_case(&e) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
    let dll_name = dll_file_name(dll_name_arg)?;
    let mut dll_name = dll_name.into_iter().peekable();
    if !next_str(&mut dll_name, "nvrtc64_") {
        return Ok(false);
    }
    if !version_segment(&mut dll_name) {
        return Ok(false);
    }
    if !next_str(&mut dll_name, ".dll") {
        return Ok(false);
    }
    Ok(dll_name.next() == None)
}

fn create_process(
    launch: &HostLaunchInfo,
    dwcreationflags: u32,
    source_proc_info: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
    create_process_underlying: impl Fn(
        u32,
        *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
    ) -> i32,
) -> BOOL {
    let trace_child_injection = launch_targets_process_name(launch, "sn3dprocessmanager.exe")
        || launch_targets_process_name(launch, "exstar hub.exe");
    if launch_targets_einscan_net_svr(launch)
        && env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .is_some_and(|name| name.eq_ignore_ascii_case("Sn3DprocessManager.exe"))
    {
        match acquire_einscan_launch_latch_silent() {
            Err(ERROR_ALREADY_EXISTS) => {
                log_exstar_host(format_args!(
                    "kind=compat action=launch_einscan_net_svr trigger=native-create-process status=duplicate-native-launch-observed command_line={}",
                    launch.command_line.as_deref().unwrap_or("<null>")
                ));
            }
            Ok(()) | Err(_) => {}
        }
    }
    let detour_paths = match unsafe { &*&raw const DETOUR_PATHS } {
        Some(paths) => paths,
        None => {
            let result = create_process_underlying(dwcreationflags, source_proc_info);
            log_exstar_child_launch(launch, dwcreationflags, source_proc_info, result);
            return result;
        }
    };
    // Add CREATE_BREAKAWAY_FROM_JOB so child processes escape the Job Object
    // created by zluda.exe. The Job Object now allows breakaway (we added
    // JOB_OBJECT_LIMIT_BREAKAWAY_OK to zluda_inject/src/bin.rs).
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
    let flags = dwcreationflags | windows_sys::Win32::System::Threading::CREATE_SUSPENDED | CREATE_BREAKAWAY_FROM_JOB;
    let mut proc_info_backup: windows_sys::Win32::System::Threading::PROCESS_INFORMATION =
        unsafe { mem::zeroed() };
    let proc_info = unsafe { source_proc_info.as_mut() }.unwrap_or(&mut proc_info_backup);
    let created = create_process_underlying(flags, proc_info);
    if created == 0 {
        log_exstar_child_launch(launch, flags, proc_info, created);
        return 0;
    }
    for (index, paths) in detour_paths.override_paths.iter().enumerate() {
        let (path_ascii, _) = unwrap_or::unwrap_some_or!(paths, continue);
        let mut unsafe_path = [path_ascii.as_ptr().cast()];
        let detour_result =
            unsafe { DetourUpdateProcessWithDll(proc_info.hProcess, unsafe_path.as_mut_ptr(), 1) };
        if trace_child_injection {
            let error = if detour_result != 0 {
                0
            } else {
                unsafe { GetLastError() }
            };
            log_exstar_host(format_args!(
                "kind=launch_inject stage=override target={} success={} error={} pid={} dll={} index={}",
                launch.command_line.as_deref().unwrap_or("<null>"),
                detour_result != 0,
                error,
                proc_info.dwProcessId,
                path_ascii.to_string_lossy(),
                index
            ));
        }
        if detour_result != 0 {
            let path_bytes = path_ascii.to_bytes_with_nul();
            unsafe {
                DetourCopyPayloadToProcess(
                    proc_info.hProcess,
                    std::ptr::from_ref(&LIBRARIES[index].guid).cast(),
                    path_bytes.as_ptr().cast_mut().cast(),
                    path_bytes.len() as u32,
                )
            };
        }
    }
    if let Some(self_path) = unsafe { &*&raw const SELF_PATH } {
        let mut unsafe_path = [self_path.as_ptr().cast()];
        let detour_result =
            unsafe { DetourUpdateProcessWithDll(proc_info.hProcess, unsafe_path.as_mut_ptr(), 1) };
        if trace_child_injection {
            let error = if detour_result != 0 {
                0
            } else {
                unsafe { GetLastError() }
            };
            log_exstar_host(format_args!(
                "kind=launch_inject stage=self target={} success={} error={} pid={} dll={}",
                launch.command_line.as_deref().unwrap_or("<null>"),
                detour_result != 0,
                error,
                proc_info.dwProcessId,
                self_path.to_string_lossy()
            ));
        }
    }
    let result = if dwcreationflags & windows_sys::Win32::System::Threading::CREATE_SUSPENDED == 0 {
        if unsafe { ResumeThread(HANDLE(proc_info.hThread)) } == u32::MAX {
            unsafe { TerminateProcess(HANDLE(proc_info.hProcess), 1) }.ok();
            FALSE
        } else {
            TRUE
        }
    } else {
        TRUE
    };
    if trace_child_injection
        && dwcreationflags & windows_sys::Win32::System::Threading::CREATE_SUSPENDED == 0
    {
        let wait_result = unsafe { WaitForSingleObject(proc_info.hProcess, 50) };
        let mut exit_code = 0u32;
        let exit_code_ok = unsafe { GetExitCodeProcess(proc_info.hProcess, &mut exit_code) } != 0;
        let exited_quickly = wait_result == WAIT_OBJECT_0;
        let still_running = wait_result == WAIT_TIMEOUT;
        log_exstar_host(format_args!(
            "kind=launch_probe target={} pid={} wait_result={} exited_quickly={} still_running={} exit_code_ok={} exit_code={}",
            launch.command_line.as_deref().unwrap_or("<null>"),
            proc_info.dwProcessId,
            wait_result,
            exited_quickly,
            still_running,
            exit_code_ok,
            exit_code
        ));
        if wait_result == WAIT_TIMEOUT {
            let late_wait_result = unsafe { WaitForSingleObject(proc_info.hProcess, 950) };
            let mut late_exit_code = 0u32;
            let late_exit_code_ok =
                unsafe { GetExitCodeProcess(proc_info.hProcess, &mut late_exit_code) } != 0;
            log_exstar_host(format_args!(
                "kind=launch_probe_late target={} pid={} wait_result={} exited_by_1000ms={} still_running_after_1000ms={} exit_code_ok={} exit_code={}",
                launch.command_line.as_deref().unwrap_or("<null>"),
                proc_info.dwProcessId,
                late_wait_result,
                late_wait_result == WAIT_OBJECT_0,
                late_wait_result == WAIT_TIMEOUT,
                late_exit_code_ok,
                late_exit_code
            ));
        }
        if launch_targets_process_name(launch, "sn3dprocessmanager.exe") {
            let mut watcher_handle = HANDLE::default();
            if unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    HANDLE(proc_info.hProcess),
                    GetCurrentProcess(),
                    &mut watcher_handle,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
            }
            .is_ok()
            {
                let watcher_target = launch
                    .command_line
                    .clone()
                    .unwrap_or_else(|| "<null>".to_string());
                let watcher_pid = proc_info.dwProcessId;
                let watcher_handle_raw = watcher_handle.0 as usize;
                thread::spawn(move || {
                    let watcher_handle =
                        watcher_handle_raw as windows_sys::Win32::Foundation::HANDLE;
                    let wait_result = unsafe { WaitForSingleObject(watcher_handle, 10_000) };
                    let mut exit_code = 0u32;
                    let exit_code_ok =
                        unsafe { GetExitCodeProcess(watcher_handle, &mut exit_code) } != 0;
                    log_exstar_host(format_args!(
                        "kind=launch_probe_final target={} pid={} wait_result={} exited_by_10000ms={} still_running_after_10000ms={} exit_code_ok={} exit_code={}",
                        watcher_target,
                        watcher_pid,
                        wait_result,
                        wait_result == WAIT_OBJECT_0,
                        wait_result == WAIT_TIMEOUT,
                        exit_code_ok,
                        exit_code
                    ));
                    unsafe { CloseHandle(HANDLE(watcher_handle)) }.ok();
                });
            }
        }
    }
    if !ptr::eq(proc_info, source_proc_info) {
        unsafe { CloseHandle(HANDLE(proc_info.hProcess)) }.ok();
        unsafe { CloseHandle(HANDLE(proc_info.hThread)) }.ok();
    }
    log_exstar_child_launch(launch, flags, proc_info, result);
    result
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateProcessW(
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL {
    let launch = HostLaunchInfo {
        api_name: "CreateProcessW",
        application_name: decode_pcwstr(lpapplicationname),
        command_line: decode_pcwstr(lpcommandline.cast_const()),
        current_directory: decode_pcwstr(lpcurrentdirectory),
    };
    create_process(
        &launch,
        dwcreationflags,
        lpprocessinformation,
        |creation_flags, proc_info| {
            CREATE_PROCESS_W(
                lpapplicationname,
                lpcommandline,
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                creation_flags,
                lpenvironment,
                lpcurrentdirectory,
                lpstartupinfo,
                proc_info,
            )
        },
    )
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateProcessAsUserA(
    htoken: windows_sys::Win32::Foundation::HANDLE,
    lpapplicationname: PCSTR,
    lpcommandline: PSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOA,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL {
    let launch = HostLaunchInfo {
        api_name: "CreateProcessAsUserA",
        application_name: decode_pcstr(lpapplicationname),
        command_line: decode_pcstr(lpcommandline.cast_const()),
        current_directory: decode_pcstr(lpcurrentdirectory),
    };
    create_process(
        &launch,
        dwcreationflags,
        lpprocessinformation,
        |creation_flags, proc_info| {
            CREATE_PROCESS_AS_USER_A(
                htoken,
                lpapplicationname,
                lpcommandline,
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                creation_flags,
                lpenvironment,
                lpcurrentdirectory,
                lpstartupinfo,
                proc_info,
            )
        },
    )
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateProcessAsUserW(
    htoken: windows_sys::Win32::Foundation::HANDLE,
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    lpprocessattributes: *const SECURITY_ATTRIBUTES,
    lpthreadattributes: *const SECURITY_ATTRIBUTES,
    binherithandles: BOOL,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL {
    let launch = HostLaunchInfo {
        api_name: "CreateProcessAsUserW",
        application_name: decode_pcwstr(lpapplicationname),
        command_line: decode_pcwstr(lpcommandline.cast_const()),
        current_directory: decode_pcwstr(lpcurrentdirectory),
    };
    create_process(
        &launch,
        dwcreationflags,
        lpprocessinformation,
        |creation_flags, proc_info| {
            CREATE_PROCESS_AS_USER_W(
                htoken,
                lpapplicationname,
                lpcommandline,
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                creation_flags,
                lpenvironment,
                lpcurrentdirectory,
                lpstartupinfo,
                proc_info,
            )
        },
    )
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateProcessWithLogonW(
    lpusername: PCWSTR,
    lpdomain: PCWSTR,
    lppassword: PCWSTR,
    dwlogonflags: windows_sys::Win32::System::Threading::CREATE_PROCESS_LOGON_FLAGS,
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL {
    let launch = HostLaunchInfo {
        api_name: "CreateProcessWithLogonW",
        application_name: decode_pcwstr(lpapplicationname),
        command_line: decode_pcwstr(lpcommandline.cast_const()),
        current_directory: decode_pcwstr(lpcurrentdirectory),
    };
    create_process(
        &launch,
        dwcreationflags,
        lpprocessinformation,
        |creation_flags, proc_info| {
            CREATE_PROCESS_WITH_LOGON_W(
                lpusername,
                lpdomain,
                lppassword,
                dwlogonflags,
                lpapplicationname,
                lpcommandline,
                creation_flags,
                lpenvironment,
                lpcurrentdirectory,
                lpstartupinfo,
                proc_info,
            )
        },
    )
}

#[allow(non_snake_case)]
unsafe extern "system" fn ZludaCreateProcessWithTokenW(
    htoken: windows_sys::Win32::Foundation::HANDLE,
    dwlogonflags: windows_sys::Win32::System::Threading::CREATE_PROCESS_LOGON_FLAGS,
    lpapplicationname: PCWSTR,
    lpcommandline: PWSTR,
    dwcreationflags: windows_sys::Win32::System::Threading::PROCESS_CREATION_FLAGS,
    lpenvironment: *const c_void,
    lpcurrentdirectory: PCWSTR,
    lpstartupinfo: *const windows_sys::Win32::System::Threading::STARTUPINFOW,
    lpprocessinformation: *mut windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
) -> BOOL {
    let launch = HostLaunchInfo {
        api_name: "CreateProcessWithTokenW",
        application_name: decode_pcwstr(lpapplicationname),
        command_line: decode_pcwstr(lpcommandline.cast_const()),
        current_directory: decode_pcwstr(lpcurrentdirectory),
    };
    create_process(
        &launch,
        dwcreationflags,
        lpprocessinformation,
        |creation_flags, proc_info| {
            CREATE_PROCESS_WITH_TOKEN_W(
                htoken,
                dwlogonflags,
                lpapplicationname,
                lpcommandline,
                creation_flags,
                lpenvironment,
                lpcurrentdirectory,
                lpstartupinfo,
                proc_info,
            )
        },
    )
}

// This type encapsulates typical calling sequence of detours and cleanup.
// We have two ways we do detours:
// * If we are loaded before nvcuda.dll, we hook LoadLibrary*
// * If we are loaded after nvcuda.dll, we override every cu* function
// Additionally, within both of those we attach to CreateProcess*
struct DetourDetachGuard {
    state: DetourUndoState,
    suspended_threads: Vec<HANDLE>,
    // First element is the original fn, second is the new fn
    overriden_non_cuda_fns: Vec<(*mut *mut c_void, *mut c_void)>,
}

impl DetourDetachGuard {
    // First element in the pair is ptr to original fn, second argument is the
    // new function. We accept *mut *mut c_void instead of *mut c_void as the
    // first element in the pair, because somehow otherwise original functions
    // also get overriden, so for example ZludaLoadLibraryExW ends calling
    // itself recursively until stack overflow exception occurs
    unsafe fn new<'a>() -> Option<Self> {
        let mut result = DetourDetachGuard {
            state: DetourUndoState::DoNothing,
            suspended_threads: Vec::new(),
            overriden_non_cuda_fns: Vec::new(),
        };
        if DetourTransactionBegin() != NO_ERROR as i32 {
            return None;
        }
        result.state = DetourUndoState::AbortTransactionResumeThreads;
        if !Self::suspend_all_threads_except_current(&mut result.suspended_threads) {
            return None;
        }
        for thread_handle in result.suspended_threads.iter().copied() {
            if DetourUpdateThread(thread_handle.0) != NO_ERROR as i32 {
                return None;
            }
        }
        // Initialize LockFileEx from kernel32 before detouring
        let k32 = GetModuleHandleA(c"kernel32.dll".as_ptr().cast());
        if let Some(lock_fn) = GetProcAddress(k32 as _, c"LockFileEx".as_ptr().cast()) {
            LOCK_FILE_EX = std::mem::transmute(lock_fn);
        }
        // Hook CreateDXGIFactory to spoof NVIDIA GPU vendor — resolve now, add to this transaction
        {
            let dxgi_module = GetModuleHandleA(c"dxgi.dll".as_ptr().cast());
            if !dxgi_module.is_null() {
                let proc_name = c"CreateDXGIFactory";
                if let Some(create_fn) = GetProcAddress(dxgi_module as _, proc_name.as_ptr().cast()) {
                    DXGI_CREATE_FACTORY_ORIGINAL = std::mem::transmute(create_fn);
                    DXGI_CREATE_FACTORY_HOOKED.store(true, std::sync::atomic::Ordering::SeqCst);
                    result.overriden_non_cuda_fns.push((
                        &raw mut DXGI_CREATE_FACTORY_ORIGINAL as *mut _ as *mut *mut c_void,
                        ZludaCreateDXGIFactory as *mut c_void,
                    ));
                }
            }
        }
        result.overriden_non_cuda_fns.extend_from_slice(&[
            (
                &raw mut LOAD_LIBRARY_A as *mut _ as *mut *mut c_void,
                ZludaLoadLibraryA as *mut c_void,
            ),
            (
                &raw mut LOAD_LIBRARY_W as *mut _ as _,
                ZludaLoadLibraryW as _,
            ),
            (
                &raw mut LOAD_LIBRARY_EX_A as *mut _ as _,
                ZludaLoadLibraryExA as _,
            ),
            (
                &raw mut LOAD_LIBRARY_EX_W as *mut _ as _,
                ZludaLoadLibraryExW as _,
            ),
            (
                &raw mut SLEEP_EX as *mut _ as _,
                ZludaSleepEx as _,
            ),
            (
                &raw mut GET_USER_DEFAULT_LOCALE_NAME as *mut _ as _,
                ZludaGetUserDefaultLocaleName as _,
            ),
            (
                &raw mut LOCK_FILE_EX as *mut _ as _,
                ZludaLockFileEx as _,
            ),
            (
                &raw mut CREATE_MUTEX_A as *mut _ as _,
                ZludaCreateMutexA as _,
            ),
            (
                &raw mut CREATE_MUTEX_W as *mut _ as _,
                ZludaCreateMutexW as _,
            ),
            (
                &raw mut WAIT_FOR_SINGLE_OBJECT as *mut _ as _,
                ZludaWaitForSingleObject as _,
            ),
            (
                &raw mut EXIT_PROCESS_FN as *mut _ as _,
                ZludaExitProcess as _,
            ),
            (
                &raw mut EXIT_THREAD_FN as *mut _ as _,
                ZludaExitThread as _,
            ),
            (
                &raw mut TERMINATE_PROCESS_FN as *mut _ as _,
                ZludaTerminateProcess as _,
            ),
            (
                &raw mut CREATE_PROCESS_A as *mut _ as _,
                ZludaCreateProcessA as _,
            ),
            (
                &raw mut CREATE_PROCESS_W as *mut _ as _,
                ZludaCreateProcessW as _,
            ),
            (
                &raw mut CREATE_PROCESS_AS_USER_A as *mut _ as _,
                ZludaCreateProcessAsUserA as _,
            ),
            (
                &raw mut CREATE_PROCESS_AS_USER_W as *mut _ as _,
                ZludaCreateProcessAsUserW as _,
            ),
            (
                &raw mut CREATE_PROCESS_WITH_LOGON_W as *mut _ as _,
                ZludaCreateProcessWithLogonW as _,
            ),
            (
                &raw mut CREATE_PROCESS_WITH_TOKEN_W as *mut _ as _,
                ZludaCreateProcessWithTokenW as _,
            ),
            (&raw mut SHOW_WINDOW as *mut _ as _, ZludaShowWindow as _),
            (
                &raw mut SET_WINDOW_POS as *mut _ as _,
                ZludaSetWindowPos as _,
            ),
            (
                &raw mut DESTROY_WINDOW as *mut _ as _,
                ZludaDestroyWindow as _,
            ),
            (&raw mut LDR_LOAD_DLL as *mut _ as _, ZludaLdrLoadDll as _),
            (
                &raw mut NT_TERMINATE_PROCESS as *mut _ as _,
                ZludaNtTerminateProcess as _,
            ),
        ]);
        for (original_fn, new_fn) in result.overriden_non_cuda_fns.iter().copied() {
            if DetourAttach(original_fn, new_fn) != NO_ERROR as i32 {
                return None;
            }
        }
        if DetourTransactionCommit() != NO_ERROR as i32 {
            return None;
        }
        result.state = DetourUndoState::DoNothing;
        // HACK ALERT
        // I really have no idea how this could happen.
        // Perhaps a thread was closed?
        if !result.resume_threads() {
            // Don't panic in debug builds — a thread may have exited between
            // suspend and resume, which is harmless. The panic was killing
            // the process during DllMain.
        }
        result.state = DetourUndoState::DetachDetours;
        Some(result)
    }

    unsafe fn suspend_all_threads_except_current(threads: &mut Vec<HANDLE>) -> bool {
        let thread_snapshot = unwrap_or::unwrap_ok_or!(
            CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0),
            _,
            return false
        );
        let current_thread = GetCurrentThreadId();
        let current_process = GetCurrentProcessId();
        let mut thread = THREADENTRY32::default();
        thread.dwSize = mem::size_of::<THREADENTRY32>() as u32;
        if Thread32First(thread_snapshot, &mut thread).is_err() {
            CloseHandle(thread_snapshot).ok();
            return false;
        }
        loop {
            if thread.th32OwnerProcessID == current_process && thread.th32ThreadID != current_thread
            {
                let thread_handle = unwrap_or::unwrap_ok_or!(
                    OpenThread(THREAD_SUSPEND_RESUME, false, thread.th32ThreadID),
                    _,
                    {
                        CloseHandle(thread_snapshot).ok();
                        return false;
                    }
                );
                if SuspendThread(thread_handle) == (-1i32 as u32) {
                    CloseHandle(thread_handle).ok();
                    CloseHandle(thread_snapshot).ok();
                    return false;
                }
                threads.push(thread_handle);
            }
            if Thread32Next(thread_snapshot, &mut thread).is_err() {
                break;
            }
        }
        CloseHandle(thread_snapshot).ok();
        true
    }

    // returns true on success
    unsafe fn resume_threads(&self) -> bool {
        let mut success = true;
        for t in self.suspended_threads.iter().copied() {
            if ResumeThread(t) == -1i32 as u32 {
                success = false;
            }
            if CloseHandle(t).is_err() {
                success = false;
            }
        }
        success
    }
}

impl Drop for DetourDetachGuard {
    fn drop(&mut self) {
        match self.state {
            DetourUndoState::DoNothing => {}
            DetourUndoState::AbortTransactionResumeThreads => {
                unsafe { DetourTransactionAbort() };
                unsafe { self.resume_threads() };
            }
            DetourUndoState::DetachDetours => unsafe {
                DetourTransactionBegin();
                DetourUpdateThread(GetCurrentThread().0);
                for (original_fn, new_fn) in self.overriden_non_cuda_fns.iter().copied() {
                    DetourDetach(original_fn, new_fn);
                }
                if let Ok(mut nvrtc_detours) = nvrtc_detours().lock() {
                    for (_, mut detoured) in nvrtc_detours.drain() {
                        DetourDetach(
                            std::ptr::from_mut(&mut detoured).cast(),
                            Zluda_nvrtcCompileProgram as _,
                        );
                    }
                }
                DetourTransactionCommit();
            },
        }
    }
}

// Along with Drop impl this forms a state machine for undoing detours.
// I would like to model this as a an usual full state machine with fields in
// variants, but you can't move fields out of type that implements Drop
enum DetourUndoState {
    DoNothing,
    AbortTransactionResumeThreads,
    DetachDetours,
}

#[allow(non_snake_case)]
#[no_mangle]
unsafe extern "system" fn DllMain(
    instance_handle: *mut c_void,
    dwReason: u32,
    _: *const u8,
) -> i32 {
    use windows_sys::Win32::Foundation::{FALSE, TRUE};
    use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
    if dwReason == DLL_PROCESS_ATTACH {
        windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(
            c"[ZLUDA] DllMain: DLL_PROCESS_ATTACH begin".as_ptr().cast()
        );
        if DetourRestoreAfterWith() == FALSE {
            windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(
                c"[ZLUDA] DllMain: DetourRestoreAfterWith FAILED".as_ptr().cast()
            );
            return FALSE;
        }
        windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(
            c"[ZLUDA] DllMain: DetourRestoreAfterWith OK".as_ptr().cast()
        );
        match DetourDetachGuard::new() {
            Some(g) => {
                windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(
                    c"[ZLUDA] DllMain: DetourDetachGuard OK".as_ptr().cast()
                );
                DETOUR_DROP = Some(g);
                DETOUR_PATHS = Some(DetourPaths::new());
                SELF_PATH = Some(zluda_windows::get_module_path(instance_handle));
                ensure_exstar_vectored_exception_handler();
                exstar_host_trace_existing_modules();
                // For scanservice.exe, set locale env vars to bypass QSystemLocale::query hang.
                // The hang occurs in Windows locale APIs (KERNELBASE GetLocaleInfoEx or similar)
                // called from Qt5Core's QSystemLocale during platform plugin initialization.
                // Forcing a simple "C" locale makes Qt skip the expensive Windows locale query.
                if let Some((_, exe_name)) = exstar_current_exe("locale_override") {
                    if exe_name.eq_ignore_ascii_case("scanservice.exe")
                        || exe_name.eq_ignore_ascii_case("scanhub.exe")
                        || exe_name.eq_ignore_ascii_case("TestOpenglHelper.exe")
                    {
                        // Set env vars that Qt checks before calling QSystemLocale
                        let vars = [
                            (c"LC_ALL", c"C"),
                            (c"LC_MESSAGES", c"C"),
                            (c"LANG", c"C"),
                        ];
                        extern "system" {
                            fn SetEnvironmentVariableA(
                                lpname: *const u8,
                                lpvalue: *const u8,
                            ) -> i32;
                        }
                        for (name, value) in &vars {
                            SetEnvironmentVariableA(
                                name.as_ptr().cast(),
                                value.as_ptr().cast(),
                            );
                        }
                        log_exstar_host(format_args!(
                            "kind=compat action=locale_override exe={} LC_ALL=C",
                            exe_name
                        ));
                    }
                }
                if exstar_trace_logging_enabled() {
                    let exe_name = exstar_current_exe("dllmain_complete")
                        .map(|(_, n)| n)
                        .unwrap_or_else(|| "<unknown>".to_string());
                    log_exstar_host(format_args!(
                        "kind=init phase=dllmain_complete exe={}",
                        exe_name
                    ));
                }
                // If dxgi.dll is already loaded (e.g. via static import from Qt5Gui.dll),
                // hook CreateDXGIFactory immediately at DllMain time.
                let dxgi_handle = GetModuleHandleA(c"dxgi.dll".as_ptr().cast());
                if !dxgi_handle.is_null() {
                    dxgi_try_hook_create_factory(dxgi_handle as HMODULE);
                }
                // Spawn a background thread to auto-dismiss EXStar warning dialogs.
                // The dialog is a custom Qt widget that doesn't use QMessageBox or
                // QDialog::exec, so we catch it at the Win32 window level.
                exstar_spawn_warning_dialog_closer();
                TRUE
            }
            None => {
                windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(
                    c"[ZLUDA] DllMain: DetourDetachGuard FAILED".as_ptr().cast()
                );
                FALSE
            }
        }
    } else if dwReason == DLL_PROCESS_DETACH {
        if !EXSTAR_VECTORED_EXCEPTION_HANDLER.is_null() {
            RemoveVectoredExceptionHandler(EXSTAR_VECTORED_EXCEPTION_HANDLER);
            EXSTAR_VECTORED_EXCEPTION_HANDLER = ptr::null_mut();
        }
        DETOUR_PATHS = None;
        match (&mut *&raw mut DETOUR_DROP).take() {
            Some(_) => TRUE,
            None => FALSE,
        }
    } else {
        TRUE
    }
}

fn get_payload(guid: &detours_sys::GUID) -> Option<&'static [u8]> {
    let mut size = 0;
    let payload_ptr = unsafe { detours_sys::DetourFindPayloadEx(guid, &mut size) };
    if payload_ptr != ptr::null_mut() {
        Some(unsafe { slice::from_raw_parts(payload_ptr as *const _, size as usize) })
    } else {
        None
    }
}

mod nvrtc {
    use std::ffi::CStr;

    use winnow::ascii::alphanumeric1;
    use winnow::ascii::multispace0;
    use winnow::combinator::alt;
    use winnow::prelude::*;
    use winnow::PResult;

    fn continuation(input: &mut &str) -> PResult<()> {
        (
            multispace0,
            '=',
            multispace0,
            alphanumeric1,
            '_',
            alphanumeric1,
        )
            .void()
            .parse_next(input)
    }

    pub(crate) fn is_arch_option(s: &CStr) -> bool {
        let mut text = match s.to_str() {
            Ok(text) => text,
            Err(_) => return false,
        };
        (alt(("--gpu-architecture", "-arch")), continuation)
            .parse_next(&mut text)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::is_nvrtc;
    use widestring::{u16cstr, U16CStr};
    use windows::{core::PWSTR, Win32::Foundation::UNICODE_STRING};

    fn assert_is_nvrtc(text: &U16CStr) {
        assert!(unsafe {
            is_nvrtc(&UNICODE_STRING {
                Buffer: PWSTR(text.as_ptr().cast_mut()),
                Length: (text.len() * 2) as u16,
                MaximumLength: 0,
            })
            .unwrap()
        });
    }

    #[test]
    fn is_nvrtc1() {
        assert_is_nvrtc(u16cstr!("nvrtc64_112_0.dll"));
    }

    #[test]
    fn is_nvrtc2() {
        assert_is_nvrtc(u16cstr!("nvrtc64_130.dll"));
    }

    #[test]
    fn is_nvrtc3() {
        assert_is_nvrtc(u16cstr!(
            r#"C:\Users\vosen\.conda\envs\pytorch\lib\site-packages\torch\lib\nvrtc64_112_0.dll"#
        ));
    }

    #[test]
    fn is_nvrtc4() {
        assert_is_nvrtc(u16cstr!(
            r#"C:\Users\vosen\.conda\envs\pytorch\lib\site-packages\torch\lib\nvrtc64_120.DLL"#
        ));
    }

    #[test]
    fn watchdog_thread_order_keeps_main_first_and_skips_current() {
        assert_eq!(
            super::watchdog_thread_order(20, 30, &[10, 20, 30, 40, 20, 50]),
            vec![20, 10, 40, 50]
        );
    }

    #[test]
    fn watchdog_thread_order_works_when_main_missing() {
        assert_eq!(
            super::watchdog_thread_order(77, 30, &[10, 30, 40, 10]),
            vec![10, 40]
        );
    }

    #[test]
    fn preserve_child_hub_exit_only_during_startup_before_real_window() {
        assert!(super::exstar_should_preserve_child_hub_exit(true, false));
        assert!(!super::exstar_should_preserve_child_hub_exit(true, true));
        assert!(!super::exstar_should_preserve_child_hub_exit(false, false));
        assert!(!super::exstar_should_preserve_child_hub_exit(false, true));
    }
}
