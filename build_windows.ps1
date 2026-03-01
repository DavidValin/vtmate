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
      -DCMAKE_CXX_STANDARD=17 `
      -DCMAKE_CXX_STANDARD_REQUIRED=ON `
      -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
      -DCMAKE_C_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELWITHDEBINFO="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELWITHDEBINFO="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_DEBUG="/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_DEBUG="/MTd /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_INSTALL_PREFIX="$ESPEAK_INSTALL" `
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
      -DCMAKE_CXX_STANDARD=17 `
      -DCMAKE_CXX_STANDARD_REQUIRED=ON `
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
# BUILD ONNX RUNTIME (Single Block, No Duplicates)
# ==========================================================
Write-Host "=== Building ONNX Runtime ==="

# Clone ONNX Runtime if not present
git clone --recursive https://github.com/microsoft/onnxruntime $ONNX_SRC

# Update submodules
Push-Location $ONNX_SRC
git fetch --tags
git checkout tags/v1.23.2 -b build-v1.23.2
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
    "-DCMAKE_CXX_STANDARD=17",
    "-DCMAKE_CXX_STANDARD_REQUIRED=ON",
    "-DCMAKE_BUILD_TYPE=Release",
    "-DFETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DCMAKE_COMPILE_WARNING_AS_ERROR=OFF",
    "-DCMAKE_POSITION_INDEPENDENT_CODE=OFF",
    "-Donnxruntime_BUILD_SHARED_LIB=OFF",
    "-Donnxruntime_ENABLE_STATIC_ANALYSIS=OFF",
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded",
    "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW",
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
    "-Donnxruntime_USE_FULL_PROTOBUF=OFF",
    "-Donnxruntime_MSVC_STATIC_RUNTIME=ON",
    "-DABSL_ENABLE_INSTALL=ON",
    "-DABSL_MSVC_STATIC_RUNTIME=ON",
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


# ==========================================================
# INSTALL RE2 as static lib
# ==========================================================

$AbslCMakePath = "$ONNX_BUILD/_deps/abseil_cpp-build/Findabsl.cmake"
$AbslInclude  = "$ONNX_BUILD/_deps/abseil_cpp-build"

Set-Content -Path $AbslCMakePath -Value @"
# UseAbsl.cmake - Imported Abseil targets for RE2
# Set AbslInclude to your ONNXRuntime Abseil build folder
set(AbslInclude "$ONNX_BUILD/_deps/abseil_cpp-build")

# --------------------
# Base
# --------------------
add_library(absl::base STATIC IMPORTED)
set_target_properties(absl::base PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_base.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::absl_log STATIC IMPORTED)
set_target_properties(absl::absl_log PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_log_severity.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::malloc_internal STATIC IMPORTED)
set_target_properties(absl::malloc_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_malloc_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::raw_logging_internal STATIC IMPORTED)
set_target_properties(absl::raw_logging_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_raw_logging_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::spinlock_wait STATIC IMPORTED)
set_target_properties(absl::spinlock_wait PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_spinlock_wait.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::strerror STATIC IMPORTED)
set_target_properties(absl::strerror PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_strerror.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::throw_delegate STATIC IMPORTED)
set_target_properties(absl::throw_delegate PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_throw_delegate.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::tracing_internal STATIC IMPORTED)
set_target_properties(absl::tracing_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_tracing_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Container
# --------------------
add_library(absl::hashtablez_sampler STATIC IMPORTED)
set_target_properties(absl::hashtablez_sampler PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/container/Release/absl_hashtablez_sampler.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::raw_hash_set STATIC IMPORTED)
set_target_properties(absl::raw_hash_set PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/container/Release/absl_raw_hash_set.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# CRC
# --------------------
add_library(absl::crc_cord_state STATIC IMPORTED)
set_target_properties(absl::crc_cord_state PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc_cord_state.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::crc_cpu_detect STATIC IMPORTED)
set_target_properties(absl::crc_cpu_detect PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc_cpu_detect.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::crc_internal STATIC IMPORTED)
set_target_properties(absl::crc_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::crc32c STATIC IMPORTED)
set_target_properties(absl::crc32c PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc32c.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Debugging
# --------------------
add_library(absl::debugging_internal STATIC IMPORTED)
set_target_properties(absl::debugging_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_debugging_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::decode_rust_punycode STATIC IMPORTED)
set_target_properties(absl::decode_rust_punycode PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_decode_rust_punycode.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::demangle_internal STATIC IMPORTED)
set_target_properties(absl::demangle_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_demangle_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::demangle_rust STATIC IMPORTED)
set_target_properties(absl::demangle_rust PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_demangle_rust.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::examine_stack STATIC IMPORTED)
set_target_properties(absl::examine_stack PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_examine_stack.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::leak_check STATIC IMPORTED)
set_target_properties(absl::leak_check PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_leak_check.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::stacktrace STATIC IMPORTED)
set_target_properties(absl::stacktrace PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_stacktrace.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::symbolize STATIC IMPORTED)
set_target_properties(absl::symbolize PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_symbolize.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::utf8_for_code_point STATIC IMPORTED)
set_target_properties(absl::utf8_for_code_point PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_utf8_for_code_point.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Flags
# --------------------
add_library(absl::flags_commandlineflag_internal STATIC IMPORTED)
set_target_properties(absl::flags_commandlineflag_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_commandlineflag_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_commandlineflag STATIC IMPORTED)
set_target_properties(absl::flags_commandlineflag PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_commandlineflag.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_config STATIC IMPORTED)
set_target_properties(absl::flags_config PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_config.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_internal STATIC IMPORTED)
set_target_properties(absl::flags_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_marshalling STATIC IMPORTED)
set_target_properties(absl::flags_marshalling PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_marshalling.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_private_handle_accessor STATIC IMPORTED)
set_target_properties(absl::flags_private_handle_accessor PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_private_handle_accessor.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_program_name STATIC IMPORTED)
set_target_properties(absl::flags_program_name PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_program_name.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::flags_reflection STATIC IMPORTED)
set_target_properties(absl::flags_reflection PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_reflection.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Hash
# --------------------
add_library(absl::city STATIC IMPORTED)
set_target_properties(absl::city PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/hash/Release/absl_city.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::hash STATIC IMPORTED)
set_target_properties(absl::hash PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/hash/Release/absl_hash.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::low_level_hash STATIC IMPORTED)
set_target_properties(absl::low_level_hash PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/hash/Release/absl_low_level_hash.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Log
# --------------------
add_library(absl::log_globals STATIC IMPORTED)
set_target_properties(absl::log_globals PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_globals.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_check_op STATIC IMPORTED)
set_target_properties(absl::log_internal_check_op PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_check_op.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_conditions STATIC IMPORTED)
set_target_properties(absl::log_internal_conditions PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_conditions.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_fnmatch STATIC IMPORTED)
set_target_properties(absl::log_internal_fnmatch PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_fnmatch.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_format STATIC IMPORTED)
set_target_properties(absl::log_internal_format PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_format.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_globals STATIC IMPORTED)
set_target_properties(absl::log_internal_globals PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_globals.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Profiling
# --------------------
add_library(absl::exponential_biased STATIC IMPORTED)
set_target_properties(absl::exponential_biased PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/profiling/Release/absl_exponential_biased.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Strings
# --------------------
add_library(absl::log_internal_log_sink_set STATIC IMPORTED)
set_target_properties(absl::log_internal_log_sink_set PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_log_internal_log_sink_set.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_message STATIC IMPORTED)
set_target_properties(absl::log_internal_message PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_log_internal_message.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_nullguard STATIC IMPORTED)
set_target_properties(absl::log_internal_nullguard PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_log_internal_nullguard.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_proto STATIC IMPORTED)
set_target_properties(absl::log_internal_proto PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_log_internal_proto.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_internal_structured_proto STATIC IMPORTED)
set_target_properties(absl::log_internal_structured_proto PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_log_internal_structured_proto.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::log_sink STATIC IMPORTED)
set_target_properties(absl::log_sink PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_log_sink.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::vlog_config_internal STATIC IMPORTED)
set_target_properties(absl::vlog_config_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_vlog_config_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::int128 STATIC IMPORTED)
set_target_properties(absl::int128 PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/numeric/Release/absl_int128.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::cord_internal STATIC IMPORTED)
set_target_properties(absl::cord_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cord_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::cord STATIC IMPORTED)
set_target_properties(absl::cord PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cord.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::cordz_functions STATIC IMPORTED)
set_target_properties(absl::cordz_functions PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cordz_functions.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::cordz_handle STATIC IMPORTED)
set_target_properties(absl::cordz_handle PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cordz_handle.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::cordz_info STATIC IMPORTED)
set_target_properties(absl::cordz_info PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cordz_info.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::str_format_internal STATIC IMPORTED)
set_target_properties(absl::str_format_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_str_format_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::string_view STATIC IMPORTED)
set_target_properties(absl::string_view PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_string_view.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::strings_internal STATIC IMPORTED)
set_target_properties(absl::strings_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_strings_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::strings STATIC IMPORTED)
set_target_properties(absl::strings PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_strings.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Synchronization
# --------------------
add_library(absl::graphcycles_internal STATIC IMPORTED)
set_target_properties(absl::graphcycles_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/synchronization/Release/absl_graphcycles_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::kernel_timeout_internal STATIC IMPORTED)
set_target_properties(absl::kernel_timeout_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/synchronization/Release/abbl_kernel_timeout_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::synchronization STATIC IMPORTED)
set_target_properties(absl::synchronization PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/synchronization/Release/absl_synchronization.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

# --------------------
# Time
# --------------------
add_library(absl::civil_time STATIC IMPORTED)
set_target_properties(absl::civil_time PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/time/Release/absl_civil_time.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::time_zone STATIC IMPORTED)
set_target_properties(absl::time_zone PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/time/Release/absl_time_zone.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)

add_library(absl::time STATIC IMPORTED)
set_target_properties(absl::time PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/time/Release/absl_time.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
"@

$Re2ZipUrl = "https://github.com/google/re2/archive/refs/tags/2024-07-02.zip"
$DownloadDir = "C:\Temp\re2_download"
$InstallDir  = "$ONNX_BUILD/_deps/onnx-build/Release"

Write-Host "All files under $ONNX_BUILD/_deps/abseil_cpp-build :"
Get-ChildItem "$ONNX_BUILD/_deps/abseil_cpp-build" -Recurse -File -Force
Write-Host "====================================================="

# Create folders
New-Item -ItemType Directory -Path $DownloadDir -Force
New-Item -ItemType Directory -Path $InstallDir -Force
# download
$ZipFile = Join-Path $DownloadDir "re2-2024-07-02.zip"
Write-Host "Downloading RE2..."
Invoke-WebRequest -Uri $Re2ZipUrl -OutFile $ZipFile
# extract
$ExtractDir = Join-Path $DownloadDir "re2_src"
Write-Host "Extracting RE2..."
Expand-Archive -Path $ZipFile -DestinationPath $ExtractDir -Force
# After extraction, RE2 source will be in a folder like 're2-2024-07-02'
$SourceDir = Join-Path $ExtractDir "re2-2024-07-02"
# build static lib
$BuildDir = Join-Path $SourceDir "build"
New-Item -ItemType Directory -Path $BuildDir -Force
Set-Location $BuildDir

# $AbslLibs = @(
#     "$AbslInclude/absl/base/Release/absl_base.lib",
#     "$AbslInclude/absl/base/Release/absl_log_severity.lib",
#     "$AbslInclude/absl/base/Release/absl_malloc_internal.lib",
#     "$AbslInclude/absl/base/Release/absl_raw_logging_internal.lib",
#     "$AbslInclude/absl/base/Release/absl_spinlock_wait.lib",
#     "$AbslInclude/absl/base/Release/absl_strerror.lib",
#     "$AbslInclude/absl/base/Release/absl_throw_delegate.lib",
#     "$AbslInclude/absl/base/Release/absl_tracing_internal.lib",
#     "$AbslInclude/absl/container/Release/absl_hashtablez_sampler.lib",
#     "$AbslInclude/absl/container/Release/absl_raw_hash_set.lib",
#     "$AbslInclude/absl/crc/Release/absl_crc_cord_state.lib",
#     "$AbslInclude/absl/crc/Release/absl_crc_cpu_detect.lib",
#     "$AbslInclude/absl/crc/Release/absl_crc_internal.lib",
#     "$AbslInclude/absl/crc/Release/absl_crc32c.lib",
#     "$AbslInclude/absl/debugging/Release/absl_debugging_internal.lib",
#     "$AbslInclude/absl/debugging/Release/absl_decode_rust_punycode.lib",
#     "$AbslInclude/absl/debugging/Release/absl_demangle_internal.lib",
#     "$AbslInclude/absl/debugging/Release/absl_demangle_rust.lib",
#     "$AbslInclude/absl/debugging/Release/absl_examine_stack.lib",
#     "$AbslInclude/absl/debugging/Release/absl_leak_check.lib",
#     "$AbslInclude/absl/debugging/Release/absl_stacktrace.lib",
#     "$AbslInclude/absl/debugging/Release/absl_symbolize.lib",
#     "$AbslInclude/absl/debugging/Release/absl_utf8_for_code_point.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_commandlineflag_internal.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_commandlineflag.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_config.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_internal.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_marshalling.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_private_handle_accessor.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_program_name.lib",
#     "$AbslInclude/absl/flags/Release/absl_flags_reflection.lib",
#     "$AbslInclude/absl/hash/Release/absl_city.lib",
#     "$AbslInclude/absl/hash/Release/absl_hash.lib",
#     "$AbslInclude/absl/hash/Release/absl_low_level_hash.lib",
#     "$AbslInclude/absl/log/Release/absl_log_globals.lib",
#     "$AbslInclude/absl/log/Release/absl_log_internal_check_op.lib",
#     "$AbslInclude/absl/log/Release/absl_log_internal_conditions.lib",
#     "$AbslInclude/absl/log/Release/absl_log_internal_fnmatch.lib",
#     "$AbslInclude/absl/log/Release/absl_log_internal_format.lib",
#     "$AbslInclude/absl/log/Release/absl_log_internal_globals.lib",
#     "$AbslInclude/absl/profiling/Release/absl_exponential_biased.lib",
#     "$AbslInclude/absl/strings/Release/absl_log_internal_log_sink_set.lib",
#     "$AbslInclude/absl/strings/Release/absl_log_internal_message.lib",
#     "$AbslInclude/absl/strings/Release/absl_log_internal_nullguard.lib",
#     "$AbslInclude/absl/strings/Release/absl_log_internal_proto.lib",
#     "$AbslInclude/absl/strings/Release/absl_log_internal_structured_proto.lib",
#     "$AbslInclude/absl/strings/Release/absl_log_sink.lib",
#     "$AbslInclude/absl/strings/Release/absl_vlog_config_internal.lib",
#     "$AbslInclude/absl/numeric/Release/absl_int128.lib",
#     "$AbslInclude/absl/strings/Release/absl_cord_internal.lib",
#     "$AbslInclude/absl/strings/Release/absl_cord.lib",
#     "$AbslInclude/absl/strings/Release/absl_cordz_functions.lib",
#     "$AbslInclude/absl/strings/Release/absl_cordz_handle.lib",
#     "$AbslInclude/absl/strings/Release/absl_cordz_info.lib",
#     "$AbslInclude/absl/strings/Release/absl_str_format_internal.lib",
#     "$AbslInclude/absl/strings/Release/absl_string_view.lib",
#     "$AbslInclude/absl/strings/Release/absl_strings_internal.lib",
#     "$AbslInclude/absl/strings/Release/absl_strings.lib",
#     "$AbslInclude/absl/synchronization/Release/absl_graphcycles_internal.lib",
#     "$AbslInclude/absl/synchronization/Release/absl_kernel_timeout_internal.lib",
#     "$AbslInclude/absl/synchronization/Release/absl_synchronization.lib",
#     "$AbslInclude/absl/time/Release/absl_civil_time.lib",
#     "$AbslInclude/absl/time/Release/absl_time_zone.lib",
#     "$AbslInclude/absl/time/Release/absl_time.lib"
# )

# Convert the array to a semicolon-separated string for CMake
$re2InstallDir   = "$ONNX_BUILD/_deps/onnx-build/Release/re2"

Write-Host "Configuring CMake..."
cmake -G "Visual Studio 17 2022" `
      -A x64 `
      -DCMAKE_BUILD_TYPE=Release `
      -DCMAKE_INSTALL_PREFIX=$re2InstallDir `
      -DBUILD_SHARED_LIBS=OFF `
      -DRE2_BUILD_TESTING=OFF `
      -DRE2_USE_EXTERNAL_ABSL=ON `
      -DCMAKE_MODULE_PATH=$AbslInclude `
      $SourceDir

cmake --build . --config Release
cmake --build . --config Release --target INSTALL

Write-Host "Done installing re2.lib!"
Write-Host "Static library: $re2InstallDir\lib"
Write-Host "Headers: $re2InstallDir\include"


# ==========================================================
# EXPORT ENVIRONMENT
# ==========================================================
$env:ONNXRUNTIME_INCLUDE_DIR = Join-Path $ONNX_SRC "include"
$env:ORT_STRATEGY            = "system"
$env:ORT_LIB_LOCATION        = $ONNX_BUILD
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


Write-Host "`n=== FINAL .lib files in $ONNX_BUILD ==="
Get-ChildItem -Path $ONNX_BUILD -Filter *.lib -Recurse -File |
    ForEach-Object { Write-Host $_.FullName }

Write-Host "`n=== VCPKG .lib files in $env:VCPKG_ROOT ==="
Get-ChildItem -Path "$env:VCPKG_ROOT" -Recurse -File -Filter *.lib |
    ForEach-Object { Write-Host $_.FullName }

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


# Move vcpkg re2.lib dep to target folder so ort-sys can find it
# NOTE: (for some reason onnx runtime doesnt build re2.lib)
# Copy-Item -Path "C:\vcpkg\installed\x64-windows-static\lib\re2.lib" -Destination "$ONNX_BUILD\_deps\onnx-build\Release\re2.lib" -Force
# Remove-Item -Path "C:\vcpkg\installed\*" -Recurse -Force
# Remove-Item -Path "C:\vcpkg\buildtrees\*" -Recurse -Force
# Remove-Item -Path "C:\vcpkg\packages\*" -Recurse -Force

# Before cargo build
$env:RUSTFLAGS = "-C target-feature=+crt-static `
                  -C codegen-units=1 `
                  -C opt-level=3 `
                  -C link-arg=$ONNX_BUILD/_deps/onnx-build/Release/re2/lib/re2.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_base.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_log_severity.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_malloc_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_raw_logging_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_spinlock_wait.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_strerror.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_throw_delegate.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/base/Release/absl_tracing_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/container/Release/absl_hashtablez_sampler.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/container/Release/absl_raw_hash_set.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/crc/Release/absl_crc_cord_state.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/crc/Release/absl_crc_cpu_detect.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/crc/Release/absl_crc_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/crc/Release/absl_crc32c.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_debugging_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_decode_rust_punycode.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_demangle_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_demangle_rust.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_examine_stack.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_leak_check.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_stacktrace.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_symbolize.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/debugging/Release/absl_utf8_for_code_point.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_commandlineflag_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_commandlineflag.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_config.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_marshalling.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_private_handle_accessor.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_program_name.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/flags/Release/absl_flags_reflection.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/hash/Release/absl_city.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/hash/Release/absl_hash.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/hash/Release/absl_low_level_hash.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_globals.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_check_op.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_conditions.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_fnmatch.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_format.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_globals.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_log_sink_set.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_message.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_nullguard.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_proto.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_internal_structured_proto.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_log_sink.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/log/Release/absl_vlog_config_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/numeric/Release/absl_int128.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/profiling/Release/absl_exponential_biased.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_cord_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_cord.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_cordz_functions.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_cordz_handle.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_cordz_info.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_str_format_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_string_view.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_strings_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/strings/Release/absl_strings.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/synchronization/Release/absl_graphcycles_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/synchronization/Release/absl_kernel_timeout_internal.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/synchronization/Release/absl_synchronization.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/time/Release/absl_civil_time.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/time/Release/absl_time_zone.lib `
                  -C link-arg=$ONNX_BUILD/_deps/abseil_cpp-build/absl/time/Release/absl_time.lib `
                  -C link-arg=$ONNX_BUILD/_deps/protobuf-build/Release/libprotobuf-lite.lib `
                  -C link-arg=$ONNX_BUILD/_deps/protobuf-build/Release/libprotobuf.lib `
                  -C link-arg=$ONNX_BUILD/_deps/protobuf-build/Release/libprotoc.lib `
                  -C link-arg=$ONNX_BUILD/_deps/pytorch_cpuinfo-build/Release/cpuinfo.lib `
                  -C link-arg=$ONNX_BUILD/_deps/flatbuffers-build/Release/flatbuffers.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_common.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_flatbuffers.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_framework.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_graph.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_lora.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_mlas.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_optimizer.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_providers_shared.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_providers.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_session.lib `
                  -C link-arg=$ONNX_BUILD/Release/onnxruntime_util.lib `
                  -C link-arg=$ONNX_BUILD/_deps/onnx-build/Release/onnx_proto.lib `
                  -C link-arg=$ONNX_BUILD/_deps/onnx-build/Release/onnx.lib `
                  -C link-arg=/DEFAULTLIB:legacy_stdio_definitions.lib `
                  -C link-arg=/DEFAULTLIB:OLDNAMES.lib "

$env:CXXFLAGS="/std:c++17 /MT /D_CRT_SECURE_NO_WARNINGS /D_CRT_NONSTDC_NO_DEPRECATE"

Set-Location $PROJECT_ROOT

Write-Host "Ensuring Rust target $TARGET is installed..."
rustup target add $TARGET

Write-Host "Building Rust binary..."
cargo build --release --target $TARGET --features ($CARGO_FEATURES -join ",") -vv

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