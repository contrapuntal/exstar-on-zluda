@echo off
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0watch_scanservice_cdb.ps1"
