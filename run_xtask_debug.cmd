@echo off
setlocal

set "VS_DEV_CMD=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"
set "REPO_ROOT=%~dp0"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
set "BUILD_LOG=%REPO_ROOT%run_xtask_debug.log"

if not exist "%VS_DEV_CMD%" (
    echo VS developer command script not found: "%VS_DEV_CMD%"
    exit /b 1
)

echo [run_xtask_debug] entering VS developer environment
call "%VS_DEV_CMD%" -arch=x64 -host_arch=x64
if errorlevel 1 exit /b %errorlevel%

echo [run_xtask_debug] developer environment ready
cd /d "%REPO_ROOT%"
echo [run_xtask_debug] working directory: %CD%
echo [run_xtask_debug] log file: %BUILD_LOG%

if exist "%BUILD_LOG%" del "%BUILD_LOG%"

cargo --version
echo [run_xtask_debug] cargo xtask started > "%BUILD_LOG%"
echo [run_xtask_debug] streaming output to terminal and to %BUILD_LOG%
echo [run_xtask_debug] first build is slow (15-60 min) — Cargo compiles ~200 deps + LLVM
rem  Pipe through PowerShell's Tee-Object so build output appears in the terminal
rem  in real time AND is appended to the log file. `exit $LASTEXITCODE` inside the
rem  scriptblock preserves cargo's exit code rather than PowerShell's own.
powershell -NoProfile -ExecutionPolicy Bypass -Command "& { cargo xtask 2>&1 | Tee-Object -FilePath '%BUILD_LOG%' -Append; exit $LASTEXITCODE }"
set "BUILD_EXIT=%errorlevel%"
echo [run_xtask_debug] cargo xtask exit code: %BUILD_EXIT%
echo [run_xtask_debug] cargo xtask exit code: %BUILD_EXIT%>> "%BUILD_LOG%"
exit /b %BUILD_EXIT%