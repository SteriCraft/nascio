@echo off
rem Opens the Nascio viewer on a generated world.
rem Usage: viewer.bat [world_dir] [viewer options...]   (default: out\m4\world-42)
rem   viewer.bat
rem   viewer.bat out\m4\world-7
rem   viewer.bat --fly --day 15 --hour 18 --pos 21000,1500
rem   viewer.bat out\m4\world-42 --map-layer temperature --day 196
rem Options: --fly --pos x,z --day D --hour H --map-layer NAME --no-shadows --curvature-radius M
rem Uses a separate target dir so it does not lock the build that Claude is using.
setlocal
cd /d "%~dp0"

rem The raw command line keeps the commas of `--pos x,z` (shift/%%1 would split them).
set "WORLD=out\m4\world-42"
set "ARGS= %*"
set "FIRST=%~1"
if not "%FIRST%"=="" if not "%FIRST:~0,1%"=="-" (
    set "WORLD=%~1"
    call set "ARGS=%%ARGS:*%1=%%"
)

if not exist "%WORLD%\world.toml" (
    echo World "%WORLD%" not found. Generating seed 42, full resolution, about 70 s...
    cargo run -p nascio-worldgen --release --target-dir target-viewer -- generate --seed 42 --out "%WORLD%"
    if errorlevel 1 goto :fail
)

cargo run -p nascio-viewer --release --target-dir target-viewer -- "%WORLD%"%ARGS%
if errorlevel 1 goto :fail
exit /b 0

:fail
echo.
echo Viewer failed.
pause
exit /b 1
