<#
.SYNOPSIS
    Validate the structure and contents of an exstar-on-zluda release zip.

.DESCRIPTION
    Extracts the zip to a temp directory, then checks:
      - All expected files are present (binaries, launcher scripts, README, LICENSEs)
      - The bundled launcher PowerShell script parses cleanly
      - The bundled launcher carries expected guard code (layout-aware path
        discovery, Mark-of-the-Web unblock) -- catches regressions where
        a refactor accidentally drops the guards
      - README.txt is pure ASCII (no encoding mojibake)

    Bails with a non-zero exit on any failure. Run this after
    package_release.ps1 and before release_publish.ps1.

.PARAMETER Version
    Required. Version string without the leading "v" (e.g. "0.1.2"). The
    zip `exstar-on-zluda-v<Version>-windows-x64.zip` must be in the repo root.

.EXAMPLE
    .\validate_release_zip.ps1 -Version 0.1.2
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path $PSScriptRoot).Path
$tag = "v$Version"
$zipName = "exstar-on-zluda-${tag}-windows-x64.zip"
$zipPath = Join-Path $repoRoot $zipName

if (-not (Test-Path $zipPath)) {
    throw "Zip not found at $zipPath. Run package_release.ps1 -Version $Version first."
}

# Stage the extraction.
$extractDir = Join-Path $env:TEMP ("exstar-zipcheck-{0}" -f [Guid]::NewGuid())
New-Item -ItemType Directory -Path $extractDir | Out-Null

try {
    Write-Host "=== Validating $zipName ==="
    Write-Host "  extracting to $extractDir"

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $extractDir)

    # 1. File-presence checks.
    $required = @(
        'bin\zluda.exe',
        'bin\zluda_redirect.dll',
        'bin\nvcuda.dll',
        'launcher\launch_exstar_zluda.cmd',
        'launcher\launch_exstar_zluda.ps1',
        'launcher\kill_exstar_zluda.cmd',
        'launcher\kill_exstar_zluda.ps1',
        'README.txt',
        'LICENSE-MIT',
        'LICENSE-APACHE'
    )
    $missing = @()
    foreach ($rel in $required) {
        if (-not (Test-Path (Join-Path $extractDir $rel))) { $missing += $rel }
    }
    if ($missing.Count -gt 0) {
        throw "Missing required files in zip: $($missing -join ', ')"
    }
    Write-Host "  [ok] all $($required.Count) required files present"

    # 2. Forbidden / unwanted files (developer-only launchers should not ship).
    $forbidden = @(
        'launcher\launch_exstar_stock_zluda.cmd',
        'launcher\launch_exstar_stock_zluda.ps1'
    )
    $stowaway = @()
    foreach ($rel in $forbidden) {
        if (Test-Path (Join-Path $extractDir $rel)) { $stowaway += $rel }
    }
    if ($stowaway.Count -gt 0) {
        throw "Developer-only files leaked into zip: $($stowaway -join ', ')"
    }
    Write-Host "  [ok] no developer-only launchers in zip"

    # 3. README.txt is pure ASCII (catches PS5/CP1252 mojibake at packaging time).
    $readmeBytes = [System.IO.File]::ReadAllBytes((Join-Path $extractDir 'README.txt'))
    $highBytes = @($readmeBytes | Where-Object { $_ -gt 127 })
    if ($highBytes.Count -gt 0) {
        $sample = ($highBytes | Select-Object -First 5 | ForEach-Object { '0x{0:X2}' -f $_ }) -join ', '
        throw "README.txt has $($highBytes.Count) non-ASCII byte(s) -- mojibake likely. Sample: $sample"
    }
    Write-Host "  [ok] README.txt is pure ASCII ($($readmeBytes.Length) bytes)"

    # 4. Launcher script parses cleanly.
    $launcherPs1 = Join-Path $extractDir 'launcher\launch_exstar_zluda.ps1'
    $tokens = $null
    $parseErrors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile(
        $launcherPs1, [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count -gt 0) {
        $first = $parseErrors[0]
        throw "Launcher script has $($parseErrors.Count) parse error(s). First: line $($first.Extent.StartLineNumber): $($first.Message)"
    }
    Write-Host "  [ok] launcher PS1 parses cleanly"

    # 5. Launcher carries expected guard code (regression checks).
    $launcherSource = Get-Content $launcherPs1 -Raw
    $guards = @(
        @{ Name = 'layout-aware candidate list';  Pattern = '\$candidates\s*=\s*@\(' },
        @{ Name = 'Mark-of-the-Web unblock';       Pattern = '\bUnblock-File\b' },
        @{ Name = 'layout-aware Test-Path on candidate'; Pattern = "Test-Path\s*\(Join-Path\s*\`$c\.Z\s*'zluda\.exe'\)" }
    )
    foreach ($g in $guards) {
        if ($launcherSource -notmatch $g.Pattern) {
            throw "Launcher missing guard '$($g.Name)'. Refactor regression?"
        }
    }
    Write-Host "  [ok] all $($guards.Count) launcher guards present"

    Write-Host ""
    Write-Host "=== Validation passed ==="
    Write-Host "  Zip is structurally sound and contains the expected guards."
    Write-Host "  Recommended: extract the zip on a fresh machine and run"
    Write-Host "    launcher\launch_exstar_zluda.cmd"
    Write-Host "  manually before publishing -- runtime issues (UAC, EXStar install"
    Write-Host "  state, scanner connection) cannot be validated programmatically."
} finally {
    if (Test-Path $extractDir) {
        Remove-Item -Recurse -Force $extractDir
    }
}
