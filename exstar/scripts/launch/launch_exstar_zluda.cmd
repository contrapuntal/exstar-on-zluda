@echo off
setlocal
rem  Forward arguments (e.g. /Diagnose) through to the PowerShell script so
rem  `launch_exstar_zluda.cmd /Diagnose` enables host tracing as documented.
powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0launch_exstar_zluda.ps1" %*
