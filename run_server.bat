@echo off
rem Starts a local Nascio server (127.0.0.1:7777) for development.
rem Usage: run_server.bat [world_dir] [server options...]   (default: out\gisements\world-42)
rem   run_server.bat
rem   run_server.bat out\m7\world-42 --data out\s3-data
rem   run_server.bat --bind 0.0.0.0:7777 --server-id lab
rem Deltas, accounts and the certificate go to <world_dir>\data unless --data is given.
rem Registration is open (--open-registration): the viewer, netbot and swarm log in with a
rem name and a secret, without a site token. Connect with:
rem   viewer.bat <world_dir> --connect 127.0.0.1:7777 --cert <world_dir>\data\server\cert.der --name me --secret password1
rem Uses a separate target dir so a running server does not lock the other builds.
setlocal
cd /d "%~dp0"

set "WORLD=out\gisements\world-42"
set "ARGS= %*"
set "FIRST=%~1"
if not "%FIRST%"=="" if not "%FIRST:~0,1%"=="-" (
    set "WORLD=%~1"
    call set "ARGS=%%ARGS:*%1=%%"
)

if not exist "%WORLD%\world.toml" (
    echo World "%WORLD%" not found. Generate it first, for example:
    echo   cargo run -p nascio-worldgen --release -- generate --seed 42 --scale 1 --out %WORLD%
    goto :fail
)

rem Default data folder, unless the options name one.
set "DATA= --data %WORLD%\data"
echo %ARGS% | findstr /c:"--data" >nul && set "DATA="

cargo run -p nascio-server --release --target-dir target-server -- "%WORLD%"%DATA% --open-registration%ARGS%
if errorlevel 1 goto :fail
exit /b 0

:fail
echo.
echo Server failed.
pause
exit /b 1
