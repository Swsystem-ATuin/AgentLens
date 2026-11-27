@echo off
"%~dp0agent-lens.exe" personal
if errorlevel 1 (
    echo.
    pause
    exit /b 1
)
