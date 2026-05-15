# EXStar Hub on ZLUDA: Working Compatibility Reference

## Scope

This document records the working EXStar Hub compatibility state as of:

- ZLUDA repo: `%USERPROFILE%\proj\zluda-exstar-runtime`
- ZLUDA branch: `master`
- ZLUDA commit baseline from this debugging track: `e07032098bdd3de8ec31ffd1ed5cdf96d55caae8`
- EXStar Hub build observed on screen: `v1.1.0-16`
- Date of final working validation: `2026-03-28`

This is both:

1. A record of what made the current EXStar Hub build work.
2. A reference workflow for future EXStar versions where offsets or control flow may change.

## Final Outcome

A normal ZLUDA launch now reaches the real EXStar Hub home screen instead of hanging on the splash screen.

Validated results:

- The bootstrap Hub starts.
- `Sn3DprocessManager.exe` starts and stays alive.
- All expected helper processes start.
- The child `EXStar Hub.exe` reaches the real `./App_EA.xml` application window.
- The child `./App_EA.xml` window is promoted to the foreground on first show.
- The child no longer crashes in `PrestartCheck.dll`.
- `Preview` no longer reproduces the earlier `0x12` path caused by a stale deployed `nvcuda.dll`.
- Quitting no longer hangs forever on `Exiting...`.
- A no-trace launch shows the actual EXStar home UI, not the splash.

Reference artifact:

- `<runtime-repo>\target\debug\debug\launcher\desktop-capture-no-trace-prestart-fix.png`

## What Actually Broke

The final blocking issue was not `Sn3DBox::init`.

The real failure chain for the current EXStar version was:

1. The child Hub successfully created the real `./App_EA.xml` window.
2. `PrestartCheck.dll` scheduled repeated timer callbacks at:
   - `PrestartCheck.dll+0xbff0`
   - `PrestartCheck.dll+0xbf80`
3. The child later crashed from:
   - `PrestartCheck.dll+0x6645`
   - into `Qt5Core!QObject::qt_static_metacall+0xeb9`
4. Direct debugger evidence showed:
   - splash QML errors: `$prestartCheckController is not defined`
   - invalid Qt object-private pointer use during the later timer callback

That meant the splash/controller path was partially initialized, then later code dereferenced state that had never been created.

## Root Cause

The old `PrestartCheck.dll` binary patch was too aggressive.

Historical on-disk patch at file offset `0x4DF1`:

- original bytes:
  - `84 DB 0F 84 A7 07 00 00`
- old patched bytes:
  - `90 90 E9 35 0C 00 00 90`

In the current EXStar build, that old patch changed control flow at:

- `PrestartCheck.dll+0x59f1`

It no longer skipped only the GPU/CUDA failure branch. It jumped forward too far:

- old bad jump target: `PrestartCheck.dll+0x662d`

That bypassed a large block that appears to construct or register the prestart controller used by the splash QML and later timer callbacks.

The direct symptom match was strong:

- QML reported `$prestartCheckController` undefined.
- Later `PrestartCheck.dll+0x6645` crashed when timer-driven logic touched invalid Qt object state.

## Current Fix

Do not rely on the old on-disk `PrestartCheck.dll` patch as the final fix for this EXStar version.

Instead, ZLUDA now corrects `PrestartCheck.dll` in memory at load time.

### Runtime patch logic

When `PrestartCheck.dll` loads, `zluda_redirect.dll` patches the loaded image at:

- `PrestartCheck.dll+0x59f1`

Accepted existing byte patterns:

- original:
  - `84 DB 0F 84 A7 07 00 00`
- previously bad patch:
  - `90 90 E9 35 0C 00 00 90`

Replacement bytes:

- `84 DB E9 A8 07 00 00 90`

Effect:

- preserves the `test bl, bl`
- converts the old conditional failure jump into an unconditional near jump
- new jump target:
  - `PrestartCheck.dll+0x61a0`

This skips the GPU/CUDA-fail branch while preserving the intermediate controller/setup work that the current EXStar build needs.

### Where the fix lives

Source:

- `zluda_redirect/src/lib.rs`

Key function:

- `exstar_patch_prestartcheck_module`

Hook point:

- executed when `PrestartCheck.dll` is observed in the loader path

## ZLUDA Changes That Matter

The working result depends on multiple compatibility layers, not only the `PrestartCheck` fix.

### 1. Child process survival

EXStar launches a deep helper tree. The child processes must escape the Job Object created by `zluda.exe`.

Relevant fixes:

- `zluda_inject/src/bin.rs`
  - launcher Job Object allows breakaway
- `zluda_redirect/src/lib.rs`
  - CreateProcess path adds `CREATE_BREAKAWAY_FROM_JOB`

Without this, helper processes terminate early and startup fails far before the UI stage.

### 2. NVIDIA identity spoofing

EXStar expects NVIDIA CUDA hardware and performs adapter/vendor checks.

Relevant fix:

- DXGI adapter spoof
- `IDXGIAdapter::GetDesc` reports vendor `0x10DE`

This is necessary but not sufficient.

### 3. Manager-side environment / bootstrap compat

Relevant class of fixes:

- manager environment-detection bypasses
- process-preservation / kill-skip compat for core peers
- always-on quit compat

Without these, the manager kills the processes needed to complete startup.

### 4. PrestartCheck runtime correction

This is the final fix that moved startup from “splash forever” to “real Hub UI”.

### 5. Child app-window foreground handoff

Some launches were only apparently stuck. The child Hub had already created the
real `./App_EA.xml` window, but the transient `EXStar Hub` splash/bootstrap
surface could remain frontmost and make startup look frozen at 49%.

Current behavior:

- when `./App_EA.xml` first becomes visible, `zluda_redirect.dll` performs a
  one-shot handoff:
  - `ShowWindow(hwnd, SW_SHOW)`
  - `BringWindowToTop(hwnd)`
  - `SetForegroundWindow(hwnd)`
- this is intentionally scoped only to the real child app window, after it is
  already visible
- this is not the old broad “force main window visible” compat path

Validation signal in host trace:

- `kind=compat action=foreground_child_app_window`

### 6. Runtime preview path under active investigation
Startup is now separated from the former `Preview`/`0x12` runtime path.

The important operational lesson was deployment, not only code:

- `applications\nvcuda.dll` had become stale relative to `target\debug\nvcuda.dll`
- `scanservice.exe` was crashing in the stale deployed DLL
- Windows event log showed:
  - faulting module: `<runtime-repo>\target\debug\nvcuda.dll`
  - exception: `0xc0000409`
  - offset: `0x7ff9ad`

Relevant code change:

- `zluda/src/impl/memory.rs`
  - implements `cuMemcpy2DAsync_v2`
  - implements `cuMemcpy2DAsync_v2_ptsz`
  - uses `hipMemcpyParam2DAsync`

Working rule:

- always deploy the fresh `target\debug\nvcuda.dll` into `applications\`
- do not assume only `zluda.exe` and `zluda_redirect.dll` need syncing

Current status:

- the stale-`nvcuda.dll` `0x12` path is cleared in the current deployed build

### 7. Child Hub shutdown bridge

The final quit bug was separate from startup and preview.

Observed failure:

- the real child `EXStar Hub.exe` received shutdown-related traffic
- `quitApp` was seen on child Hub ingress
- posting `WM_CLOSE` to the real `./App_EA.xml` window was not enough
- the UI stayed on `Exiting...` indefinitely

Current fix:

- once the related manager exits, `zluda_redirect.dll` drives child Hub shutdown directly
- it posts `WM_CLOSE`
- then attempts Qt shutdown
- then forces `ExitProcess(0)` for the child Hub if it still has not exited

Why this is acceptable:

- by this point the manager is already gone
- the child Hub is no longer a useful independent process
- the practical requirement is for EXStar shutdown to complete cleanly instead of hanging forever

### 8. Do not reintroduce broken UI hooks

The following hooks caused regressions and must stay out:

- `QDialog::exec`
- `QMessageBox::*`

They broke Qt widget initialization or caused stack corruption.

## Current Validation Procedure

### Build

Use:

- `<runtime-repo>\run_xtask_debug.cmd`

### Deploy

Always manually sync both files after building:

- `target\debug\zluda.exe` -> `applications\zluda.exe`
- `target\debug\zluda_redirect.dll` -> `applications\zluda_redirect.dll`

This is critical. A stale deployed DLL caused multiple false regressions during debugging.

### Launch

Use:

```powershell
Get-Process -Name 'EXStar*','Sn3D*','scanservice','scanhub','softwareUpgrade','TestOpenglHelper','DeviceHelper','informationCollect','SnSyncService','einscan_net_svr','zluda' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 5
Start-Process -FilePath "<runtime-repo>\target\debug\zluda.exe" -ArgumentList "--zluda", "C:\Program Files\Shining3d\EXStar Hub\EXStar Hub.exe" -WorkingDirectory "C:\Program Files\Shining3d\EXStar Hub"
```

Optional trace env:

```powershell
$env:ZLUDA_DEBUG='1'
$env:ZLUDA_EXSTAR_HOST_TRACE='1'
$env:ZLUDA_EXSTAR_HOST_TRACE_PATH='<runtime-repo>\target\debug\debug\launcher\manual-trace.log'
```

### Working-state signals

The current build is considered working when all of the following are true:

- child `EXStar Hub.exe` survives well past splash teardown
- host trace shows:
  - `kind=compat action=patch_prestartcheck status=patched`
  - `kind=compat action=app_window_shown title="./App_EA.xml"`
  - `kind=compat action=foreground_child_app_window`
- no later `PrestartCheck.dll+0x6645` AV
- no-trace desktop shows the home screen instead of the splash

## Critical Traps

These were repeatedly validated during the debugging process.

### Do not assume the splash close is a failure

- `EXStar Hub.exe+0x6b49` splash close is normal.
- The splash going away does not mean startup failed.

### Do not use `QDialog::exec` hooks

- They caused `0xc0000409` crashes.

### Do not use `QMessageBox` hooks as the main dialog strategy

- They did not solve the actual problem path.

### Do not over-broaden the warning-dialog closer

A heuristic that closed “small modal Qt windows” ended up closing the manager window itself.

Only close dialogs when there is strong evidence from title or child text.

### Do not trust stale deployment

Always verify hashes or recopy:

- `applications\zluda.exe`
- `applications\zluda_redirect.dll`

### Do not keep the old `PrestartCheck.dll` patch as gospel

The historical patch was valid for an earlier understanding of the code, but for the current EXStar build it skipped too much.

### Do not stop at “window created”

The child window being created was not the end of the problem. The important question was whether later controller/timer logic stayed valid.

### Do not treat “49% splash” as proof that startup failed

At least one validated run had a healthy child `./App_EA.xml` window already
alive while the startup shell still made the UI look stuck. Always confirm:

- child `MainWindowTitle`
- host trace `app_window_shown`
- host trace `foreground_child_app_window`

## Future-Version Workflow

If EXStar Hub changes version, assume the current offsets may move.

Do not start by copying offsets blindly. Start by re-establishing the failure boundary.

### Step 1. Reproduce with host trace

Goal:

- determine whether failure is before manager startup, before child Hub startup, before real app window show, or after app window show

Key questions:

- does child `EXStar Hub.exe` exist?
- does it reach `./App_EA.xml`?
- does it later crash?
- which module/timer/dialog path is last before failure?

### Step 2. Distinguish process-level failures from UI-stage failures

If helpers die early:

- re-check Job Object breakaway and manager kill compat first

If the child shows the real app window and then dies:

- inspect late startup modules like `PrestartCheck.dll`, `AppUi.dll`, and the splash/controller bridge

### Step 3. Prefer direct debugger evidence over inference

Attach to the child Hub, not only the launcher, when the final failure is in the child UI process.

Useful symptoms:

- QML `ReferenceError`
- first-chance AV in Qt metacall/event dispatch
- invalid `QObjectPrivate` / bogus object-private pointer patterns

### Step 4. Map patches back to local control flow

If a historical binary patch exists:

1. disassemble the original DLL around the patch site
2. identify:
   - original branch condition
   - historical jump target
   - what code region is being skipped
3. compare the skip region to current runtime symptoms

In this case, that method exposed that the old patch skipped the controller setup block.

### Step 5. Prefer runtime correction in ZLUDA over permanent on-disk mutation

For future EXStar versions, the most robust pattern is:

- detect the module on load
- validate current bytes
- patch the loaded image in memory
- log the result

Advantages:

- version-aware
- reversible
- easier to instrument
- avoids editing Program Files binaries when ACLs block writes

### Step 6. Accept multiple known byte patterns

When patching a module in memory, support:

- pristine bytes
- bytes from an older local patch if users already modified the install

This avoids requiring a fully clean EXStar install before validation.

### Step 7. Only claim success after no-trace validation

A traced launch can alter timing and mask bugs.

Final validation must be:

- ordinary launch
- no host trace
- real UI visible
- process tree stable for a meaningful window after startup

## Current Working Reference Points

These offsets are for the current EXStar build only and must not be treated as stable across releases.

### PrestartCheck.dll

- patch decision site:
  - `+0x59f1`
- current working jump target:
  - `+0x61a0`
- old bad jump target:
  - `+0x662d`
- recurring timer callbacks:
  - `+0xbff0`
  - `+0xbf80`
- old crash site:
  - `+0x6645`

### EXStar Hub.exe

- real child window observed as:
  - `./App_EA.xml`

### Trace artifact of working run

- `<runtime-repo>\target\debug\debug\launcher\manual-trace-prestart-fix.log`

## Recommended Maintenance

If EXStar Hub updates:

1. keep the ZLUDA runtime patch mechanism
2. revalidate `PrestartCheck.dll` bytes and branch targets
3. re-check whether the controller setup block still lives between the decision site and the late callback path
4. update this document with the new offsets and symptoms

If the future version moves away from `PrestartCheck.dll`, reuse the same method:

- identify the last stable good state
- attach to the child UI process
- map the crash or silent exit to the exact skipped or invalid initialization block
- patch the loaded image in memory only after verifying the control-flow intent
