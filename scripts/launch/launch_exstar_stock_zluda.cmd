@echo off
setlocal
echo === EXStar Hub with STOCK (unmodified) ZLUDA - Baseline Comparison ===
echo.
echo This uses the unmodified (stock) ZLUDA binaries from %ZLUDA_STOCK_DIR%
echo No EXStar compat patches, no hooks, no env var overrides.
echo Monitoring for 60 seconds...
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0launch_exstar_stock_zluda.ps1"
echo.
echo Log written to: exstar-on-zluda\logs\launcher\launch_exstar_STOCK_*.log
pause
