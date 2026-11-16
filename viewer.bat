@echo off
rem Opens the Nascio viewer on a generated world.
rem Usage: viewer.bat [world_dir]   (default: out\world-42)
rem Uses a separate target dir so it does not lock the build that Claude is using.
cd /d "%~dp0"
set "WORLD=%~1"
if "%WORLD%"=="" set "WORLD=out\world-42"

if not exist "%WORLD%" (
    echo World "%WORLD%" not found. Generating seed 42...
    cargo run -p nascio-worldgen --release --target-dir target-viewer -- generate --seed 42 --out "%WORLD%"
    if errorlevel 1 goto :fail
)

cargo run -p nascio-viewer --release --target-dir target-viewer -- "%WORLD%"
if errorlevel 1 goto :fail
exit /b 0

:fail
echo.
echo Viewer failed.
pause
exit /b 1
