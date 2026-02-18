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
set "ONNX_SRC=%VENDOR_DIR%\onnxruntime"
set "ONNX_BUILD=%ONNX_SRC%\build-static"

REM ===== Check required tools =====
where cl.exe >nul 2>nul || (echo ERROR: Open "x64 Native Tools Command Prompt for VS" first & exit /b 1)
where cmake >nul 2>nul || (echo ERROR: cmake not found & exit /b 1)
where git >nul 2>nul || (echo ERROR: git not found & exit /b 1)
where powershell >nul 2>nul || (echo ERROR: powershell not found & exit /b 1)
where cargo >nul 2>nul || (echo ERROR: cargo not found & exit /b 1)

REM ===== Force static MSVC runtime for Rust =====
set "RUSTFLAGS=-C target-feature=+crt-static"

REM ===== Determine Variant =====
set "VARIANT=%~1"
if "%VARIANT%"=="" set "VARIANT=cpu"

if "%VARIANT%"=="cpu" (
    set WIN_WITH_OPENBLAS=0
    set WIN_WITH_CUDA=0
    set WIN_WITH_VULKAN=0
) else if "%VARIANT%"=="openblas" (
    set WIN_WITH_OPENBLAS=1
    set WIN_WITH_CUDA=0
    set WIN_WITH_VULKAN=0
) else if "%VARIANT%"=="vulkan" (
    set WIN_WITH_OPENBLAS=0
    set WIN_WITH_CUDA=0
    set WIN_WITH_VULKAN=1
) else if "%VARIANT%"=="cuda" (
    set WIN_WITH_OPENBLAS=0
    set WIN_WITH_CUDA=1
    set WIN_WITH_VULKAN=0
) else (
    echo ERROR: Unknown variant "%VARIANT%"
    exit /b 1
)

echo.
echo === Building variant: %VARIANT% ===
echo WIN_WITH_OPENBLAS=%WIN_WITH_OPENBLAS%
echo WIN_WITH_CUDA=%WIN_WITH_CUDA%
echo WIN_WITH_VULKAN=%WIN_WITH_VULKAN%
echo.

REM ===== Prepare directories =====
mkdir "%TARGET_DIR%\%VARIANT%" >nul 2>nul
mkdir "%DIST_DIR%" >nul 2>nul
mkdir "%VENDOR_DIR%" >nul 2>nul

REM ===== eSpeak NG Build =====
if not exist "%ESPEAK_INSTALL%\lib\espeak-ng.lib" (
    echo === Building eSpeak NG (MSVC) ===
    if not exist "%ESPEAK_SRC%" (
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
          -DESPEAKNG_BUILD_EXAMPLES=OFF ^
          -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded
    if errorlevel 1 exit /b 1
    cmake --build "%ESPEAK_BUILD%" --config Release --target INSTALL
    if errorlevel 1 exit /b 1
    popd
)

REM ===== Download OpenBLAS if needed =====
if "%WIN_WITH_OPENBLAS%"=="1" (
    if not exist "%OPENBLAS_DIR%\lib\libopenblas.a" (
        echo Downloading OpenBLAS...
        powershell -Command "Invoke-WebRequest -Uri '%OPENBLAS_URL%' -OutFile '%OPENBLAS_ZIP%'"
        if errorlevel 1 exit /b 1
        echo Extracting OpenBLAS...
        powershell -Command "Expand-Archive -LiteralPath '%OPENBLAS_ZIP%' -DestinationPath '%VENDOR_DIR%' -Force"
        if errorlevel 1 exit /b 1
        move /Y "%VENDOR_DIR%\OpenBLAS-0.3.30-x64-64" "%OPENBLAS_DIR%"
        del "%OPENBLAS_ZIP%"
        echo OpenBLAS ready.
    )
)

REM ===== ONNX Runtime Static Build =====
if not exist "%ONNX_BUILD%\Release\onnxruntime.lib" (
    echo === Building ONNX Runtime (Static, MultiThreaded) ===

    REM Clone ONNX Runtime if missing
    if not exist "%ONNX_SRC%" (
        echo Cloning ONNX Runtime repository...
        git clone --recursive https://github.com/microsoft/onnxruntime "%ONNX_SRC%"
        if errorlevel 1 exit /b 1
    )

    REM Initialize submodules safely (without fetch/reset)
    pushd "%ONNX_SRC%"
    git submodule sync
    git submodule update --init --recursive --force
    if errorlevel 1 exit /b 1
    popd

    REM Set dynamic CUDA/Vulkan flags
    set "ONNX_CUDA_FLAG=OFF"
    set "ONNX_VULKAN_FLAG=OFF"
    if "%WIN_WITH_CUDA%"=="1" set "ONNX_CUDA_FLAG=ON"
    if "%WIN_WITH_VULKAN%"=="1" set "ONNX_VULKAN_FLAG=ON"

    REM Determine correct CMake source path
    set "ONNX_CMAKE_SRC=%ONNX_SRC%"
    if not exist "%ONNX_CMAKE_SRC%\CMakeLists.txt" (
        REM fallback to subfolder (common in recent ONNX Runtime)
        set "ONNX_CMAKE_SRC=%ONNX_SRC%\onnxruntime"
    )

    REM Build ONNX Runtime
    mkdir "%ONNX_BUILD%" >nul 2>nul
    pushd "%ONNX_BUILD%"
    cmake -G "Visual Studio 17 2022" ^
          -A x64 ^
          -DCMAKE_BUILD_TYPE=Release ^
          -DBUILD_SHARED_LIBS=OFF ^
          -Donnxruntime_USE_CUDA=%ONNX_CUDA_FLAG% ^
          -Donnxruntime_USE_VULKAN=%ONNX_VULKAN_FLAG% ^
          -Donnxruntime_BUILD_SHARED_LIB=OFF ^
          -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded ^
          "%ONNX_CMAKE_SRC%"
    if errorlevel 1 exit /b 1
    cmake --build . --config Release
    if errorlevel 1 exit /b 1
    popd
)

REM ===== Export environment =====
set "ESPEAKNG_INCLUDE_DIR=%ESPEAK_INSTALL%\include"
set "ESPEAKNG_LIB_DIR=%ESPEAK_INSTALL%\lib"
set "ONNXRUNTIME_LIB_DIR=%ONNX_BUILD%\Release"
set "ONNXRUNTIME_INCLUDE_DIR=%ONNX_SRC%\include"

REM ===== Build Rust target =====
set "TARGET=x86_64-pc-windows-msvc"
set "DST_BIN=%TARGET_DIR%\%VARIANT%\%BIN_BASE%-%VARIANT%.exe"

cargo build --release --target %TARGET%
if errorlevel 1 exit /b 1

REM ===== Copy binary =====
set "SRC_BIN=%PROJECT_ROOT%target\%TARGET%\release\%BIN_BASE%.exe"
if not exist "%SRC_BIN%" (
    echo ERROR: Built binary not found at %SRC_BIN%
    exit /b 1
)

copy /Y "%SRC_BIN%" "%DST_BIN%" >nul
echo Built %DST_BIN%
exit /b 0
