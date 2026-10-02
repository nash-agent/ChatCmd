@echo off
setlocal
cd /d "%~dp0"

echo.
echo ChatCMD / Astra Workspace - automatic Windows setup
echo.
echo Automatic:
echo   prerequisites ^> web build ^> Rust build ^> install ^> startup ^> health check
echo.
echo Manual after setup:
echo   Load unpacked browser bridge ^> ChatGPT sign-in ^> per-PC MCP access code ^> project folders
echo.

powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\setup-pc2.ps1"
set "EC=%ERRORLEVEL%"

echo.

if not "%EC%"=="0" (
  echo SETUP FAILED with exit code %EC%.
  echo Fix the error shown above, then run SETUP_PC2.cmd again.
) else (
  echo SETUP FINISHED.
)

echo.
pause
exit /b %EC%
