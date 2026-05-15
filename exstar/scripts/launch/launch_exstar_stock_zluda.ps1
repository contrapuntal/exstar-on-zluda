param(
    [switch]$Diagnose
)

$ErrorActionPreference = 'Continue'

if (-not $env:ZLUDA_STOCK_DIR) {
    throw "Set ZLUDA_STOCK_DIR to the directory containing the unmodified (stock) zluda.exe."
}
$stockZludaDir = $env:ZLUDA_STOCK_DIR
$exstarDir = 'C:\Program Files\Shining3d\EXStar Hub'
$processNames = @(
    'zluda.exe',
    'EXStar Hub.exe',
    'Sn3DprocessManager.exe',
    'scanhub.exe',
    'scanservice.exe',
    'SnSyncService.exe',
    'einscan_net_svr.exe',
    'informationCollect.exe',
    'softwareUpgrade.exe',
    'PreviewTool.exe',
    'TestOpenglHelper.exe',
    'Shining3DUserAccount.exe',
    'sn3DCommunity.exe'
)
$mainWindowTitles = @('./App_EA.xml')
$warningWindowPatterns = @(
    'Software is unable to repeat',
    'Warning',
    'Sorry',
    'error'
)

if (-not ('LauncherWindowProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class LauncherWindowInfo {
    public int Pid { get; set; }
    public string Hwnd { get; set; }
    public string Title { get; set; }
    public bool Visible { get; set; }
}
public static class LauncherWindowProbe {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int maxCount);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hWnd, StringBuilder text, int maxCount);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(uint dwProcessId);
    public static string ReadWindowText(IntPtr hWnd) {
        var sb = new StringBuilder(512);
        GetWindowText(hWnd, sb, sb.Capacity);
        return sb.ToString();
    }
    public static string ReadWindowClass(IntPtr hWnd) {
        var sb = new StringBuilder(256);
        GetClassName(hWnd, sb, sb.Capacity);
        return sb.ToString();
    }
    public static List<LauncherWindowInfo> Snapshot() {
        var result = new List<LauncherWindowInfo>();
        EnumWindows((hWnd, lParam) => {
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);
            var sb = new StringBuilder(512);
            GetWindowText(hWnd, sb, sb.Capacity);
            var title = sb.ToString();
            if (title.Length > 0) {
                result.Add(new LauncherWindowInfo {
                    Pid = (int)pid,
                    Hwnd = "0x" + hWnd.ToInt64().ToString("x"),
                    Title = title,
                    Visible = IsWindowVisible(hWnd)
                });
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@
}

function Write-LauncherLog {
    param([string]$Message)

    $ts = Get-Date -Format 'yyyy-MM-dd HH:mm:ss.fff'
    $line = "[{0}] {1}" -f $ts, $Message
    Add-Content -Path $script:LogPath -Value $line
    Write-Host $line
}

function Get-TrackedProcesses {
    Get-Process -ErrorAction SilentlyContinue | Where-Object {
        ($_.ProcessName + '.exe') -in $processNames
    } | Sort-Object ProcessName, Id
}

function Get-TrackedTopWindows {
    $tracked = Get-Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ProcessName -eq 'EXStar Hub' -or $_.ProcessName -eq 'Sn3DprocessManager'
    }
    $trackedPids = @($tracked.Id)
    $warningRegex = ($warningWindowPatterns -join '|')
    [LauncherWindowProbe]::Snapshot() | Where-Object {
        $_.Pid -in $trackedPids -or $_.Title -match $warningRegex
    } | Sort-Object Pid, Title
}

function Write-ProcessSnapshot {
    param([string]$Label)

    Write-LauncherLog $Label
    $snapshot = Get-TrackedProcesses
    if (-not $snapshot) {
        Write-LauncherLog '  <none>'
        return
    }
    foreach ($p in $snapshot) {
        Write-LauncherLog ("  {0} pid={1} path={2}" -f ($p.ProcessName + '.exe'), $p.Id, $p.Path)
    }
}

function Write-WindowSnapshot {
    param([string]$Label)

    Write-LauncherLog $Label
    $windows = Get-Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ProcessName -eq 'EXStar Hub' -or $_.ProcessName -eq 'Sn3DprocessManager'
    } | Sort-Object ProcessName, Id
    if (-not $windows) {
        Write-LauncherLog '  <none>'
        return
    }
    foreach ($p in $windows) {
        Write-LauncherLog ("  {0} pid={1} hwnd=0x{2:x} title={3} responding={4}" -f $p.ProcessName, $p.Id, $p.MainWindowHandle, $p.MainWindowTitle, $p.Responding)
    }
    foreach ($w in (Get-TrackedTopWindows)) {
        Write-LauncherLog ("  top_window pid={0} hwnd={1} visible={2} title={3}" -f $w.Pid, $w.Hwnd, $w.Visible, $w.Title)
    }
}

function Stop-TrackedProcesses {
    param([string]$Reason)

    Write-ProcessSnapshot ("pre_cleanup_processes reason={0}" -f $Reason)
    $targets = Get-TrackedProcesses
    foreach ($p in $targets) {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        if (-not (Get-TrackedProcesses)) {
            break
        }
        Start-Sleep -Milliseconds 500
    }
    Write-ProcessSnapshot ("post_cleanup_processes reason={0}" -f $Reason)
}

function Test-MainHubWindow {
    $mainTitleSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($title in $mainWindowTitles) {
        $null = $mainTitleSet.Add($title)
    }

    Get-TrackedTopWindows | Where-Object {
        $_.Visible -and $mainTitleSet.Contains($_.Title)
    } | Select-Object -First 1
}

function Test-BootstrapHubWindow {
    Get-TrackedTopWindows | Where-Object {
        $_.Visible -and $_.Title -eq 'EXStar Hub'
    } | Select-Object -First 1
}

function Promote-MainHubWindow {
    param([object]$Window)

    if (-not $Window) {
        return
    }

    try {
        $hwndValue = [Convert]::ToInt64(($Window.Hwnd -replace '^0x', ''), 16)
        $hwnd = [IntPtr]::new($hwndValue)
    } catch {
        Write-LauncherLog ("promote_main_window status=parse_failed hwnd={0} error={1}" -f $Window.Hwnd, $_.Exception.Message)
        return
    }

    $allowResult = $false
    try {
        $allowResult = [LauncherWindowProbe]::AllowSetForegroundWindow([uint32]$Window.Pid)
    } catch {
        $allowResult = $false
    }

    $previousForeground = [LauncherWindowProbe]::GetForegroundWindow()
    $previousForegroundHex = ("0x{0:x}" -f $previousForeground.ToInt64())
    $previousForegroundTitle = [LauncherWindowProbe]::ReadWindowText($previousForeground)
    $previousForegroundClass = [LauncherWindowProbe]::ReadWindowClass($previousForeground)
    $minimizedForeground = $false
    if ($previousForeground -ne [IntPtr]::Zero -and (
            $previousForegroundTitle -like '*zluda*' -or
            $previousForegroundClass -eq 'CASCADIA_HOSTING_WINDOW_CLASS'
        )) {
        $minimizedForeground = [LauncherWindowProbe]::ShowWindow($previousForeground, 6)
        Start-Sleep -Milliseconds 100
    }

    $showResult = [LauncherWindowProbe]::ShowWindow($hwnd, 5)
    $topResult = [LauncherWindowProbe]::BringWindowToTop($hwnd)
    Start-Sleep -Milliseconds 150
    $foregroundResult = [LauncherWindowProbe]::SetForegroundWindow($hwnd)
    $foreground = [LauncherWindowProbe]::GetForegroundWindow()
    $foregroundHex = ("0x{0:x}" -f $foreground.ToInt64())
    Write-LauncherLog ("promote_main_window pid={0} hwnd={1} title={2} allow_foreground_result={3} minimized_foreground={4} previous_foreground_hwnd={5} previous_foreground_class={6} previous_foreground_title={7} show_result={8} top_result={9} foreground_result={10} foreground_hwnd={11}" -f $Window.Pid, $Window.Hwnd, $Window.Title, $allowResult, $minimizedForeground, $previousForegroundHex, $previousForegroundClass, $previousForegroundTitle, $showResult, $topResult, $foregroundResult, $foregroundHex)
}

function Start-DelayedMainWindowPromotion {
    param([object]$Window)

    if (-not $Window) {
        return
    }

    $hwndLiteral = $Window.Hwnd
    $pidLiteral = [int]$Window.Pid
    $command = @"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CodexPromote {
  [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(uint dwProcessId);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder text, int maxCount);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder text, int maxCount);
  public static string ReadWindowText(IntPtr hWnd) { var sb = new System.Text.StringBuilder(512); GetWindowText(hWnd, sb, sb.Capacity); return sb.ToString(); }
  public static string ReadWindowClass(IntPtr hWnd) { var sb = new System.Text.StringBuilder(256); GetClassName(hWnd, sb, sb.Capacity); return sb.ToString(); }
}
'@;
\$hwnd = [IntPtr]::new([Convert]::ToInt64('$hwndLiteral'.Replace('0x',''), 16));
1..4 | ForEach-Object {
  Start-Sleep -Milliseconds 500
  \$fg = [CodexPromote]::GetForegroundWindow()
  \$fgTitle = [CodexPromote]::ReadWindowText(\$fg)
  \$fgClass = [CodexPromote]::ReadWindowClass(\$fg)
  if (\$fg -ne [IntPtr]::Zero -and (\$fgTitle -like '*zluda*' -or \$fgClass -eq 'CASCADIA_HOSTING_WINDOW_CLASS')) {
    [CodexPromote]::ShowWindow(\$fg, 6) | Out-Null
    Start-Sleep -Milliseconds 100
  }
  [CodexPromote]::AllowSetForegroundWindow([uint32]$pidLiteral) | Out-Null
  [CodexPromote]::ShowWindow(\$hwnd, 5) | Out-Null
  [CodexPromote]::BringWindowToTop(\$hwnd) | Out-Null
  [CodexPromote]::SetForegroundWindow(\$hwnd) | Out-Null
}
"@
    Start-Process powershell -WindowStyle Hidden -ArgumentList @(
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-Command', $command
    ) | Out-Null
    Write-LauncherLog ("delayed_promote_scheduled pid={0} hwnd={1}" -f $Window.Pid, $Window.Hwnd)
}

function Clear-LaunchEnvironment {
    @(
        'ZLUDA_EXSTAR_DEVICE_QUALIFICATION_COMPAT',
        'ZLUDA_EXSTAR_FORCE_MAIN_WINDOW_VISIBLE',
        'ZLUDA_EXSTAR_HUB_STARTUP_COMPAT_TIMEOUT_MS',
        'ZLUDA_EXSTAR_HOST_TRACE',
        'ZLUDA_EXSTAR_EXE_TRACE',
        'ZLUDA_EXSTAR_HOST_TRACE_PATH',
        'ZLUDA_EXSTAR_LIGHT_TRACE',
        'ZLUDA_DEBUG',
        'ZLUDA_DEBUG_LAUNCH',
        'ZLUDA_DEBUG_SYNC',
        'QT_PLUGIN_PATH',
        'QT_QPA_PLATFORM_PLUGIN_PATH'
    ) | ForEach-Object {
        Remove-Item "Env:$_" -ErrorAction SilentlyContinue
    }
}

$exstarRoot = (Resolve-Path "$PSScriptRoot\..\..").Path
$script:LogDir = Join-Path $exstarRoot 'logs\launcher'
New-Item -ItemType Directory -Path $script:LogDir -Force | Out-Null
$script:RunStamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$script:LogPath = Join-Path $script:LogDir ("launch_exstar_STOCK_{0}.log" -f $script:RunStamp)

Write-LauncherLog 'begin STOCK ZLUDA baseline'
Write-LauncherLog ("stock_zluda_dir={0}" -f $stockZludaDir)
Write-LauncherLog ("exstar_dir={0}" -f $exstarDir)
if ($Diagnose) {
    Write-LauncherLog 'diagnose=true (no extra stock-only tracing available)'
} else {
    Write-LauncherLog 'diagnose=false'
}

Clear-LaunchEnvironment
Stop-TrackedProcesses 'initial'

Write-LauncherLog 'launching stock zluda.exe'
$launched = Start-Process `
    -FilePath (Join-Path $stockZludaDir 'zluda.exe') `
    -ArgumentList '--zluda', (Join-Path $exstarDir 'EXStar Hub.exe') `
    -WorkingDirectory $exstarDir `
    -WindowStyle Hidden `
    -PassThru
Write-LauncherLog ("launched stock zluda pid={0}" -f $launched.Id)

$mainWindowDetected = $false
$bootstrapWindowDetected = $false
foreach ($iteration in 1..20) {
    Start-Sleep -Seconds 3
    $elapsed = $iteration * 3
    Write-ProcessSnapshot ("post_launch_processes t+{0}s" -f $elapsed)
    Write-WindowSnapshot ("post_launch_windows t+{0}s" -f $elapsed)

    $mainWindow = Test-MainHubWindow
    if ($mainWindow) {
        Promote-MainHubWindow $mainWindow
        Start-DelayedMainWindowPromotion $mainWindow
        Write-LauncherLog ("main_window_detected pid={0} hwnd={1} title={2} t+{3}s" -f $mainWindow.Pid, $mainWindow.Hwnd, $mainWindow.Title, $elapsed)
        $mainWindowDetected = $true
        break
    }

    $bootstrapWindow = Test-BootstrapHubWindow
    if ($bootstrapWindow) {
        $bootstrapWindowDetected = $true
        Write-LauncherLog ("bootstrap_window_detected pid={0} hwnd={1} title={2} t+{3}s" -f $bootstrapWindow.Pid, $bootstrapWindow.Hwnd, $bootstrapWindow.Title, $elapsed)
    }
}

if ($mainWindowDetected) {
    Write-LauncherLog 'final_result=main_window_detected_stock'
} elseif ($bootstrapWindowDetected) {
    Write-LauncherLog 'final_result=bootstrap_window_only_stock'
} else {
    Write-LauncherLog 'final_result=no_hub_window_detected_stock'
}

Write-LauncherLog 'end (processes left running for inspection)'
