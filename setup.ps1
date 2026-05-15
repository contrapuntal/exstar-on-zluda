<#
.SYNOPSIS
    Bootstrap the zluda-exstar-runtime sibling repo and run its debug build.

.DESCRIPTION
    Clones the runtime repo as a sibling directory of this repo if it doesn't
    already exist, then invokes its build wrapper. On success the binaries
    needed by scripts\launch\launch_exstar_zluda.cmd are in
    ..\zluda-exstar-runtime\target\debug\.

.PARAMETER RuntimeRepoUrl
    Git URL of the runtime repo. Override if you forked it.

.PARAMETER SkipBuild
    Skip the build step (useful when you only want the clone).

.EXAMPLE
    .\setup.ps1
    .\setup.ps1 -RuntimeRepoUrl https://github.com/me/zluda-exstar-runtime.git
    .\setup.ps1 -SkipBuild
#>
[CmdletBinding()]
param(
    [string]$RuntimeRepoUrl = 'https://github.com/contrapuntal/zluda-exstar-runtime.git',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$companion = (Resolve-Path $PSScriptRoot).Path
$siblingParent = Split-Path $companion -Parent
$runtimeDir = Join-Path $siblingParent 'zluda-exstar-runtime'

Write-Host '==> exstar-on-zluda setup'
Write-Host ('    companion: {0}' -f $companion)
Write-Host ('    runtime:   {0}' -f $runtimeDir)

# Pre-flight: git available?
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw 'git was not found on PATH. Install Git for Windows and re-run.'
}

# Step 1: ensure runtime sibling exists
if (-not (Test-Path $runtimeDir)) {
    Write-Host ('==> Cloning runtime from {0}' -f $RuntimeRepoUrl)
    git clone $RuntimeRepoUrl $runtimeDir
    if ($LASTEXITCODE -ne 0) {
        throw "git clone failed (exit $LASTEXITCODE). Override the URL with -RuntimeRepoUrl."
    }
} else {
    Write-Host '==> Runtime sibling already present (skipping clone)'
}

# Step 2: build
if ($SkipBuild) {
    Write-Host '==> Skipping build (-SkipBuild set)'
} else {
    $buildScript = Join-Path $runtimeDir 'run_xtask_debug.cmd'
    if (-not (Test-Path $buildScript)) {
        throw "Expected build script not found at $buildScript"
    }
    Write-Host ('==> Building runtime: {0}' -f $buildScript)
    & cmd /c $buildScript
    if ($LASTEXITCODE -ne 0) {
        throw "Build failed (exit $LASTEXITCODE). See output above."
    }
}

# Step 3: verify deliverables
$debugDir = Join-Path $runtimeDir 'target\debug'
$expected = @('zluda.exe', 'zluda_redirect.dll', 'nvcuda.dll')
$missing = @()
foreach ($file in $expected) {
    if (-not (Test-Path (Join-Path $debugDir $file))) { $missing += $file }
}

if ($missing.Count -gt 0 -and -not $SkipBuild) {
    Write-Warning ('Build finished but these expected files are missing: {0}' -f ($missing -join ', '))
    Write-Warning ('Check {0}' -f $debugDir)
    exit 1
}

Write-Host ''
Write-Host '==> Ready.'
Write-Host '    Next: .\scripts\launch\launch_exstar_zluda.cmd'
