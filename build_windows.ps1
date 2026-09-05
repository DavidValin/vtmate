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
$BIN_BASE       = "vtmate"
$PROJECT_ROOT   = Split-Path -Parent $MyInvocation.MyCommand.Path
$DIST_DIR       = Join-Path $PROJECT_ROOT "dist"
$TARGET_DIR     = Join-Path $PROJECT_ROOT "target-cross"
$VENDOR_DIR     = Join-Path $PROJECT_ROOT "vendor"

# cargo's own target dir. whisper-rs-sys' nested CMake ExternalProject for
# ggml-vulkan's shader compiler builds many directories deep under it (e.g.
# .../build/whisper-rs-sys-<hash>/out/build/.../CMakeFiles/CMakeScratch/
# TryCompile-<hash>/cmTC_<hash>.dir/Debug/...), and the hash segments cargo
# and CMake generate vary in length between runs - keeping this near the
# drive root instead of under PROJECT_ROOT\target maximizes the headroom
# under Windows' 260-char MAX_PATH.
$env:CARGO_TARGET_DIR = Join-Path (Split-Path -Qualifier $PROJECT_ROOT) "c"

# MSBuild's FileTracker component (the .tlog bookkeeping files under
# cmTC_*.dir\...) does not support long paths at all, independent of
# CARGO_TARGET_DIR or the OS's own long-path support. Tracking is a pure
# incremental-rebuild optimization, irrelevant to a one-shot CI build, so
# disable it outright rather than depend on path-length budgets holding.
$env:TrackFileAccess = "false"

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
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $OPENBLAS_DIR, $ONNX_SRC, $ONNX_BUILD, $env:CARGO_TARGET_DIR, $TARGET_DIR, $DIST_DIR

# ==========================================================
# LOCATE VISUAL STUDIO / LOAD MSVC ENVIRONMENT
# Located via vswhere rather than a hardcoded install path: the edition
# and version of Visual Studio on the GitHub runners changes without
# notice (windows-latest moved off VS2022 Enterprise), but vswhere itself
# lives at a fixed location on every machine that has VS installed.
# ==========================================================
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
    Write-Error "vswhere.exe not found at $vswhere - is Visual Studio installed?"
    exit 1
}

$vsArgs  = @("-latest", "-products", "*",
             "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64")
$VS_PATH = & $vswhere @vsArgs -property installationPath
if (-not $VS_PATH) {
    Write-Error "No Visual Studio install with the C++ x64 toolset was found."
    exit 1
}
$VS_VERSION = & $vswhere @vsArgs -property installationVersion
$VS_MAJOR   = ($VS_VERSION -split '\.')[0]
Write-Host "Visual Studio: $VS_PATH (version $VS_VERSION)"

if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    Write-Host "=== Loading MSVC environment (x64) ==="

    $vsdev = Join-Path $VS_PATH "Common7\Tools\VsDevCmd.bat"
    if (-not (Test-Path $vsdev)) {
        Write-Error "VsDevCmd.bat not found at $vsdev"
        exit 1
    }

    # Import the environment VsDevCmd sets up into this PowerShell session.
    cmd /c "`"$vsdev`" -arch=amd64 -host_arch=amd64 && set" | ForEach-Object {
        if ($_ -match "^(.*?)=(.*)$") {
            Set-Item -Path "Env:$($matches[1])" -Value $matches[2]
        }
    }

    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        Write-Error "Loaded VsDevCmd from $vsdev but cl.exe is still not on PATH."
        exit 1
    }
}
Write-Host "cl.exe: $((Get-Command cl.exe).Source)"

# ==========================================================
# CHECK REQUIRED TOOLS
# ==========================================================
foreach ($tool in "cl.exe","cmake","git","cargo","powershell") {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Error "ERROR: Required tool $tool not found."
        exit 1
    }
}

# ==========================================================
# RESOLVE CMAKE GENERATOR
# The generator name is version-specific ("Visual Studio 17 2022",
# "Visual Studio 18 ...") and the runner image upgrades VS without warning,
# so ask the installed CMake which generator matches the installed VS major
# version instead of hardcoding one.
# ==========================================================
$genMatch = cmake --help |
            Select-String -Pattern "Visual Studio $VS_MAJOR [0-9]{4}" |
            Select-Object -First 1
if (-not $genMatch) {
    Write-Error "Installed CMake has no generator for Visual Studio $VS_MAJOR. Upgrade CMake."
    exit 1
}
$CMAKE_GENERATOR = $genMatch.Matches[0].Value
Write-Host "CMake generator: $CMAKE_GENERATOR"

# cmake-rs (espeak-rs-sys, whisper-rs-sys) reads CMAKE_GENERATOR from the
# environment and supplies -Ax64 / -Thost=x64 itself, so this single export
# keeps the crate sub-builds on the same toolchain as ours.
$env:CMAKE_GENERATOR = $CMAKE_GENERATOR

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
        # CUDA 12.3 rejects MSVC >= 19.40 (crt/host_config.h), so it cannot
        # build against VS 2026 on current runners. 13.3 allows VS 2019-2026.
        $CUDA_VERSION = "13.3.0"
        $CUDA_MM = "13.3"
        $cuda_root = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CUDA_MM"
        $CUDA_INSTALLER = "$env:TEMP\cuda_installer.exe"
        $CUDA_URL = "https://developer.download.nvidia.com/compute/cuda/$CUDA_VERSION/network_installers/cuda_${CUDA_VERSION}_windows_network.exe"

        Invoke-WebRequest -Uri $CUDA_URL -OutFile $CUDA_INSTALLER -UseBasicParsing

        if (-not (Test-Path $CUDA_INSTALLER)) {
            Write-Error "Failed to download CUDA installer."
            exit 1
        }

        # -Wait blocks forever if the installer stalls or prompts. In CI that
        # burns the whole job timeout with no output, so bound it explicitly.
        $arguments = "--silent --toolkit --installpath `"$cuda_root`""
        $proc = Start-Process -FilePath $CUDA_INSTALLER -ArgumentList $arguments -PassThru
        if (-not $proc.WaitForExit(45 * 60 * 1000)) {
            try { $proc.Kill() } catch {}
            Write-Error "CUDA installer timed out after 45 minutes (network installer stalled?)"
            exit 1
        }
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

    # ------------------------------------------------------
    # cuDNN (required by the ONNX Runtime CUDA execution provider).
    # Pulled from NVIDIA's public redist mirror - no developer login needed.
    # ------------------------------------------------------
    if (-not $env:CUDNN_HOME -or -not (Test-Path $env:CUDNN_HOME)) {
        # The _cudaXX suffix must match the installed CUDA major version.
        # This tracks CUDA 13.x above; going back to CUDA 12.x means _cuda12.
        $CUDNN_VERSION = "9.16.0.29"
        $CUDNN_NAME    = "cudnn-windows-x86_64-${CUDNN_VERSION}_cuda13-archive"
        $CUDNN_URL     = "https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/windows-x86_64/$CUDNN_NAME.zip"
        $CUDNN_ZIP     = "$env:TEMP\cudnn.zip"
        $CUDNN_ROOT    = "C:\cudnn\$CUDNN_VERSION"

        Write-Host "cuDNN not detected. Downloading $CUDNN_NAME ..."
        Invoke-WebRequest -Uri $CUDNN_URL -OutFile $CUDNN_ZIP -UseBasicParsing
        if (-not (Test-Path $CUDNN_ZIP)) {
            Write-Error "Failed to download cuDNN."
            exit 1
        }

        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $CUDNN_ROOT) | Out-Null
        Expand-Archive -Path $CUDNN_ZIP -DestinationPath (Split-Path -Parent $CUDNN_ROOT) -Force
        # The archive unpacks to <name>/ - normalise to a version-only path.
        $extracted = Join-Path (Split-Path -Parent $CUDNN_ROOT) $CUDNN_NAME
        if (Test-Path $CUDNN_ROOT) { Remove-Item -Recurse -Force $CUDNN_ROOT }
        Rename-Item -Path $extracted -NewName $CUDNN_VERSION -Force
        Remove-Item -Force $CUDNN_ZIP

        $env:CUDNN_HOME = $CUDNN_ROOT
        Write-Host "cuDNN installed to $env:CUDNN_HOME"
    }
    else {
        Write-Host "cuDNN already present."
    }

    $env:CUDNN_PATH = $env:CUDNN_HOME
    $env:Path       = "$env:CUDNN_HOME\bin;$env:Path"

    $CUDNN_LIB_DIR = Join-Path $env:CUDNN_HOME "lib\x64"
    if (-not (Test-Path (Join-Path $CUDNN_LIB_DIR "cudnn.lib"))) {
        Write-Error "cudnn.lib not found under $CUDNN_LIB_DIR"
        exit 1
    }
    Write-Host "CUDNN_HOME = $env:CUDNN_HOME"
}
else {
    Remove-Item Env:CUDAToolkit_ROOT -ErrorAction SilentlyContinue
    Remove-Item Env:CUDA_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:CUDA_HOME -ErrorAction SilentlyContinue
    Remove-Item Env:CUDA_ROOT -ErrorAction SilentlyContinue
}

# ==========================================================
# ENSURE VULKAN SDK IF REQUIRED (BUILD-TIME)
# ggml-vulkan needs the loader import lib *and* glslc to compile its
# shaders; neither is present on a stock windows-latest runner.
# ==========================================================
if ($WITH_VULKAN) {
    $sdkOk = $env:VULKAN_SDK -and (Test-Path $env:VULKAN_SDK)
    if (-not $sdkOk) {
        Write-Host "Vulkan SDK not detected. Installing Vulkan SDK for build..."
        $VULKAN_VERSION   = "1.3.296.0"
        $vulkan_root      = "C:\VulkanSDK\$VULKAN_VERSION"
        $VULKAN_INSTALLER = "$env:TEMP\vulkan_sdk.exe"
        $VULKAN_URL       = "https://sdk.lunarg.com/sdk/download/$VULKAN_VERSION/windows/VulkanSDK-$VULKAN_VERSION-Installer.exe"

        Invoke-WebRequest -Uri $VULKAN_URL -OutFile $VULKAN_INSTALLER -UseBasicParsing
        if (-not (Test-Path $VULKAN_INSTALLER)) {
            Write-Error "Failed to download Vulkan SDK installer."
            exit 1
        }

        $arguments = "--root `"$vulkan_root`" --accept-licenses --default-answer --confirm-command install"
        $proc = Start-Process -FilePath $VULKAN_INSTALLER -ArgumentList $arguments -Wait -PassThru
        if ($proc.ExitCode -ne 0) {
            Write-Error "Vulkan SDK installation failed with exit code $($proc.ExitCode)"
            exit 1
        }

        $env:VULKAN_SDK = $vulkan_root
        Write-Host "Vulkan SDK successfully installed for build."
    }
    else {
        Write-Host "Vulkan SDK already present."
    }

    $env:Path = "$env:VULKAN_SDK\Bin;$env:Path"
    Write-Host "VULKAN_SDK = $env:VULKAN_SDK"

    # ggml-vulkan shells out to glslc at build time - fail loudly here rather
    # than 40 minutes later inside whisper-rs-sys.
    if (-not (Get-Command glslc -ErrorAction SilentlyContinue)) {
        Write-Error "Vulkan SDK installed but glslc not found in PATH."
        exit 1
    }
}
else {
    Remove-Item Env:VULKAN_SDK -ErrorAction SilentlyContinue
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
      -G $CMAKE_GENERATOR `
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

    if (Test-Path $RENAMED_LIB) {
        Write-Host "OpenBLAS library already built — reusing $RENAMED_LIB"
    } else {
        Write-Host "OpenBLAS library not found — building from source..."

        $tmp_build = Join-Path $env:TEMP "openblas_build"
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp_build
        New-Item -ItemType Directory -Force -Path $tmp_build | Out-Null

        $src_dir = Join-Path $tmp_build "OpenBLAS"
        git clone --depth 1 --branch v0.3.30 https://github.com/xianyi/OpenBLAS $src_dir

        Push-Location $src_dir
        cmake -S . -B build -G $CMAKE_GENERATOR -A x64 `
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
          -DUSE_OPENMP=OFF `
          -DUSE_THREAD=ON `
          -DNUM_THREADS=64 `
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
    }

    # Ensure the variable points to the renamed library
    $OPENBLAS_LIB = $RENAMED_LIB

    # Set environment variables
    $env:OpenBLAS_DIR = $PREBUILT_OPENBLAS_DIR
    $env:OpenBLAS_LIBRARIES = $OPENBLAS_LIB
    $env:OpenBLAS_INCLUDE_DIR = $INCLUDE_DIR
}

# $ONNX_CUDA_FLAG drives ONNX Runtime's CUDA execution provider.
# $ONNX_VULKAN_FLAG and $ONNX_USE_BLAS drive the ggml/whisper backend
# (ORT itself exposes neither a Vulkan EP nor a BLAS switch); they are
# consumed by the GGML_* env + CMAKE_ARGS block further down.
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

# ==========================================================
# PREBUILT ONNX RUNTIME (CUDA VARIANT ONLY)
#
# Building ORT with the CUDA execution provider from source does not fit in
# GitHub's 6h per-job ceiling - a run with only three target architectures was
# still compiling contrib_ops when the limit hit. It also drags in the CUDA 13
# vs ORT 1.24 incompatibilities (deprecated longlong4 escalated by nvcc's
# -Werror all-warnings, which -Xcompiler flags cannot undo).
#
# Microsoft ships a prebuilt ORT for exactly this pin and CUDA major, so link
# that instead. Unlike cpu/vulkan this variant is therefore NOT a single static
# exe: onnxruntime DLLs ship beside it, as cuDNN/cuBLAS already do.
# ==========================================================
$ORT_PREBUILT = $null
if ($WITH_CUDA) {
    $ortVer  = "1.24.1"
    $ortName = "onnxruntime-win-x64-gpu_cuda13-$ortVer"
    $ortZip  = "$env:TEMP\$ortName.zip"
    Write-Host "Downloading prebuilt $ortName ..."
    Invoke-WebRequest -UseBasicParsing -OutFile $ortZip `
      -Uri "https://github.com/microsoft/onnxruntime/releases/download/v$ortVer/$ortName.zip"
    New-Item -ItemType Directory -Force -Path $VENDOR_DIR | Out-Null
    Expand-Archive -Path $ortZip -DestinationPath $VENDOR_DIR -Force
    Remove-Item -Force $ortZip

    # The archive is named ..._cuda13-<ver>.zip but unpacks to
    # onnxruntime-win-x64-gpu-<ver>/ without the _cuda13 part, so locate the
    # import library rather than assuming the directory name.
    $ortLib = Get-ChildItem -Path $VENDOR_DIR -Recurse -Filter "onnxruntime.lib" -ErrorAction SilentlyContinue |
              Where-Object { $_.FullName -like "*onnxruntime-win-x64-gpu*" } |
              Select-Object -First 1
    if (-not $ortLib) {
        Write-Host "Contents of ${VENDOR_DIR}:"
        Get-ChildItem $VENDOR_DIR -Directory | ForEach-Object { Write-Host "  $($_.Name)" }
        Write-Error "prebuilt ORT: onnxruntime.lib not found after extracting $ortName"
        exit 1
    }
    $ortDir = Split-Path -Parent (Split-Path -Parent $ortLib.FullName)
    $ORT_PREBUILT = $ortDir
    Write-Host "Using prebuilt ONNX Runtime at $ORT_PREBUILT"
}

# Everything from here to EXPORT ENVIRONMENT builds ORT from source, which the
# CUDA variant skips in favour of the prebuilt package fetched above.
if (-not $ORT_PREBUILT) {

# ==========================================================
# CLONE ONNX RUNTIME
# NOTE: must happen BEFORE absl/re2 are built, because those
# install into $ONNX_BUILD (= $ONNX_SRC\build-static) and a
# later delete of $ONNX_SRC would wipe them out.
# ==========================================================
if (Test-Path $ONNX_SRC) {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $ONNX_SRC
}
# v1.23.2, NOT the v1.24.1 that build_linux.sh pins. The abseil build below
# (20250512.0) and the lib set this script expects are tuned for 1.23.2;
# building 1.24.1 fails at link with a missing absl_low_level_hash.lib.
# This used to be a v1.24.1 clone followed by a checkout of v1.23.2 - same
# result, but the tag now says what actually gets built.
git clone --recursive --depth 1 -b v1.23.2 https://github.com/microsoft/onnxruntime $ONNX_SRC
if ($LASTEXITCODE -ne 0) { exit 1 }

# Update submodules
Push-Location $ONNX_SRC
# No checkout here: the clone above already pins the tag. This used to
# `git checkout tags/v1.23.2`, silently overriding that pin - which is why
# builds reported VER_STRING="1.23.2".
git submodule update --init --recursive --force
if ($LASTEXITCODE -ne 0) { exit 1 }
Pop-Location

# ==========================================================
# BUILD ABSL AND RE2 BEFORE ONNX RUNTIME CONFIGURE
# ==========================================================
Write-Host "=== Building Abseil (absl) and RE2 static libraries ==="

# -----------------------------------------------------------
# Step 1: Build abseil-cpp (absl) from source
# -----------------------------------------------------------
$AbslVersion = "20250512.0"
$AbslUrl = "https://github.com/abseil/abseil-cpp/archive/refs/tags/$AbslVersion.zip"
$AbslDownloadDir = "$env:TEMP\absl_download"
$AbslSourceDir = "$AbslDownloadDir\abseil-cpp-$AbslVersion"
$AbslBuildDir = "$AbslSourceDir\build"
$AbslInstallDir = "$ONNX_BUILD/_deps/abseil_cpp-build"

Write-Host "Downloading Abseil (absl) $AbslVersion..."
New-Item -ItemType Directory -Force -Path $AbslDownloadDir | Out-Null
$AbslZipFile = Join-Path $AbslDownloadDir "abseil-cpp-$AbslVersion.zip"
Invoke-WebRequest -Uri $AbslUrl -OutFile $AbslZipFile -UseBasicParsing
Expand-Archive -Path $AbslZipFile -DestinationPath $AbslDownloadDir -Force

# Use forward-slash paths for cmake to avoid escape sequence issues with backslashes
$AbslInstallDirFS = $AbslInstallDir -replace '\\', '/'

Write-Host "Configuring Abseil (absl) with CMake..."
New-Item -ItemType Directory -Force -Path $AbslBuildDir | Out-Null
cmake -S "$AbslSourceDir" -B "$AbslBuildDir" `
      -G $CMAKE_GENERATOR `
      -A x64 `
      -DCMAKE_BUILD_TYPE=Release `
      -DCMAKE_INSTALL_PREFIX="$AbslInstallDirFS" `
      -DBUILD_SHARED_LIBS=OFF `
      -DABSL_BUILD_TESTING=OFF `
      -DABSL_PROPAGATE_CXX_STD=ON `
      -DABSL_MSVC_STATIC_RUNTIME=ON `
      -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
      -DCMAKE_CXX_STANDARD=17 `
      -DCMAKE_CXX_STANDARD_REQUIRED=ON `
      -DCMAKE_C_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_C_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_CXX_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS" `
      -DCMAKE_EXE_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib" `
     -DCMAKE_STATIC_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib" `
      -DABSL_ENABLE_INSTALL=ON
    if ($LASTEXITCODE -ne 0) { exit 1 }

    Write-Host "Building Abseil (absl)..."
    cmake --build "$AbslBuildDir" --config Release --verbose
    if ($LASTEXITCODE -ne 0) { exit 1 }

    Write-Host "Installing Abseil (absl) to $AbslInstallDirFS..."
    cmake --build "$AbslBuildDir" --config Release --target INSTALL
    if ($LASTEXITCODE -ne 0) { exit 1 }

# -----------------------------------------------------------
# Step 2: Create Findabsl.cmake for RE2 to locate abseil
# -----------------------------------------------------------
$AbslCMakePath = "$AbslInstallDir/Findabsl.cmake"
# Use forward slashes to avoid CMake escape sequence interpretation of backslashes
$AbslInclude  = ("$AbslInstallDir") -replace '\\', '/'

Set-Content -Path $AbslCMakePath -Value @"
# UseAbsl.cmake - Imported Abseil targets for RE2
# Set AbslInclude to your abseil build folder
set(AbslInclude "$AbslInclude")

# --------------------
# Base
# --------------------
if(NOT TARGET absl::base)
add_library(absl::base STATIC IMPORTED)
set_target_properties(absl::base PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_base.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::absl_log)
add_library(absl::absl_log STATIC IMPORTED)
set_target_properties(absl::absl_log PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_log_severity.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"

)
endif()

if(NOT TARGET absl::malloc_internal)
add_library(absl::malloc_internal STATIC IMPORTED)
set_target_properties(absl::malloc_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_malloc_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::raw_logging_internal)
add_library(absl::raw_logging_internal STATIC IMPORTED)
set_target_properties(absl::raw_logging_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_raw_logging_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::spinlock_wait)
add_library(absl::spinlock_wait STATIC IMPORTED)
set_target_properties(absl::spinlock_wait PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_spinlock_wait.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::strerror)
add_library(absl::strerror STATIC IMPORTED)
set_target_properties(absl::strerror PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_strerror.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::throw_delegate)
add_library(absl::throw_delegate STATIC IMPORTED)
set_target_properties(absl::throw_delegate PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_throw_delegate.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::tracing_internal)
add_library(absl::tracing_internal STATIC IMPORTED)
set_target_properties(absl::tracing_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/base/Release/absl_tracing_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::hashtablez_sampler)
add_library(absl::hashtablez_sampler STATIC IMPORTED)
set_target_properties(absl::hashtablez_sampler PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/container/Release/absl_hashtablez_sampler.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::raw_hash_set)
add_library(absl::raw_hash_set STATIC IMPORTED)
set_target_properties(absl::raw_hash_set PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/container/Release/absl_raw_hash_set.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::crc_cord_state)
add_library(absl::crc_cord_state STATIC IMPORTED)
set_target_properties(absl::crc_cord_state PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc_cord_state.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::crc_cpu_detect)
add_library(absl::crc_cpu_detect STATIC IMPORTED)
set_target_properties(absl::crc_cpu_detect PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc_cpu_detect.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::crc_internal)
add_library(absl::crc_internal STATIC IMPORTED)
set_target_properties(absl::crc_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::crc32c)
add_library(absl::crc32c STATIC IMPORTED)
set_target_properties(absl::crc32c PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/crc/Release/absl_crc32c.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::debugging_internal)
add_library(absl::debugging_internal STATIC IMPORTED)
set_target_properties(absl::debugging_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_debugging_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::decode_rust_punycode)
add_library(absl::decode_rust_punycode STATIC IMPORTED)
set_target_properties(absl::decode_rust_punycode PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_decode_rust_punycode.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::demangle_internal)
add_library(absl::demangle_internal STATIC IMPORTED)
set_target_properties(absl::demangle_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_demangle_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::demangle_rust)
add_library(absl::demangle_rust STATIC IMPORTED)
set_target_properties(absl::demangle_rust PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_demangle_rust.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::examine_stack)
add_library(absl::examine_stack STATIC IMPORTED)
set_target_properties(absl::examine_stack PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_examine_stack.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::leak_check)
add_library(absl::leak_check STATIC IMPORTED)
set_target_properties(absl::leak_check PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_leak_check.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::stacktrace)
add_library(absl::stacktrace STATIC IMPORTED)
set_target_properties(absl::stacktrace PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_stacktrace.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::symbolize)
add_library(absl::symbolize STATIC IMPORTED)
set_target_properties(absl::symbolize PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_symbolize.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::utf8_for_code_point)
add_library(absl::utf8_for_code_point STATIC IMPORTED)
set_target_properties(absl::utf8_for_code_point PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/debugging/Release/absl_utf8_for_code_point.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_commandlineflag_internal)
add_library(absl::flags_commandlineflag_internal STATIC IMPORTED)
set_target_properties(absl::flags_commandlineflag_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_commandlineflag_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_commandlineflag)
add_library(absl::flags_commandlineflag STATIC IMPORTED)
set_target_properties(absl::flags_commandlineflag PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_commandlineflag.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_config)
add_library(absl::flags_config STATIC IMPORTED)
set_target_properties(absl::flags_config PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_config.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_internal)
add_library(absl::flags_internal STATIC IMPORTED)
set_target_properties(absl::flags_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_marshalling)
add_library(absl::flags_marshalling STATIC IMPORTED)
set_target_properties(absl::flags_marshalling PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_marshalling.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_private_handle_accessor)
add_library(absl::flags_private_handle_accessor STATIC IMPORTED)
set_target_properties(absl::flags_private_handle_accessor PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_private_handle_accessor.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_program_name)
add_library(absl::flags_program_name STATIC IMPORTED)
set_target_properties(absl::flags_program_name PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_program_name.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags_reflection)
add_library(absl::flags_reflection STATIC IMPORTED)
set_target_properties(absl::flags_reflection PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags_reflection.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::city)
add_library(absl::city STATIC IMPORTED)
set_target_properties(absl::city PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/hash/Release/absl_city.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::hash)
add_library(absl::hash STATIC IMPORTED)
set_target_properties(absl::hash PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/hash/Release/absl_hash.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::low_level_hash)
add_library(absl::low_level_hash STATIC IMPORTED)
set_target_properties(absl::low_level_hash PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/hash/Release/absl_low_level_hash.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_globals)
add_library(absl::log_globals STATIC IMPORTED)
set_target_properties(absl::log_globals PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_globals.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_check_op)
add_library(absl::log_internal_check_op STATIC IMPORTED)
set_target_properties(absl::log_internal_check_op PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_check_op.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_conditions)
add_library(absl::log_internal_conditions STATIC IMPORTED)
set_target_properties(absl::log_internal_conditions PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_conditions.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_fnmatch)
add_library(absl::log_internal_fnmatch STATIC IMPORTED)
set_target_properties(absl::log_internal_fnmatch PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_fnmatch.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_format)
add_library(absl::log_internal_format STATIC IMPORTED)
set_target_properties(absl::log_internal_format PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_format.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_globals)
add_library(absl::log_internal_globals STATIC IMPORTED)
set_target_properties(absl::log_internal_globals PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_globals.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_log_sink_set)
add_library(absl::log_internal_log_sink_set STATIC IMPORTED)
set_target_properties(absl::log_internal_log_sink_set PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_log_sink_set.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_message)
add_library(absl::log_internal_message STATIC IMPORTED)
set_target_properties(absl::log_internal_message PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_message.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_nullguard)
add_library(absl::log_internal_nullguard STATIC IMPORTED)
set_target_properties(absl::log_internal_nullguard PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_nullguard.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_proto)
add_library(absl::log_internal_proto STATIC IMPORTED)
set_target_properties(absl::log_internal_proto PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_proto.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_internal_structured_proto)
add_library(absl::log_internal_structured_proto STATIC IMPORTED)
set_target_properties(absl::log_internal_structured_proto PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_internal_structured_proto.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::log_sink)
add_library(absl::log_sink STATIC IMPORTED)
set_target_properties(absl::log_sink PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log_sink.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::vlog_config_internal)
add_library(absl::vlog_config_internal STATIC IMPORTED)
set_target_properties(absl::vlog_config_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_vlog_config_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::int128)
add_library(absl::int128 STATIC IMPORTED)
set_target_properties(absl::int128 PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/numeric/Release/absl_int128.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::exponential_biased)
add_library(absl::exponential_biased STATIC IMPORTED)
set_target_properties(absl::exponential_biased PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/profiling/Release/absl_exponential_biased.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::cord_internal)
add_library(absl::cord_internal STATIC IMPORTED)
set_target_properties(absl::cord_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cord_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::cord)
add_library(absl::cord STATIC IMPORTED)
set_target_properties(absl::cord PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cord.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::cordz_functions)
add_library(absl::cordz_functions STATIC IMPORTED)
set_target_properties(absl::cordz_functions PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cordz_functions.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::cordz_handle)
add_library(absl::cordz_handle STATIC IMPORTED)
set_target_properties(absl::cordz_handle PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cordz_handle.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::cordz_info)
add_library(absl::cordz_info STATIC IMPORTED)
set_target_properties(absl::cordz_info PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_cordz_info.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::str_format_internal)
add_library(absl::str_format_internal STATIC IMPORTED)
set_target_properties(absl::str_format_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_str_format_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::string_view)
add_library(absl::string_view STATIC IMPORTED)
set_target_properties(absl::string_view PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_string_view.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::strings_internal)
add_library(absl::strings_internal STATIC IMPORTED)
set_target_properties(absl::strings_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_strings_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::strings)
add_library(absl::strings STATIC IMPORTED)
set_target_properties(absl::strings PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_strings.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::graphcycles_internal)
add_library(absl::graphcycles_internal STATIC IMPORTED)
set_target_properties(absl::graphcycles_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/synchronization/Release/absl_graphcycles_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::kernel_timeout_internal)
add_library(absl::kernel_timeout_internal STATIC IMPORTED)
set_target_properties(absl::kernel_timeout_internal PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/synchronization/Release/absl_kernel_timeout_internal.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::synchronization)
add_library(absl::synchronization STATIC IMPORTED)
set_target_properties(absl::synchronization PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/synchronization/Release/absl_synchronization.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::civil_time)
add_library(absl::civil_time STATIC IMPORTED)
set_target_properties(absl::civil_time PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/time/Release/absl_civil_time.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::time_zone)
add_library(absl::time_zone STATIC IMPORTED)
set_target_properties(absl::time_zone PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/time/Release/absl_time_zone.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::time)
add_library(absl::time STATIC IMPORTED)
set_target_properties(absl::time PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/time/Release/absl_time.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

# --------------------
# RE2 requires these additional absl targets
# --------------------
if(NOT TARGET absl::absl_check)
add_library(absl::absl_check STATIC IMPORTED)
set_target_properties(absl::absl_check PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_check.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::absl_log)
add_library(absl::absl_log STATIC IMPORTED)
set_target_properties(absl::absl_log PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/log/Release/absl_log.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flags)
add_library(absl::flags STATIC IMPORTED)
set_target_properties(absl::flags PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/flags/Release/absl_flags.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::str_format)
add_library(absl::str_format STATIC IMPORTED)
set_target_properties(absl::str_format PROPERTIES
    IMPORTED_LOCATION "${AbslInclude}/absl/strings/Release/absl_str_format.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

# Header-only targets (no .lib)
if(NOT TARGET absl::core_headers)
add_library(absl::core_headers INTERFACE IMPORTED)
set_target_properties(absl::core_headers PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::fixed_array)
add_library(absl::fixed_array INTERFACE IMPORTED)
set_target_properties(absl::fixed_array PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flat_hash_map)
add_library(absl::flat_hash_map INTERFACE IMPORTED)
set_target_properties(absl::flat_hash_map PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::flat_hash_set)
add_library(absl::flat_hash_set INTERFACE IMPORTED)
set_target_properties(absl::flat_hash_set PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::inlined_vector)
add_library(absl::inlined_vector INTERFACE IMPORTED)
set_target_properties(absl::inlined_vector PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::optional)
add_library(absl::optional INTERFACE IMPORTED)
set_target_properties(absl::optional PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()

if(NOT TARGET absl::span)
add_library(absl::span INTERFACE IMPORTED)
set_target_properties(absl::span PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${AbslInclude}"
)
endif()
"@

# -----------------------------------------------------------
# Step 3: Build RE2 from source, linking against pre-built abseil
# -----------------------------------------------------------
$Re2Version = "2024-07-02"
$Re2Url = "https://github.com/google/re2/archive/refs/tags/$Re2Version.zip"
$Re2DownloadDir = "$env:TEMP\re2_download"
$Re2SourceDir = "$Re2DownloadDir\re2-$Re2Version"
$Re2BuildDir = "$Re2SourceDir\build"
$Re2InstallDir = "$ONNX_BUILD/_deps/onnx-build/Release"

Write-Host "Downloading RE2 $Re2Version..."
New-Item -ItemType Directory -Force -Path $Re2DownloadDir | Out-Null
$Re2ZipFile = Join-Path $Re2DownloadDir "re2-$Re2Version.zip"
Invoke-WebRequest -Uri $Re2Url -OutFile $Re2ZipFile -UseBasicParsing
Expand-Archive -Path $Re2ZipFile -DestinationPath $Re2DownloadDir -Force

# Use forward-slash paths for cmake to avoid escape sequence issues with backslashes
$Re2InstallDirFS = $Re2InstallDir -replace '\\', '/'
$AbslModulePath  = $AbslInstallDir -replace '\\', '/'

Write-Host "Configuring RE2 with CMake..."
New-Item -ItemType Directory -Force -Path $Re2BuildDir | Out-Null
cmake -S "$Re2SourceDir" -B "$Re2BuildDir" `
      -G $CMAKE_GENERATOR `
      -A x64 `
      -DCMAKE_BUILD_TYPE=Release `
      -DCMAKE_INSTALL_PREFIX="$Re2InstallDirFS" `
      -DBUILD_SHARED_LIBS=OFF `
      -DRE2_BUILD_TESTING=OFF `
      -DRE2_USE_EXTERNAL_ABSL=ON `
      -DCMAKE_MODULE_PATH="$AbslModulePath" `
      -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
      -DCMAKE_CXX_STANDARD=17 `
      -DCMAKE_CXX_STANDARD_REQUIRED=ON `
      -DCMAKE_C_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS /I$AbslInstallDirFS/include" `
      -DCMAKE_CXX_FLAGS="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS /I$AbslInstallDirFS/include" `
      -DCMAKE_C_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS /I$AbslInstallDirFS/include" `
      -DCMAKE_CXX_FLAGS_RELEASE="/MT /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS /I$AbslInstallDirFS/include" `
      -DCMAKE_EXE_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib" `
      -DCMAKE_STATIC_LINKER_FLAGS="/DEFAULTLIB:legacy_stdio_definitions.lib /DEFAULTLIB:OLDNAMES.lib"
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host "Building RE2..."
cmake --build "$Re2BuildDir" --config Release --verbose
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host "Installing RE2 to $Re2InstallDirFS..."
cmake --build "$Re2BuildDir" --config Release --target INSTALL
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host "Done installing re2.lib!"
Write-Host "Static library: $Re2InstallDir\lib"
Write-Host "Headers: $Re2InstallDir\include"

# Copy re2.lib to where ort-sys expects it
$Re2SysDir = "$ONNX_BUILD/_deps/re2-build"
New-Item -ItemType Directory -Path $Re2SysDir -Force
Copy-Item -Path "$Re2InstallDir/lib/re2.lib" -Destination "$Re2SysDir/re2.lib" -Force
New-Item -ItemType Directory -Path "$Re2SysDir/Release" -Force
Copy-Item -Path "$Re2InstallDir/lib/re2.lib" -Destination "$Re2SysDir/Release/re2.lib" -Force
Write-Host "Copied re2.lib to $Re2SysDir/re2.lib and $Re2SysDir/Release/re2.lib"

# Clean up download artifacts
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Re2DownloadDir
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $AbslDownloadDir

Write-Host "=== Abseil (absl) and RE2 static libraries built successfully ==="

# ==========================================================
# BUILD ONNX RUNTIME (Single Block, No Duplicates)
# ==========================================================
Write-Host "=== Building ONNX Runtime ==="

# -----------------------------
# Set ONNX flags depending on variant
# -----------------------------
# NOTE: $ORT_EXTRA_CMAKE_ARGS must be initialised for every variant. Under
# `Set-StrictMode -Version Latest` reading an unassigned variable is a
# terminating error, so leaving it unset for vulkan/cuda crashed the script
# at the `if ($ORT_EXTRA_CMAKE_ARGS)` check below.
$ORT_EXTRA_CMAKE_ARGS = @()

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
    "-G", $CMAKE_GENERATOR,
    "-A", "x64",
    "-DCMAKE_CXX_STANDARD=17",
    "-DCMAKE_CXX_STANDARD_REQUIRED=ON",
    "-DCMAKE_BUILD_TYPE=Release",
    "-Dabsl_DIR=$AbslInstallDirFS",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DCMAKE_COMPILE_WARNING_AS_ERROR=OFF",
    "-DCMAKE_POSITION_INDEPENDENT_CODE=OFF",
    "-Donnxruntime_BUILD_SHARED_LIB=OFF",
    "-Donnxruntime_ENABLE_STATIC_ANALYSIS=OFF",
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded",
    "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW",
    "-DCMAKE_C_FLAGS=/MT /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_CXX_FLAGS=/MT /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_C_FLAGS_RELEASE=/MT /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_CXX_FLAGS_RELEASE=/MT /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_C_FLAGS_RELWITHDEBINFO=/MT /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_CXX_FLAGS_RELWITHDEBINFO=/MT /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_C_FLAGS_DEBUG=/MTd /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
    "-DCMAKE_CXX_FLAGS_DEBUG=/MTd /wd4875 /D_CRT_NONSTDC_NO_DEPRECATE /D_CRT_SECURE_NO_WARNINGS",
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
    # Point CMake straight at nvcc. Under the Visual Studio generator CMake
    # otherwise looks for CUDA MSBuild integration inside the VS install, and
    # the toolkit installs none for VS 2026 - so detection returns NOTFOUND
    # even though nvcc is on PATH and runs fine.
    $nvcc = Join-Path $cuda_root "bin\nvcc.exe"
    if (-not (Test-Path $nvcc)) { Write-Error "nvcc.exe not found at $nvcc"; exit 1 }
    # CUDA 13 dropped Maxwell/Pascal/Volta, but ORT still defaults to a list
    # starting at compute_60, so nvcc aborts with
    # "nvcc fatal : Unsupported gpu architecture 'compute_60'".
    # Turing (75) is the oldest CUDA 13 supports.
    $ONNX_CMAKE_ARGS += @(
        "-DCUDAToolkit_ROOT=$cuda_root",
        # Turing / Ampere-consumer / Ada. Each extra arch is a full extra
        # device-code pass over every .cu with -rdc=true; five arches ran ~4h.
        # Add 80 (A100) or 90 (Hopper) back if datacenter GPUs matter.
        "-DCMAKE_CUDA_ARCHITECTURES=75;86;89",
        # CUDA 13 bundles CCCL, which hard-errors when built with MSVC's
        # traditional preprocessor. /Zc:preprocessor would change how the rest
        # of ORT's MSVC code preprocesses, so use CCCL's own opt-out instead.
        # /WX- because ORT passes nvcc -Werror all-warnings, and MSVC 19.51
        # emits a new warning inside nvcc's own generated cudafe stub:
        # "error C2220: the following warning is treated as an error".
        # /sdl (which ORT passes) promotes C4996 deprecation warnings to errors,
        # and CUDA 13 deprecated longlong4/ulonglong4 in favour of the *_16a /
        # *_32a forms that ORT 1.24.1 predates - 72 such errors. /WX- alone does
        # not undo /sdl, so disable it and silence the two warnings involved.
        "-DCMAKE_CUDA_FLAGS=-DCCCL_IGNORE_MSVC_TRADITIONAL_PREPROCESSOR_WARNING -Xcompiler=/WX- -Xcompiler=/sdl- -Xcompiler=/wd4996 -Xcompiler=/wd4211",
        "-DCMAKE_CUDA_COMPILER=$($nvcc -replace '\\','/')",
        "-Donnxruntime_CUDNN_HOME=$env:CUDNN_HOME",
        "-DCUDNN_HOME=$env:CUDNN_HOME",
        "-DCMAKE_CUDA_RUNTIME_LIBRARY=Static"
    )
}

# Run CMake with the assembled arguments
cmake @ONNX_CMAKE_ARGS
if ($LASTEXITCODE -ne 0) { Write-Error "ONNX Runtime CMake configure failed"; exit 1 }

# -----------------------------
# Build ONNX Runtime
# -----------------------------
cmake --build $ONNX_BUILD --config Release
# Without this check a partial ORT build sails on and only surfaces ~40
# minutes later as an unrelated-looking LNK1181 on a missing .lib.
if ($LASTEXITCODE -ne 0) { Write-Error "ONNX Runtime build failed"; exit 1 }

# Combine all onnxruntime .lib files into a single onnxruntime.lib
# so ort-sys can find it (avoids complex directory matching in its build script)
Write-Host "Combining onnxruntime .lib files into single onnxruntime.lib..."
$onnxLibFiles = Get-ChildItem -Path "$ONNX_BUILD\Release" -Filter "onnxruntime_*.lib" | Select-Object -ExpandProperty FullName
& "lib.exe" /out:"$ONNX_BUILD\Release\onnxruntime.lib" -nologo $onnxLibFiles
# Copy to base directory where ort-sys's static_link() checks for it
Copy-Item -Path "$ONNX_BUILD\Release\onnxruntime.lib" -Destination "$ONNX_BUILD\onnxruntime.lib" -Force

}  # end: build ORT from source

# ==========================================================
# EXPORT ENVIRONMENT
# ==========================================================


# ==========================================================
# EXPORT ENVIRONMENT
# ==========================================================
if ($ORT_PREBUILT) {
    # The prebuilt package is a shared build: onnxruntime.dll and its provider
    # DLLs ship next to the exe rather than being linked in.
    $env:ONNXRUNTIME_INCLUDE_DIR = Join-Path $ORT_PREBUILT "include"
    $env:ORT_STRATEGY            = "system"
    $env:ORT_LIB_LOCATION        = Join-Path $ORT_PREBUILT "lib"
    $env:ORT_PREFER_DYNAMIC_LINK = "1"
    $env:ONNXRUNTIME_LIB_DIR     = Join-Path $ORT_PREBUILT "lib"
} else {
    $env:ONNXRUNTIME_INCLUDE_DIR = Join-Path $ONNX_SRC "include"
    $env:ORT_STRATEGY            = "system"
    $env:ORT_LIB_LOCATION        = $ONNX_BUILD
    $env:ORT_PREFER_DYNAMIC_LINK = "0"
    $env:ONNXRUNTIME_LIB_DIR     = Join-Path $ONNX_BUILD "Release"
}
# -----------------------------------------------------------
$env:GGML_BLAS               = $ONNX_USE_BLAS
$env:BLAS_STATIC             = $ONNX_USE_BLAS
$env:GGML_BLAS_STATIC        = $ONNX_USE_BLAS
$env:GGML_VULKAN             = $ONNX_VULKAN_FLAG
$env:BLAS_VENDOR             = "OpenBLAS"
$env:BLA_VENDOR              = "OpenBLAS"
$env:GGML_BLAS_VENDOR        = "OpenBLAS"
$env:BLAS_INCLUDE_DIRS       = $INCLUDE_DIR
$env:BLAS_LIBRARIES          = $OPENBLAS_LIB
$env:OPENBLAS_PATH           = $PREBUILT_OPENBLAS_DIR
$env:OPENBLAS_DIR            = $PREBUILT_OPENBLAS_DIR
$env:CMAKE_PREFIX_PATH       = "${PREBUILT_OPENBLAS_DIR};${ONNX_BUILD}"
$env:CMAKE_ARGS              = "-DGGML_BLAS=$ONNX_USE_BLAS -DGGML_BLAS_STATIC=$ONNX_USE_BLAS -DGGML_VULKAN=$ONNX_VULKAN_FLAG -DGGML_BLAS_VENDOR=OpenBLAS -DBLAS_VENDOR=OpenBLAS -DOPENBLAS_PATH=$PREBUILT_OPENBLAS_DIR -DBLAS_INCLUDE_DIRS=$INCLUDE_DIR -DBLAS_LIBRARIES=$OPENBLAS_LIB -DBLA_VENDOR=OpenBLAS -DBLAS_ROOT=$PREBUILT_OPENBLAS_DIR -DBLAS_DIR=$PREBUILT_OPENBLAS_DIR -DBLAS_LIBDIR=$LIB_DIR -DBLA_STATIC=ON"
$env:WHISPER_RS_STATIC_CRT   = "1"
$env:ORT_SYS_STATIC_CRT      = "1"
$env:ESPEAK_RS_STATIC_CRT    = "1"
# Forces /MT inside espeak-rs-sys / whisper-rs-sys, which build their own
# C deps and otherwise default to /MD (see cmake/static-msvc.toolchain.cmake).
$env:CMAKE_TOOLCHAIN_FILE    = Join-Path $PROJECT_ROOT "cmake\static-msvc.toolchain.cmake"
# Pins ggml to x86-64-v3 (/arch:AVX2) instead of the runner's own CPU, which
# whisper-rs-sys would otherwise detect via GGML_NATIVE (see cmake/ggml-portable.cmake).
$env:CMAKE_PROJECT_INCLUDE   = Join-Path $PROJECT_ROOT "cmake\ggml-portable.cmake"
$env:CFLAGS                  = "/MT /D_CRT_SECURE_NO_WARNINGS /D_CRT_NONSTDC_NO_DEPRECATE"
$env:ESPEAK_NG_DIR           = $ESPEAK_INSTALL


# $ONNX_BUILD only exists on the from-source path; the prebuilt CUDA variant
# never creates it.
$ortLibDir = if ($ORT_PREBUILT) { Join-Path $ORT_PREBUILT "lib" } else { $ONNX_BUILD }
Write-Host "`n=== FINAL .lib files in $ortLibDir ==="
Get-ChildItem -Path $ortLibDir -Filter *.lib -Recurse -File -ErrorAction SilentlyContinue |
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
# whisper-rs-sys' build script recursively emits a cargo:rustc-link-search
# for every subdirectory under its nested whisper.cpp/ggml CMake build tree.
# The Visual Studio generator creates a deeply nested per-target/per-config
# tree there (and, with Vulkan, an extra nested vulkan-shaders-gen
# ExternalProject on top of it) - enough subdirectories that the combined
# length of all those -L flags can exceed Windows' ~32k PATH limit, which
# rustc hits when it internally builds a DLL search path to load a
# proc-macro (see https://github.com/rust-lang/rust/issues/110889 - an
# open, unfixed rustc bug, not something this build controls). Ninja
# produces a flat build directory instead, cutting that subdirectory count
# drastically. Only whisper-rs-sys/ort-sys's own internal cmake-rs builds
# read CMAKE_GENERATOR from the environment, so switching it here - after
# every cmake invocation this script drives directly (espeak-ng, abseil,
# onnxruntime) has already run with the Visual Studio generator above -
# only affects that nested build, not the rest of the script.
if (-not (Get-Command ninja -ErrorAction SilentlyContinue)) {
    choco install ninja -y
    if (-not (Get-Command ninja -ErrorAction SilentlyContinue)) {
        Write-Error "ninja not found and could not be installed."
        exit 1
    }
}
$env:CMAKE_GENERATOR = "Ninja"

$TARGET = "x86_64-pc-windows-msvc"

$CARGO_FEATURES = @()
if ($WITH_OPENBLAS) { $CARGO_FEATURES += "whisper-openblas" }
if ($WITH_VULKAN)   { $CARGO_FEATURES += "whisper-vulkan" }
if ($WITH_CUDA)     { $CARGO_FEATURES += "whisper-cuda" }
if ($WITH_CUDA)     { $CARGO_FEATURES += "ort-cuda" }


# Move vcpkg re2.lib dep to target folder so ort-sys can find it
# NOTE: (for some reason onnx runtime doesnt build re2.lib)
# Copy-Item -Path "C:\vcpkg\installed\x64-windows-static\lib\re2.lib" -Destination "$ONNX_BUILD\_deps\onnx-build\Release\re2.lib" -Force
# Remove-Item -Path "C:\vcpkg\installed\*" -Recurse -Force
# Remove-Item -Path "C:\vcpkg\buildtrees\*" -Recurse -Force
# Remove-Item -Path "C:\vcpkg\packages\*" -Recurse -Force

# Before cargo build
if ($ORT_PREBUILT) {
    # The prebuilt ORT is one import library backed by DLLs, so none of the
    # per-component static libs the from-source path enumerates below exist.
    # crt-static still applies: only ORT is dynamic here, as with cuDNN/cuBLAS.
    $env:RUSTFLAGS = "-C target-feature=+crt-static `
                  -C codegen-units=1 `
                  -C opt-level=3 `
                  -L native=$ORT_PREBUILT/lib `
                  -C link-arg=$ORT_PREBUILT/lib/onnxruntime.lib `
                  -C link-arg=/DEFAULTLIB:legacy_stdio_definitions.lib `
                  -C link-arg=/DEFAULTLIB:OLDNAMES.lib `
                  -C link-arg=/NODEFAULTLIB:msvcrt.lib `
                  -C link-arg=/NODEFAULTLIB:msvcrtd.lib `
                  -C link-arg=/NODEFAULTLIB:ucrt.lib `
                  -C link-arg=/NODEFAULTLIB:ucrtd.lib `
                  -C link-arg=/NODEFAULTLIB:vcruntime.lib `
                  -C link-arg=/NODEFAULTLIB:vcruntimed.lib `
                  -C link-arg=/DEFAULTLIB:libcmt.lib `
                  -C link-arg=/DEFAULTLIB:libucrt.lib `
                  -C link-arg=/DEFAULTLIB:libvcruntime.lib "
} else {
$env:RUSTFLAGS = "-C target-feature=+crt-static `
                  -C codegen-units=1 `
                  -C opt-level=3 `
                  -C link-arg=$ONNX_BUILD/_deps/re2-build/Release/re2.lib `
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
                  -C link-arg=/DEFAULTLIB:OLDNAMES.lib `
                  -C link-arg=/NODEFAULTLIB:msvcrt.lib `
                  -C link-arg=/NODEFAULTLIB:msvcrtd.lib `
                  -C link-arg=/NODEFAULTLIB:ucrt.lib `
                  -C link-arg=/NODEFAULTLIB:ucrtd.lib `
                  -C link-arg=/NODEFAULTLIB:vcruntime.lib `
                  -C link-arg=/NODEFAULTLIB:vcruntimed.lib `
                  -C link-arg=/DEFAULTLIB:libcmt.lib `
                  -C link-arg=/DEFAULTLIB:libucrt.lib `
                  -C link-arg=/DEFAULTLIB:libvcruntime.lib "
}

# ----------------------------------------------------------
# CUDA variant: link the CUDA EP plus the CUDA/cuDNN runtimes.
# The explicit -C link-arg list above enumerates the ORT libs by name and
# predates the CUDA provider, so onnxruntime_providers_cuda.lib has to be
# added here. cudnn/cublas stay dynamic - they are driver-side and exempt
# from the fully-static rule.
# ----------------------------------------------------------
if ($WITH_CUDA) {
    $CUDA_LIB_DIR = Join-Path $env:CUDA_PATH "lib\x64"

    # RUSTFLAGS is split on spaces, and the toolkit lives under
    # "C:\Program Files\NVIDIA GPU Computing Toolkit\...", so passing it
    # directly makes rustc see "Files\NVIDIA" as a second input filename
    # ("error: multiple input filenames provided"). Expose it under a
    # space-free junction and pass that instead.
    $CUDA_LINK_DIR = "C:\cuda-lib"
    if (-not (Test-Path $CUDA_LINK_DIR)) {
        New-Item -ItemType Junction -Path $CUDA_LINK_DIR -Target $CUDA_LIB_DIR | Out-Null
    }
    if (-not (Test-Path (Join-Path $CUDA_LINK_DIR "cudart_static.lib"))) {
        Write-Error "CUDA lib junction $CUDA_LINK_DIR does not expose $CUDA_LIB_DIR"
        exit 1
    }
    if ($CUDNN_LIB_DIR -match ' ') {
        Write-Error "CUDNN_LIB_DIR contains a space and cannot go in RUSTFLAGS: $CUDNN_LIB_DIR"
        exit 1
    }

    $env:RUSTFLAGS += " -L native=$CUDA_LINK_DIR" +
                      " -L native=$CUDNN_LIB_DIR"

    if ($ORT_PREBUILT) {
        # The CUDA EP lives in onnxruntime_providers_cuda.dll, loaded at run
        # time by onnxruntime.dll; there is no static lib to link, and CUDA's
        # own runtime is already inside those DLLs.
        Write-Host "Prebuilt ORT: CUDA EP is provided by onnxruntime_providers_cuda.dll"
    } else {
        $env:RUSTFLAGS += " -C link-arg=$ONNX_BUILD/Release/onnxruntime_providers_cuda.lib" +
                          " -C link-arg=cudart_static.lib" +
                          " -C link-arg=cublas.lib" +
                          " -C link-arg=cublasLt.lib" +
                          " -C link-arg=cudnn.lib"
    }

    Write-Host "CUDA link dirs: $CUDA_LINK_DIR (-> $CUDA_LIB_DIR) ; $CUDNN_LIB_DIR"
}

$env:CXXFLAGS="/std:c++17 /MT /D_CRT_SECURE_NO_WARNINGS /D_CRT_NONSTDC_NO_DEPRECATE"

if ($WITH_CUDA) {
    # ggml's CUDA backend is compiled by whisper-rs-sys through its own cmake
    # run, which passes -DCMAKE_CUDA_FLAGS=-Xcompiler=-fPIC and so overrides
    # anything we would set there. cl.exe reads the CL environment variable on
    # every invocation, including the ones nvcc spawns, so inject there.
    #
    # /Zc:preprocessor, not CCCL_IGNORE_MSVC_TRADITIONAL_PREPROCESSOR_WARNING:
    # CUDA 13 bundles CCCL, which really does require the conforming
    # preprocessor. The IGNORE define only silences the guard, after which
    # CCCL's own macro concatenation fails to expand:
    #   cub/util_macro.cuh(24): error: expected a "{"
    #      namespace cub { inline namespace _V_300303_CCCL_PP_CAT(_SM_, 750) {
    # This is safe now that ONNX Runtime is prebuilt - ggml is the only thing
    # still compiled by cl.exe here.
    $env:CL = "/Zc:preprocessor"

    # ggml compiles the CUDA host code with CMake's default MSVC Release
    # flags, which include /MD, while the rest of the binary is /MT via
    # +crt-static. Mixing CRTs leaves the dynamic-CRT imports unresolved:
    #   *.cu.obj : error LNK2001: unresolved external symbol __imp_modff
    #   vtmate.exe : fatal error LNK1120: 19 unresolved externals
    # CL options are prepended to the command line, so a /MT there would lose
    # to ggml's explicit /MD. _CL_ is appended instead, and for MSVC the last
    # runtime-library flag wins.
    $env:_CL_ = "/MT"

    # ggml drives cl.exe through ccache, which does not support MSVC's CL
    # environment variable - the options are invisible to it and the object
    # files silently never appear, so linking dies with
    #   LNK1181: cannot open input file '...\ggml-base.dir\ggml.c.obj'
    # Turn ccache into a pass-through for this variant. It costs nothing here:
    # CI runners start with an empty cache, so nothing was being reused.
    $env:CCACHE_DISABLE = "1"
    Write-Host "CL = $env:CL ; _CL_ = $env:_CL_ ; CCACHE_DISABLE = $env:CCACHE_DISABLE"
}

Set-Location $PROJECT_ROOT

Write-Host "Ensuring Rust target $TARGET is installed..."
rustup target add $TARGET

Write-Host "Building Rust binary..."
# NOTE: -vv dumps every rustc command line and blows past GitHub's per-step log
# size limit, which truncates the log *before* the actual error. Keep it off.
cargo build --release --target $TARGET --features ($CARGO_FEATURES -join ",")
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed"; exit 1 }

$SRC_BIN = Join-Path $env:CARGO_TARGET_DIR "$TARGET\release\$BIN_BASE.exe"
# Fallback: try plain release folder if cross-target folder does not exist
if (-not (Test-Path $SRC_BIN)) {
    $SRC_BIN = Join-Path $env:CARGO_TARGET_DIR "release\$BIN_BASE.exe"
}

# Artifact naming: <bin>-<version>-windows-x86_64-<variant>/vtmate.exe, same
# convention as the Linux/macOS artifacts. The release workflow zips the
# directory contents (exe plus any DLLs bundled below), so the archive
# extracts to vtmate.exe directly. $VERSION comes from Cargo.toml.
$VERSION = (Select-String -Path (Join-Path $PROJECT_ROOT "Cargo.toml") -Pattern '^\s*version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $VERSION) { Write-Error "Failed to read version from Cargo.toml"; exit 1 }
$DST_BIN = Join-Path $TARGET_DIR "$BIN_BASE-$VERSION-windows-x86_64-$VARIANT\$BIN_BASE.exe"

if (-not (Test-Path $SRC_BIN)) {
    Write-Error "ERROR: Built binary not found."
    exit 1
}

# $TARGET_DIR is wiped by the clean step and only recreated one level deep,
# so the per-variant subdirectory has to be made here. Copy-Item does not
# create missing intermediate directories.
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $DST_BIN) | Out-Null

Copy-Item -Force $SRC_BIN $DST_BIN
Write-Host "Built $DST_BIN"

# cuDNN and cuBLAS are not part of the NVIDIA driver, so they must ship
# alongside the exe - the driver alone will not satisfy them at runtime.
if ($WITH_CUDA) {
    $binDir = Split-Path -Parent $DST_BIN
    foreach ($pattern in "cudnn*64*.dll", "cublas*64*.dll") {
        Get-ChildItem -Path (Join-Path $env:CUDNN_HOME "bin") -Filter $pattern -ErrorAction SilentlyContinue |
            ForEach-Object { Copy-Item -Force $_.FullName $binDir }
        Get-ChildItem -Path (Join-Path $env:CUDA_PATH "bin")  -Filter $pattern -ErrorAction SilentlyContinue |
            ForEach-Object { Copy-Item -Force $_.FullName $binDir }
    }
    Write-Host "Bundled CUDA/cuDNN runtime DLLs into $binDir"
}

# The prebuilt ORT is a shared build, so its DLLs must ship with the exe.
if ($ORT_PREBUILT) {
    $binDir = Split-Path -Parent $DST_BIN
    Get-ChildItem -Path (Join-Path $ORT_PREBUILT "lib") -Filter "*.dll" |
        ForEach-Object { Copy-Item -Force $_.FullName $binDir }
    if (-not (Test-Path (Join-Path $binDir "onnxruntime.dll"))) {
        Write-Error "onnxruntime.dll was not bundled into $binDir"
        exit 1
    }
    Write-Host "Bundled prebuilt ONNX Runtime DLLs into $binDir"
}

# ==========================================================
# VERIFY FULLY STATIC LINK
# The goal is a binary with no non-OS DLL imports. Anything from the
# MSVC CRT, the MSVC OpenMP runtime, or one of our own vendored libs
# means something got linked dynamically.
# ==========================================================
Write-Host "`n=== Import table for $DST_BIN ==="
$dumpbin = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
if (-not $dumpbin) {
    Write-Warning "dumpbin.exe not found; skipping static-link verification."
} else {
    $deps = & dumpbin.exe /NOLOGO /DEPENDENTS $DST_BIN |
            Select-String -Pattern '^\s{4}(\S+\.dll)$' |
            ForEach-Object { $_.Matches[0].Groups[1].Value }

    $deps | ForEach-Object { Write-Host "  $_" }

    # Only the CUDA/Vulkan loaders and plain Win32 system DLLs may be
    # dynamic. Everything below indicates a CRT or vendored library that
    # failed to link statically.
    #
    # ucrtbase / api-ms-win-crt-* are listed deliberately: they ship with
    # Windows, but their presence means the UCRT was linked dynamically,
    # which is exactly what +crt-static and /MT are supposed to prevent.
    # (api-ms-win-core-* and other non-crt contracts stay allowed.)
    $forbidden = @(
        'vcruntime\d*\.dll', 'msvcp\d*\.dll', 'msvcr\d*\.dll',
        'ucrtbase(d)?\.dll',  'api-ms-win-crt-.*\.dll',
        'vcomp\d*\.dll',
        'libopenblas\.dll',   'openblas\.dll',
        'onnxruntime.*\.dll', 'espeak-ng\.dll', 'whisper\.dll', 'ggml.*\.dll'
    )

    # The CUDA variant deliberately links the prebuilt shared ORT (building it
    # from source does not fit GitHub's 6h job limit), so its DLLs are expected
    # here and are bundled next to the exe. Everything else stays forbidden.
    if ($ORT_PREBUILT) {
        $forbidden = $forbidden | Where-Object { $_ -ne 'onnxruntime.*\.dll' }
    }

    $bad = $deps | Where-Object {
        $d = $_
        $forbidden | Where-Object { $d -match "^$_$" }
    }

    if ($bad) {
        Write-Host ""
        Write-Error ("NOT STATIC: binary imports " + ($bad -join ", "))
        exit 1
    }
    Write-Host "OK: no dynamic CRT / OpenMP / vendored-library imports."
}

if ($UPLOAD_ENABLED) {
    Write-Host "Uploading artifact for $VARIANT..."
    gh run upload-artifact "$BIN_BASE-$VERSION-windows-x86_64-$VARIANT" (Split-Path -Parent $DST_BIN)
}

Write-Host "`nSUCCESS: $DST_BIN"
exit 0