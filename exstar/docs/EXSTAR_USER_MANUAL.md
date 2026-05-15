# EXStar Hub on ZLUDA: User Manual

## Purpose

This manual explains how to use the current working ZLUDA build with the current working EXStar Hub build.

Use this file if you want to:

- build the current ZLUDA tree
- deploy the EXStar-specific runtime
- launch EXStar Hub under ZLUDA
- diagnose startup failures
- verify that the working compatibility path is still active

For the low-level technical explanation, use:

- `<runtime-repo>\EXSTAR_COMPAT_REFERENCE.md`

## Current Known Working Combination

- ZLUDA workspace:
  - `%USERPROFILE%\proj\zluda-exstar-runtime`
- EXStar install:
  - `C:\Program Files\Shining3d\EXStar Hub`
- EXStar UI version seen during working validation:
  - `v1.1.0-16`

## Requirements

The current workflow assumes these are already available on the machine:

- Rust toolchain
- MSVC / Visual Studio C++ build tools
- CMake
- PowerShell

Useful local paths:

- build wrapper:
  - `<runtime-repo>\run_xtask_debug.cmd`
- deployed launcher:
  - `<runtime-repo>\target\debug\zluda.exe`
- deployed redirect DLL:
  - `<runtime-repo>\target\debug\zluda_redirect.dll`
- EXStar diagnose launcher:
  - `<runtime-repo>\target\debug\launch_exstar_zluda_diagnose.cmd`

## Build

From the repo root:

```powershell
cmd /c <runtime-repo>\run_xtask_debug.cmd
```

Expected result:

- `cargo xtask` exits `0`

## Deploy

This step is mandatory.

Do not assume the build script copied the final binaries into `applications\`.

Manually sync all three:

```powershell
Copy-Item -LiteralPath <runtime-repo>\target\debug\zluda.exe `
  -Destination <runtime-repo>\target\debug\zluda.exe -Force

Copy-Item -LiteralPath <runtime-repo>\target\debug\zluda_redirect.dll `
  -Destination <runtime-repo>\target\debug\zluda_redirect.dll -Force

Copy-Item -LiteralPath <runtime-repo>\target\debug\nvcuda.dll `
  -Destination <runtime-repo>\target\debug\nvcuda.dll -Force
```

Recommended verification:

```powershell
Get-FileHash <runtime-repo>\target\debug\zluda.exe
Get-FileHash <runtime-repo>\target\debug\zluda.exe
Get-FileHash <runtime-repo>\target\debug\zluda_redirect.dll
Get-FileHash <runtime-repo>\target\debug\zluda_redirect.dll
Get-FileHash <runtime-repo>\target\debug\nvcuda.dll
Get-FileHash <runtime-repo>\target\debug\nvcuda.dll
```

The hashes should match pairwise.

## Normal Launch

Use a clean process tree before launching:

```powershell
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','TestOpenglHelper','DeviceHelper','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 5
Start-Process -FilePath "<runtime-repo>\target\debug\zluda.exe" `
  -ArgumentList "--zluda", "C:\Program Files\Shining3d\EXStar Hub\EXStar Hub.exe" `
  -WorkingDirectory "C:\Program Files\Shining3d\EXStar Hub"
```

## Expected Successful Startup

On a successful launch:

- a bootstrap `EXStar Hub.exe` appears first
- `Sn3DprocessManager.exe` starts
- helper processes start:
  - `scanservice.exe`
  - `scanhub.exe`
  - `softwareUpgrade.exe`
  - `informationCollect.exe`
  - `SnSyncService.exe`
  - `einscan_net_svr.exe`
- the real EXStar home window appears

Visible success criteria:

- the EXStar home screen replaces the splash
- the UI shows project and device panels instead of freezing at splash progress
- if the splash looks stuck, confirm the real child window is not merely behind it

Process-level success criteria:

- an `EXStar Hub.exe` process remains alive with a visible main window
- manager and helper processes remain alive after startup stabilizes

## Diagnostic Launch

If startup regresses, use host trace.

```powershell
$env:ZLUDA_DEBUG='1'
$env:ZLUDA_EXSTAR_HOST_TRACE='1'
$env:ZLUDA_EXSTAR_HOST_TRACE_PATH='<runtime-repo>\target\debug\debug\launcher\manual-trace.log'
```

Then launch EXStar normally under ZLUDA.

You can also use:

- `<runtime-repo>\target\debug\launch_exstar_zluda_diagnose.cmd`

Useful log directory:

- `<runtime-repo>\target\debug\debug\launcher`

## What to Check First If It Breaks

### 1. Deployed binaries may be stale

This was a frequent cause of false regressions.

Always re-copy:

- `applications\zluda.exe`
- `applications\zluda_redirect.dll`
- `applications\nvcuda.dll`

### 2. Confirm the PrestartCheck runtime patch landed

In a traced run, look for:

- `kind=compat action=patch_prestartcheck status=patched`

If this does not appear, the current EXStar-specific fix is not active.

### 3. Confirm the child app window exists

In a traced run, look for:

- `kind=compat action=app_window_shown title="./App_EA.xml"`

If this appears, the child Hub reached the real application window.

### 4. Confirm the child app window was promoted to foreground

In a traced run, look for:

- `kind=compat action=foreground_child_app_window`

If this appears, the false “49% splash” case should no longer depend on manual
window activation.

### 5. Look for the old crash signature

The previously fatal path was:

- repeated timers at:
  - `PrestartCheck.dll+0xbff0`
  - `PrestartCheck.dll+0xbf80`
- then AV from:
  - `PrestartCheck.dll+0x6645`

If that pattern returns, the `PrestartCheck` control-flow correction is missing or wrong for the current build.

### 6. If `Preview` still throws `0x12`

That is a runtime `scanservice.exe` path, not the startup window path.

First check deployment before assuming a fresh logic bug:

- confirm `applications\nvcuda.dll` matches `target\debug\nvcuda.dll`

The previously reproduced `0x12` was a stale deployed `nvcuda.dll` problem even
after the source-side `cuMemcpy2DAsync_v2` work was already present.

## Current EXStar-Specific Runtime Fix

The working ZLUDA build contains a runtime correction for `PrestartCheck.dll`.

You do not need to patch the EXStar install on disk for the current working state.

The current runtime behavior:

- when `PrestartCheck.dll` loads, ZLUDA patches the loaded image in memory
- this corrects the old over-broad GPU-bypass patch
- the fix preserves the controller/setup block needed by the splash and late startup timers
- when `./App_EA.xml` first becomes visible, ZLUDA promotes that child window
  to the foreground so the splash/bootstrap shell does not remain misleadingly
  frontmost
- when the related manager exits during shutdown, ZLUDA now forces the real child
  Hub to terminate instead of hanging forever on `Exiting...`

If EXStar updates, this may need to be revalidated.

## Recommended Quick Validation Script

After rebuilding and deploying, this is a practical smoke test:

```powershell
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','TestOpenglHelper','DeviceHelper','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 5
Start-Process -FilePath "<runtime-repo>\target\debug\zluda.exe" `
  -ArgumentList "--zluda", "C:\Program Files\Shining3d\EXStar Hub\EXStar Hub.exe" `
  -WorkingDirectory "C:\Program Files\Shining3d\EXStar Hub"
Start-Sleep 35
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue |
  Select-Object Id,ProcessName,MainWindowHandle,MainWindowTitle,Responding
```

Healthy signs after the sleep:

- an `EXStar Hub.exe` process is still alive
- the home UI is visible
- manager and core helpers are still alive

## Known Traps

Do not do these unless you are deliberately re-debugging a regression:

- do not re-add `QDialog::exec` hooks
- do not re-add broad `QMessageBox` hooks as a primary fix
- do not broaden the warning-dialog closer to generic “small Qt windows”
- do not assume the splash closing is the failure point
- do not trust the old `PrestartCheck.dll` disk patch as the final fix
- do not forget to deploy `applications\nvcuda.dll`

## Useful Tools

Useful local tools for deeper debugging:

- `%USERPROFILE%\Applications\Debuggers-x64\cdb.exe`
- `%USERPROFILE%\Applications\Procdump\procdump64.exe`
- `%USERPROFILE%\Applications\dumpbin-14.50.35722-x64\dumpbin.exe`
- Ghidra
- Process Monitor

## Current Reference Files

Start here in this order:

1. `<runtime-repo>\EXSTAR_USER_MANUAL.md`
2. `<runtime-repo>\EXSTAR_COMPAT_REFERENCE.md`
3. `<runtime-repo>\EXSTAR_DEBUGGING.md`
