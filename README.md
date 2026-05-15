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
- **Tested on**: AMD Ryzen AI MAX+ 395 with 128 GB RAM

## Validated EXStar Hub versions (as of 2026-05-14)

| Version | Status |
| --- | --- |
| v1.1.1-9 | working (latest) |

## Quickstart — from source

### Disk + time budget

- **Clone download**: ~260 MB (most of which is the vendored LLVM source tree)
- **Source on disk after clone**: ~1.5 GB
- **`target/` after first debug build**: ~15 GB (Cargo builds + caches every
  transitive dependency; this is normal for a workspace this size)
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
- Visual Studio Build Tools (C++ workload) or full VS 2022
- CMake
- PowerShell

### First-time setup (one-time)

```powershell
git lfs install
git clone https://github.com/contrapuntal/exstar-on-zluda.git
cd exstar-on-zluda
.\run_xtask_debug.cmd
```

Re-run `.\run_xtask_debug.cmd` only when you pull updates (`git pull`) or
change source — not on every launch. Cargo's incremental cache makes
subsequent builds fast (seconds to minutes).

### Day-to-day: launch EXStar Hub

```powershell
.\exstar\scripts\launch\launch_exstar_zluda.cmd
```

This is the only command you need for everyday use once the repo is built.

(If you cloned before running `git lfs install`, run `git lfs pull` inside the
repo to fetch the actual binary blobs.)

The build takes 10–30 minutes the first time and produces `target\debug\zluda.exe`,
`target\debug\zluda_redirect.dll`, and helpers. The launcher script finds them
relative to the repo root automatically.

## Repo layout

```
exstar-on-zluda/
├── README.md                  (you are here)
├── Cargo.toml                 \
├── zluda/                      |
├── zluda_redirect/             | Rust workspace — the ZLUDA fork with EXStar runtime hooks
├── ptx/, ptx_parser/, ...      |
├── llvm_zluda/                /
├── target/debug/              compiled binaries (gitignored)
└── exstar/                    EXStar-specific user-facing content
    ├── docs/
    │   ├── EXSTAR_USER_MANUAL.md
    │   └── EXSTAR_COMPAT_REFERENCE.md
    ├── scripts/
    │   ├── launch/            launcher scripts (kill, normal, diagnose, stock)
    │   └── debug/             diagnostic helpers
    └── logs/                  per-run log output (gitignored)
```

## What's modified vs. upstream ZLUDA

EXStar-specific code lives under `zluda/zluda_redirect/src/` — byte-signature-validated
probes that hook EXStar Hub, `Sn3DprocessManager`, and `AppUi.dll` to bypass
NVIDIA-CUDA-specific startup checks.

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
