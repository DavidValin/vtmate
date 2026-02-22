# ==========================================================
# PowerShell Build Script (MSVC)
# ==========================================================
param(
    [string]$VARIANT = "cpu"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ==========================================================
# CONFIG
# ==========================================================
$BIN_BASE       = "ai-mate"
$PROJECT_ROOT   = Split-Path -Parent $MyInvocation.MyCommand.Path
$DIST_DIR       = Join-Path $PROJECT_ROOT "dist"
$TARGET_DIR     = Join-Path $PROJECT_ROOT "target-cross"
$VENDOR_DIR     = Join-Path $PROJECT_ROOT "vendor"
$OPENBLAS_DIR   = Join-Path $VENDOR_DIR "openblas"
$ONNX_SRC       = Join-Path $VENDOR_DIR "onnxruntime"
$ONNX_BUILD     = Join-Path $ONNX_SRC "build-static"
$UPLOAD_ENABLED = $true

# ==========================================================
# CLEAN OLD BUILDS
# ==========================================================
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $OPENBLAS_DIR, $ONNX_BUILD, "$PROJECT_ROOT\target", $TARGET_DIR, $DIST_DIR

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
# USE DYNAMIC RUST CRT TO MATCH ONNX /MD
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
# CREATE REQUIRED DIRECTORIES
# ==========================================================
foreach ($dir in $TARGET_DIR, $DIST_DIR, $VENDOR_DIR) {
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
}

# ==========================================================
# ENSURE CUDA TOOLKIT IF REQUIRED (BUILD-TIME)
# ==========================================================
if ($WITH_CUDA) {
    $nvcc = Get-Command nvcc -ErrorAction SilentlyContinue
    if (-not $nvcc) {
        Write-Host "CUDA not detected. Installing CUDA Toolkit for build..."
        $CUDA_VERSION = "12.3.2"
        $CUDA_INSTALLER = "$env:TEMP\cuda_installer.exe"
        $CUDA_URL = "https://developer.download.nvidia.com/compute/cuda/$CUDA_VERSION/network_installers/cuda_${CUDA_VERSION}_windows_network.exe"

        Invoke-WebRequest -Uri $CUDA_URL -OutFile $CUDA_INSTALLER -UseBasicParsing

        if (-not (Test-Path $CUDA_INSTALLER)) {
            Write-Error "Failed to download CUDA installer."
            exit 1
        }

        $arguments = "--silent --toolkit --installpath `"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CUDA_VERSION`""
        $proc = Start-Process -FilePath $CUDA_INSTALLER -ArgumentList $arguments -Wait -PassThru
        if ($proc.ExitCode -ne 0) {
            Write-Error "CUDA installation failed with exit code $($proc.ExitCode)"
            exit 1
        }

        $cuda_root = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CUDA_VERSION"
        $env:CUDA_PATH = $cuda_root
        $env:CUDAToolkit_ROOT = $cuda_root
        $env:Path = "$cuda_root\bin;$env:Path"

        # Verify nvcc
        if (-not (Get-Command nvcc -ErrorAction SilentlyContinue)) {
            Write-Error "CUDA installed but nvcc not found in PATH."
            exit 1
        }

        Write-Host "CUDA successfully installed for build."
    }
    else {
        Write-Host "CUDA already present."
        $cuda_root = Split-Path -Parent $nvcc.Source
        $env:CUDA_PATH = $cuda_root
        $env:CUDAToolkit_ROOT = $cuda_root
        $env:Path = "$cuda_root\bin;$env:Path"
        Write-Host "CUDA_PATH = $env:CUDA_PATH"
    }
} 
else {
    Remove-Item Env:CUDAToolkit_ROOT -ErrorAction SilentlyContinue
    Remove-Item Env:CUDA_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:CUDA_HOME -ErrorAction SilentlyContinue
    Remove-Item Env:CUDA_ROOT -ErrorAction SilentlyContinue
}

# ==========================================================
# BUILD OPENBLAS STATIC AND LINK
# ==========================================================
if ($WITH_OPENBLAS) {
    Write-Host "=== Windows build [OpenBLAS] variant ==="
    $PREBUILT_OPENBLAS_DIR = Join-Path $PROJECT_ROOT "assets\openblas-windows-portable"
    $LIB_DIR = Join-Path $PREBUILT_OPENBLAS_DIR "lib"
    $INCLUDE_DIR = Join-Path $PREBUILT_OPENBLAS_DIR "include"
    $FINAL_LIB = Join-Path $LIB_DIR "openblas.lib"

    # Ensure directories exist
    foreach ($dir in @($LIB_DIR, $INCLUDE_DIR)) {
        if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
          # Check for existing library
          $POSSIBLE_LIBS = @("libopenblas.lib", "openblas.lib")
          $OPENBLAS_LIB = $null
          foreach ($lib in $POSSIBLE_LIBS) {
              $libPath = Join-Path $LIB_DIR $lib
              if (Test-Path $libPath) {
                  $OPENBLAS_LIB = $libPath
                  break
              }
          }
          # If a prebuilt library exists, rename to openblas.lib
          if ($OPENBLAS_LIB -and $OPENBLAS_LIB -ne $FINAL_LIB) {
              Copy-Item $OPENBLAS_LIB $FINAL_LIB -Force
              $OPENBLAS_LIB = $FINAL_LIB
              Write-Host "Copied existing OpenBLAS lib to $FINAL_LIB"
              # If library is still missing, build OpenBLAS
              if (-not (Test-Path $FINAL_LIB)) {
                  Write-Host "OpenBLAS library not found — building from source..."

                  $tmp_build = Join-Path $env:TEMP "openblas_build"
                  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp_build
                  New-Item -ItemType Directory -Force -Path $tmp_build | Out-Null

                  $src_dir = Join-Path $tmp_build "OpenBLAS"
                  git clone --depth 1 --branch v0.3.30 https://github.com/xianyi/OpenBLAS $src_dir
                  $build_dir = Join-Path $src_dir "build"
                  New-Item -ItemType Directory -Force -Path $build_dir | Out-Null

                  Push-Location $src_dir
                  cmake -S . -B build -G "Visual Studio 17 2022" -A x64 `
                    -DBUILD_SHARED_LIBS=OFF `
                    -DNO_LAPACK=ON `
                    -DUSE_OPENMP=ON `
                    -DCMAKE_INSTALL_PREFIX="$PREBUILT_OPENBLAS_DIR"

                  cmake --build build --config Release --target INSTALL
                  Pop-Location

                  Remove-Item -Recurse -Force $tmp_build
                  Write-Host "OpenBLAS build completed"
              }
          }
    }

    # Ensure the variable points to the final library
    $OPENBLAS_LIB = $FINAL_LIB

    # Set environment variables
    $env:OpenBLAS_DIR = $PREBUILT_OPENBLAS_DIR
    $env:OpenBLAS_LIBRARIES = $OPENBLAS_LIB
    $env:OpenBLAS_INCLUDE_DIR = $INCLUDE_DIR
}

# ==========================================================
# BUILD ONNX RUNTIME (Single Block, No Duplicates)
# ==========================================================
if (-not (Test-Path (Join-Path $ONNX_BUILD "Release\onnxruntime.lib"))) {

    Write-Host "=== Building ONNX Runtime ==="

    # Clone ONNX Runtime if not present
    if (-not (Test-Path (Join-Path $ONNX_SRC "cmake\CMakeLists.txt"))) {
        git clone --recursive https://github.com/microsoft/onnxruntime $ONNX_SRC
    }

    # Update submodules
    Push-Location $ONNX_SRC
    git submodule update --init --recursive --force
    Pop-Location

    # -----------------------------
    # Set ONNX flags depending on variant
    # -----------------------------
    switch ($VARIANT) {
        "cpu" {
            $ONNX_CUDA_FLAG   = "OFF"
            $ONNX_VULKAN_FLAG = "OFF"
            $ONNX_USE_BLAS    = "ON"
        }
        "vulkan" {
            $ONNX_CUDA_FLAG   = "OFF"
            $ONNX_VULKAN_FLAG = "ON"
            $ONNX_USE_BLAS    = "ON"
        }
        "cuda" {
            $ONNX_CUDA_FLAG   = "ON"
            $ONNX_VULKAN_FLAG = "OFF"
            $ONNX_USE_BLAS    = "ON"
        }
    }

    # Make sure the build directory exists
    if (-not (Test-Path $ONNX_BUILD)) {
        New-Item -ItemType Directory -Path $ONNX_BUILD | Out-Null
    }

    # -----------------------------
    # Configure ONNX Runtime using CMake
    # -----------------------------
    cmake -S "$ONNX_SRC/cmake" -B "$ONNX_BUILD" -G "Visual Studio 17 2022" -A x64 `
        -DCMAKE_BUILD_TYPE=Release `
        -DBUILD_SHARED_LIBS=OFF `
        -Donnxruntime_BUILD_SHARED_LIB=OFF `
        -Donnxruntime_MSVC_STATIC_RUNTIME=ON `
        -Donnxruntime_USE_CUDA=$ONNX_CUDA_FLAG `
        -Donnxruntime_USE_VULKAN=$ONNX_VULKAN_FLAG `
        -Donnxruntime_USE_EIGEN=ON `
        -Donnxruntime_USE_OPENMP=ON `
        -Donnxruntime_USE_BLAS=$ONNX_USE_BLAS `
        -Donnxruntime_BUILD_UNIT_TESTS=OFF `
        -Donnxruntime_BUILD_TESTS=OFF `
        -Donnxruntime_ENABLE_TESTING=OFF `
        -DBUILD_TESTING=OFF `
        -DCUDAToolkit_ROOT=$env:CUDAToolkit_ROOT

    # -----------------------------
    # Build ONNX Runtime
    # -----------------------------
    cmake --build $ONNX_BUILD --config Release
}

# ==========================================================
# EXPORT ENVIRONMENT
# ==========================================================
$env:ONNXRUNTIME_INCLUDE_DIR = Join-Path $ONNX_SRC "include"
$env:ONNXRUNTIME_LIB_DIR     = Join-Path $ONNX_BUILD "Release"
$env:OpenBLAS_DIR            = $PREBUILT_OPENBLAS_DIR
$env:GGML_BLAS               = "ON"
$env:GGML_BLAS_VENDOR        = "OpenBLAS"
$env:CMAKE_PREFIX_PATH       = "$PREBUILT_OPENBLAS_DIR;$ONNX_BUILD"
$env:CMAKE_ARGS              = "-DGGML_BLAS=ON -DGGML_BLAS_VENDOR=OpenBLAS -DBLA_STATIC=ON"

# Set ORT crate feature flags
if ($WITH_CUDA)    { $env:ORT_USE_CUDA = "1" } else { Remove-Item Env:ORT_USE_CUDA -ErrorAction SilentlyContinue }
if ($WITH_OPENBLAS){ $env:ORT_USE_OPENMP = "1" } else { Remove-Item Env:ORT_USE_OPENMP -ErrorAction SilentlyContinue }
if ($WITH_VULKAN) { $env:ORT_USE_VULKAN = "1" } else { Remove-Item Env:ORT_USE_VULKAN -ErrorAction SilentlyContinue }

Write-Host "ORT_USE_CUDA = $env:ORT_USE_CUDA"
Write-Host "ORT_USE_OPENMP = $env:ORT_USE_OPENMP"
Write-Host "ORT_USE_VULKAN = $env:ORT_USE_VULKAN"

# ==========================================================
# BUILD RUST BINARY WITH FEATURES
# ==========================================================
$TARGET = "x86_64-pc-windows-msvc"

$CARGO_FEATURES = @()
if ($WITH_OPENBLAS) { $CARGO_FEATURES += "whisper-openblas" }
if ($WITH_VULKAN)   { $CARGO_FEATURES += "whisper-vulkan" }
if ($WITH_CUDA)     { $CARGO_FEATURES += "whisper-cuda" }

$env:RUSTFLAGS = "-C target-feature=+crt-static -C codegen-units=1 -C opt-level=3 -C link-arg=-L$PREBUILT_OPENBLAS_DIR\lib -C link-arg=-L$ONNX_BUILD\lib"

Write-Host "Ensuring Rust target $TARGET is installed..."
rustup target add $TARGET

Write-Host "Building Rust binary..."
cargo build --release --target $TARGET --features ($CARGO_FEATURES -join ",")

$SRC_BIN = Join-Path $PROJECT_ROOT "target\$TARGET\release\$BIN_BASE.exe"
# Fallback: try plain release folder if cross-target folder does not exist
if (-not (Test-Path $SRC_BIN)) {
    $SRC_BIN = Join-Path $PROJECT_ROOT "target\release\$BIN_BASE.exe"
}

$DST_BIN = Join-Path $TARGET_DIR "$VARIANT\$BIN_BASE-$VARIANT.exe"

if (-not (Test-Path $SRC_BIN)) {
    Write-Error "ERROR: Built binary not found."
    exit 1
}

Copy-Item -Force $SRC_BIN $DST_BIN
Write-Host "Built $DST_BIN"

if ($UPLOAD_ENABLED) {
    Write-Host "Uploading artifact for $VARIANT..."
    gh run upload-artifact "$BIN_BASE-$VARIANT" $DST_BIN
}

Write-Host "`nSUCCESS: $DST_BIN"
exit 0