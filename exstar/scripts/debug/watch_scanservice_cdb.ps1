$ErrorActionPreference = 'Stop'

param(
    [int]$TimeoutSeconds = 300,
    [int]$PollMilliseconds = 250
)

$exstarRoot = (Resolve-Path "$PSScriptRoot\..\..").Path
$debugDir = Join-Path $exstarRoot 'logs\launcher'
# Override the cdb.exe path via $env:CDB_PATH. If unset, fall back to the
# Microsoft Store WinDbg install (path includes a build number; adjust if needed).
$fallbackCdb = 'C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2601.12001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe'

if ($env:CDB_PATH -and (Test-Path $env:CDB_PATH)) {
    $cdbPath = $env:CDB_PATH
} elseif (Test-Path $fallbackCdb) {
    $cdbPath = $fallbackCdb
} else {
    throw 'cdb.exe not found. Set CDB_PATH to your cdb.exe location, or install the WinDbg app.'
}

New-Item -ItemType Directory -Path $debugDir -Force | Out-Null

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
Write-Host ("Waiting for scanservice.exe for up to {0}s" -f $TimeoutSeconds)

$scanservice = $null
while ((Get-Date) -lt $deadline) {
    $scanservice = Get-Process -Name 'scanservice' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($scanservice) {
        break
    }
    Start-Sleep -Milliseconds $PollMilliseconds
}

if (-not $scanservice) {
    Write-Host 'Timed out waiting for scanservice.exe.'
    exit 1
}

$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$logPath = Join-Path $debugDir ("cdb-scanservice-auto-{0}-{1}.log" -f $scanservice.Id, $timestamp)
$command = '.symfix; .reload; !runaway 7; ~* kb; .ecxr; kv; q'

Write-Host ("Attaching cdb to scanservice.exe pid={0}" -f $scanservice.Id)
Write-Host ("Writing debugger log to {0}" -f $logPath)

& $cdbPath -p $scanservice.Id -c $command *> $logPath
$exitCode = $LASTEXITCODE

Write-Host ("cdb finished with exit code {0}" -f $exitCode)
Write-Host ("Debugger log: {0}" -f $logPath)
exit $exitCode
