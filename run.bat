@echo off
cd /d "%~dp0"
taskkill /im shoter.exe /f >nul 2>&1
echo Building Shoter...
cargo build > build.log 2>&1
if errorlevel 1 (
  type build.log
  echo.
  echo BUILD FAILED - see build.log
  pause
  exit /b 1
)
start "" target\debug\shoter.exe
echo Shoter is running. Press PrtSc.
timeout /t 3 >nul
