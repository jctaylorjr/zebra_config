@echo off
rem Builds Zebra Configure on this PC: see build-windows.ps1. Double-click it, or run it with
rem -Installer to also build the setup program.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows.ps1" %*
set code=%errorlevel%
echo.
pause
exit /b %code%
