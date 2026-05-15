# zluda-exstar-runtime

A fork of [ZLUDA](https://github.com/vosen/ZLUDA) (CUDA-on-AMD-GPUs) with
runtime patches that let Shining3D's **EXStar Hub** scanner software run on
AMD GPUs.

This is the buildable Rust workspace. End-user documentation, launcher
scripts, and the recommended quickstart live in the companion repo:

> [**exstar-on-zluda**](https://github.com/contrapuntal/exstar-on-zluda) — start there if you just want to run EXStar Hub.

## What's modified vs. upstream ZLUDA

Most EXStar-specific changes live under `zluda/zluda_redirect/src/` —
byte-signature-validated probes that hook EXStar Hub, `Sn3DprocessManager`,
and `AppUi.dll` to bypass NVIDIA-CUDA-specific startup checks.

ZLUDA core code (PTX parser, LLVM IR generation, cuBLAS/cuDNN/cuFFT shims)
is unmodified from upstream and continues to track
[vosen/ZLUDA](https://github.com/vosen/ZLUDA).

## Validated EXStar Hub versions (as of 2026-05-14)

| Version | Status |
| --- | --- |
| v1.1.0.16 | working |
| v1.1.1-8 | working |
| v1.1.1-9 | working (latest) |

## Build

Prerequisites on Windows:

- Rust (stable, MSVC toolchain)
- Visual Studio Build Tools (C++ workload) or full VS
- CMake
- PowerShell

From the repo root:

```cmd
run_xtask_debug.cmd
```

Outputs land in `target\debug\` (`zluda.exe`, `zluda_redirect.dll`,
`nvcuda.dll`, and helpers).

## License

Dual-licensed under MIT (`LICENSE-MIT`) and Apache-2.0 (`LICENSE-APACHE`),
matching upstream ZLUDA. Upstream ZLUDA copyright remains with its original
authors; EXStar-specific patches in this fork are licensed under the same
terms.
