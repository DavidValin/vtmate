@echo off
setlocal enabledelayedexpansion

REM ===== Config =====
set "BIN_NAME=ai-mate"
set "PROJECT_ROOT=%~dp0"
set "DIST_DIR=%PROJECT_ROOT%dist"
set "TARGET_DIR=%PROJECT_ROOT%target-cross"
set "ASSETS_DIR=%PROJECT_ROOT%assets"
set "VENDOR_DIR=%PROJECT_ROOT%vendor"
set "ESPEAK_SRC=%VENDOR_DIR%\espeak-ng"
set "ESPEAK_BUILD=%ESPEAK_SRC%\build-msvc"
set "ESPEAK_INSTALL=%ESPEAK_BUILD%\install"

REM ===== Toolchain Checks =====
where cl.exe >nul 2>nul
if errorlevel 1 (
    echo ERROR: Open "x64 Native Tools Command Prompt for VS" first.
    exit /b 1
)

where cmake >nul 2>nul
if errorlevel 1 (
    echo ERROR: cmake not found.
    exit /b 1
)

where git >nul 2>nul
if errorlevel 1 (
    echo ERROR: git not found.
    exit /b 1
)

REM ===== eSpeak NG Build =====
if not exist "%ESPEAK_INSTALL%\lib\espeak-ng.lib" (
    echo.
    echo === Building eSpeak NG (MSVC) ===

    if not exist "%ESPEAK_SRC%" (
        mkdir "%VENDOR_DIR%" >nul 2>nul
        git clone https://github.com/espeak-ng/espeak-ng "%ESPEAK_SRC%"
        if errorlevel 1 exit /b 1
    )

    pushd "%ESPEAK_SRC%"

    cmake -S . ^
          -B "%ESPEAK_BUILD%" ^
          -G "Visual Studio 17 2022" ^
          -A x64 ^
          -DCMAKE_BUILD_TYPE=Release ^
          -DCMAKE_INSTALL_PREFIX="%ESPEAK_INSTALL%" ^
          -DBUILD_SHARED_LIBS=OFF ^
          -DESPEAKNG_BUILD_TESTS=OFF ^
          -DESPEAKNG_BUILD_EXAMPLES=OFF

    if errorlevel 1 exit /b 1

    cmake --build "%ESPEAK_BUILD%" --config Release --target INSTALL
    if errorlevel 1 exit /b 1

    popd
)

REM ===== Export environment for espeak-rs-sys =====
set "ESPEAKNG_INCLUDE_DIR=%ESPEAK_INSTALL%\include"
set "ESPEAKNG_LIB_DIR=%ESPEAK_INSTALL%\lib"

echo Using eSpeak includes: %ESPEAKNG_INCLUDE_DIR%
echo Using eSpeak lib dir : %ESPEAKNG_LIB_DIR%

REM ===== Rust target =====
set "TARGET=x86_64-pc-windows-msvc"

REM ===== Build Variants =====
if "%WIN_WITH_OPENBLAS%"=="" set "WIN_WITH_OPENBLAS=1"
if "%WIN_WITH_VULKAN%"=="" set "WIN_WITH_VULKAN=1"
if "%WIN_WITH_CUDA%"=="" set "WIN_WITH_CUDA=0"

call :build_variant cpu "%TARGET_DIR%\cpu"
if errorlevel 1 exit /b 1

if "%WIN_WITH_OPENBLAS%"=="1" (
    call :build_variant openblas "%TARGET_DIR%\openblas"
    if errorlevel 1 exit /b 1
)

if "%WIN_WITH_VULKAN%"=="1" (
    call :build_variant vulkan "%TARGET_DIR%\vulkan"
    if errorlevel 1 exit /b 1
)

if "%WIN_WITH_CUDA%"=="1" (
    call :build_variant cuda "%TARGET_DIR%\cuda"
    if errorlevel 1 exit /b 1
)

echo DONE
exit /b 0

REM ===== Functions =====
:build_variant
set "VARIANT=%~1"
set "OUT_DIR=%~2"
echo.
echo == Building %VARIANT% ==
cargo build --release --target %TARGET%
if errorlevel 1 exit /b 1
exit /b 0
