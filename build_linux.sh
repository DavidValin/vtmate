#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="ai-mate"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
PKG_DIR="${DIST_DIR}/packages"
ASSETS_DIR="${PROJECT_ROOT}/assets"
ESPEAK_ARCHIVE="${ASSETS_DIR}/espeak-ng-data.tar.gz"

DO_PACKAGE=1
DOCKER_NO_CACHE=1
SEL_ARCH="all"   # amd64,arm64,all

# Linux variant toggles
WITH_CUDA="${WITH_CUDA:-0}"          # amd64 only
WITH_ROCM="${WITH_ROCM:-0}"          # amd64 only
WITH_VULKAN="${WITH_VULKAN:-0}"

# Host cache mounts (Linux Docker)
HOST_HOME="${HOME}"
HOST_K_CACHE="${HOST_HOME}/.cache/k"
HOST_WHISPER_MODELS="${HOST_HOME}/.whisper-models"
CONT_K_CACHE="/root/.cache/k"
CONT_WHISPER_MODELS="/root/.whisper-models"

# -----------------------------
# Helper functions (usage, normalize list, etc.)
# -----------------------------
usage() {
  cat <<'USAGE'
Usage:
  ./build_linux.sh [--arch <list>] [--skip-package] [--cache|--no-cache]

--arch comma-separated: amd64,arm64,all

Env:
  WITH_CUDA=0|1           (amd64) default 0
  WITH_ROCM=0|1           (amd64) default 0
  WITH_VULKAN=0|1         default 0
USAGE
}

lower() { echo "$1" | tr '[:upper:]' '[:lower:]'; }
normalize_list() {
  local s="${1-}"
  s="$(lower "$s")"
  s="${s//[[:space:]]/}"
  while [[ "$s" == *",,"* ]]; do s="${s//,,/,}"; done
  s="${s#,}"; s="${s%,}"
  echo "$s"
}
list_has() {
  local list="${1-}" tok="${2-}"
  [[ -n "$list" && -n "$tok" && ",${list}," == *",${tok},"* ]]
}
want_arch() { [[ "${SEL_ARCH}" == "all" ]] && return 0; list_has "${SEL_ARCH}" "$1"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) SEL_ARCH="$(normalize_list "${2-}")"; shift 2 ;;
    --skip-package) DO_PACKAGE=0; shift ;;
    --cache) DOCKER_NO_CACHE=0; shift ;;
    --no-cache) DOCKER_NO_CACHE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) 
       echo "⚠ Ignoring unknown arg: $1"
       shift
       ;;
  esac
done

VERSION="$(
  awk -F\" '
    $1 ~ /^[[:space:]]*version[[:space:]]*=[[:space:]]*/ { print $2; exit }
  ' "${PROJECT_ROOT}/Cargo.toml"
)"
[[ -n "${VERSION}" ]] || { echo "Failed to read version from Cargo.toml"; exit 1; }

mkdir -p "${DIST_DIR}" "${PKG_DIR}" "${PROJECT_ROOT}/target-cross" "${ASSETS_DIR}"
mkdir -p "${HOST_K_CACHE}" "${HOST_WHISPER_MODELS}"

echo "Version: ${VERSION}"
echo "Linux: arch=${SEL_ARCH}"
echo "Linux amd64: WITH_CUDA=${WITH_CUDA} WITH_ROCM=${WITH_ROCM}"
echo "Cache mounts:"
echo "  ${HOST_K_CACHE} -> ${CONT_K_CACHE}"
echo "  ${HOST_WHISPER_MODELS} -> ${CONT_WHISPER_MODELS}"

# Features
FEATURES_COMMON="whisper-openblas"
FEATURES_CPU="${FEATURES_COMMON}"
FEATURES_VULKAN="${FEATURES_COMMON},whisper-vulkan"
FEATURES_CUDA="${FEATURES_COMMON},whisper-cuda"
FEATURES_ROCM="${FEATURES_COMMON},whisper-hipblas"

# -----------------------------
# Packaging helpers
# -----------------------------
sha256_file() {
  local file="$1" out="$2"
  if command -v shasum >/dev/null 2>&1; then
    (cd "$(dirname "$file")" && shasum -a 256 "$(basename "$file")") > "$out"
    return 0
  fi
  if command -v openssl >/dev/null 2>&1; then
    local line hash
    line="$(openssl dgst -sha256 "$file")"
    hash="${line##* }"
    echo "${hash}  $(basename "$file")" > "$out"
    return 0
  fi
  echo "ERROR: No SHA256 tool found."
  exit 1
}
make_tgz() { local src="$1" tgz="$2"; tar -C "$(dirname "$src")" -czf "$tgz" "$(basename "$src")"; }
package_one() {
  local src="$1"
  [[ -f "$src" ]] || return 0
  local base tgz sha
  base="$(basename "$src")"
  tgz="${PKG_DIR}/${base}.tar.gz"
  sha="${PKG_DIR}/${base}.tar.gz.sha256"
  make_tgz "$src" "$tgz"
  sha256_file "$tgz" "$sha"
}

# -----------------------------
# Docker helpers
# -----------------------------
docker_ok=0
command -v docker >/dev/null 2>&1 && docker_ok=1
can_run_amd64() { docker run --rm --platform=linux/amd64 alpine:3.19 uname -m >/dev/null 2>&1; }
FORCE_AMD64_DOCKER=0
if [[ "$docker_ok" -eq 1 ]] && can_run_amd64; then FORCE_AMD64_DOCKER=1; fi

# Ensure embedded eSpeak-ng data archive exists
ensure_espeak_data_archive() {
  if [[ -f "${ESPEAK_ARCHIVE}" ]]; then
    echo "✔ Found embedded asset: ${ESPEAK_ARCHIVE}"
    return 0
  fi

  echo "== Generating embedded asset: ${ESPEAK_ARCHIVE} =="

  if [[ "$docker_ok" -ne 1 ]]; then
    echo "ERROR: Docker not found and ${ESPEAK_ARCHIVE} is missing."
    exit 1
  fi
  local tmp img df
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.espeak.asset"
  img="local/${BIN_NAME}-espeak-asset:${VERSION}-$$"
  cat > "$df" <<'DOCKERFILE'
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates tar gzip espeak-ng-data \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /out
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)
  docker build "${build_args[@]}" --platform=linux/amd64 -f "$df" -t "$img" "$tmp"

  docker run --rm --platform=linux/amd64 \
    -v "${ASSETS_DIR}:/out" -w /out \
    "$img" \
    bash -lc '
      set -euo pipefail
      cp -a /usr/share/espeak-ng-data ./espeak-ng-data
      rm -rf ./espeak-ng-data/voices
      tar -czf espeak-ng-data.tar.gz espeak-ng-data
      rm -rf ./espeak-ng-data
    '

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
  [[ -f "${ESPEAK_ARCHIVE}" ]] || { echo "ERROR: failed to generate ${ESPEAK_ARCHIVE}"; exit 1; }
  echo "✔ Generated: ${ESPEAK_ARCHIVE}"
}

# -----------------------------
# eSpeak-ng static build
# -----------------------------
build_static_espeak_ng() {
  local arch="$1" docker_platform="$2" target_dir="/work/deps/espeak-ng-install"
  echo "== Building static eSpeak-ng for ${arch} =="

  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.espeak.static"
  CACHE_BUST="$(date +%s)"
  img="local/espeak-ng-static:${arch}-${VERSION}-$$"

  cat > "$df" <<DOCKERFILE
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST=${CACHE_BUST}

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential git cmake pkg-config libasound2-dev ca-certificates \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /work
RUN git clone --depth 1 https://github.com/espeak-ng/espeak-ng.git /work/espeak-ng

WORKDIR /work/espeak-ng
RUN mkdir build && cd build && \
    cmake -DCMAKE_BUILD_TYPE=Release \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
      -DCMAKE_SKIP_RPATH=ON \
      -DCMAKE_INSTALL_RPATH="" \
      -DCMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
      -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER \
      -DCMAKE_FIND_ROOT_PATH_MODE_LIBRARY=ONLY \
      -DCMAKE_FIND_ROOT_PATH_MODE_INCLUDE=ONLY \
      -DCMAKE_FIND_ROOT_PATH_MODE_PACKAGE=ONLY \
      -DCMAKE_INSTALL_PREFIX=/work/deps/espeak-ng-install .. && \
    make -j$(nproc) && \
    make install

RUN mkdir -p /out/espeak-ng-data && \
    cp -a ${target_dir}/share/espeak-ng-data/* /out/espeak-ng-data/
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  # Mount host deps folder so the dictionary archive is copied out
  docker build "${build_args[@]}" --platform="${docker_platform}" -f "$df" -t "$img" "$tmp"
  docker run --rm --platform="${docker_platform}" -v "${PROJECT_ROOT}/target-cross:/out" "$img"
  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp"
  echo "✔ eSpeak-ng static build complete for ${arch}"
}

# -----------------------------
# Linux copy helper
# -----------------------------
ARTIFACTS=()
add_artifact() { [[ -f "$1" ]] && ARTIFACTS+=("$1"); }

linux_copy_out() {
  local arch="$1" target="$2" variant="$3"
  local src_dir="${PROJECT_ROOT}/target-cross/linux-${arch}-${variant}/${target}/release"
  local out="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-${arch}-${variant}"
  cp "${src_dir}/${BIN_NAME}" "$out"
  chmod +x "$out" || true
  add_artifact "$out"
  echo "✔ Built: $out"
}

# -----------------------------
# AMD64 Docker Build
# -----------------------------
build_linux_amd64_docker_variants() {
  [[ "$docker_ok" -eq 1 ]] || { echo "Skipping linux/amd64: docker not found."; return 0; }
  [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]] || { echo "Skipping linux/amd64: cannot run linux/amd64 containers."; return 0; }

  build_static_espeak_ng amd64 linux/amd64


  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.amd64"
  img="local/${BIN_NAME}-linux-amd64:${VERSION}-$$"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<DOCKERFILE
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST=${CACHE_BUST}

# Install build tools + OpenBLAS
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git xz-utils \
    build-essential pkg-config \
    cmake ninja-build \
    clang libclang-dev llvm-dev \
    perl \
    libssl-dev \
    libasound2-dev \
    libxdo-dev \
    libx11-dev \
    libblas-dev \
    libopenblas-dev \
    gfortran \
    libvulkan-dev vulkan-tools vulkan-utility-libraries-dev \
    spirv-tools glslang-tools \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends glslc || true; \
    rm -rf /var/lib/apt/lists/*; \
    (command -v glslc >/dev/null 2>&1 && echo "glslc installed") || echo "glslc not available"

# CUDA (dynamic only)
ARG WITH_CUDA=1
RUN if [ "\$WITH_CUDA" = "1" ]; then \
      apt-get update && apt-get install -y --no-install-recommends nvidia-cuda-toolkit && \
      rm -rf /var/lib/apt/lists/* ; \
    fi

ARG WITH_ROCM=0
RUN if [ "\$WITH_ROCM" = "1" ]; then \
      apt-get update && apt-get install -y --no-install-recommends rocm-hip-sdk hipblas rocblas && \
      rm -rf /var/lib/apt/lists/* ; \
    fi

# Rust + musl target for static linking
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:\${PATH}"
RUN rustup update stable
RUN rustup target add x86_64-unknown-linux-gnu
WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux amd64 build (Docker image) =="
  docker build "${build_args[@]}" --platform=linux/amd64 \
    --build-arg WITH_CUDA="${WITH_CUDA}" \
    --build-arg WITH_ROCM="${WITH_ROCM}" \
    --build-arg CACHE_BUST="${CACHE_BUST}" \
    -f "$df" -t "$img" "$tmp"

  echo "== Linux amd64 cargo builds (cpu + optional variants) =="
  docker run --rm --platform=linux/amd64 \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -v "${HOST_K_CACHE}:${CONT_K_CACHE}" \
    -v "${HOST_WHISPER_MODELS}:${CONT_WHISPER_MODELS}" \
    -e WITH_VULKAN="${WITH_VULKAN}" \
    -e WITH_CUDA="${WITH_CUDA}" \
    -e WITH_ROCM="${WITH_ROCM}" \
    -e CMAKE_SKIP_RPATH=ON \
    -e CMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
    "$img" \
    bash -lc '
      set -euo pipefail

      ARCH=amd64
      target=x86_64-unknown-linux-gnu

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-${ARCH}-${variant}"

        echo "---- Building linux/${ARCH} [$variant] features: $feats"

        # Force static linking for system libraries except CUDA
        export OPENBLAS_STATIC=1
        export GGML_BLAS_VENDOR=OpenBLAS
        export BLAS_INCLUDE_DIRS=/usr/include
        export BLAS_LIBRARIES=/usr/lib/x86_64-linux-gnu/libopenblas.a
        export CMAKE_PREFIX_PATH=/usr/include:/usr/lib/x86_64-linux-gnu

        export RUSTFLAGS="-C target-feature=+crt-static -C codegen-units=1 -C opt-level=3 -C link-arg=-Wl,-static -C link-arg=-Wl,--gc-sections -C link-arg=-Wl,--icf=safe"
        export CARGO_PROFILE_RELEASE_LTO=false
        export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
        export CARGO_PROFILE_RELEASE_DEBUG=false
        export CARGO_PROFILE_RELEASE_STRIP=symbols
        export CARGO_PROFILE_RELEASE_INCREMENTAL=false

        export ESPEAK_NG_DIR="/work/deps/espeak-ng-install"

        CARGO_TARGET_DIR="$ctd" \
        cargo build --release --target "$target" --no-default-features --features "$feats"
      }

      build_variant cpu "'"${FEATURES_CPU}"'"

      if [ "${WITH_VULKAN}" = "1" ] && command -v glslc >/dev/null 2>&1; then
        build_variant vulkan "'"${FEATURES_VULKAN}"'"
      fi

      if [ "${WITH_CUDA}" = "1" ]; then
        build_variant cuda "'"${FEATURES_CUDA}"'"
      fi

      if [ "${WITH_ROCM}" = "1" ]; then
        build_variant rocm "'"${FEATURES_ROCM}"'"
      fi
    '

  linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "cpu"
  if [[ "${WITH_VULKAN}" == "1" ]] && [[ -f "${PROJECT_ROOT}/target-cross/linux-amd64-vulkan/x86_64-unknown-linux-gnu/release/${BIN_NAME}" ]]; then
    linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "vulkan"
  fi
  [[ "${WITH_CUDA}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "cuda"
  [[ "${WITH_ROCM}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "rocm"

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

# -----------------------------
# ARM64 Docker Build
# -----------------------------
build_linux_arm64_docker_variants() {
  [[ "$docker_ok" -eq 1 ]] || { echo "Skipping linux/arm64: docker not found."; return 0; }

  build_static_espeak_ng arm64 linux/arm64

 
  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.arm64"
  img="local/${BIN_NAME}-linux-arm64:${VERSION}-$$"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<DOCKERFILE
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST=${CACHE_BUST}

# Install build tools + OpenBLAS (static) + Vulkan (dynamic)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git xz-utils \
    build-essential pkg-config \
    cmake ninja-build \
    clang libclang-dev llvm-dev \
    perl \
    libssl-dev \
    libasound2-dev \
    libxdo-dev \
    libx11-dev \
    libblas-dev \
    libopenblas-dev \
    gfortran \
    libvulkan-dev vulkan-tools vulkan-utility-libraries-dev \
    spirv-tools glslang-tools \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends glslc || true; \
    rm -rf /var/lib/apt/lists/*; \
    (command -v glslc >/dev/null 2>&1 && echo "glslc installed") || echo "glslc not available"

# Rust + target
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:\${PATH}"
RUN rustup update stable
RUN rustup target add aarch64-unknown-linux-gnu
WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux arm64 build (Docker image) =="
  docker build "${build_args[@]}" --platform=linux/arm64 \
    --build-arg CACHE_BUST="${CACHE_BUST}" \
    -f "$df" -t "$img" "$tmp"

  echo "== Linux arm64 cargo builds (cpu + optional variants) =="
  docker run --rm --platform=linux/arm64 \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -v "${HOST_K_CACHE}:${CONT_K_CACHE}" \
    -v "${HOST_WHISPER_MODELS}:${CONT_WHISPER_MODELS}" \
    -e WITH_VULKAN="${WITH_VULKAN}" \
    -e CMAKE_SKIP_RPATH=ON \
    -e CMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
    "$img" \
    bash -lc '
      set -euo pipefail

      ARCH=arm64
      target=aarch64-unknown-linux-gnu

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-${ARCH}-${variant}"

        echo "---- Building linux/${ARCH} [$variant] features: $feats"

        # OpenBLAS static + system libraries static
        export OPENBLAS_STATIC=1
        export GGML_BLAS_VENDOR=OpenBLAS
        export BLAS_INCLUDE_DIRS=/usr/include
        export BLAS_LIBRARIES=/usr/lib/aarch64-linux-gnu/libopenblas.a
        export CMAKE_PREFIX_PATH=/usr/include:/usr/lib/aarch64-linux-gnu

        # Static linking flags for Rust
        export RUSTFLAGS="-C target-feature=+crt-static -C codegen-units=1 -C opt-level=3 -C link-arg=-Wl,--gc-sections"
        export CARGO_PROFILE_RELEASE_LTO=false
        export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
        export CARGO_PROFILE_RELEASE_DEBUG=false
        export CARGO_PROFILE_RELEASE_STRIP=symbols
        export CARGO_PROFILE_RELEASE_INCREMENTAL=false

        export ESPEAK_NG_DIR="/work/deps/espeak-ng-install"

        CARGO_TARGET_DIR="$ctd" \
        cargo build --release --target "$target" --no-default-features --features "$feats"
      }

      # Always build CPU variant statically
      build_variant cpu "'"${FEATURES_CPU}"'"

      # Vulkan dynamic variant (link system libvulkan.so)
      if [ "${WITH_VULKAN}" = "1" ] && command -v glslc >/dev/null 2>&1; then
        build_variant vulkan "'"${FEATURES_VULKAN}"'"
      fi
    '

  # Copy out artifacts
  linux_copy_out "arm64" "aarch64-unknown-linux-gnu" "cpu"
  if [[ "${WITH_VULKAN}" == "1" ]] && [[ -f "${PROJECT_ROOT}/target-cross/linux-arm64-vulkan/aarch64-unknown-linux-gnu/release/${BIN_NAME}" ]]; then
    linux_copy_out "arm64" "aarch64-unknown-linux-gnu" "vulkan"
  fi

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

# -----------------------------
# Run builds
# -----------------------------
ensure_espeak_data_archive

if want_arch amd64; then build_linux_amd64_docker_variants; fi
if want_arch arm64; then build_linux_arm64_docker_variants; fi

# -----------------------------
# Packaging
# -----------------------------
if [[ "${DO_PACKAGE}" -eq 1 ]]; then
  echo "== Packaging tar.gz + SHA256 =="
  for f in "${ARTIFACTS[@]}"; do
    package_one "$f"
  done
else
  echo "Skipping packaging (--skip-package)"
fi

echo "✔ Linux build complete"
