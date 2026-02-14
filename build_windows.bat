@echo off
setlocal enabledelayedexpansion

REM ===== Config =====
set "BIN_BASE=ai-mate"
set "PROJECT_ROOT=%~dp0"
set "DIST_DIR=%PROJECT_ROOT%dist"
set "TARGET_DIR=%PROJECT_ROOT%target-cross"
set "ASSETS_DIR=%PROJECT_ROOT%assets"
set "VENDOR_DIR=%PROJECT_ROOT%vendor"
set "ESPEAK_SRC=%VENDOR_DIR%\espeak-ng"
set "ESPEAK_BUILD=%ESPEAK_SRC%\build-msvc"
set "ESPEAK_INSTALL=%ESPEAK_BUILD%\install"
set "OPENBLAS_DIR=%VENDOR_DIR%\openblas"
set "OPENBLAS_URL=https://github.com/OpenMathLib/OpenBLAS/releases/download/v0.3.30/OpenBLAS-0.3.30-x64-64.zip"
set "OPENBLAS_ZIP=%VENDOR_DIR%\openblas.zip"

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

where powershell >nul 2>nul
if errorlevel 1 (
    echo ERROR: powershell not found.
    exit /b 1
)

REM ===== Force static MSVC runtime =====
set "RUSTFLAGS=-C target-feature=+crt-static"

REM ===== Clean previous builds =====
echo Cleaning previous builds...
rd /s /q "%TARGET_DIR%" 2>nul
rd /s /q "%DIST_DIR%" 2>nul
rd /s /q "%ESPEAK_BUILD%" 2>nul
rd /s /q "%OPENBLAS_DIR%" 2>nul
echo Done cleaning.

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

REM ===== Download OpenBLAS if needed =====
if "%WIN_WITH_OPENBLAS%"=="" set "WIN_WITH_OPENBLAS=1"
if "%WIN_WITH_OPENBLAS%"=="1" (
    if not exist "%OPENBLAS_DIR%\lib\libopenblas.a" (
        echo Downloading OpenBLAS...
        mkdir "%VENDOR_DIR%" >nul 2>nul
        powershell -Command "Invoke-WebRequest -Uri '%OPENBLAS_URL%' -OutFile '%OPENBLAS_ZIP%'"
        if errorlevel 1 exit /b 1

        echo Extracting OpenBLAS...
        powershell -Command "Expand-Archive -LiteralPath '%OPENBLAS_ZIP%' -DestinationPath '%VENDOR_DIR%' -Force"
        if errorlevel 1 exit /b 1

        move /Y "%VENDOR_DIR%\OpenBLAS-v0.3.24-x64" "%OPENBLAS_DIR%"
        del "%OPENBLAS_ZIP%"
        echo OpenBLAS ready.
    )
)

REM ===== Export environment for espeak-rs-sys =====
set "ESPEAKNG_INCLUDE_DIR=%ESPEAK_INSTALL%\include"
set "ESPEAKNG_LIB_DIR=%ESPEAK_INSTALL%\lib"

echo Using eSpeak includes: %ESPEAKNG_INCLUDE_DIR%
echo Using eSpeak lib dir : %ESPEAKNG_LIB_DIR%

REM ===== Rust target =====
set "TARGET=x86_64-pc-windows-msvc"

REM ===== Build Variants =====
if "%WIN_WITH_VULKAN%"=="" set "WIN_WITH_VULKAN=1"
if "%WIN_WITH_CUDA%"=="" set "WIN_WITH_CUDA=1"

call :build_variant cpu
if errorlevel 1 exit /b 1

if "%WIN_WITH_OPENBLAS%"=="1" (
    call :build_variant openblas
    if errorlevel 1 exit /b 1
)

if "%WIN_WITH_VULKAN%"=="1" (
    call :build_variant vulkan
    if errorlevel 1 exit /b 1
)

if "%WIN_WITH_CUDA%"=="1" (
    call :build_variant cuda
    if errorlevel 1 exit /b 1
)

echo.
echo ALL VARIANTS BUILT SUCCESSFULLY!
exit /b 0

REM ===== Functions =====
:build_variant
set "VARIANT=%~1"
set "OUT_DIR=%TARGET_DIR%\%VARIANT%"
echo.
echo === Building %VARIANT% variant ===
mkdir "%OUT_DIR%" >nul 2>nul

REM Build release with Rust
cargo build --release --target %TARGET%
if errorlevel 1 exit /b 1

REM Copy and rename binary
set "SRC_BIN=%PROJECT_ROOT%target\%TARGET%\release\%BIN_BASE%.exe"
set "DST_BIN=%OUT_DIR%\%BIN_BASE%-%VARIANT%.exe"

if not exist "%SRC_BIN%" (
    echo ERROR: Built binary not found at %SRC_BIN%
    exit /b 1
)

copy /Y "%SRC_BIN%" "%DST_BIN%" >nul
echo Built %DST_BIN%
exit /b 0
