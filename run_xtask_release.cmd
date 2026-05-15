@echo off
setlocal

set "REPO_ROOT=%~dp0"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
set "BUILD_LOG=%REPO_ROOT%run_xtask_release.log"

rem  Discover any installed VS edition (Build Tools / Community / Professional /
rem  Enterprise) via vswhere; fall back to the Build Tools hard-coded path.
rem  We capture vswhere output through a temp file rather than `for /f` to dodge
rem  cmd.exe's paren-matching breaking on the literal "(x86)" in the typical
rem  vswhere install path.
set "VSWHERE=C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
set "VS_DEV_CMD="
set "_VS_TMP=%TEMP%\__exstar_vs_install.txt"

if exist "%VSWHERE%" "%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath > "%_VS_TMP%" 2>nul
if exist "%_VS_TMP%" set /p "_VS_INSTALL=" < "%_VS_TMP%"
if exist "%_VS_TMP%" del "%_VS_TMP%"
if defined _VS_INSTALL set "VS_DEV_CMD=%_VS_INSTALL%\Common7\Tools\VsDevCmd.bat"

if not exist "%VS_DEV_CMD%" set "VS_DEV_CMD=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"

if not exist "%VS_DEV_CMD%" goto :no_vs

echo [run_xtask_release] entering VS developer environment
call "%VS_DEV_CMD%" -arch=x64 -host_arch=x64
if errorlevel 1 exit /b %errorlevel%

echo [run_xtask_release] developer environment ready
cd /d "%REPO_ROOT%"
echo [run_xtask_release] working directory: %CD%
echo [run_xtask_release] log file: %BUILD_LOG%

if exist "%BUILD_LOG%" del "%BUILD_LOG%"

cargo --version
echo [run_xtask_release] cargo xtask --release started > "%BUILD_LOG%"
echo [run_xtask_release] streaming output to terminal and to %BUILD_LOG%
echo [run_xtask_release] release build is slow ^(15-60 min from a cold cache^)
rem  Pipe through PowerShell's Tee-Object so build output appears in the terminal
rem  in real time AND is appended to the log file. `exit $LASTEXITCODE` inside the
rem  scriptblock preserves cargo's exit code rather than PowerShell's own.
powershell -NoProfile -ExecutionPolicy Bypass -Command "& { cargo xtask --release 2>&1 | Tee-Object -FilePath '%BUILD_LOG%' -Append; exit $LASTEXITCODE }"
set "BUILD_EXIT=%errorlevel%"
echo [run_xtask_release] cargo xtask --release exit code: %BUILD_EXIT%
echo [run_xtask_release] cargo xtask --release exit code: %BUILD_EXIT%>> "%BUILD_LOG%"
exit /b %BUILD_EXIT%

:no_vs
echo VS developer command script not found.
echo Tried vswhere at:  %VSWHERE%
echo Tried fallback at: %VS_DEV_CMD%
echo Install VS 2022 Build Tools or any VS 2022 edition with the
echo Desktop development with C++ workload.
exit /b 1
