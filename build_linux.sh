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
WITH_CUDA="${WITH_CUDA:-1}"          # amd64 only
WITH_ROCM="${WITH_ROCM:-0}"          # amd64 only
LINUX_WITH_OPENBLAS="${LINUX_WITH_OPENBLAS:-1}"
LINUX_WITH_VULKAN="${LINUX_WITH_VULKAN:-1}"

# Host cache mounts (Linux Docker)
HOST_HOME="${HOME}"
HOST_K_CACHE="${HOST_HOME}/.cache/k"
HOST_WHISPER_MODELS="${HOST_HOME}/.whisper-models"
CONT_K_CACHE="/root/.cache/k"
CONT_WHISPER_MODELS="/root/.whisper-models"

usage() {
  cat <<'USAGE'
Usage:
  ./build_linux.sh [--arch <list>] [--skip-package] [--cache|--no-cache]

--arch comma-separated: amd64,arm64,all

Env:
  WITH_CUDA=0|1           (amd64) default 1
  WITH_ROCM=0|1           (amd64) default 0
  LINUX_WITH_OPENBLAS=0|1 default 1
  LINUX_WITH_VULKAN=0|1   default 1
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
    *) echo "Unknown arg: $1"; usage; exit 1 ;;
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
echo "Linux variants: OPENBLAS=${LINUX_WITH_OPENBLAS} VULKAN=${LINUX_WITH_VULKAN}"
echo "Cache mounts:"
echo "  ${HOST_K_CACHE} -> ${CONT_K_CACHE}"
echo "  ${HOST_WHISPER_MODELS} -> ${CONT_WHISPER_MODELS}"

# Features
FEATURES_COMMON="whisper-logs"
FEATURES_CPU="${FEATURES_COMMON}"
FEATURES_OPENBLAS="${FEATURES_COMMON},whisper-openblas"
FEATURES_VULKAN="${FEATURES_COMMON},whisper-vulkan"
FEATURES_CUDA="${FEATURES_COMMON},whisper-cuda"
FEATURES_ROCM="${FEATURES_COMMON},whisper-hipblas"

# Packaging helpers
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

# Docker helpers
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

  rm -f "${ESPEAK_ARCHIVE}"
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

build_linux_amd64_docker_variants() {
  [[ "$docker_ok" -eq 1 ]] || { echo "Skipping linux/amd64: docker not found."; return 0; }
  [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]] || { echo "Skipping linux/amd64: cannot run linux/amd64 containers."; return 0; }

  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.amd64"
  img="local/${BIN_NAME}-linux-amd64:${VERSION}-$$"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<DOCKERFILE
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST=${CACHE_BUST}

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
    libopenblas-dev \
    libvulkan-dev vulkan-tools vulkan-utility-libraries-dev \
    spirv-tools glslang-tools \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends glslc || true; \
    rm -rf /var/lib/apt/lists/*; \
    (command -v glslc >/dev/null 2>&1 && echo "glslc installed") || echo "glslc not available"

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

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:\${PATH}"
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
    -e LINUX_WITH_OPENBLAS="${LINUX_WITH_OPENBLAS}" \
    -e LINUX_WITH_VULKAN="${LINUX_WITH_VULKAN}" \
    -e WITH_CUDA="${WITH_CUDA}" \
    -e WITH_ROCM="${WITH_ROCM}" \
    "$img" \
    bash -lc '
      set -euo pipefail
      target=x86_64-unknown-linux-gnu

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-amd64-${variant}"
        echo "---- Building linux/amd64 [$variant] features: $feats"
        CARGO_TARGET_DIR="$ctd" cargo build --release --target "$target" --no-default-features --features "$feats"
      }

      build_variant cpu "'"${FEATURES_CPU}"'"

      if [ "${LINUX_WITH_OPENBLAS}" = "1" ]; then
        if [ -d /usr/include/x86_64-linux-gnu/openblas-pthread ]; then
          export BLAS_INCLUDE_DIRS=/usr/include/x86_64-linux-gnu/openblas-pthread
        elif [ -d /usr/include/x86_64-linux-gnu/openblas ]; then
          export BLAS_INCLUDE_DIRS=/usr/include/x86_64-linux-gnu/openblas
        elif [ -d /usr/include/openblas ]; then
          export BLAS_INCLUDE_DIRS=/usr/include/openblas
        else
          export BLAS_INCLUDE_DIRS=/usr/include
        fi
        build_variant openblas "'"${FEATURES_OPENBLAS}"'"
      fi

      if [ "${LINUX_WITH_VULKAN}" = "1" ]; then
        if command -v glslc >/dev/null 2>&1; then
          build_variant vulkan "'"${FEATURES_VULKAN}"'"
        else
          echo "WARN: glslc missing; skipping linux/amd64 vulkan variant"
        fi
      fi

      if [ "${WITH_CUDA}" = "1" ]; then
        build_variant cuda "'"${FEATURES_CUDA}"'"
      fi

      if [ "${WITH_ROCM}" = "1" ]; then
        build_variant rocm "'"${FEATURES_ROCM}"'"
      fi
    '

  linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "cpu"
  [[ "${LINUX_WITH_OPENBLAS}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "openblas"
  if [[ "${LINUX_WITH_VULKAN}" == "1" ]] && [[ -f "${PROJECT_ROOT}/target-cross/linux-amd64-vulkan/x86_64-unknown-linux-gnu/release/${BIN_NAME}" ]]; then
    linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "vulkan"
  fi
  [[ "${WITH_CUDA}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "cuda"
  [[ "${WITH_ROCM}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-gnu" "rocm"

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

build_linux_arm64_docker_variants() {
  [[ "$docker_ok" -eq 1 ]] || { echo "Skipping linux/arm64: docker not found."; return 0; }

  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.arm64"
  img="local/${BIN_NAME}-linux-arm64:${VERSION}-$$"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<DOCKERFILE
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST=${CACHE_BUST}

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
    libopenblas-dev \
    libvulkan-dev vulkan-tools vulkan-utility-libraries-dev \
    spirv-tools glslang-tools \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends glslc || true; \
    rm -rf /var/lib/apt/lists/*; \
    (command -v glslc >/dev/null 2>&1 && echo "glslc installed") || echo "glslc not available"

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:\${PATH}"
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
    -e LINUX_WITH_OPENBLAS="${LINUX_WITH_OPENBLAS}" \
    -e LINUX_WITH_VULKAN="${LINUX_WITH_VULKAN}" \
    "$img" \
    bash -lc '
      set -euo pipefail
      target=aarch64-unknown-linux-gnu

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-arm64-${variant}"
        echo "---- Building linux/arm64 [$variant] features: $feats"
        CARGO_TARGET_DIR="$ctd" cargo build --release --target "$target" --no-default-features --features "$feats"
      }

      build_variant cpu "'"${FEATURES_CPU}"'"

      if [ "${LINUX_WITH_OPENBLAS}" = "1" ]; then
        if [ -d /usr/include/aarch64-linux-gnu/openblas-pthread ]; then
          export BLAS_INCLUDE_DIRS=/usr/include/aarch64-linux-gnu/openblas-pthread
        elif [ -d /usr/include/aarch64-linux-gnu/openblas ]; then
          export BLAS_INCLUDE_DIRS=/usr/include/aarch64-linux-gnu/openblas
        elif [ -d /usr/include/openblas ]; then
          export BLAS_INCLUDE_DIRS=/usr/include/openblas
        else
          export BLAS_INCLUDE_DIRS=/usr/include
        fi
        build_variant openblas "'"${FEATURES_OPENBLAS}"'"
      fi

      if [ "${LINUX_WITH_VULKAN}" = "1" ]; then
        if command -v glslc >/dev/null 2>&1; then
          build_variant vulkan "'"${FEATURES_VULKAN}"'"
        else
          echo "WARN: glslc missing; skipping linux/arm64 vulkan variant"
        fi
      fi
    '

  linux_copy_out "arm64" "aarch64-unknown-linux-gnu" "cpu"
  [[ "${LINUX_WITH_OPENBLAS}" == "1" ]] && linux_copy_out "arm64" "aarch64-unknown-linux-gnu" "openblas"
  if [[ "${LINUX_WITH_VULKAN}" == "1" ]] && [[ -f "${PROJECT_ROOT}/target-cross/linux-arm64-vulkan/aarch64-unknown-linux-gnu/release/${BIN_NAME}" ]]; then
    linux_copy_out "arm64" "aarch64-unknown-linux-gnu" "vulkan"
  fi

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

# Run
ensure_espeak_data_archive

if want_arch amd64; then build_linux_amd64_docker_variants; fi
if want_arch arm64; then build_linux_arm64_docker_variants; fi

# Package
if [[ "${DO_PACKAGE}" -eq 1 ]]; then
  echo "== Packaging tar.gz + SHA256 =="
  for f in "${ARTIFACTS[@]}"; do
    package_one "$f"
  done
else
  echo "Skipping packaging (--skip-package)"
fi

echo "✔ Linux build complete"
