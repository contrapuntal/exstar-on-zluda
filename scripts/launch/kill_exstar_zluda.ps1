$ErrorActionPreference = 'Continue'

$processPatterns = @(
    'EXStar*',
    'Sn3D*',
    'scanservice',
    'scanhub',
    'softwareUpgrade',
    'TestOpenglHelper',
    'DeviceHelper',
    'informationCollect',
    'SnSyncService',
    'einscan_net_svr',
    'zluda'
)

$targets = Get-Process -Name $processPatterns -ErrorAction SilentlyContinue | Sort-Object ProcessName, Id
if (-not $targets) {
    Write-Host 'No EXStar/ZLUDA processes found.'
    exit 0
}

foreach ($process in $targets) {
    Write-Host ("Stopping {0} pid={1}" -f $process.ProcessName, $process.Id)
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
}

Start-Sleep -Seconds 1

$remaining = Get-Process -Name $processPatterns -ErrorAction SilentlyContinue | Sort-Object ProcessName, Id
if ($remaining) {
    Write-Host 'Processes still present after cleanup:'
    foreach ($process in $remaining) {
        Write-Host ("  {0} pid={1}" -f $process.ProcessName, $process.Id)
    }
    exit 1
}

Write-Host 'All EXStar/ZLUDA processes stopped.'
