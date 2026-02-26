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

$ESPEAK_SRC     = Join-Path $VENDOR_DIR "espeak-ng"
$ESPEAK_BUILD   = Join-Path $ESPEAK_SRC "build-msvc"
$ESPEAK_INSTALL = Join-Path $ESPEAK_BUILD "install"

$PROTOC_SRC     = Join-Path $PROJECT_ROOT "protobuf"
$PROTOC_BUILD   = Join-Path $PROJECT_ROOT "protobuf\build"
$PROTOC_INSTALL = Join-Path $PROJECT_ROOT "protobuf\install"

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
        $cuda_root = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CUDA_VERSION"
        $CUDA_INSTALLER = "$env:TEMP\cuda_installer.exe"
        $CUDA_URL = "https://developer.download.nvidia.com/compute/cuda/$CUDA_VERSION/network_installers/cuda_${CUDA_VERSION}_windows_network.exe"

        Invoke-WebRequest -Uri $CUDA_URL -OutFile $CUDA_INSTALLER -UseBasicParsing

        if (-not (Test-Path $CUDA_INSTALLER)) {
            Write-Error "Failed to download CUDA installer."
            exit 1
        }

        $arguments = "--silent --toolkit --installpath `"$cuda_root`""
        $proc = Start-Process -FilePath $CUDA_INSTALLER -ArgumentList $arguments -Wait -PassThru
        if ($proc.ExitCode -ne 0) {
            Write-Error "CUDA installation failed with exit code $($proc.ExitCode)"
            exit 1
        }

        # Set environment variables
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
        $cuda_root = Split-Path -Parent (Split-Path -Parent $nvcc.Source)
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
# BUILD ESPEAK-NG STATIC
# ==========================================================
$ESPEAK_LIB = Join-Path $ESPEAK_INSTALL "lib" "espeak-ng.lib"

if (-not (Test-Path $ESPEAK_LIB)) {

    Write-Host ""
    Write-Host "=== Building eSpeak NG (MSVC) ==="

    # Clone repository if source doesn't exist
    if (-not (Test-Path $ESPEAK_SRC)) {
        New-Item -ItemType Directory -Force -Path $VENDOR_DIR | Out-Null
        git clone https://github.com/espeak-ng/espeak-ng $ESPEAK_SRC
        if ($LASTEXITCODE -ne 0) { exit 1 }
    }

    # Change directory to source
    Push-Location $ESPEAK_SRC

    # Configure with CMake
    cmake -S . `
      -B $ESPEAK_BUILD `
      -G "Visual Studio 17 2022" `
      -A x64 `
      -DCMAKE_BUILD_TYPE=Release `
      -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
      -DCMAKE_C_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELWITHDEBINFO="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELWITHDEBINFO="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_DEBUG="/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_DEBUG="/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_INSTALL_PREFIX=$ESPEAK_INSTALL `
      -DBUILD_SHARED_LIBS=OFF `
      -DESPEAKNG_BUILD_TESTS=OFF `
      -DESPEAKNG_BUILD_EXAMPLES=OFF `
      -DCMAKE_EXE_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib" `
      -DCMAKE_STATIC_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib"
    if ($LASTEXITCODE -ne 0) { exit 1 }

    # Build and install
    cmake --build $ESPEAK_BUILD --config Release --target INSTALL
    if ($LASTEXITCODE -ne 0) { exit 1 }

    Pop-Location
}

# ==========================================================
# BUILD OPENBLAS STATIC AND LINK
# ==========================================================
if ($WITH_OPENBLAS) {
    Write-Host "=== Windows build [OpenBLAS] variant ==="
    $PREBUILT_OPENBLAS_DIR = Join-Path $PROJECT_ROOT "assets\openblas-windows-portable"
    $LIB_DIR = Join-Path $PREBUILT_OPENBLAS_DIR "lib"
    $INCLUDE_DIR = Join-Path $PREBUILT_OPENBLAS_DIR "include\openblas"
    $FINAL_LIB = Join-Path $LIB_DIR "openblas.lib"
    $RENAMED_LIB = Join-Path $LIB_DIR "libopenblas.lib"

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
      -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
      -DCMAKE_C_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELWITHDEBINFO="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELWITHDEBINFO="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_DEBUG="/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_DEBUG="/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_EXE_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib" `
      -DCMAKE_STATIC_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib" `
      -DNO_LAPACK=ON `
      -DUSE_OPENMP=ON `
      -DCMAKE_INSTALL_PREFIX="$PREBUILT_OPENBLAS_DIR"

    cmake --build build --config Release --target INSTALL
    Pop-Location

    # Rename openblas.lib to libopenblas.lib
    if (Test-Path $FINAL_LIB) {
        Rename-Item -Path $FINAL_LIB -NewName "libopenblas.lib" -Force
        Write-Host "Renamed openblas.lib to libopenblas.lib"
    }

    Remove-Item -Recurse -Force $tmp_build
    Write-Host "OpenBLAS build completed"

    # Ensure the variable points to the renamed library
    $OPENBLAS_LIB = $RENAMED_LIB

    # Set environment variables
    $env:OpenBLAS_DIR = $PREBUILT_OPENBLAS_DIR
    $env:OpenBLAS_LIBRARIES = $OPENBLAS_LIB
    $env:OpenBLAS_INCLUDE_DIR = $INCLUDE_DIR
}

# ==========================================================
# BUILD PROTOC STATIC
# ==========================================================

# git clone -b v3.21.12 https://github.com/protocolbuffers/protobuf.git $PROTOC_SRC
# ensure directories
# New-Item -ItemType Directory -Force -Path $PROTOC_BUILD, $PROTOC_INSTALL
# cd $PROTOC_BUILD
# cmake $PROTOC_SRC\cmake `
#     -G "Visual Studio 17 2022" `
#     -A x64 `
#     -DCMAKE_BUILD_TYPE=Release `
#     -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
#     -DCMAKE_C_FLAGS_RELEASE=/MT `
#     -DCMAKE_CXX_FLAGS_RELEASE=/MT `
#     -DCMAKE_C_FLAGS_RELWITHDEBINFO=/MT `
#     -DCMAKE_CXX_FLAGS_RELWITHDEBINFO=/MT `
#     -DCMAKE_C_FLAGS_DEBUG=/MTd `
#     -DCMAKE_CXX_FLAGS_DEBUG=/MTd `
#     -Dprotobuf_MSVC_STATIC_RUNTIME=ON `
#     -DPROTOBUF_USE_DLLS=OFF `
#     -DMAKE_WINDOWS_EXPORT_ALL_SYMBOLS=ON `
#     -DBUILD_SHARED_LIBS=OFF `
#     -DCMAKE_INSTALL_PREFIX="$PROTOC_INSTALL" `
#     -Dprotobuf_BUILD_TESTS=OFF
# cmake --build . --config Release --target INSTALL
# $PROTOC_BIN = "$PROTOC_INSTALL\bin\protoc.exe"

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

            $ORT_EXTRA_CMAKE_ARGS = @(
              "-DORT_MINIMAL_BUILD=ON"
            )
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
    $ONNX_CMAKE_ARGS = @(
        "-S", "$ONNX_SRC/cmake",
        "-B", "$ONNX_BUILD",
        "-G", "Visual Studio 17 2022",
        "-A", "x64",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DFETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DCMAKE_COMPILE_WARNING_AS_ERROR=OFF",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=OFF",
        "-DFETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER",
        "-Donnxruntime_BUILD_SHARED_LIB=OFF",
        "-Donnxruntime_ENABLE_STATIC_ANALYSIS=OFF",
        "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded",
        "-DCMAKE_C_FLAGS=/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_CXX_FLAGS=/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_C_FLAGS_RELEASE=/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_CXX_FLAGS_RELEASE=/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_C_FLAGS_RELWITHDEBINFO=/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_CXX_FLAGS_RELWITHDEBINFO=/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_C_FLAGS_DEBUG=/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_CXX_FLAGS_DEBUG=/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
        "-DCMAKE_EXE_LINKER_FLAGS=/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib",
        "-DCMAKE_STATIC_LINKER_FLAGS=/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib",
        "-Donnxruntime_BUILD_UNIT_TESTS=OFF",
        "-Donnxruntime_USE_AVX=OFF",
        "-Donnxruntime_USE_AVX2=OFF",
        "-Donnxruntime_USE_AVX512=OFF",
        "-Donnxruntime_RUN_ONNX_TESTS=OFF",
        "-Donnxruntime_USE_XNNPACK=OFF",
        "-Donnxruntime_USE_DML=OFF",
        "-DBUILD_TESTING=OFF",
        "-DONNX_USE_MSVC_STATIC_RUNTIME=ON",
        "-DONNX_USE_PROTOBUF_SHARED_LIBS=OFF",
        "-Donnxruntime_USE_FULL_PROTOBUF=ON",
        "-Donnxruntime_USE_CUDA=$ONNX_CUDA_FLAG"
    )

    if ($ORT_EXTRA_CMAKE_ARGS) {
      $ONNX_CMAKE_ARGS += $ORT_EXTRA_CMAKE_ARGS
    }

    # Conditionally add CUDA-specific options only if CUDA is ON
    if ($ONNX_CUDA_FLAG -eq "ON") {
        $cuda_root = $env:CUDAToolkit_ROOT
        $ONNX_CMAKE_ARGS += @(
            "-DCUDAToolkit_ROOT=$cuda_root"
            # Add other CUDA-related flags here if needed
        )
    }

    # Run CMake with the assembled arguments
    cmake @ONNX_CMAKE_ARGS

    # -----------------------------
    # Build ONNX Runtime
    # -----------------------------
    cmake --build $ONNX_BUILD --config Release

    $ORT_LIB_LOCATION = Join-Path $ONNX_BUILD "Release"
}


# ==========================================================
# EXPORT ENVIRONMENT
# ==========================================================
$env:ONNXRUNTIME_INCLUDE_DIR = Join-Path $ONNX_SRC "include"
$env:ORT_STRATEGY            = "system"
$env:ORT_LIB_LOCATION        = $ORT_LIB_LOCATION
$env:ORT_PREFER_DYNAMIC_LINK = "0"
$env:ONNXRUNTIME_LIB_DIR     = Join-Path $ONNX_BUILD "Release"
# -----------------------------------------------------------
$env:GGML_BLAS               = "ON"
$env:BLAS_STATIC             = "ON"
$env:GGML_BLAS_STATIC        = "ON"
$env:BLAS_VENDOR             = "OpenBLAS"
$env:BLA_VENDOR              = "OpenBLAS"
$env:GGML_BLAS_VENDOR        = "OpenBLAS"
$env:BLAS_INCLUDE_DIRS       = $INCLUDE_DIR
$env:BLAS_LIBRARIES          = $OPENBLAS_LIB
$env:OPENBLAS_PATH           = $PREBUILT_OPENBLAS_DIR
$env:OPENBLAS_DIR            = $PREBUILT_OPENBLAS_DIR
$env:CMAKE_PREFIX_PATH       = "${PREBUILT_OPENBLAS_DIR};${ONNX_BUILD}"
$env:CMAKE_ARGS              = "-DGGML_BLAS=ON -DGGML_BLAS_STATIC=ON -DGGML_BLAS_VENDOR=OpenBLAS -DBLAS_VENDOR=OpenBLAS -DOPENBLAS_PATH=$PREBUILT_OPENBLAS_DIR -DBLAS_INCLUDE_DIRS=$INCLUDE_DIR -DBLAS_LIBRARIES=$OPENBLAS_LIB -DBLA_VENDOR=OpenBLAS -DBLAS_ROOT=$PREBUILT_OPENBLAS_DIR -DBLAS_DIR=$PREBUILT_OPENBLAS_DIR -DBLAS_LIBDIR=$LIB_DIR -DBLA_STATIC=ON"
$env:WHISPER_RS_STATIC_CRT   = "1"
$env:ORT_SYS_STATIC_CRT      = "1"
$env:ESPEAK_RS_STATIC_CRT    = "1"
$env:ESPEAK_NG_DIR           = $ESPEAK_INSTALL

# Merge all ORT + deps libs into one (onnx produces multiple .lib files)
# Write-Host "`n=== ONNX .lib files BEFORE merge ==="
# $allLibs = Get-ChildItem -Path $ONNX_BUILD -Filter *.lib -Recurse | Select-Object -ExpandProperty FullName
# $allLibs | ForEach-Object { Write-Host $_ }
# lib /OUT:"$ORT_LIB_LOCATION\onnxruntime_merged.lib" $allLibs
# # Remove all original .lib files except merged one
# Get-ChildItem "$ORT_LIB_LOCATION" -Filter *.lib | Where-Object { $_.Name -ne "onnxruntime_merged.lib" } | Remove-Item
# # Rename merged lib
# Rename-Item "$ORT_LIB_LOCATION\onnxruntime_merged.lib" "onnxruntime.lib"
# Write-Host "`n=== ONNX .lib files AFTER merge ==="
# Get-ChildItem "$ORT_LIB_LOCATION" -Filter *.lib | ForEach-Object { Write-Host $_.FullName }


# Set ORT crate feature flags
if ($WITH_CUDA)    { $env:ORT_USE_CUDA = "1" } else { Remove-Item Env:ORT_USE_CUDA -ErrorAction SilentlyContinue }
if ($WITH_OPENBLAS){ $env:ORT_USE_OPENMP = "1" } else { Remove-Item Env:ORT_USE_OPENMP -ErrorAction SilentlyContinue }

Write-Host "ORT_USE_CUDA = $env:ORT_USE_CUDA"
Write-Host "ORT_USE_OPENMP = $env:ORT_USE_OPENMP"

# ==========================================================
# BUILD RUST BINARY WITH FEATURES
# ==========================================================
$TARGET = "x86_64-pc-windows-msvc"

$CARGO_FEATURES = @()
if ($WITH_OPENBLAS) { $CARGO_FEATURES += "whisper-openblas" }
if ($WITH_VULKAN)   { $CARGO_FEATURES += "whisper-vulkan" }
if ($WITH_CUDA)     { $CARGO_FEATURES += "whisper-cuda" }

# Before cargo build
$env:RUSTFLAGS = "-C target-feature=+crt-static `
                  -C codegen-units=1 `
                  -C opt-level=3 `
                  -C link-arg=/DEFAULTLIB:legacy_stdio_definitions.lib `
                  -C link-arg=/DEFAULTLIB:OLDNAMES.lib"


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