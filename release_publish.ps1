<#
.SYNOPSIS
    Publish an exstar-on-zluda release to GitHub via the gh CLI.

.DESCRIPTION
    Pre-flight: gh CLI installed, release zip present, tag exists locally
    AND on origin. If any check fails, the script bails before calling gh.

    On success: computes the zip's SHA256, generates the release body with
    the download link, hash, source commit, and disclaimer, then invokes
    `gh release create`. The release body is also written to a temp file
    that gh consumes via --notes-file.

    Run package_release.ps1 first to build the zip; tag and push the tag
    separately before running this script.

.PARAMETER Version
    Required. Version string without the leading "v", e.g. "0.1.0". The
    tag `v<Version>` and zip `exstar-on-zluda-v<Version>-windows-x64.zip`
    are expected to already exist.

.PARAMETER Draft
    If set, the release is created as a draft (not visible publicly until
    promoted via the GitHub UI). Useful for previewing the body / asset.

.EXAMPLE
    .\release_publish.ps1 -Version 0.1.0
    .\release_publish.ps1 -Version 0.1.0 -Draft
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [switch]$Draft
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path $PSScriptRoot).Path
$tag = "v$Version"
$zipName = "exstar-on-zluda-${tag}-windows-x64.zip"
$zipPath = Join-Path $repoRoot $zipName

# Pre-flight 1: locate gh CLI. Try PATH first, then the default install path
# (`C:\Program Files\GitHub CLI\gh.exe`) -- non-interactive PowerShell sessions
# don't always inherit the per-user PATH where gh lives.
$ghCmd = Get-Command gh -ErrorAction SilentlyContinue
if ($ghCmd) {
    $ghExe = $ghCmd.Source
} elseif (Test-Path 'C:\Program Files\GitHub CLI\gh.exe') {
    $ghExe = 'C:\Program Files\GitHub CLI\gh.exe'
} else {
    throw "gh CLI not found on PATH or at the default install path. Install from https://cli.github.com, or use the GitHub web UI for this release."
}

# Pre-flight 2: zip present.
if (-not (Test-Path $zipPath)) {
    throw "Release zip not found at $zipPath. Run `.\package_release.ps1 -Version $Version` first."
}

# Pre-flight 3: tag exists locally and points at a real commit.
$tagSha = (git -C $repoRoot rev-parse --verify "refs/tags/$tag^{}" 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $tagSha) {
    throw "Git tag $tag does not exist locally. Create + push it before publishing."
}
$tagSha = $tagSha.Trim()
$tagShaShort = $tagSha.Substring(0, 10)

# Pre-flight 4: tag pushed to origin.
$remoteTag = git -C $repoRoot ls-remote --tags origin $tag 2>$null
if (-not $remoteTag) {
    throw "Git tag $tag is not on origin. Run `git push origin $tag` first."
}

# Compute artifacts the release body will reference.
$hash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash
$sizeMB = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)

# Build the release body. Markdown for GitHub's rendering.
$notesFile = Join-Path $env:TEMP ("exstar-release-notes-{0}.md" -f $tag)
$notes = @"
## Downloads

- ``$zipName`` ($sizeMB MB) -- Windows x64
  - SHA256: ``$hash``
  - Built from commit ``$tagShaShort``

## What this is

Pre-built binaries for running Shining3D EXStar Hub on AMD GPUs via a patched ZLUDA runtime. Unzip and run ``launcher\launch_exstar_zluda.cmd``. See README.txt inside the zip for the full disclaimer, limitations, and troubleshooting steps.

## Risk

This project is not affiliated with, endorsed by, or supported by Shining3D. Shining3D officially requires NVIDIA hardware; this build bypasses those checks so EXStar runs on AMD via ZLUDA. You assume all compatibility, stability, support, warranty, and licensing risk that follows from running EXStar in an unsupported configuration.

## Source

[contrapuntal/exstar-on-zluda](https://github.com/contrapuntal/exstar-on-zluda) at commit ``$tagShaShort``. Upstream ZLUDA: [vosen/ZLUDA](https://github.com/vosen/ZLUDA).
"@

# Write the notes as UTF-8 without BOM so gh / GitHub render cleanly.
[System.IO.File]::WriteAllText(
    $notesFile,
    $notes,
    (New-Object System.Text.UTF8Encoding $false)
)

Write-Host "=== About to publish $tag ==="
Write-Host "  zip:    $zipPath ($sizeMB MB)"
Write-Host "  sha256: $hash"
Write-Host "  commit: $tagShaShort"
Write-Host "  draft:  $($Draft.IsPresent)"
Write-Host ""

# Build the gh argument list.
$ghArgs = @(
    'release', 'create', $tag,
    $zipPath,
    '--title', "$tag - exstar-on-zluda release",
    '--notes-file', $notesFile
)
if ($Draft) { $ghArgs += '--draft' }

& $ghExe @ghArgs
$ghExit = $LASTEXITCODE

Remove-Item -Force $notesFile -ErrorAction SilentlyContinue

if ($ghExit -ne 0) {
    throw "gh release create failed (exit $ghExit)."
}

Write-Host ""
Write-Host "=== Published ==="
& $ghExe release view $tag --json url,name,tagName,isDraft,assets `
    --jq '{url, name, tag: .tagName, draft: .isDraft, assets: [.assets[].name]}'
