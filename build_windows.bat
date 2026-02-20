@echo off
setlocal EnableDelayedExpansion

REM ==========================================================
REM CONFIG
REM ==========================================================
set "BIN_BASE=ai-mate"
set "PROJECT_ROOT=%~dp0"
set "DIST_DIR=%PROJECT_ROOT%dist"
set "TARGET_DIR=%PROJECT_ROOT%target-cross"
set "VENDOR_DIR=%PROJECT_ROOT%vendor"
set "ESPEAK_SRC=%VENDOR_DIR%\espeak-ng"
set "ESPEAK_BUILD=%ESPEAK_SRC%\build-msvc"
set "ESPEAK_INSTALL=%ESPEAK_BUILD%\install"
set "OPENBLAS_DIR=%VENDOR_DIR%\openblas"
set "OPENBLAS_URL=https://github.com/OpenMathLib/OpenBLAS/releases/download/v0.3.30/OpenBLAS-0.3.30-x64-64.zip"
set "OPENBLAS_ZIP=%VENDOR_DIR%\openblas.zip"
set "ONNX_SRC=%VENDOR_DIR%\onnxruntime"
set "ONNX_BUILD=%ONNX_SRC%\build-static"
set "UPLOAD_ENABLED=1"

REM ==========================================================
REM CLEAN OLD BUILDS
REM ==========================================================
rmdir /s /q "%ESPEAK_BUILD%" 2>nul
rmdir /s /q "%OPENBLAS_DIR%" 2>nul
rmdir /s /q "%ONNX_BUILD%" 2>nul
rmdir /s /q "%PROJECT_ROOT%target" 2>nul
rmdir /s /q "%TARGET_DIR%" 2>nul
rmdir /s /q "%DIST_DIR%" 2>nul

REM ==========================================================
REM CHECK REQUIRED TOOLS
REM ==========================================================
where cl.exe >nul 2>nul || (echo ERROR: Open "x64 Native Tools Command Prompt for VS" & exit /b 1)
where cmake >nul 2>nul || (echo ERROR: cmake not found & exit /b 1)
where git >nul 2>nul || (echo ERROR: git not found & exit /b 1)
where cargo >nul 2>nul || (echo ERROR: cargo not found & exit /b 1)
where powershell >nul 2>nul || (echo ERROR: powershell not found & exit /b 1)

REM ==========================================================
REM USE DYNAMIC RUST CRT TO MATCH ONNX /MD
REM ==========================================================
REM Set Rust optimization and single-thread compilation
set "RUSTFLAGS=-C opt-level=3"
set "CARGO_BUILD_JOBS=1"

REM ==========================================================
REM DETERMINE VARIANT
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
REM ENSURE CUDA TOOLKIT IF REQUIRED (BUILD-TIME)
REM ==========================================================
if "%WIN_WITH_CUDA%"=="1" (
    where nvcc >nul 2>nul
    if errorlevel 1 (
        echo CUDA not detected. Installing CUDA Toolkit for build...
        set "CUDA_VERSION=12.3.2"
        set "CUDA_INSTALLER=%TEMP%\cuda_installer.exe"
        set "CUDA_URL=https://developer.download.nvidia.com/compute/cuda/%CUDA_VERSION%/network_installers/cuda_%CUDA_VERSION%_windows_network.exe"
        powershell -Command "Invoke-WebRequest -Uri '%CUDA_URL%' -OutFile '%CUDA_INSTALLER%'"
        if errorlevel 1 (
            echo ERROR: Failed to download CUDA installer.
            exit /b 1
        )
        REM Silent install nvcc + runtime "%CUDA_INSTALLER%" -s nvcc_%CUDA_VERSION% cudart_%CUDA_VERSION%
        if errorlevel 1 (
            echo ERROR: CUDA installation failed.
            exit /b 1
        )
        set "CUDA_PATH=C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.3"
        set "PATH=%CUDA_PATH%\bin;%PATH%"
        set "CUDAToolkit_ROOT=%CUDA_PATH%"
        where nvcc >nul 2>nul
        if errorlevel 1 (
            echo ERROR: CUDA installed but nvcc not found.
            exit /b 1
        )
        echo CUDA successfully installed for build.
    ) else (
        echo CUDA already present.
        for %%I in (nvcc.exe) do set "CUDA_BIN=%%~dp$PATH:I"
        for %%I in ("!CUDA_BIN!..\") do set "CUDA_PATH=%%~fI"
        set "CUDAToolkit_ROOT=!CUDA_PATH!"
    )
    echo CUDA_PATH = %CUDA_PATH%
)

REM ==========================================================
REM BUILD ESPEAK NG (STATIC LIB, DYNAMIC CRT /MD)
REM ==========================================================
if not exist "%ESPEAK_INSTALL%\lib\espeak-ng.lib" (
    echo === Building eSpeak NG ===
    if not exist "%ESPEAK_SRC%" git clone https://github.com/espeak-ng/espeak-ng "%ESPEAK_SRC%" || exit /b 1
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
          -DCMAKE_C_FLAGS="/MD" ^
          -DCMAKE_CXX_FLAGS="/MD"
    cmake --build "%ESPEAK_BUILD%" --config Release --target INSTALL || exit /b 1
)

REM ==========================================================
REM BUILD OPENBLAS STATIC AND LINK (unchanged)
REM ==========================================================
if "%WIN_WITH_OPENBLAS%"=="1" (
    if not exist "%OPENBLAS_DIR%\lib\libopenblas.lib" (
        echo === Building OpenBLAS static with MSVC ===
        if not exist "%OPENBLAS_DIR%" git clone --branch v0.3.30 https://github.com/xianyi/OpenBLAS "%OPENBLAS_DIR%" || exit /b 1
        mkdir "%OPENBLAS_DIR%\build" 2>nul
        cmake -S "%OPENBLAS_DIR%" ^
              -B "%OPENBLAS_DIR%\build" ^
              -G "Visual Studio 17 2022" ^
              -A x64 ^
              -DBUILD_SHARED_LIBS=OFF ^
              -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded ^
              -DCMAKE_INSTALL_PREFIX="%OPENBLAS_DIR%\install"
        cmake --build "%OPENBLAS_DIR%\build" --config Release --target INSTALL || exit /b 1
        echo OpenBLAS static library ready.
    )
    set "OPENBLAS_LIB_DIR=%OPENBLAS_DIR%\install\lib"
    set "OPENBLAS_INCLUDE_DIR=%OPENBLAS_DIR%\install\include"
    set "RUSTFLAGS=%RUSTFLAGS% -C link-args=\"/LIBPATH:%OPENBLAS_LIB_DIR% libopenblas.lib\" -C include=%OPENBLAS_INCLUDE_DIR%"
)

REM ==========================================================
REM BUILD ONNX RUNTIME (STATIC LIB, DYNAMIC CRT /MD)
REM ==========================================================
if not exist "%ONNX_BUILD%\Release\onnxruntime.lib" (
    echo === Building ONNX Runtime ===
    if not exist "%ONNX_SRC%" git clone --recursive https://github.com/microsoft/onnxruntime "%ONNX_SRC%" || exit /b 1
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
          -Donnxruntime_USE_EIGEN=ON ^
          -Donnxruntime_USE_OPENMP=OFF ^
          -Donnxruntime_BUILD_UNIT_TESTS=OFF ^
          -Donnxruntime_BUILD_TESTS=OFF ^
          -Donnxruntime_ENABLE_TESTING=OFF ^
          -DBUILD_TESTING=OFF
    cmake --build "%ONNX_BUILD%" --config Release || exit /b 1
)

REM ==========================================================
REM EXPORT ENVIRONMENT
REM ==========================================================
set "ESPEAKNG_INCLUDE_DIR=%ESPEAK_INSTALL%\include"
set "ESPEAKNG_LIB_DIR=%ESPEAK_INSTALL%\lib"
set "ONNXRUNTIME_INCLUDE_DIR=%ONNX_SRC%\include"
set "ONNXRUNTIME_LIB_DIR=%ONNX_BUILD%\Release"

REM ==========================================================
REM BUILD RUST BINARY
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
echo Built %DST_BIN%

REM ==========================================================
REM UPLOAD ARTIFACT IMMEDIATELY
REM ==========================================================
if "%UPLOAD_ENABLED%"=="1" (
    echo Uploading artifact for %VARIANT%...
    gh run upload-artifact "%BIN_BASE%-%VARIANT%" "%DST_BIN%" || echo WARNING: Upload failed
)

echo.
echo SUCCESS: %DST_BIN%
exit /b 0