@echo off
setlocal enabledelayedexpansion

REM ===== Config =====
set BIN_NAME=ai-mate
set PROJECT_ROOT=%~dp0
set DIST_DIR=%PROJECT_ROOT%dist
set PKG_DIR=%DIST_DIR%\packages
set TARGET_DIR=%PROJECT_ROOT%target-cross
set ASSETS_DIR=%PROJECT_ROOT%assets
set ESPEAK_ARCHIVE=%ASSETS_DIR%\espeak-ng-data.tar.gz

REM Variant toggles (defaults)
if "%WIN_WITH_OPENBLAS%"=="" set WIN_WITH_OPENBLAS=1
if "%WIN_WITH_VULKAN%"=="" set WIN_WITH_VULKAN=1
if "%WIN_WITH_CUDA%"=="" set WIN_WITH_CUDA=0

REM ===== Preflight =====
where cl.exe >nul 2>nul
if errorlevel 1 (
  echo ERROR: cl.exe not found. Open "x64 Native Tools Command Prompt for VS" and retry.
  exit /b 1
)

where cargo >nul 2>nul
if errorlevel 1 (
  echo ERROR: cargo not found in PATH.
  exit /b 1
)

REM Read version from Cargo.toml
for /f "tokens=2 delims==" %%A in ('findstr /R /C:"^[ ]*version[ ]*=[ ]*\"" "%PROJECT_ROOT%Cargo.toml"') do (
  set RAW=%%A
)
set VERSION=%RAW:"=%
if "%VERSION%"=="" (
  echo ERROR: Failed to read version from Cargo.toml
  exit /b 1
)

mkdir "%DIST_DIR%" >nul 2>nul
mkdir "%PKG_DIR%" >nul 2>nul
mkdir "%TARGET_DIR%" >nul 2>nul
mkdir "%ASSETS_DIR%" >nul 2>nul

echo Version: %VERSION%
echo WIN_WITH_OPENBLAS=%WIN_WITH_OPENBLAS% WIN_WITH_VULKAN=%WIN_WITH_VULKAN% WIN_WITH_CUDA=%WIN_WITH_CUDA%

REM ===== Ensure embedded asset exists (via Docker if missing) =====
if exist "%ESPEAK_ARCHIVE%" (
  echo Found embedded asset: %ESPEAK_ARCHIVE%
) else (
  where docker >nul 2>nul
  if errorlevel 1 (
    echo ERROR: Docker not found and %ESPEAK_ARCHIVE% is missing.
    exit /b 1
  )
  call :ensure_espeak_asset
  if errorlevel 1 exit /b 1
)

REM ===== Features =====
set FEATURES_COMMON=whisper-logs
set FEATURES_CPU=%FEATURES_COMMON%
set FEATURES_OPENBLAS=%FEATURES_COMMON%,whisper-openblas
set FEATURES_VULKAN=%FEATURES_COMMON%,whisper-vulkan
set FEATURES_CUDA=%FEATURES_COMMON%,whisper-cuda

set TARGET=x86_64-pc-windows-msvc

REM ===== Build variants =====
call :build_variant cpu "%FEATURES_CPU%" "%DIST_DIR%\%BIN_NAME%-%VERSION%-windows-msvc-amd64-cpu.exe"
if errorlevel 1 exit /b 1

if "%WIN_WITH_OPENBLAS%"=="1" (
  call :build_variant openblas "%FEATURES_OPENBLAS%" "%DIST_DIR%\%BIN_NAME%-%VERSION%-windows-msvc-amd64-openblas.exe"
  if errorlevel 1 exit /b 1
)

if "%WIN_WITH_VULKAN%"=="1" (
  call :build_variant vulkan "%FEATURES_VULKAN%" "%DIST_DIR%\%BIN_NAME%-%VERSION%-windows-msvc-amd64-vulkan.exe"
  if errorlevel 1 exit /b 1
)

if "%WIN_WITH_CUDA%"=="1" (
  call :build_variant cuda "%FEATURES_CUDA%" "%DIST_DIR%\%BIN_NAME%-%VERSION%-windows-msvc-amd64-cuda.exe"
  if errorlevel 1 exit /b 1
)

REM ===== Package =====
echo.
echo Packaging tar.gz + SHA256...
for %%F in ("%DIST_DIR%\%BIN_NAME%-%VERSION%-windows-msvc-amd64-*.exe") do (
  call :package_one "%%~fF"
  if errorlevel 1 exit /b 1
)

echo.
echo DONE
exit /b 0

REM ===== Functions =====

:ensure_espeak_asset
set TMPDIR=%TEMP%\espeak_asset_%RANDOM%
mkdir "%TMPDIR%" >nul 2>nul

set DF=%TMPDIR%\Dockerfile.espeak.asset
> "%DF%" (
  echo FROM ubuntu:noble
  echo ENV DEBIAN_FRONTEND=noninteractive
  echo RUN apt-get update ^&^& apt-get install -y --no-install-recommends ca-certificates tar gzip espeak-ng-data ^&^& rm -rf /var/lib/apt/lists/*
  echo WORKDIR /out
)

set IMG=local/%BIN_NAME%-espeak-asset:%VERSION%-%RANDOM%

docker build --pull --platform=linux/amd64 -f "%DF%" -t "%IMG%" "%TMPDIR%"
if errorlevel 1 exit /b 1

if exist "%ESPEAK_ARCHIVE%" del /f /q "%ESPEAK_ARCHIVE%"

docker run --rm --platform=linux/amd64 ^
  -v "%ASSETS_DIR%:/out" -w /out ^
  "%IMG%" ^
  bash -lc "set -euo pipefail; cp -a /usr/share/espeak-ng-data ./espeak-ng-data; rm -rf ./espeak-ng-data/voices; tar -czf espeak-ng-data.tar.gz espeak-ng-data; rm -rf ./espeak-ng-data"
if errorlevel 1 exit /b 1

docker image rm -f "%IMG%" >nul 2>nul

rmdir /s /q "%TMPDIR%" >nul 2>nul

if not exist "%ESPEAK_ARCHIVE%" (
  echo ERROR: failed to generate %ESPEAK_ARCHIVE%
  exit /b 1
)
echo Generated: %ESPEAK_ARCHIVE%
exit /b 0

:build_variant
set VARIANT=%~1
set FEATS=%~2
set OUT=%~3

set CARGO_TARGET_DIR=%TARGET_DIR%\windows-msvc-amd64-%VARIANT%

echo.
echo == Building [%VARIANT%] features: %FEATS% ==
cargo build --release --target %TARGET% --no-default-features --features "%FEATS%"
if errorlevel 1 exit /b 1

copy /y "%CARGO_TARGET_DIR%\%TARGET%\release\%BIN_NAME%.exe" "%OUT%" >nul
if errorlevel 1 (
  echo ERROR: Failed to copy output for %VARIANT%
  exit /b 1
)
echo Built: %OUT%
exit /b 0

:package_one
set SRC=%~1
set BASE=%~nx1
set TGZ=%PKG_DIR%\%BASE%.tar.gz
set SHA=%PKG_DIR%\%BASE%.tar.gz.sha256

powershell -NoProfile -Command ^
  "tar -C (Split-Path -Parent '%SRC%') -czf '%TGZ%' (Split-Path -Leaf '%SRC%');" ^
  " $h=(Get-FileHash -Algorithm SHA256 '%TGZ%').Hash.ToLower();" ^
  " \"$h  %BASE%.tar.gz\" | Out-File -Encoding ascii '%SHA%';"
if errorlevel 1 exit /b 1

exit /b 0
