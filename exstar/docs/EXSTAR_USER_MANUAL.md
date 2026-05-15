# EXStar Hub on ZLUDA: User Manual

## Purpose

This manual covers building and launching the patched ZLUDA runtime against
a locally-installed Shining3D **EXStar Hub** on an AMD GPU.

Use this file if you want to:

- build the ZLUDA tree in this repo
- launch EXStar Hub under the freshly-built ZLUDA
- diagnose startup failures
- verify that the compatibility path is still working after an upgrade

For low-level technical detail (which probes patch which DLLs, byte offsets,
known-good control-flow targets), see `EXSTAR_COMPAT_REFERENCE.md` alongside
this file.

## Current Known Working Combination

- This repo: `contrapuntal/exstar-on-zluda` on `master`
- EXStar install path: `C:\Program Files\Shining3d\EXStar Hub`
- Validated EXStar Hub version (as of 2026-05-14):
  - `v1.1.1-9` (latest)

The repo contains only the ZLUDA fork and supporting scripts. Install EXStar
Hub separately from Shining3D.

## Prerequisites

- Git **with Git LFS** (`.gitattributes` puts `*.dll` and `*.bc` under LFS)
- Rust (stable, MSVC toolchain — install via [rustup](https://rustup.rs))
- Visual Studio Build Tools (C++ workload) or full VS 2022
- CMake
- PowerShell 5+ (built into Windows)
- Shining3D EXStar Hub installed at `C:\Program Files\Shining3d\EXStar Hub`

If you cloned without LFS, run `git lfs install && git lfs pull` from the
repo root before building.

## Build

From the repo root:

```cmd
run_xtask_debug.cmd
```

Expected: `cargo xtask exit code: 0`. First-time builds take 10–30 minutes.

Outputs land in `target\debug\` — at minimum: `zluda.exe`,
`zluda_redirect.dll`, `nvcuda.dll`. The launcher scripts find these via
repo-relative paths; there is no manual deploy step.

## Normal Launch

```cmd
.\exstar\scripts\launch\launch_exstar_zluda.cmd
```

The launcher kills any existing EXStar processes, sets the Qt plugin
environment, launches `zluda.exe --zluda "...\EXStar Hub.exe"`, and watches for
the real `./App_EA.xml` application window to appear.

Per-run logs land in `exstar\logs\launcher\launch_exstar_zluda_<timestamp>.log`.

If you prefer the raw invocation:

```powershell
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','TestOpenglHelper','DeviceHelper','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 5
Start-Process -FilePath ".\target\debug\zluda.exe" `
  -ArgumentList "--zluda", "C:\Program Files\Shining3d\EXStar Hub\EXStar Hub.exe" `
  -WorkingDirectory "C:\Program Files\Shining3d\EXStar Hub"
```

(Run from the repo root.)

## Expected Successful Startup

On a working launch:

- A bootstrap `EXStar Hub.exe` appears first.
- `Sn3DprocessManager.exe` starts.
- Helper processes start: `scanservice.exe`, `scanhub.exe`,
  `softwareUpgrade.exe`, `informationCollect.exe`, `SnSyncService.exe`,
  `einscan_net_svr.exe`.
- The real EXStar home window appears (window class `./App_EA.xml`).

Visible success criteria:

- The EXStar home screen replaces the splash.
- The UI shows project and device panels instead of freezing at splash progress.
- If the splash looks stuck, confirm the real child window is not merely behind
  it — the launcher promotes it automatically; if you launched manually you may
  need to bring it forward.

Process-level success criteria:

- An `EXStar Hub.exe` process remains alive with a visible main window.
- Manager and helper processes remain alive after startup stabilizes.

## Diagnostic Launch

If a launch regresses, the diagnose launcher enables host tracing:

```cmd
.\exstar\scripts\launch\launch_exstar_zluda.cmd /Diagnose
```

This sets `ZLUDA_DEBUG=1`, `ZLUDA_EXSTAR_HOST_TRACE=1`,
`ZLUDA_EXSTAR_EXE_TRACE=1`, and routes the host trace into
`exstar\logs\launcher\launch_exstar_zluda_<timestamp>_host.log`.

You can also set the trace env vars manually and launch from PowerShell:

```powershell
$env:ZLUDA_DEBUG='1'
$env:ZLUDA_EXSTAR_HOST_TRACE='1'
$env:ZLUDA_EXSTAR_HOST_TRACE_PATH = "$PWD\exstar\logs\launcher\manual-trace.log"
.\exstar\scripts\launch\launch_exstar_zluda.cmd
```

## What to Check First If It Breaks

### 1. Stale binaries on disk

Rebuild with `run_xtask_debug.cmd`. If the rebuild fails with
"failed to remove file …zluda_redirect.dll, Access is denied", a previous
EXStar/zluda process still holds the DLL — run
`.\exstar\scripts\launch\kill_exstar_zluda.cmd`, then rebuild.

### 2. Confirm the PrestartCheck runtime patch landed

In a traced run, look for:

- `kind=compat action=patch_prestartcheck status=patched`

If absent, the EXStar-specific in-memory patch did not apply — likely a new
EXStar version moved the bytes (see `EXSTAR_COMPAT_REFERENCE.md` for the
known-good byte patterns).

### 3. Confirm the child app window exists

In a traced run, look for:

- `kind=compat action=app_window_shown title="./App_EA.xml"`

If present, the child Hub reached the real application window.

### 4. Confirm the child app window was promoted to foreground

In a traced run, look for:

- `kind=compat action=foreground_child_app_window`

If present, the false "49% splash" case should no longer require manual
window activation.

### 5. Look for the old crash signature

The previously fatal path was:

- repeated timers at:
  - `PrestartCheck.dll+0xbff0`
  - `PrestartCheck.dll+0xbf80`
- then AV from:
  - `PrestartCheck.dll+0x6645`

If that pattern returns, the `PrestartCheck` control-flow correction is missing
or wrong for the current build.

### 6. If `Preview` throws `0x12`

That is a runtime `scanservice.exe` path, not the startup window path. The
fix lives in `zluda/src/impl/memory.rs` (`cuMemcpy2DAsync_v2`). A stale
locally-cached `nvcuda.dll` was the historical cause; rebuilding from a clean
`target\debug\` is the first thing to try.

## Current EXStar-Specific Runtime Fix

The working build patches `PrestartCheck.dll` in memory at load time — no
on-disk modification of EXStar binaries is needed. Briefly:

- When `PrestartCheck.dll` loads, ZLUDA patches the in-memory image to skip the
  GPU/CUDA-fail branch while preserving the controller/setup block needed by
  the splash and late startup timers.
- When `./App_EA.xml` first becomes visible, ZLUDA promotes that child window
  to the foreground so the splash/bootstrap shell doesn't remain misleadingly
  frontmost.
- When the related manager exits during shutdown, ZLUDA forces the real child
  Hub to terminate cleanly instead of hanging on "Exiting…".

See `EXSTAR_COMPAT_REFERENCE.md` for byte-level detail.

When EXStar Hub updates, the patch may need to be revalidated against the new
binary layout.

## Recommended Quick Validation Script

After rebuilding:

```powershell
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','TestOpenglHelper','DeviceHelper','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 5
Start-Process -FilePath ".\target\debug\zluda.exe" `
  -ArgumentList "--zluda", "C:\Program Files\Shining3d\EXStar Hub\EXStar Hub.exe" `
  -WorkingDirectory "C:\Program Files\Shining3d\EXStar Hub"
Start-Sleep 35
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue |
  Select-Object Id,ProcessName,MainWindowHandle,MainWindowTitle,Responding
```

Healthy signs after the sleep:

- An `EXStar Hub.exe` process is still alive.
- The home UI is visible.
- Manager and core helpers are still alive.

## Known Traps

Do not do these unless you are deliberately re-debugging a regression:

- Do not re-add `QDialog::exec` hooks.
- Do not re-add broad `QMessageBox` hooks as a primary fix.
- Do not broaden the warning-dialog closer to generic "small Qt windows".
- Do not assume the splash closing is the failure point.
- Do not trust the old `PrestartCheck.dll` on-disk patch as the final fix.

## Related Reading

- `EXSTAR_COMPAT_REFERENCE.md` (alongside this file) — byte-level patch detail,
  validation procedure, future-version workflow
- Repo root `README.md` — quickstart, repo layout, license, disclaimer
