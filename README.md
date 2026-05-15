# exstar-on-zluda

Run **EXStar Hub** (Shining3D's 3D-scanner software) on AMD GPUs via a
patched [ZLUDA](https://github.com/vosen/ZLUDA) runtime.

## Disclaimer

- This project is **not affiliated with, endorsed by, or supported by Shining3D**.
- **EXStar Hub binaries are not included.** You need to install EXStar Hub
  legitimately from Shining3D yourself before this repo is useful.
- Shining3D's official position is that EXStar Hub on Windows requires an NVIDIA
  GPU. This project intentionally bypasses those checks so the software can run
  on AMD GPUs via ZLUDA. **You assume any compatibility, stability, support,
  warranty, or licensing risk that follows from running EXStar in a configuration
  Shining3D does not officially support.**
- No Shining3D documentation, screenshots, or assets are redistributed here; the
  technical descriptions in this repo are the author's own observations of how
  the patched runtime interacts with a locally-installed EXStar.

## Target

- **Software**: Shining3D EXStar Hub for Windows
- **GPUs**: AMD — only the specific configuration in *Tested on* below has
  been validated; other AMD GPUs that ZLUDA itself supports may also work,
  but this project has not tested them
- **OS**: Windows 10 / 11
- **Tested on**: AMD Ryzen AI MAX+ 395 (Radeon **8060S** integrated GPU)
  with 128 GB RAM, paired with a Shining3D **Einstar Rockit** handheld
  3D scanner

## Validated EXStar Hub versions (as of 2026-05-14)

| Version | Status |
| --- | --- |
| v1.1.1-9 | working (latest) |

Earlier versions (`v1.1.0.16` and `v1.1.1-8`) were also validated during the
patch's evolution; see `exstar/docs/EXSTAR_COMPAT_REFERENCE.md` for the
historical track. Only `v1.1.1-9` is the currently advertised target.

## Quickstart

The fastest path to a working install uses the pre-built release zip — no
toolchain, no compile, ~30 MB download.

1. Go to the [latest release](https://github.com/contrapuntal/exstar-on-zluda/releases/latest)
   and download `exstar-on-zluda-v<version>-windows-x64.zip`.
2. (Optional, recommended) Verify the zip's SHA256 against the value in the
   release body.
3. Download and install Shining3D EXStar Hub from
   [the official download page](https://support.einstar.com/support/solutions/articles/60001635292-download-exstar-hub-desktop-software-for-einstar-rockit-einstar-2-)
   at the default location `C:\Program Files\Shining3d\EXStar Hub`.
4. Unzip and run:

   ```cmd
   launcher\launch_exstar_zluda.cmd
   ```

The `README.txt` inside the zip restates the disclaimer, limitations, and the
troubleshooting basics in case you start there instead of here.

## Build from source

Skip this section unless you're contributing or want to verify the published
binaries against the source yourself. The Quickstart above gets you the same
binaries pre-compiled.

> **Heads-up: building is a challenging install.** Building requires the
> Rust + MSVC + CMake toolchain, ~20 GB of free disk, and a 15–60 minute
> first build. If you only want to *run* EXStar on AMD, use the release zip
> from Quickstart above.

### Disk + time budget

- **Clone download**: ~260 MB (most of which is the vendored LLVM source tree)
- **Source on disk after clone**: ~1.5 GB
- **`target/` after first debug build**: ~15 GB (Cargo builds and caches every
  transitive dependency, normal for a workspace this size)
- **Recommended free disk space**: **≥ 20 GB** to leave room for `target/`,
  the Cargo registry cache (`~/.cargo/registry/`, shared across Rust projects),
  and slack for incremental rebuilds
- **First-build time**: 15–60 min depending on CPU and network — most of it is
  compiling LLVM bindings and ~200 transitive crates. Subsequent rebuilds use
  Cargo's incremental cache and are fast (seconds to minutes).

### Prerequisites on Windows

- Git **with Git LFS** (`.gitattributes` puts `*.dll` and `*.bc` under LFS;
  cloning without LFS leaves them as pointer files and the build fails)
- Rust (stable, MSVC toolchain — install via [rustup](https://rustup.rs))
- Visual Studio 2022 — Build Tools, Community, Professional, or Enterprise,
  all with the **Desktop development with C++** workload installed
  (`run_xtask_debug.cmd` finds whichever you have via `vswhere`)
- CMake
- PowerShell

### First-time setup (one-time)

```powershell
git lfs install
git clone https://github.com/contrapuntal/exstar-on-zluda.git
cd exstar-on-zluda
.\run_xtask_debug.cmd
```

If you cloned before running `git lfs install`, run `git lfs pull` inside the
repo to fetch the binary blobs before building.

Re-run `.\run_xtask_debug.cmd` after `git pull` or any source change.
Day-to-day launches need no rebuild — Cargo's incremental cache makes
subsequent builds fast (seconds to minutes).

The build produces `target\debug\zluda.exe`, `target\debug\zluda_redirect.dll`,
and helpers. Once it's done, launch the same way as the binary release:

```cmd
.\exstar\scripts\launch\launch_exstar_zluda.cmd
```

The launcher script finds the binaries relative to the repo root automatically.

### Cutting a release

For maintainers cutting a public release:

```powershell
.\run_xtask_release.cmd          # build with --release (smaller, optimized)
.\package_release.ps1 -Version 0.1.0   # bundle binaries + scripts + README → zip
git tag -a v0.1.0 -m "..."
git push origin v0.1.0
.\release_publish.ps1 -Version 0.1.0   # upload zip to a new GitHub Release via gh CLI
```

## Repo layout

```
exstar-on-zluda/
├── README.md                       (you are here)
├── LICENSE-MIT, LICENSE-APACHE     dual license, inherited from upstream ZLUDA
│
├── run_xtask_debug.cmd             \  build entry points
├── run_xtask_release.cmd           /  (see *Build from source*)
├── package_release.ps1             \
├── release_publish.ps1              > maintainer scripts for cutting a release
├── validate_release_zip.ps1        /
│
├── Cargo.toml                      \
├── zluda/                           |  Rust workspace — the ZLUDA fork.
├── zluda_redirect/                  |  EXStar-specific runtime hooks live
├── ptx/, ptx_parser/, ...           |  under zluda_redirect/src/
├── llvm_zluda/                      |
├── ...                              /  (~30 other workspace crates)
├── ext/                            vendored LLVM source — most of the clone size
├── target/{debug,release}/         compiled binaries (gitignored)
│
└── exstar/                         EXStar-specific user-facing content
    ├── docs/
    │   ├── EXSTAR_USER_MANUAL.md
    │   └── EXSTAR_COMPAT_REFERENCE.md
    ├── scripts/
    │   ├── launch/                 launch + kill scripts; stock-zluda A/B
    │   │                           harness for developer baseline comparison
    │   └── debug/                  diagnostic helpers (cdb watchdog, event-log query)
    └── logs/                       per-run log output (gitignored)
```

## What's modified vs. upstream ZLUDA

`zluda_redirect/src/` holds byte-signature-validated probes that hook
EXStar Hub, `Sn3DprocessManager`, and `AppUi.dll`, bypassing NVIDIA-CUDA
startup checks.

ZLUDA core code (PTX parser, LLVM IR generation, cuBLAS/cuDNN/cuFFT shims) is
unmodified from upstream and continues to track
[vosen/ZLUDA](https://github.com/vosen/ZLUDA).

## Documentation

- `exstar/docs/EXSTAR_USER_MANUAL.md` — step-by-step launch instructions
- `exstar/docs/EXSTAR_COMPAT_REFERENCE.md` — version matrix and what changed per release

## Troubleshooting

Failed launches and successful runs both leave structured logs under
`exstar/logs/launcher/`. If EXStar hangs on the splash screen or a scan crashes,
start with the most recent file there.

Common gotcha: if rebuilding fails with "failed to remove file …zluda_redirect.dll
Access is denied", a previous EXStar / zluda process is still holding the DLL —
run `.\exstar\scripts\launch\kill_exstar_zluda.cmd` then re-run `.\run_xtask_debug.cmd`.

## License

Dual-licensed under MIT (`LICENSE-MIT`) and Apache-2.0 (`LICENSE-APACHE`), matching
upstream ZLUDA.

Upstream ZLUDA copyright remains with its original authors.
