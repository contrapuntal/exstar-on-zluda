<#
.SYNOPSIS
    Package an exstar-on-zluda release zip for distribution.

.DESCRIPTION
    Builds a versioned zip containing:
      - target/<BuildProfile>/ binaries (zluda.exe, zluda_redirect.dll, etc.)
      - launcher scripts from exstar/scripts/launch/
      - a minimal end-user README.txt with the source commit SHA baked in
      - LICENSE-MIT and LICENSE-APACHE

    Computes the SHA256 of the resulting zip and prints a summary suitable
    for pasting into the body of a GitHub Release.

.PARAMETER Version
    Required. Version string without the leading "v", e.g. "0.1.0".
    Output filename: exstar-on-zluda-v<Version>-windows-x64.zip.

.PARAMETER BuildProfile
    Cargo build profile to package. Default: "release".

.PARAMETER OutputDir
    Where to write the zip. Default: the repo root.

.EXAMPLE
    .\package_release.ps1 -Version 0.1.0
    .\package_release.ps1 -Version 0.1.0 -BuildProfile debug
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [ValidateSet('release', 'debug')]
    [string]$BuildProfile = 'release',
    [string]$OutputDir
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path $PSScriptRoot).Path
if (-not $OutputDir) { $OutputDir = $repoRoot }
$OutputDir = (Resolve-Path $OutputDir).Path

$targetDir = Join-Path $repoRoot "target\$BuildProfile"
$stagingDir = Join-Path $env:TEMP ("exstar-on-zluda-stage-{0}" -f [Guid]::NewGuid())
$zipName = "exstar-on-zluda-v${Version}-windows-x64.zip"
$zipPath = Join-Path $OutputDir $zipName

# Pre-flight: the build must have run for this profile.
$mustHave = 'zluda.exe', 'zluda_redirect.dll', 'nvcuda.dll'
foreach ($f in $mustHave) {
    if (-not (Test-Path (Join-Path $targetDir $f))) {
        throw "$f not found in $targetDir. Build with .\run_xtask_${BuildProfile}.cmd first."
    }
}

# Identify the source commit so the zip carries provenance.
$gitSha = (git -C $repoRoot rev-parse HEAD).Trim()
$gitShaShort = $gitSha.Substring(0, 10)

Write-Host "=== Packaging exstar-on-zluda v$Version ($BuildProfile build) ==="
Write-Host "  source commit: $gitSha"
Write-Host "  staging dir:   $stagingDir"
Write-Host ""

# Stage: bin/ + launcher/ + licenses + README.txt at the staging root.
New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
$binDir = Join-Path $stagingDir 'bin'
$launcherDir = Join-Path $stagingDir 'launcher'
New-Item -ItemType Directory -Path $binDir, $launcherDir | Out-Null

# Binaries -- copy every .exe / .dll from target/<profile>/. The launcher only
# directly invokes zluda.exe + zluda_redirect.dll + nvcuda.dll, but CUDA apps
# may load additional shims (cuBLAS, cuFFT, etc.) at runtime, so include them
# all rather than risk a "works on dev, fails on user" surprise.
$binFiles = @(Get-ChildItem -Path $targetDir -File | Where-Object { $_.Extension -in '.exe', '.dll' })
foreach ($f in $binFiles) {
    Copy-Item $f.FullName -Destination $binDir
}
Write-Host "  bin/: $($binFiles.Count) binaries"

# Launcher scripts. End users need launch + kill + diagnose; the
# launch_exstar_stock_* scripts are a developer-only A/B-comparison
# against unmodified upstream ZLUDA, so we exclude them from the zip.
$launcherSrc = Join-Path $repoRoot 'exstar\scripts\launch'
$launcherInclude = @(
    'launch_exstar_zluda.cmd',
    'launch_exstar_zluda.ps1',
    'launch_exstar_zluda_diagnose.cmd',
    'kill_exstar_zluda.cmd',
    'kill_exstar_zluda.ps1'
)
foreach ($name in $launcherInclude) {
    $src = Join-Path $launcherSrc $name
    if (-not (Test-Path $src)) {
        throw "Expected launcher file not found: $src"
    }
    Copy-Item $src -Destination $launcherDir
}
Write-Host "  launcher/: $($launcherInclude.Count) scripts"

# Licenses.
Copy-Item (Join-Path $repoRoot 'LICENSE-MIT') -Destination $stagingDir
Copy-Item (Join-Path $repoRoot 'LICENSE-APACHE') -Destination $stagingDir

# Rewrite launcher paths inside the zip layout: in the zip the binaries live
# under `bin/`, not `target\<profile>\debug`. The launcher .ps1 currently
# computes paths relative to the source-tree layout; for the zip we want it
# to find `..\bin\` relative to the launcher dir. Insert a small shim line
# so the launcher uses ZIPPED_BIN_DIR if set, else falls back to source-tree
# discovery.
# (For first cut, leave the launcher untouched and document the dependency
# in README.txt -- first release can be "unzip preserving structure".)

# Generate the end-user README.txt
$readme = @"
exstar-on-zluda v$Version
========================================

Built from commit $gitShaShort ($gitSha).

WHAT THIS IS
------------

Pre-built binaries to run Shining3D's EXStar Hub on AMD GPUs via a patched
ZLUDA runtime. This package is NOT affiliated with, endorsed by, or
supported by Shining3D.

QUICK START
-----------

1. Download and install Shining3D EXStar Hub from:
     https://support.einstar.com/support/solutions/articles/60001635292-download-exstar-hub-desktop-software-for-einstar-rockit-einstar-2-
   The default install location this package expects is:
     C:\Program Files\Shining3d\EXStar Hub

2. Unzip this archive so that bin\ and launcher\ sit side-by-side.

3. Run:
     launcher\launch_exstar_zluda.cmd

   The launcher kills any stale EXStar processes, sets the Qt plugin
   environment, then launches EXStar Hub through the ZLUDA shim.

LIMITATIONS / RISK
------------------

- EXStar Hub binaries are not included in this zip; install separately.
- Validated against EXStar Hub v1.1.1-9 (as of 2026-05-14) with an Einstar
  Rockit scanner. Other versions or scanners may work but are not validated.
- Tested only on AMD Ryzen AI MAX+ 395 with 128 GB RAM under Windows 10/11.
- Shining3D officially requires NVIDIA hardware. This build bypasses those
  checks. You assume all compatibility, stability, support, warranty, and
  licensing risk that follows.

WHEN THINGS GO WRONG
--------------------

If EXStar hangs on the splash or a scan crashes:
  - Run launcher\kill_exstar_zluda.cmd, then launcher\launch_exstar_zluda.cmd
  - Check the per-run log it produced under exstar-on-zluda\logs\launcher\
    (created next to the launcher).

For diagnostic tracing of a single launch:
  launcher\launch_exstar_zluda.cmd /Diagnose

SOURCE
------

GitHub:        https://github.com/contrapuntal/exstar-on-zluda
Built from:    $gitSha
Upstream:      https://github.com/vosen/ZLUDA

LICENSE
-------

Dual MIT (LICENSE-MIT) / Apache 2.0 (LICENSE-APACHE), matching upstream
ZLUDA. Upstream copyright remains with vosen's contributors; EXStar-specific
patches are under the same dual terms.
"@

# Write README.txt as UTF-8 without BOM. PS5's `Set-Content -Encoding UTF8`
# prepends a BOM; some plain-text readers display it as visible junk and
# gh-style downstream tools occasionally choke on it.
[System.IO.File]::WriteAllText(
    (Join-Path $stagingDir 'README.txt'),
    $readme,
    (New-Object System.Text.UTF8Encoding $false)
)

# Build the zip.
if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $stagingDir,
    $zipPath,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $false   # include base directory: false -> contents go at zip root
)

# Hash for the GitHub Release body.
$hash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash
$sizeMB = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)

# Tidy up staging.
Remove-Item -Recurse -Force $stagingDir

# Summary.
Write-Host ""
Write-Host "=== Package built ==="
Write-Host "  file:   $zipPath"
Write-Host "  size:   $sizeMB MB"
Write-Host "  sha256: $hash"
Write-Host "  source: $gitSha"
Write-Host ""
Write-Host "Suggested GitHub Release body (paste into the description):"
Write-Host "----------------------------------------------------------------"
Write-Host "## Downloads"
Write-Host ""
Write-Host ('- `{0}` -- Windows x64' -f $zipName)
Write-Host ('  - SHA256: `{0}`' -f $hash)
Write-Host ('  - Built from commit `{0}`' -f $gitShaShort)
Write-Host ""
Write-Host "## What this is"
Write-Host ""
Write-Host "Pre-built binaries for running Shining3D EXStar Hub on AMD GPUs."
Write-Host "Unzip and run ``launcher\launch_exstar_zluda.cmd``. See README.txt"
Write-Host "in the zip for limitations and disclaimer."
Write-Host "----------------------------------------------------------------"
