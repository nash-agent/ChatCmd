@echo off
setlocal
cd /d "%~dp0"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
set "EC=%ERRORLEVEL%"
echo.
if not "%EC%"=="0" (
  echo Installation failed with exit code %EC%.
) else (
  echo Installation finished.
)
echo.
pause
exit /b %EC%
