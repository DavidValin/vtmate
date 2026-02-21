# ==========================================================
# build_windows.ps1 - Complete CI-ready PowerShell Build
# ==========================================================
# This script:
# - Loads MSVC x64 environment
# - Builds eSpeak NG (static)
# - Builds OpenBLAS (static)
# - Builds ONNX Runtime (static)
# - Supports CPU / Vulkan / CUDA variants
# - Handles safe paths
# - Installs CUDA if missing
# ==========================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

param(
    [string]$VARIANT = "cpu"
)

Write-Host "=== Loading MSVC environment (x64) ==="

# Load MSVC environment
$VsDevCmd = "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\Common7\Tools\VsDevCmd.bat"
if (-not (Test-Path $VsDevCmd)) {
    Write-Error "VS Dev Command Prompt not found at $VsDevCmd"
    exit 1
}

# Import environment variables from VsDevCmd
cmd /c "`"$VsDevCmd`" -arch=x64 && set" | ForEach-Object {
    if ($_ -match "^(.*?)=(.*)$") {
        Set-Item -Path "Env:$($matches[1])" -Value $matches[2]
    }
}

# Verify cl.exe
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    Write-Error "cl.exe not found after loading MSVC. Ensure Visual Studio 2022 x64 Native Tools are installed."
    exit 1
}

Write-Host "=== MSVC environment loaded successfully ===`n"

# ==========================================================
# CONFIG
# ==========================================================
$BIN_BASE       = "ai-mate"
$PROJECT_ROOT   = Split-Path -Parent $MyInvocation.MyCommand.Path
$DIST_DIR       = Join-Path $PROJECT_ROOT "dist"
$TARGET_DIR     = Join-Path $PROJECT_ROOT "target-cross"
$VENDOR_DIR     = Join-Path $PROJECT_ROOT "vendor"
$ESPEAK_SRC     = Join-Path $VENDOR_DIR "espeak-ng"
$ESPEAK_BUILD   = Join-Path $ESPEAK_SRC "build-msvc"
$ESPEAK_INSTALL = Join-Path $ESPEAK_BUILD "install"
$OPENBLAS_DIR   = Join-Path $VENDOR_DIR "openblas"
$ONNX_SRC       = Join-Path $VENDOR_DIR "onnxruntime"
$ONNX_BUILD     = Join-Path $ONNX_SRC "build-static"
$UPLOAD_ENABLED = $true

# ==========================================================
# CLEAN OLD BUILDS
# ==========================================================
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $ESPEAK_BUILD, $OPENBLAS_DIR, $ONNX_BUILD, "$PROJECT_ROOT\target", $TARGET_DIR, $DIST_DIR

# ==========================================================
# CHECK REQUIRED TOOLS
# ==========================================================
foreach ($tool in "cl.exe","cmake","git","cargo","powershell") {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Error "ERROR: Required tool $tool not found. Launch PowerShell from 'x64 Native Tools Command Prompt for VS'."
        exit 1
    }
}

# ==========================================================
# RUST CRT
# ==========================================================
$env:RUSTFLAGS = "-C opt-level=3"
$env:CARGO_BUILD_JOBS = 1

# ==========================================================
# DETERMINE VARIANT
# ==========================================================
switch ($VARIANT) {
    "cpu" {
        $WITH_OPENBLAS = $true
        $WITH_CUDA     = $false
        $WITH_VULKAN   = $false
    }
    "vulkan" {
        $WITH_OPENBLAS = $true
        $WITH_CUDA     = $false
        $WITH_VULKAN   = $true
    }
    "cuda" {
        $WITH_OPENBLAS = $true
        $WITH_CUDA     = $true
        $WITH_VULKAN   = $false
    }
    default {
        Write-Error "ERROR: Unknown variant $VARIANT"
        exit 1
    }
}

Write-Host "`n============================================"
Write-Host "Building variant: $VARIANT"
if ($WITH_OPENBLAS) { Write-Host "OpenBLAS: ENABLED" }
if ($WITH_CUDA)     { Write-Host "CUDA: ENABLED" }
if ($WITH_VULKAN)   { Write-Host "Vulkan: ENABLED" }
Write-Host "============================================`n"

# ==========================================================
# CREATE DIRECTORIES
# ==========================================================
foreach ($dir in $TARGET_DIR, $DIST_DIR, $VENDOR_DIR) {
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
}

# ==========================================================
# ENSURE CUDA TOOLKIT
# ==========================================================
if ($WITH_CUDA) {
    $nvcc = Get-Command nvcc -ErrorAction SilentlyContinue
    if (-not $nvcc) {
        Write-Host "CUDA not detected. Installing CUDA Toolkit..."
        $CUDA_VERSION = "12.3.2"
        $CUDA_INSTALLER = "$env:TEMP\cuda_installer.exe"
        $CUDA_URL = "https://developer.download.nvidia.com/compute/cuda/$CUDA_VERSION/network_installers/cuda_${CUDA_VERSION}_windows_network.exe"
        Invoke-WebRequest -Uri $CUDA_URL -OutFile $CUDA_INSTALLER -UseBasicParsing

        $arguments = "--silent --toolkit --installpath `"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CUDA_VERSION`""
        $proc = Start-Process -FilePath $CUDA_INSTALLER -ArgumentList $arguments -Wait -PassThru
        if ($proc.ExitCode -ne 0) {
            Write-Error "CUDA installation failed with exit code $($proc.ExitCode)"
            exit 1
        }

        $env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CUDA_VERSION"
        $env:Path = "$env:CUDA_PATH\bin;$env:Path"
        $env:CUDAToolkit_ROOT = $env:CUDA_PATH
    }
    else {
        $env:CUDA_PATH = Split-Path -Parent $nvcc.Source
        $env:CUDAToolkit_ROOT = $env:CUDA_PATH
    }
}

# ==========================================================
# BUILD eSpeak NG
# ==========================================================
$ESPEAK_INSTALL_SAFE = $ESPEAK_INSTALL
if (-not (Test-Path (Join-Path $ESPEAK_INSTALL_SAFE "lib\espeak-ng.lib"))) {
    Write-Host "=== Building eSpeak NG ==="
    if (-not (Test-Path $ESPEAK_SRC)) {
        git clone "https://github.com/espeak-ng/espeak-ng" $ESPEAK_SRC
    }
    $CMAKE_ARGS = @(
        "-DCMAKE_BUILD_TYPE=Release",
        "-DCMAKE_INSTALL_PREFIX=$ESPEAK_INSTALL_SAFE",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DESPEAKNG_BUILD_TESTS=OFF",
        "-DESPEAKNG_BUILD_EXAMPLES=OFF",
        "-DESPEAKNG_BUILD_PROGRAM=OFF",
        "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL",
        "-DCMAKE_C_FLAGS=/MD",
        "-DCMAKE_CXX_FLAGS=/MD"
    )
    cmake -S $ESPEAK_SRC -B $ESPEAK_BUILD -G "Visual Studio 17 2022" -A x64 $CMAKE_ARGS
    cmake --build $ESPEAK_BUILD --config Release --target INSTALL
} else {
    Write-Host "eSpeak NG already built, skipping."
}

# ==========================================================
# BUILD OPENBLAS
# ==========================================================
if ($WITH_OPENBLAS) {
    Write-Host "=== Building OpenBLAS ==="
    $PREBUILT_OPENBLAS_DIR = Join-Path $PROJECT_ROOT "assets\openblas-windows-portable"
    $OPENBLAS_LIB = Join-Path $PREBUILT_OPENBLAS_DIR "lib\libopenblas.lib"

    foreach ($dir in @("$PREBUILT_OPENBLAS_DIR\lib","$PREBUILT_OPENBLAS_DIR\include")) {
        if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    }

    if (-not (Test-Path $OPENBLAS_LIB)) {
        $tmp_build = Join-Path $env:TEMP "openblas_build"
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp_build
        New-Item -ItemType Directory -Force -Path $tmp_build | Out-Null

        git clone --depth 1 --branch v0.3.30 https://github.com/xianyi/OpenBLAS (Join-Path $tmp_build "OpenBLAS")
        New-Item -ItemType Directory -Force -Path (Join-Path $tmp_build "OpenBLAS\build") | Out-Null

        Push-Location (Join-Path $tmp_build "OpenBLAS")
        cmake -S . -B build -G "Visual Studio 17 2022" -A x64 `
            -DBUILD_SHARED_LIBS=OFF `
            -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
            -DCMAKE_INSTALL_PREFIX="$PREBUILT_OPENBLAS_DIR" `
            -DNO_LAPACK=ON `
            -DNO_TEST=ON
        cmake --build build --config Release --target INSTALL
        Pop-Location

        Remove-Item -Recurse -Force $tmp_build
    }

    $env:OpenBLAS_DIR = $PREBUILT_OPENBLAS_DIR
    $env:OpenBLAS_LIBRARIES = $OPENBLAS_LIB
    $env:OpenBLAS_INCLUDE_DIR = Join-Path $PREBUILT_OPENBLAS_DIR "include"
}

# ==========================================================
# BUILD ONNX Runtime
# ==========================================================
if (-not (Test-Path (Join-Path $ONNX_BUILD "Release\onnxruntime.lib"))) {
    Write-Host "=== Building ONNX Runtime ==="
    if (-not (Test-Path $ONNX_SRC)) {
        git clone --recursive https://github.com/microsoft/onnxruntime $ONNX_SRC
    }
    Push-Location $ONNX_SRC
    git submodule update --init --recursive --force
    Pop-Location

    $ONNX_CUDA_FLAG = if ($WITH_CUDA) { "ON" } else { "OFF" }
    $ONNX_VULKAN_FLAG = if ($WITH_VULKAN) { "ON" } else { "OFF" }

    cmake -S (Join-Path $ONNX_SRC "cmake") -B $ONNX_BUILD -G "Visual Studio 17 2022" -A x64 `
        -DCMAKE_BUILD_TYPE=Release `
        -DBUILD_SHARED_LIBS=OFF `
        -Donnxruntime_BUILD_SHARED_LIB=OFF `
        -Donnxruntime_MSVC_STATIC_RUNTIME=OFF `
        -Donnxruntime_USE_CUDA=$ONNX_CUDA_FLAG `
        -Donnxruntime_USE_VULKAN=$ONNX_VULKAN_FLAG `
        -Donnxruntime_USE_EIGEN=ON `
        -Donnxruntime_USE_OPENMP=OFF `
        -Donnxruntime_BUILD_UNIT_TESTS=OFF `
        -Donnxruntime_BUILD_TESTS=OFF `
        -Donnxruntime_ENABLE_TESTING=OFF `
        -DBUILD_TESTING=OFF

    cmake --build $ONNX_BUILD --config Release
}

# ==========================================================
# EXPORT ENVIRONMENT
# ==========================================================
$env:ESPEAKNG_INCLUDE_DIR   = Join-Path $ESPEAK_INSTALL "include"
$env:ESPEAKNG_LIB_DIR       = Join-Path $ESPEAK_INSTALL "lib"
$env:ONNXRUNTIME_INCLUDE_DIR = Join-Path $ONNX_SRC "include"
$env:ONNXRUNTIME_LIB_DIR     = Join-Path $ONNX_BUILD "Release"

# ==========================================================
# BUILD RUST BINARY
# ==========================================================
$TARGET = "x86_64-pc-windows-msvc"
$CARGO_FEATURES = @()
if ($WITH_OPENBLAS) { $CARGO_FEATURES += "whisper-openblas" }
if ($WITH_VULKAN)   { $CARGO_FEATURES += "whisper-vulkan" }
if ($WITH_CUDA)     { $CARGO_FEATURES += "whisper-cuda" }

$env:RUSTFLAGS = "-C codegen-units=1 -C opt-level=3 -C link-arg=-L$PREBUILT_OPENBLAS_DIR\lib -C link-arg=-lopenblas"

$SRC_BIN = Join-Path $PROJECT_ROOT "target\$TARGET\release\$BIN_BASE.exe"
$DST_BIN = Join-Path $TARGET_DIR "$VARIANT\$BIN_BASE-$VARIANT.exe"

if (-not (Test-Path $SRC_BIN)) {
    Write-Error "ERROR: Built binary not found."
    exit 1
}

Copy-Item -Force $SRC_BIN $DST_BIN
Write-Host "Built $DST_BIN"

# ==========================================================
# UPLOAD ARTIFACT
# ==========================================================
if ($UPLOAD_ENABLED) {
    Write-Host "Uploading artifact for $VARIANT..."
    gh run upload-artifact "$BIN_BASE-$VARIANT" $DST_BIN
}

Write-Host "`nSUCCESS: $DST_BIN"
exit 0