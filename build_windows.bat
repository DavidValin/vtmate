@echo off
setlocal EnableDelayedExpansion

REM ==========================================================
REM  CONFIG
REM ==========================================================

set "BIN_BASE=ai-mate"
set "PROJECT_ROOT=%~dp0"
set "DIST_DIR=%PROJECT_ROOT%dist"
set "TARGET_DIR=%PROJECT_ROOT%target-cross"
set "VENDOR_DIR=%PROJECT_ROOT%vendor"

set "ESPEAK_SRC=%VENDOR_DIR%\espeak-ng"
set "ESPEAK_BUILD=%ESPEAK_SRC%\build-msvc"
set "ESPEAK_INSTALL=%ESPEAK_BUILD%\install"

set "OPENBLAS_SRC=%VENDOR_DIR%\openblas-src"
set "OPENBLAS_BUILD=%OPENBLAS_SRC%\build-msvc"
set "OPENBLAS_INSTALL=%OPENBLAS_BUILD%\install"

set "ONNX_SRC=%VENDOR_DIR%\onnxruntime"
set "ONNX_BUILD=%ONNX_SRC%\build-static"

REM ==========================================================
REM  CLEAN OLD BUILDS
REM ==========================================================

rmdir /s /q "%ESPEAK_BUILD%" 2>nul
rmdir /s /q "%OPENBLAS_BUILD%" 2>nul
rmdir /s /q "%ONNX_BUILD%" 2>nul
rmdir /s /q "%PROJECT_ROOT%target" 2>nul
rmdir /s /q "%TARGET_DIR%" 2>nul

REM ==========================================================
REM  CHECK REQUIRED TOOLS
REM ==========================================================

where cl.exe >nul 2>nul || (echo ERROR: Open "x64 Native Tools Command Prompt for VS" & exit /b 1)
where cmake >nul 2>nul || (echo ERROR: cmake not found & exit /b 1)
where git >nul 2>nul || (echo ERROR: git not found & exit /b 1)
where cargo >nul 2>nul || (echo ERROR: cargo not found & exit /b 1)

REM ==========================================================
REM  RUST CRT (Dynamic, NOT /MT)
REM ==========================================================

set "RUSTFLAGS="

REM ==========================================================
REM  DETERMINE VARIANT
REM ==========================================================

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
echo ============================================
echo Building variant: %VARIANT%
echo ============================================
echo.

mkdir "%TARGET_DIR%\%VARIANT%" >nul 2>nul
mkdir "%DIST_DIR%" >nul 2>nul
mkdir "%VENDOR_DIR%" >nul 2>nul

REM ==========================================================
REM  BUILD ESPEAK NG (STATIC, Dynamic CRT)
REM ==========================================================

if not exist "%ESPEAK_INSTALL%\lib\espeak-ng.lib" (

    echo === Building eSpeak NG (Static, Dynamic CRT /MD) ===

    if not exist "%ESPEAK_SRC%" (
        git clone https://github.com/espeak-ng/espeak-ng "%ESPEAK_SRC%" || exit /b 1
    )

    cmake -S "%ESPEAK_SRC%" ^
          -B "%ESPEAK_BUILD%" ^
          -G "Visual Studio 17 2022" ^
          -A x64 ^
          -DCMAKE_BUILD_TYPE=Release ^
          -DCMAKE_INSTALL_PREFIX="%ESPEAK_INSTALL%" ^
          -DBUILD_SHARED_LIBS=OFF ^
          -DESPEAKNG_BUILD_TESTS=OFF ^
          -DESPEAKNG_BUILD_EXAMPLES=OFF ^
          -DESPEAKNG_BUILD_PROGRAM=OFF ^
          -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL ^
          || exit /b 1

    cmake --build "%ESPEAK_BUILD%" --config Release --target INSTALL || exit /b 1
)

REM ==========================================================
REM  BUILD OPENBLAS (OPTIONAL STATIC, Dynamic CRT)
REM ==========================================================

if "%WIN_WITH_OPENBLAS%"=="1" (

    if not exist "%OPENBLAS_INSTALL%\lib\libopenblas.lib" (

        echo === Building OpenBLAS (Static, Dynamic CRT /MD) ===

        if not exist "%OPENBLAS_SRC%" (
            git clone --branch v0.3.30 --single-branch ^
            https://github.com/xianyi/OpenBLAS.git "%OPENBLAS_SRC%" || exit /b 1
        )

        cmake -S "%OPENBLAS_SRC%" ^
              -B "%OPENBLAS_BUILD%" ^
              -G "Visual Studio 17 2022" ^
              -A x64 ^
              -DCMAKE_BUILD_TYPE=Release ^
              -DBUILD_SHARED_LIBS=OFF ^
              -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL ^
              -DCMAKE_INSTALL_PREFIX="%OPENBLAS_INSTALL%" ^
              || exit /b 1

        cmake --build "%OPENBLAS_BUILD%" --config Release --target INSTALL || exit /b 1
    )
)

REM ==========================================================
REM  BUILD ONNX RUNTIME (STATIC, Dynamic CRT)
REM ==========================================================

if not exist "%ONNX_BUILD%\Release\onnxruntime.lib" (

    echo === Building ONNX Runtime (Static, Dynamic CRT /MD) ===

    if not exist "%ONNX_SRC%" (
        git clone --recursive https://github.com/microsoft/onnxruntime "%ONNX_SRC%" || exit /b 1
    )

    pushd "%ONNX_SRC%"
    git submodule update --init --recursive --force || exit /b 1
    popd

    set "ONNX_CUDA_FLAG=OFF"
    set "ONNX_VULKAN_FLAG=OFF"

    if "%WIN_WITH_CUDA%"=="1" set "ONNX_CUDA_FLAG=ON"
    if "%WIN_WITH_VULKAN%"=="1" set "ONNX_VULKAN_FLAG=ON"

    cmake -S "%ONNX_SRC%\cmake" ^
          -B "%ONNX_BUILD%" ^
          -G "Visual Studio 17 2022" ^
          -A x64 ^
          -DCMAKE_BUILD_TYPE=Release ^
          -DBUILD_SHARED_LIBS=OFF ^
          -Donnxruntime_BUILD_SHARED_LIB=OFF ^
          -Donnxruntime_MSVC_STATIC_RUNTIME=OFF ^
          -Donnxruntime_USE_CUDA=%ONNX_CUDA_FLAG% ^
          -Donnxruntime_USE_VULKAN=%ONNX_VULKAN_FLAG% ^
          -Donnxruntime_BUILD_UNIT_TESTS=OFF ^
          -Donnxruntime_BUILD_TESTS=OFF ^
          -Donnxruntime_ENABLE_TESTING=OFF ^
          -DBUILD_TESTING=OFF ^
          || exit /b 1

    cmake --build "%ONNX_BUILD%" --config Release || exit /b 1
)

REM ==========================================================
REM  EXPORT ENVIRONMENT
REM ==========================================================

set "ESPEAKNG_INCLUDE_DIR=%ESPEAK_INSTALL%\include"
set "ESPEAKNG_LIB_DIR=%ESPEAK_INSTALL%\lib"

set "OPENBLAS_INCLUDE_DIR=%OPENBLAS_INSTALL%\include"
set "OPENBLAS_LIB_DIR=%OPENBLAS_INSTALL%\lib"

set "ONNXRUNTIME_INCLUDE_DIR=%ONNX_SRC%\include"
set "ONNXRUNTIME_LIB_DIR=%ONNX_BUILD%\Release"

REM ==========================================================
REM  BUILD RUST (DYNAMIC CRT)
REM ==========================================================

set "TARGET=x86_64-pc-windows-msvc"

cargo build --release --target %TARGET% || exit /b 1

set "SRC_BIN=%PROJECT_ROOT%target\%TARGET%\release\%BIN_BASE%.exe"
set "DST_BIN=%TARGET_DIR%\%VARIANT%\%BIN_BASE%-%VARIANT%.exe"

if not exist "%SRC_BIN%" (
    echo ERROR: Built binary not found.
    exit /b 1
)

copy /Y "%SRC_BIN%" "%DST_BIN%" >nul

echo.
echo ============================================
echo SUCCESS: %DST_BIN%
echo ============================================
echo.
echo NOTE: This binary depends on ucrtbase.dll and vcruntime140.dll
echo       which are included in Windows 10+. Users must have them.
echo ============================================

exit /b 0
