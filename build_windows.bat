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
set "OPENBLAS_SRC=%VENDOR_DIR%\openblas-src"
set "OPENBLAS_BUILD=%OPENBLAS_SRC%\build-msvc"
set "OPENBLAS_INSTALL=%OPENBLAS_BUILD%\install"
set "ONNX_SRC=%VENDOR_DIR%\onnxruntime"
set "ONNX_BUILD=%ONNX_SRC%\build-static"

REM ===== Clean old builds =====
rmdir /s /q "%ESPEAK_BUILD%"
rmdir /s /q "%OPENBLAS_BUILD%"
rmdir /s /q "%ONNX_BUILD%"
rmdir /s /q "%PROJECT_ROOT%target"
rmdir /s /q "%PROJECT_ROOT%target-cross"

REM ===== Check required tools =====
where cl.exe >nul 2>nul || (echo ERROR: Open "x64 Native Tools Command Prompt for VS" first & exit /b 1)
where cmake >nul 2>nul || (echo ERROR: cmake not found & exit /b 1)
where git >nul 2>nul || (echo ERROR: git not found & exit /b 1)
where powershell >nul 2>nul || (echo ERROR: powershell not found & exit /b 1)
where cargo >nul 2>nul || (echo ERROR: cargo not found & exit /b 1)

REM ===== Force static MSVC runtime for Rust =====
set "RUSTFLAGS=-Ctarget-feature=+crt-static -C link-arg=/MT -C link-arg=/WX- -C link-arg=/ignore:4217 -C link-arg=/ignore:4286 -C link-arg=libcmt.lib -C link-arg=legacy_stdio_definitions.lib"
set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=%RUSTFLAGS%"

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

REM ===== Patch ONNX Runtime for /MT =====
if exist "%ONNX_SRC%" (
    echo === Patching ONNX Runtime for static /MT CRT ===
    for /R "%ONNX_SRC%" %%f in (*.cmake *.txt) do (
        powershell -Command "(Get-Content '%%f') -replace '/MD','/MT' | Set-Content '%%f'"
        powershell -Command "(Get-Content '%%f') -replace '/MDd','/MTd' | Set-Content '%%f'"
    )
)

REM ===== eSpeak NG Build (Static /MT) =====
if not exist "%ESPEAK_INSTALL%\lib\espeak-ng.lib" (
    echo === Building eSpeak NG (MSVC /MT) ===
    if not exist "%ESPEAK_SRC%" (
        git clone https://github.com/espeak-ng/espeak-ng "%ESPEAK_SRC%"
        if errorlevel 1 exit /b 1
    )
    pushd "%ESPEAK_SRC%"
    mkdir "%ESPEAK_BUILD%" >nul 2>nul
    cmake -S . ^
          -B "%ESPEAK_BUILD%" ^
          -G "Visual Studio 17 2022" ^
          -A x64 ^
          -DCMAKE_BUILD_TYPE=Release ^
          -DCMAKE_INSTALL_PREFIX="%ESPEAK_INSTALL%" ^
          -DBUILD_SHARED_LIBS=OFF ^
          -DESPEAKNG_BUILD_TESTS=OFF ^
          -DESPEAKNG_BUILD_EXAMPLES=OFF ^
          -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded ^
          -DCMAKE_EXE_LINKER_FLAGS="/NODEFAULTLIB:MSVCRT libcmt.lib legacy_stdio_definitions.lib"
    if errorlevel 1 exit /b 1
    cmake --build "%ESPEAK_BUILD%" --config Release --target INSTALL
    if errorlevel 1 exit /b 1
    popd
)

REM ===== OpenBLAS Build (Static /MT) =====
if "%WIN_WITH_OPENBLAS%"=="1" (
    if not exist "%OPENBLAS_INSTALL%\lib\libopenblas.lib" (
        echo === Building OpenBLAS (MSVC /MT) ===
        if not exist "%OPENBLAS_SRC%" (
            git clone --branch v0.3.30 --single-branch https://github.com/xianyi/OpenBLAS.git "%OPENBLAS_SRC%"
            if errorlevel 1 exit /b 1
        )
        mkdir "%OPENBLAS_BUILD%" >nul 2>nul
        pushd "%OPENBLAS_BUILD%"
        cmake -G "Visual Studio 17 2022" ^
              -A x64 ^
              -DCMAKE_BUILD_TYPE=Release ^
              -DBUILD_SHARED_LIBS=OFF ^
              -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded ^
              -DCMAKE_INSTALL_PREFIX="%OPENBLAS_INSTALL%" ^
              -DCMAKE_EXE_LINKER_FLAGS="/NODEFAULTLIB:MSVCRT libcmt.lib legacy_stdio_definitions.lib" ^
              "%OPENBLAS_SRC%"
        if errorlevel 1 exit /b 1
        cmake --build . --config Release --target INSTALL
        if errorlevel 1 exit /b 1
        popd
    )
)

REM ===== ONNX Runtime Fully Static Build (/MT) =====
if not exist "%ONNX_BUILD%\Release\onnxruntime.lib" (
    echo === Building ONNX Runtime (Fully Static /MT) ===
    if not exist "%ONNX_SRC%" (
        git clone --recursive https://github.com/microsoft/onnxruntime "%ONNX_SRC%"
        if errorlevel 1 exit /b 1
    )
    pushd "%ONNX_SRC%"
    git submodule sync
    git submodule update --init --recursive --force
    if errorlevel 1 exit /b 1
    popd

    set "ONNX_CUDA_FLAG=OFF"
    set "ONNX_VULKAN_FLAG=OFF"
    if "%WIN_WITH_CUDA%"=="1" set "ONNX_CUDA_FLAG=ON"
    if "%WIN_WITH_VULKAN%"=="1" set "ONNX_VULKAN_FLAG=ON"

    mkdir "%ONNX_BUILD%" >nul 2>nul
    pushd "%ONNX_BUILD%"
    cmake -G "Visual Studio 17 2022" ^
      -A x64 ^
      -DCMAKE_BUILD_TYPE=Release ^
      -DBUILD_SHARED_LIBS=OFF ^
      -Donnxruntime_BUILD_SHARED_LIB=OFF ^
      -Donnxruntime_USE_CUDA=%ONNX_CUDA_FLAG% ^
      -Donnxruntime_USE_VULKAN=%ONNX_VULKAN_FLAG% ^
      -Donnxruntime_BUILD_UNIT_TESTS=OFF ^
      -Donnxruntime_BUILD_TESTS=OFF ^
      -Donnxruntime_ENABLE_TESTING=OFF ^
      -DBUILD_TESTING=OFF ^
      -Donnxruntime_MSVC_STATIC_RUNTIME=ON ^
      -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded ^
      -DCMAKE_EXE_LINKER_FLAGS="/NODEFAULTLIB:MSVCRT libcmt.lib legacy_stdio_definitions.lib" ^
      -DONNX_CUSTOM_PROTOC_EXECUTABLE="" ^
      -DONNX_DISABLE_CONTRIB_OPS=ON ^
      "%ONNX_SRC%\cmake"
    if errorlevel 1 exit /b 1
    cmake --build . --config Release -- /ignore:4217 /ignore:4286
    if errorlevel 1 exit /b 1
    popd
)

REM ===== Export environment =====
set "ESPEAKNG_INCLUDE_DIR=%ESPEAK_INSTALL%\include"
set "ESPEAKNG_LIB_DIR=%ESPEAK_INSTALL%\lib"
set "OPENBLAS_LIB_DIR=%OPENBLAS_INSTALL%\lib"
set "OPENBLAS_INCLUDE_DIR=%OPENBLAS_INSTALL%\include"
set "ONNXRUNTIME_LIB_DIR=%ONNX_BUILD%\Release"
set "ONNXRUNTIME_INCLUDE_DIR=%ONNX_SRC%\include"

REM ===== Build Rust target fully static =====
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
