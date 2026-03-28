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
cargo xtask >> "%BUILD_LOG%" 2>&1
set "BUILD_EXIT=%errorlevel%"
echo [run_xtask_debug] cargo xtask exit code: %BUILD_EXIT%
echo [run_xtask_debug] cargo xtask exit code: %BUILD_EXIT%>> "%BUILD_LOG%"
exit /b %BUILD_EXIT%