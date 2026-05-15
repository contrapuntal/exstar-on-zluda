# exstar-on-zluda

Run **EXStar Hub** (Shining3D 3D-scanner software) on AMD GPUs via a
patched [ZLUDA](https://github.com/vosen/ZLUDA) runtime.

This repo is the **front door**: launcher scripts, the user manual, the
version compatibility matrix, and a setup script that fetches and builds
the matching runtime.

## Target

- **Software**: Shining3D EXStar Hub for Windows
- **GPUs**: AMD (RDNA / RDNA2 / RDNA3)
- **OS**: Windows 10/11

## Validated EXStar Hub versions (as of 2026-05-14)

| Version | Status |
| --- | --- |
| v1.1.0.16 | working |
| v1.1.1-8 | working |
| v1.1.1-9 | working (latest) |

## Quickstart

```powershell
git clone https://github.com/contrapuntal/exstar-on-zluda.git
cd exstar-on-zluda
.\setup.ps1            # clones the runtime sibling and builds it
.\scripts\launch\launch_exstar_zluda.cmd
```

`setup.ps1` clones the sibling [`zluda-exstar-runtime`](https://github.com/contrapuntal/zluda-exstar-runtime)
repo, runs its debug build, and reports when binaries are ready.

## Repo layout

| Repo | Role |
| --- | --- |
| **exstar-on-zluda** (you are here) | Launcher scripts, user manual, compat matrix, setup orchestrator |
| [zluda-exstar-runtime](https://github.com/contrapuntal/zluda-exstar-runtime) | Rust source — the patched ZLUDA fork |

The two are designed to live side-by-side:

```
proj/
  exstar-on-zluda/        <- this repo
  zluda-exstar-runtime/   <- cloned by setup.ps1
```

## Documentation

- `docs/EXSTAR_USER_MANUAL.md` — step-by-step launch instructions
- `docs/EXSTAR_COMPAT_REFERENCE.md` — version matrix, what's known to work, what changed per version

## Troubleshooting

Failed launches and successful runs both leave structured logs under
`logs/launcher/`. If EXStar hangs on the splash screen or a scan crashes,
the most recent file there is the place to start.

## License

MIT. See `LICENSE`.
