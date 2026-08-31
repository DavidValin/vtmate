#!/usr/bin/env bash
set -euo pipefail

# ==========================================================
# build_linux.sh - fully static Linux builds (musl)
# ==========================================================
#
# Builds vtmate for:
#   arch:    amd64, arm64
#   variant: cpu, vulkan, cuda (cuda is amd64-only)
#
# OpenBLAS is linked into every variant on every arch.
#
# Both arches cross-compile from a single linux/amd64 container using
# prebuilt musl.cc cross toolchains (x86_64-linux-musl-cross /
# aarch64-linux-musl-cross). Nothing runs under QEMU: the aarch64 gcc/g++
# themselves are ordinary amd64 host binaries that happen to emit aarch64
# code, so cross-compiling arm64 is exactly as fast as amd64 and doesn't
# need --platform=linux/arm64 or binfmt_misc at all. glslc (Vulkan shader
# compiler) is a build-time host tool too, so the same amd64-hosted glslc
# compiles shaders for both target arches; only its SPIR-V *output* (which
# is architecture-independent) ends up in the arm64 binary.
#
# Vulkan note: whisper-rs-sys links libvulkan as a normal (non-static) lib
# on Linux, same as vulkan-1.dll on Windows - the ICD loader model requires
# a runtime-resolved driver, so "fully static" Vulkan builds still carry
# that one dynamic dependency. That's expected and matches the existing
# Windows static-check policy (scripts/check-static.sh explicitly allows
# Vulkan/CUDA loaders through).
#
# This script is a from-scratch rewrite, not a copy of build_linux.sh - see
# the two build_linux_<arch>_variants() functions below for exactly what
# each container does.

BIN_NAME="vtmate"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
PKG_DIR="${DIST_DIR}/packages"
ASSETS_DIR="${PROJECT_ROOT}/assets"
ESPEAK_ARCHIVE="${ASSETS_DIR}/espeak-ng-data.tar.gz"

DO_PACKAGE=1
DOCKER_NO_CACHE=1
SEL_ARCH="all"      # amd64,arm64,all
SEL_VARIANT="all"   # cpu,vulkan,cuda,all

# Linux variant toggles - on by default, so a bare invocation still produces
# cpu+vulkan+(cuda on amd64) in one pass. --variant overrides these.
WITH_CPU="${WITH_CPU:-1}"       # amd64 + arm64
WITH_CUDA="${WITH_CUDA:-1}"     # amd64 only
WITH_VULKAN="${WITH_VULKAN:-1}" # amd64 + arm64

# Host cache mounts (Linux Docker)
HOST_HOME="${HOME}"
HOST_K_CACHE="${HOST_HOME}/.cache/k"
HOST_WHISPER_MODELS="${HOST_HOME}/.whisper-models"
CONT_K_CACHE="/root/.cache/k"
CONT_WHISPER_MODELS="/root/.whisper-models"

# -----------------------------
# Helper functions
# -----------------------------
usage() {
  cat <<'USAGE'
Usage:
  ./build_linux.sh [--arch <list>] [--variant <list>] [--skip-package] [--cache|--no-cache]

--arch    comma-separated: amd64,arm64,all
--variant comma-separated: cpu,vulkan,cuda,all  (cuda is amd64 only)
          When given, it overrides the WITH_* env toggles below. CI uses
          this to build one arch+variant per runner.

Env:
  WITH_CPU=0|1      (amd64 + arm64) default 1
  WITH_CUDA=0|1     (amd64 only) default 1
  WITH_VULKAN=0|1   (amd64 + arm64) default 1

OpenBLAS is always enabled, on every arch and every variant.
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
    --variant) SEL_VARIANT="$(normalize_list "${2-}")"; shift 2 ;;
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

# --variant, when given, is authoritative: it replaces the WITH_* toggles so
# one runner can build exactly one arch+variant.
if [[ "${SEL_VARIANT}" != "all" ]]; then
  WITH_CPU=0; WITH_VULKAN=0; WITH_CUDA=0
  list_has "${SEL_VARIANT}" cpu    && WITH_CPU=1
  list_has "${SEL_VARIANT}" vulkan && WITH_VULKAN=1
  list_has "${SEL_VARIANT}" cuda   && WITH_CUDA=1
  if [[ "${WITH_CPU}${WITH_VULKAN}${WITH_CUDA}" == "000" ]]; then
    echo "ERROR: --variant '${SEL_VARIANT}' selected no known variant (cpu,vulkan,cuda)"
    exit 1
  fi
fi

VERSION="$(
  awk -F\" '
    $1 ~ /^[[:space:]]*version[[:space:]]*=[[:space:]]*/ { print $2; exit }
  ' "${PROJECT_ROOT}/Cargo.toml"
)"
[[ -n "${VERSION}" ]] || { echo "Failed to read version from Cargo.toml"; exit 1; }

mkdir -p "${DIST_DIR}" "${PKG_DIR}" "${PROJECT_ROOT}/target-cross" "${ASSETS_DIR}"
mkdir -p "${HOST_K_CACHE}" "${HOST_WHISPER_MODELS}"

echo "Version: ${VERSION}"
echo "Linux: arch=${SEL_ARCH} variant=${SEL_VARIANT}"
echo "WITH_CPU=${WITH_CPU}  WITH_CUDA=${WITH_CUDA} (amd64 only)  WITH_VULKAN=${WITH_VULKAN}"
echo "OpenBLAS: always ON"

# Features - OpenBLAS is part of every variant's feature set, unconditionally.
FEATURES_COMMON="whisper-openblas"
FEATURES_CPU="${FEATURES_COMMON}"
FEATURES_VULKAN="${FEATURES_COMMON},whisper-vulkan"
# ort-cuda registers ONNX Runtime's CUDA execution provider on the Rust
# side; without it ORT inference stays on CPU even though the C++ build
# sets -Donnxruntime_USE_CUDA=ON.
FEATURES_CUDA="${FEATURES_COMMON},whisper-cuda,ort-cuda"

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
  # -e so a directory artifact is packaged too; make_tgz handles both.
  [[ -e "$src" ]] || return 0
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
  img="local/${BIN_NAME}-espeak-asset:cache"
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
# Linux copy helper
# -----------------------------
ARTIFACTS=()
# -e, not -f: the glibc GPU variants produce a directory (binary + the ONNX
# Runtime .so files beside it), not a single file. The explicit `return 0` also
# stops a non-match from tripping `set -e` and aborting the whole build.
add_artifact() { [[ -e "$1" ]] && ARTIFACTS+=("$1"); return 0; }

linux_copy_out() {
  local arch="$1" target="$2" variant="$3"
  local src_dir="${PROJECT_ROOT}/target-cross/linux-${arch}-${variant}/${target}/release"
  local out="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-${arch}-${variant}"
  [[ -f "${src_dir}/${BIN_NAME}" ]] || return 0
  cp "${src_dir}/${BIN_NAME}" "$out"
  chmod +x "$out" || true
  add_artifact "$out"
  echo "✔ Built: $out"
}

# ==========================================================
# AMD64 (x86_64-unknown-linux-musl) - cpu, vulkan, cuda
# ==========================================================
build_linux_amd64_variants() {
  [[ "$docker_ok" -eq 1 ]] || { echo "Skipping linux/amd64: docker not found."; return 0; }
  [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]] || { echo "Skipping linux/amd64: cannot run linux/amd64 containers."; return 0; }

  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.amd64"
  img="local/${BIN_NAME}-linux-amd64:cache-cuda${WITH_CUDA}"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<'DOCKERFILE'
# musl cross toolchain, taken from the official musl.cc container image
# rather than downloaded from musl.cc: that host drops GitHub Actions runner
# IPs (every fetch attempt hit the wget timeout), and hosting the tarballs on
# this repo's releases is not wanted. Same GCC 11.2.1 build, digest-pinned.
FROM muslcc/x86_64:x86_64-linux-musl@sha256:173c042a23a544defa3364d4472b30b36af1d827a74cd8dab7683b8500004334 AS musltc

FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST
ARG WITH_CUDA=0

ENV WITH_CUDA=${WITH_CUDA}

# ----------------------------------------------------------
# Build dependencies
# ----------------------------------------------------------
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential pkg-config curl wget ca-certificates git \
  libclang-dev clang \
  cmake python3 python3-pip perl autoconf automake libtool \
  gfortran zlib1g-dev libbz2-dev liblzma-dev libssl-dev \
  glslc \
&& rm -rf /var/lib/apt/lists/*

RUN if [ "$WITH_CUDA" = "1" ]; then \
    apt-get update && apt-get install -y --no-install-recommends nvidia-cuda-toolkit && \
    rm -rf /var/lib/apt/lists/* ; \
  fi

# cuDNN: required by the ONNX Runtime CUDA execution provider, and NOT part of
# the CUDA toolkit (apt installs neither). ORT locates it via $CUDNN_PATH
# (cmake/external/cuDNN.cmake), expecting include/ and lib/ underneath.
# The _cuda12 build matches the CUDA 12.x that nvidia-cuda-toolkit provides on
# noble; if that apt package ever moves to CUDA 13, switch to _cuda13.
ARG CUDNN_VERSION=9.16.0.29
RUN if [ "$WITH_CUDA" = "1" ]; then \
    set -eux; \
    f=cudnn-linux-x86_64-${CUDNN_VERSION}_cuda12-archive; \
    wget -nv --timeout=60 --tries=3 -O /tmp/cudnn.tar.xz \
      "https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/linux-x86_64/$f.tar.xz"; \
    mkdir -p /usr/local/cudnn; \
    tar -xJf /tmp/cudnn.tar.xz -C /tmp; \
    cp -a /tmp/$f/include /usr/local/cudnn/; \
    cp -a /tmp/$f/lib /usr/local/cudnn/; \
    rm -rf /tmp/cudnn.tar.xz /tmp/$f; \
    test -f /usr/local/cudnn/include/cudnn.h; \
  fi
ENV CUDNN_PATH=/usr/local/cudnn

# musl cross toolchain (prebuilt from musl.cc). This is an ordinary amd64
# host binary that emits amd64/musl code - no emulation involved.
# ----------------------------------------------------------
# The image carries the toolchain at its root, but /bin there also holds
# busybox applets, so it is copied into the usual prefix and deliberately NOT
# put on PATH. Prefixed symlinks in /usr/local/bin give the tool names the
# rest of this file already uses; gcc still resolves its sysroot and libexec
# relative to its real location.
COPY --from=musltc /bin                  /opt/x86_64-linux-musl-cross/bin
COPY --from=musltc /include              /opt/x86_64-linux-musl-cross/include
COPY --from=musltc /lib                  /opt/x86_64-linux-musl-cross/lib
COPY --from=musltc /libexec              /opt/x86_64-linux-musl-cross/libexec
COPY --from=musltc /share                /opt/x86_64-linux-musl-cross/share
COPY --from=musltc /x86_64-linux-musl     /opt/x86_64-linux-musl-cross/x86_64-linux-musl
RUN set -eux; \
    for t in gcc g++ cpp cc gfortran ar ranlib strip nm objdump objcopy ld as \
             readelf addr2line c++filt gcov size strings gcc-ar gcc-nm gcc-ranlib; do \
      if [ -x /opt/x86_64-linux-musl-cross/bin/$t ]; then \
        ln -sf /opt/x86_64-linux-musl-cross/bin/$t /usr/local/bin/x86_64-linux-musl-$t; \
      fi; \
    done; \
    x86_64-linux-musl-gcc --version; \
    x86_64-linux-musl-gfortran --version; \
    x86_64-linux-musl-g++ --version

# bindgen (espeak-rs-sys, whisper-rs-sys) loads libclang at build-script run
# time; without this it panics with "Unable to find libclang". Resolve the
# directory rather than hardcoding an LLVM version, and fail here if absent.
RUN set -eux; \
    d="$(dirname "$(find /usr/lib -name 'libclang.so*' 2>/dev/null | head -1)")"; \
    test -n "$d" -a -d "$d"; \
    ln -sfn "$d" /usr/local/libclang; \
    ls -l /usr/local/libclang/ | head -3
ENV LIBCLANG_PATH=/usr/local/libclang

# Install Rust and add the musl target
# ----------------------------------------------------------
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add x86_64-unknown-linux-musl
RUN rustup update stable

# ----------------------------------------------------------
# C/C++ Compiler / Linker config
# ----------------------------------------------------------
ENV CC_x86_64_unknown_linux_musl=x86_64-linux-musl-gcc
ENV CXX_x86_64_unknown_linux_musl=x86_64-linux-musl-g++
ENV CC=x86_64-linux-musl-gcc
ENV CXX=x86_64-linux-musl-g++
ENV LD=x86_64-linux-musl-g++
ENV LDFLAGS="-lgfortran -lm -lpthread -lquadmath"
ENV AR=ar
ENV RANLIB=ranlib
ENV FC=x86_64-linux-musl-gfortran
ENV FFLAGS="-static-libgfortran"
ENV CFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl"
ENV CXXFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl"
ENV CMAKE_FIND_LIBRARY_SUFFIXES=".a"
ENV CMAKE_EXE_LINKER_FLAGS=-static
ENV BINDGEN_EXTRA_CLANG_ARGS="-I/opt/x86_64-linux-musl-cross/x86_64-linux-musl/include"

# ----------------------------------------------------------
# Build openssl for musl (amd64)
# ----------------------------------------------------------
RUN set -eux; \
    curl -LO https://www.openssl.org/source/openssl-3.1.3.tar.gz \
    && tar xf openssl-3.1.3.tar.gz \
    && cd openssl-3.1.3 \
    && ./Configure linux-x86_64 no-shared no-tests no-async no-secure-memory no-engine --openssldir=/usr/local/ssl --libdir=/usr/local/lib --prefix=/usr/local \
    && make -j$(nproc) \
    && make install_sw \
    && cd .. && rm -rf openssl-3.1.3 openssl-3.1.3.tar.gz

ENV OPENSSL_STATIC=1
ENV OPENSSL_DIR=/usr/local
ENV OPENSSL_LIB_DIR=/usr/local/lib
ENV OPENSSL_INCLUDE_DIR=/usr/local/include

# ----------------------------------------------------------
# Build OpenMP for musl (amd64) - cmake picks up CC/CXX above
# ----------------------------------------------------------
ENV OPENMP_DIR=/openmp
ENV OPENMP_PREFIX=/usr/local
ENV LLVM_SRC_URL="https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.0/llvm-project-22.1.0.src.tar.xz"

RUN mkdir -p $OPENMP_DIR
WORKDIR $OPENMP_DIR
RUN wget -q -O llvm-project-22.1.0.src.tar.xz "$LLVM_SRC_URL" \
 && tar xf llvm-project-22.1.0.src.tar.xz \
 && rm llvm-project-22.1.0.src.tar.xz

RUN CFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -fopenmp" \
    CXXFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -fopenmp" \
    cmake -S $OPENMP_DIR/llvm-project-22.1.0.src/openmp \
      -B /openmp/build \
      -DCMAKE_INSTALL_PREFIX=$OPENMP_PREFIX \
      -DLIBOMP_ENABLE_SHARED=OFF \
      -DLIBOMP_ENABLE_STATIC=ON \
      -DCMAKE_BUILD_TYPE=Release

RUN cmake --build /openmp/build --parallel $(nproc) --target install

# ----------------------------------------------------------
# Build static OpenBLAS for musl (amd64)
# ----------------------------------------------------------
RUN git clone --depth 1 https://github.com/xianyi/OpenBLAS.git /openblas

RUN cd /openblas && \
    set -eux; \
    make -j$(nproc) \
      HOSTCC=gcc \
      CFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl" \
      LDFLAGS="-lgfortran -lm -lpthread -lquadmath -lpthread" \
      USE_STATIC=1 \
      STATIC_ONLY=1 \
      NO_SHARED=1 \
      USE_OPENMP=0 \
      USE_THREAD=1 \
      TARGET=GENERIC \
      NO_AVX=1 \
      VERBOSE=1 \
      libs netlib

RUN set -eux; \
  cd /openblas && \
  make install HOSTCC=gcc PREFIX=/usr/local STATIC_ONLY=1 NO_SHARED=1 && \
  cd / && rm -rf /openblas

ENV OPENBLAS_PATH=/usr/local
ENV BLAS_LIBRARIES=/usr/local/lib/libopenblas.a
ENV BLAS_INCLUDE_DIRS=/usr/local/include

# ----------------------------------------------------------
# Build espeak-ng musl version (amd64)
# ----------------------------------------------------------
RUN set -eux; \
    git clone --depth 1 https://github.com/espeak-ng/espeak-ng.git /espeak-ng; \
    cmake -S /espeak-ng -B /espeak-ng/build \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_CXX_FLAGS="-std=c++17" \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DCOMPILE_INTONATIONS=OFF \
      -DENABLE_TESTS=OFF \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
      -DCMAKE_SKIP_RPATH=ON \
      -DCMAKE_INSTALL_RPATH="" \
      -DCMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
      -DCMAKE_INSTALL_PREFIX=/usr/local; \
    cmake --build /espeak-ng/build -j$(nproc); \
    cmake --install /espeak-ng/build; \
    rm -rf /espeak-ng

ENV ESPEAK_NG_DIR="/usr/local/lib"

# ----------------------------------------------------------
# musl locale compatibility shim for FlatBuffers (strtoll_l)
# ----------------------------------------------------------
RUN printf "%s\n" \
"#pragma once" \
"" \
"#include <stdlib.h>" \
"#include <locale.h>" \
"" \
"// musl compatibility shim for *_l functions used by FlatBuffers" \
"#if !defined(__GLIBC__)" \
"" \
"static inline long long strtoll_l(const char* nptr, char** endptr, int base, locale_t loc) {" \
"  (void)loc;" \
"  return strtoll(nptr, endptr, base);" \
"}" \
"" \
"static inline unsigned long long strtoull_l(const char* nptr, char** endptr, int base, locale_t loc) {" \
"  (void)loc;" \
"  return strtoull(nptr, endptr, base);" \
"}" \
"" \
"#endif" \
> /usr/local/include/musl_locale_compat.h

# ----------------------------------------------------------
# Build protobuf, musl static (amd64). This is the only protoc used on
# this image: a static x86_64-musl binary still runs fine on this amd64
# host (no dynamic loader needed), so it doubles as the codegen tool.
# ----------------------------------------------------------
RUN git clone --depth 1 -b v3.21.12 https://github.com/protocolbuffers/protobuf.git /protobuf

RUN set -eux; \
    cmake -S /protobuf -B /protobuf/build \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_INSTALL_PREFIX=/usr/local \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DCMAKE_BUILD_TYPE=Release \
      -Dprotobuf_BUILD_TESTS=OFF; \
    cmake --build /protobuf/build -j$(nproc); \
    cmake --install /protobuf/build; \
    rm -rf /protobuf

# -----------------------------
# Build ONNX Runtime for this arch (amd64 musl, CUDA optional)
# -----------------------------
ENV ONNX_DIR=/onnxruntime
ENV ONNX_SRC=/onnxruntime-src

RUN set -eux; \
    mkdir -p "$ONNX_DIR"; \
    # Pinned, not main: ORT main pins protobuf v33.6, whose generated headers
    # include google/protobuf/runtime_version.h, while this image builds and
    # links protobuf v3.21.12. v1.24.1 pins protobuf v21.12, matching, and is
    # the newest ORT API (24) that ort-sys 2.0.0-rc.12 knows about.
    git clone --depth 1 -b v1.24.1 https://github.com/microsoft/onnxruntime.git $ONNX_SRC; \
    # ORT decides between std::chrono and the vendored HowardHinnant/date
    # purely on __cplusplus >= 202002L, but GCC 11 (this musl toolchain) has
    # no std::chrono operator<< for time_point until GCC 13, so the C++20
    # branch fails to compile ostream_sink.cc. Force the date branch, which
    # ORT already supports and fetches (deps.txt pins date v3.0.1).
    sed -i "s|#define ORT_USE_CXX20_STD_CHRONO __cplusplus >= 202002L|#define ORT_USE_CXX20_STD_CHRONO 0|" \
      $ONNX_SRC/include/onnxruntime/core/common/logging/logging.h; \
    grep -q "define ORT_USE_CXX20_STD_CHRONO 0" $ONNX_SRC/include/onnxruntime/core/common/logging/logging.h

WORKDIR $ONNX_SRC

# execinfo.h (backtrace) isn't available under musl.
RUN find . -type f -print0 | xargs -0 -r sed -i "/#include <execinfo\.h>/d"

RUN mkdir -p build
WORKDIR $ONNX_SRC/build

RUN set -eux; \
    cmake ../cmake \
      -B $ONNX_DIR \
      -DCMAKE_SYSTEM_PROCESSOR=AMD64 \
      -DCMAKE_CXX_STANDARD=20 \
      -DCMAKE_CXX_STANDARD_REQUIRED=ON \
      -DCMAKE_C_FLAGS="-march=x86-64 -include /usr/local/include/musl_locale_compat.h" \
      -DCMAKE_CXX_FLAGS="-march=x86-64 -std=c++17 -include /usr/local/include/musl_locale_compat.h" \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_C_COMPILER=$CC \
      -DCMAKE_CXX_COMPILER=$CXX \
      -DCMAKE_LINKER=$LD \
      -DCMAKE_COMPILE_WARNING_AS_ERROR=OFF \
      -Donnxruntime_BUILD_UNIT_TESTS=OFF \
      -Donnxruntime_ENABLE_EXTERNAL_CUSTOM_OP_SCHEMAS=OFF \
      -Donnxruntime_RUN_ONNX_TESTS=OFF \
      -DPython_EXECUTABLE=/usr/bin/python3 \
      -Donnxruntime_USE_VCPKG=OFF \
      -Donnxruntime_USE_MIMALLOC=OFF \
      -Donnxruntime_ENABLE_PYTHON=OFF \
      -Donnxruntime_BUILD_CSHARP=OFF \
      -Donnxruntime_BUILD_JAVA=OFF \
      -Donnxruntime_BUILD_NODEJS=OFF \
      -Donnxruntime_BUILD_OBJC=OFF \
      -Donnxruntime_BUILD_SHARED_LIB=OFF \
      -Donnxruntime_BUILD_APPLE_FRAMEWORK=OFF \
      -Donnxruntime_USE_DNNL=OFF \
      -Donnxruntime_USE_NNAPI_BUILTIN=OFF \
      -Donnxruntime_USE_VSINPU=OFF \
      -Donnxruntime_USE_RKNPU=OFF \
      -Donnxruntime_USE_VITISAI=OFF \
      -Donnxruntime_USE_TENSORRT=OFF \
      -Donnxruntime_USE_NV=OFF \
      -Donnxruntime_USE_TENSORRT_BUILTIN_PARSER=ON \
      -Donnxruntime_USE_TENSORRT_INTERFACE=OFF \
      -Donnxruntime_USE_CUDA_INTERFACE=OFF \
      -Donnxruntime_USE_NV_INTERFACE=OFF \
      -Donnxruntime_USE_OPENVINO_INTERFACE=OFF \
      -Donnxruntime_USE_VITISAI_INTERFACE=OFF \
      -Donnxruntime_USE_QNN_INTERFACE=OFF \
      -Donnxruntime_USE_MIGRAPHX_INTERFACE=OFF \
      -Donnxruntime_USE_MIGRAPHX=OFF \
      -Donnxruntime_DISABLE_RTTI=OFF \
      -Donnxruntime_DISABLE_EXCEPTIONS=OFF \
      -Donnxruntime_MINIMAL_BUILD=OFF \
      -Donnxruntime_ENABLE_LTO=OFF \
      -Donnxruntime_USE_ACL=OFF \
      -Donnxruntime_USE_ARMNN=OFF \
      -Donnxruntime_USE_JSEP=OFF \
      -Donnxruntime_USE_WEBGPU=OFF \
      -Donnxruntime_USE_EXTERNAL_DAWN=OFF \
      -Donnxruntime_WGSL_TEMPLATE=static \
      -Donnxruntime_ENABLE_TRAINING=OFF \
      -Donnxruntime_ENABLE_TRAINING_OPS=OFF \
      -Donnxruntime_ENABLE_TRAINING_APIS=OFF \
      -Donnxruntime_ENABLE_CPU_FP16_OPS=OFF \
      -Donnxruntime_USE_NCCL=OFF \
      -Donnxruntime_BUILD_BENCHMARKS=OFF \
      -Donnxruntime_ENABLE_CUDA_LINE_NUMBER_INFO=OFF \
      -Donnxruntime_USE_CUDA_NHWC_OPS=OFF \
      -Donnxruntime_BUILD_WEBASSEMBLY_STATIC_LIB=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_THREADS=OFF \
      -Donnxruntime_USE_XNNPACK=OFF \
      -Donnxruntime_USE_WEBNN=OFF \
      -Donnxruntime_USE_CANN=OFF \
      -Donnxruntime_CUDA_MINIMAL=OFF \
      -Donnxruntime_USE_CUDA=$WITH_CUDA \
      -Donnxruntime_USE_KLEIDIAI=OFF \
      -DCMAKE_INSTALL_PREFIX=$ONNX_DIR \
      -DCMAKE_BUILD_TYPE=Release \
      -Donnxruntime_USE_SYSTEM_PROTOBUF=ON \
      -DCMAKE_CUDA_COMPILER=/usr/bin/nvcc \
      -DCUDAToolkit_ROOT=/usr \
      -DProtobuf_INCLUDE_DIR=/usr/local/include \
      -DProtobuf_LIBRARIES=/usr/local/lib/libprotobuf.a \
      -DProtobuf_PROTOC_EXECUTABLE=/usr/local/bin/protoc; \
    cmake --build $ONNX_DIR --config Release

ENV ORT_STRATEGY=system
ENV ORT_LIB_LOCATION=$ONNX_DIR

WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux amd64 build (Docker image) =="
  if docker image inspect "$img" >/dev/null 2>&1; then
    echo "Docker image '$img' already exists. Skipping build."
  else
    docker build "${build_args[@]}" --platform=linux/amd64 \
        --build-arg WITH_CUDA="${WITH_CUDA}" \
        --build-arg CACHE_BUST="${CACHE_BUST}" \
        -f "$df" -t "$img" "$tmp"
  fi

  echo "== Linux amd64 cargo builds (cpu=${WITH_CPU} vulkan=${WITH_VULKAN} cuda=${WITH_CUDA}) =="
  docker run --rm --platform=linux/amd64 \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -v "${HOST_K_CACHE}:${CONT_K_CACHE}" \
    -v "${HOST_WHISPER_MODELS}:${CONT_WHISPER_MODELS}" \
    -e WITH_CPU="${WITH_CPU}" \
    -e WITH_VULKAN="${WITH_VULKAN}" \
    -e WITH_CUDA="${WITH_CUDA}" \
    -e CMAKE_SKIP_RPATH=ON \
    -e CMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
    "$img" \
    bash -lc '
      set -euo pipefail

      ARCH=amd64
      target=x86_64-unknown-linux-musl

      # rust-toolchain.toml pins the toolchain, but the image added the musl
      # target to whatever was default at image build time (stable). Cargo
      # switches to the pinned toolchain here and then finds no musl std.
      # Re-add the target from inside /work so rustup reads rust-toolchain.toml.
      cd /work
      rustup target add "$target"

      # Build ALSA as a static library for musl (needed by cpal)
      if [ ! -f /usr/local/lib/libasound.a ]; then
        echo "--- Building ALSA static library for musl ---"
        apt-get update -qq && apt-get install -y --no-install-recommends autoconf automake libtool
        curl -sL -o /tmp/alsa.tar.gz \
          "https://github.com/alsa-project/alsa-lib/archive/refs/tags/v1.2.12.tar.gz"
        tar xzf /tmp/alsa.tar.gz -C /tmp
        mv /tmp/alsa-lib-1.2.12 /tmp/alsa-lib
        cd /tmp/alsa-lib
        autoreconf -fi
        ./configure \
          --host=x86_64-linux-musl \
          --build=x86_64-linux-gnu \
          --prefix=/usr/local \
          --enable-shared=no \
          --enable-static=yes \
          --with-pkg-config-plugindir=/usr/local/lib/pkgconfig \
          CC=x86_64-linux-musl-gcc \
          CFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -O3" \
          LDFLAGS="-L/opt/x86_64-linux-musl-cross/x86_64-linux-musl/lib"
        make -j$(nproc)
        make install
        cd /
        rm -rf /tmp/alsa-lib /tmp/alsa.tar.gz
      else
        echo "--- ALSA already built, skipping ---"
      fi

      # Build abseil + re2 static libraries for musl (needed by ort-sys)
      if [ ! -f /usr/local/lib/libre2.a ]; then
        echo "--- Building abseil + re2 static libraries for musl ---"
        curl -sL -o /tmp/re2.tar.gz \
          "https://github.com/google/re2/archive/refs/tags/2024-07-02.tar.gz"
        tar xzf /tmp/re2.tar.gz -C /tmp
        mv /tmp/re2-2024-07-02 /tmp/re2

        curl -sL -o /tmp/absl.tar.gz \
          "https://github.com/abseil/abseil-cpp/archive/refs/tags/20240722.0.tar.gz"
        tar xzf /tmp/absl.tar.gz -C /tmp
        mv /tmp/abseil-cpp-20240722.0 /tmp/absl
        cmake -S /tmp/absl -B /tmp/absl-build \
          -DCMAKE_INSTALL_PREFIX=/usr/local \
          -DCMAKE_BUILD_TYPE=Release \
          -DBUILD_SHARED_LIBS=OFF \
          -DCMAKE_C_COMPILER=x86_64-linux-musl-gcc \
          -DCMAKE_CXX_COMPILER=x86_64-linux-musl-g++ \
          -DCMAKE_CXX_FLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl" \
          -DCMAKE_C_FLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl" \
          -DABSL_BUILD_TESTING=OFF
        cmake --build /tmp/absl-build --parallel $(nproc)
        cmake --install /tmp/absl-build --prefix /usr/local

        make -C /tmp/re2 -j$(nproc) \
          CXX=x86_64-linux-musl-g++ \
          CXXFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -O3 -static -I/usr/local/include" \
          LDFLAGS="-static -L/usr/local/lib" \
          AR=x86_64-linux-musl-ar \
          static
        cp /tmp/re2/obj/libre2.a /usr/local/lib/
        mkdir -p $ONNX_DIR/_deps/re2-build/
        cp /usr/local/lib/libre2.a $ONNX_DIR/_deps/re2-build/
        rm -rf /tmp/re2 /tmp/re2.tar.gz /tmp/absl /tmp/absl.tar.gz /tmp/absl-build
      else
        echo "--- abseil/re2 already built, skipping ---"
      fi

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-${ARCH}-${variant}"

        echo "---- Building linux/${ARCH} [$variant] features: $feats"
        export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
        export CARGO_PROFILE_RELEASE_DEBUG=false
        export CARGO_PROFILE_RELEASE_STRIP=symbols
        export CARGO_PROFILE_RELEASE_INCREMENTAL=false
        export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-g++
        export RUSTC_LINKER=x86_64-linux-musl-g++

        ABSL_LIBS=""
        for f in /usr/local/lib/libabsl_*.a; do
          ABSL_LIBS="$ABSL_LIBS -C link-arg=$f"
        done

        export RUSTFLAGS="-C target-feature=+crt-static -C target-cpu=x86-64-v3 -C codegen-units=1 -C opt-level=3 -C link-arg=-L/opt/x86_64-linux-musl-cross/x86_64-linux-musl/lib -C link-arg=-Wl,--start-group -C link-arg=/usr/local/lib/libopenblas.a -C link-arg=/usr/local/lib/libprotobuf.a -C link-arg=/usr/local/lib/libomp.a ${ABSL_LIBS} -C link-arg=-Wl,--end-group -C link-arg=-lm -C link-arg=-lc -C link-arg=-lgfortran -C link-arg=-lpthread -C link-arg=-lgcc"

        cd /work
        CARGO_TARGET_DIR="$ctd" \
        cargo build --release --target "$target" --features "$feats"
      }

      if [ "${WITH_CPU}" = "1" ]; then
        build_variant cpu "'"${FEATURES_CPU}"'"
      fi

      if [ "${WITH_VULKAN}" = "1" ]; then
        build_variant vulkan "'"${FEATURES_VULKAN}"'"
      fi

      if [ "${WITH_CUDA}" = "1" ]; then
        build_variant cuda "'"${FEATURES_CUDA}"'"
      fi
    '

  [[ "${WITH_CPU}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-musl" "cpu"
  [[ "${WITH_VULKAN}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-musl" "vulkan"
  [[ "${WITH_CUDA}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-musl" "cuda"
  true

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

# ==========================================================
# ARM64 (aarch64-unknown-linux-musl) - cpu, vulkan (no CUDA)
# ==========================================================
# Cross-compiled from the SAME linux/amd64 host as the amd64 build above
# (see the file header comment) using the musl.cc aarch64 cross toolchain.
# The one wrinkle vs. the amd64 image: a cross-built aarch64 protoc can't
# run on this amd64 host to generate ONNX Runtime's protobuf code, so this
# image builds protobuf twice from the same source tag - once natively
# (host protoc, used only as a codegen tool) and once cross-compiled
# (target libprotobuf.a, used for linking).
build_linux_arm64_variants() {
  [[ "$docker_ok" -eq 1 ]] || { echo "Skipping linux/arm64: docker not found."; return 0; }
  [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]] || { echo "Skipping linux/arm64: cannot run linux/amd64 containers (needed for cross-compilation)."; return 0; }

  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.arm64"
  img="local/${BIN_NAME}-linux-arm64:cache"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<'DOCKERFILE'
# musl cross toolchain, taken from the official musl.cc container image
# rather than downloaded from musl.cc: that host drops GitHub Actions runner
# IPs (every fetch attempt hit the wget timeout), and hosting the tarballs on
# this repo's releases is not wanted. Same GCC 11.2.1 build, digest-pinned.
FROM muslcc/x86_64:aarch64-linux-musl@sha256:2d106ab72b2b5ea5ac7696a6b3d7c5f4f98d24f99230fb784d991c453034c017 AS musltc

FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST

# ----------------------------------------------------------
# Build dependencies (host tools; glslc/protoc-host run natively here)
# ----------------------------------------------------------
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential pkg-config curl wget ca-certificates git \
  libclang-dev clang \
  cmake python3 python3-pip perl autoconf automake libtool \
  gfortran zlib1g-dev libbz2-dev liblzma-dev libssl-dev \
  glslc \
&& rm -rf /var/lib/apt/lists/*

# musl cross toolchain for aarch64 (prebuilt from musl.cc). Its gcc/g++ are
# ordinary amd64 host binaries that emit aarch64 code - no QEMU needed.
# ----------------------------------------------------------
# The image carries the toolchain at its root, but /bin there also holds
# busybox applets, so it is copied into the usual prefix and deliberately NOT
# put on PATH. Prefixed symlinks in /usr/local/bin give the tool names the
# rest of this file already uses; gcc still resolves its sysroot and libexec
# relative to its real location.
COPY --from=musltc /bin                  /opt/aarch64-linux-musl-cross/bin
COPY --from=musltc /include              /opt/aarch64-linux-musl-cross/include
COPY --from=musltc /lib                  /opt/aarch64-linux-musl-cross/lib
COPY --from=musltc /libexec              /opt/aarch64-linux-musl-cross/libexec
COPY --from=musltc /share                /opt/aarch64-linux-musl-cross/share
COPY --from=musltc /aarch64-linux-musl     /opt/aarch64-linux-musl-cross/aarch64-linux-musl
RUN set -eux; \
    for t in gcc g++ cpp cc gfortran ar ranlib strip nm objdump objcopy ld as \
             readelf addr2line c++filt gcov size strings gcc-ar gcc-nm gcc-ranlib; do \
      if [ -x /opt/aarch64-linux-musl-cross/bin/$t ]; then \
        ln -sf /opt/aarch64-linux-musl-cross/bin/$t /usr/local/bin/aarch64-linux-musl-$t; \
      fi; \
    done; \
    aarch64-linux-musl-gcc --version; \
    aarch64-linux-musl-gfortran --version; \
    aarch64-linux-musl-g++ --version

# bindgen (espeak-rs-sys, whisper-rs-sys) loads libclang at build-script run
# time; without this it panics with "Unable to find libclang". Resolve the
# directory rather than hardcoding an LLVM version, and fail here if absent.
RUN set -eux; \
    d="$(dirname "$(find /usr/lib -name 'libclang.so*' 2>/dev/null | head -1)")"; \
    test -n "$d" -a -d "$d"; \
    ln -sfn "$d" /usr/local/libclang; \
    ls -l /usr/local/libclang/ | head -3
ENV LIBCLANG_PATH=/usr/local/libclang

# Install Rust and add the musl target
# ----------------------------------------------------------
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add aarch64-unknown-linux-musl
RUN rustup update stable

# ----------------------------------------------------------
# C/C++ Compiler / Linker config
# ----------------------------------------------------------
ENV CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc
ENV CXX_aarch64_unknown_linux_musl=aarch64-linux-musl-g++
ENV CC=aarch64-linux-musl-gcc
ENV CXX=aarch64-linux-musl-g++
ENV LD=aarch64-linux-musl-g++
# No -lquadmath here: libquadmath is x86-only, the aarch64 musl toolchain
# does not ship it, and this ENV is inherited by every later build (OpenSSL
# first), not just the Fortran-linking OpenBLAS one.
ENV LDFLAGS="-lgfortran -lm -lpthread"
ENV AR=ar
ENV RANLIB=ranlib
ENV FC=aarch64-linux-musl-gfortran
ENV FFLAGS="-static-libgfortran"
ENV CFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl"
ENV CXXFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl"

# Some build scripts (openssl-sys header expansion) compile for the HOST
# triple. cc-rs falls back to the generic CC/CFLAGS above, handing the aarch64
# cross compiler an x86_64 job and an aarch64 sysroot - it then rejects -m64.
# Host-triple overrides take precedence in cc-rs, so point them at system gcc.
# -O2 rather than "": an empty value loses to the generic CFLAGS above, so
# host gcc kept receiving the aarch64 --sysroot and could not find /usr/include
# ("no include path in which to search for stdint.h" building ring).
ENV CC_x86_64_unknown_linux_gnu=gcc
ENV CXX_x86_64_unknown_linux_gnu=g++
ENV CFLAGS_x86_64_unknown_linux_gnu="-O2"
ENV CXXFLAGS_x86_64_unknown_linux_gnu="-O2"
ENV HOST_CC=gcc
ENV HOST_CXX=g++
ENV HOST_CFLAGS="-O2"
ENV HOST_CXXFLAGS="-O2"
ENV CMAKE_FIND_LIBRARY_SUFFIXES=".a"
ENV CMAKE_EXE_LINKER_FLAGS=-static
ENV BINDGEN_EXTRA_CLANG_ARGS="-I/opt/aarch64-linux-musl-cross/aarch64-linux-musl/include"

# ----------------------------------------------------------
# Build openssl for musl (arm64)
# ----------------------------------------------------------
RUN set -eux; \
    curl -LO https://www.openssl.org/source/openssl-3.1.3.tar.gz \
    && tar xf openssl-3.1.3.tar.gz \
    && cd openssl-3.1.3 \
    && ./Configure linux-aarch64 no-shared no-tests no-async no-secure-memory no-engine --openssldir=/usr/local/ssl --libdir=/usr/local/lib --prefix=/usr/local \
    && make -j$(nproc) \
    && make install_sw \
    && cd .. && rm -rf openssl-3.1.3 openssl-3.1.3.tar.gz

ENV OPENSSL_STATIC=1
ENV OPENSSL_DIR=/usr/local
ENV OPENSSL_LIB_DIR=/usr/local/lib
ENV OPENSSL_INCLUDE_DIR=/usr/local/include

# ----------------------------------------------------------
# Build OpenMP for musl (arm64) - cmake picks up CC/CXX above
# ----------------------------------------------------------
ENV OPENMP_DIR=/openmp
ENV OPENMP_PREFIX=/usr/local
ENV LLVM_SRC_URL="https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.0/llvm-project-22.1.0.src.tar.xz"

RUN mkdir -p $OPENMP_DIR
WORKDIR $OPENMP_DIR
RUN wget -q -O llvm-project-22.1.0.src.tar.xz "$LLVM_SRC_URL" \
 && tar xf llvm-project-22.1.0.src.tar.xz \
 && rm llvm-project-22.1.0.src.tar.xz

RUN CFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl -fopenmp" \
    CXXFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl -fopenmp" \
    cmake -S $OPENMP_DIR/llvm-project-22.1.0.src/openmp \
      -B /openmp/build \
      -DCMAKE_INSTALL_PREFIX=$OPENMP_PREFIX \
      -DLIBOMP_ENABLE_SHARED=OFF \
      -DLIBOMP_ENABLE_STATIC=ON \
      -DCMAKE_BUILD_TYPE=Release

RUN cmake --build /openmp/build --parallel $(nproc) --target install

# ----------------------------------------------------------
# Build static OpenBLAS for musl (arm64)
# ----------------------------------------------------------
RUN git clone --depth 1 https://github.com/xianyi/OpenBLAS.git /openblas

RUN cd /openblas && \
    set -eux; \
    make -j$(nproc) \
      HOSTCC=gcc \
      CFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl" \
      LDFLAGS="-lgfortran -lm -lpthread -lpthread" \
      USE_STATIC=1 \
      STATIC_ONLY=1 \
      NO_SHARED=1 \
      USE_OPENMP=0 \
      USE_THREAD=1 \
      TARGET=ARMV8 \
      VERBOSE=1 \
      libs netlib

RUN set -eux; \
  cd /openblas && \
  make install HOSTCC=gcc PREFIX=/usr/local STATIC_ONLY=1 NO_SHARED=1 && \
  cd / && rm -rf /openblas

ENV OPENBLAS_PATH=/usr/local
ENV BLAS_LIBRARIES=/usr/local/lib/libopenblas.a
ENV BLAS_INCLUDE_DIRS=/usr/local/include

# ----------------------------------------------------------
# Build espeak-ng musl version (arm64)
# ----------------------------------------------------------
RUN set -eux; \
    git clone --depth 1 https://github.com/espeak-ng/espeak-ng.git /espeak-ng; \
    cmake -S /espeak-ng -B /espeak-ng/build \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_CXX_FLAGS="-std=c++17" \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DCOMPILE_INTONATIONS=OFF \
      -DENABLE_TESTS=OFF \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
      -DCMAKE_SKIP_RPATH=ON \
      -DCMAKE_INSTALL_RPATH="" \
      -DCMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
      -DCMAKE_INSTALL_PREFIX=/usr/local; \
    cmake --build /espeak-ng/build -j$(nproc); \
    cmake --install /espeak-ng/build; \
    rm -rf /espeak-ng

ENV ESPEAK_NG_DIR="/usr/local/lib"

# ----------------------------------------------------------
# musl locale compatibility shim for FlatBuffers (strtoll_l)
# ----------------------------------------------------------
RUN printf "%s\n" \
"#pragma once" \
"" \
"#include <stdlib.h>" \
"#include <locale.h>" \
"" \
"#if !defined(__GLIBC__)" \
"" \
"static inline long long strtoll_l(const char* nptr, char** endptr, int base, locale_t loc) {" \
"  (void)loc;" \
"  return strtoll(nptr, endptr, base);" \
"}" \
"" \
"static inline unsigned long long strtoull_l(const char* nptr, char** endptr, int base, locale_t loc) {" \
"  (void)loc;" \
"  return strtoull(nptr, endptr, base);" \
"}" \
"" \
"#endif" \
> /usr/local/include/musl_locale_compat.h

# ----------------------------------------------------------
# protobuf, built TWICE from the same source tag:
#  1) natively (host gcc/g++, dynamic OK) -> a protoc that can actually run
#     on this amd64 host, used only for ONNX Runtime codegen below.
#  2) cross (aarch64-linux-musl, static) -> the libprotobuf.a that actually
#     gets linked into the target binary.
# ----------------------------------------------------------
RUN git clone --depth 1 -b v3.21.12 https://github.com/protocolbuffers/protobuf.git /protobuf

RUN set -eux; \
    cmake -S /protobuf -B /protobuf/build-host \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_INSTALL_PREFIX=/opt/host-protoc \
      -DCMAKE_BUILD_TYPE=Release \
      -Dprotobuf_BUILD_TESTS=OFF; \
    cmake --build /protobuf/build-host -j$(nproc); \
    cmake --install /protobuf/build-host

RUN set -eux; \
    cmake -S /protobuf -B /protobuf/build-target \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_INSTALL_PREFIX=/usr/local \
      -DCMAKE_C_COMPILER=$CC \
      -DCMAKE_CXX_COMPILER=$CXX \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DCMAKE_BUILD_TYPE=Release \
      -Dprotobuf_BUILD_TESTS=OFF \
      -Dprotobuf_BUILD_PROTOC_BINARIES=OFF; \
    cmake --build /protobuf/build-target -j$(nproc); \
    cmake --install /protobuf/build-target; \
    rm -rf /protobuf

# -----------------------------
# Build ONNX Runtime for this arch (arm64 musl, no CUDA)
# -----------------------------
ENV ONNX_DIR=/onnxruntime
ENV ONNX_SRC=/onnxruntime-src

RUN set -eux; \
    mkdir -p "$ONNX_DIR"; \
    # Pinned, not main: ORT main pins protobuf v33.6, whose generated headers
    # include google/protobuf/runtime_version.h, while this image builds and
    # links protobuf v3.21.12. v1.24.1 pins protobuf v21.12, matching, and is
    # the newest ORT API (24) that ort-sys 2.0.0-rc.12 knows about.
    git clone --depth 1 -b v1.24.1 https://github.com/microsoft/onnxruntime.git $ONNX_SRC; \
    # ORT decides between std::chrono and the vendored HowardHinnant/date
    # purely on __cplusplus >= 202002L, but GCC 11 (this musl toolchain) has
    # no std::chrono operator<< for time_point until GCC 13, so the C++20
    # branch fails to compile ostream_sink.cc. Force the date branch, which
    # ORT already supports and fetches (deps.txt pins date v3.0.1).
    sed -i "s|#define ORT_USE_CXX20_STD_CHRONO __cplusplus >= 202002L|#define ORT_USE_CXX20_STD_CHRONO 0|" \
      $ONNX_SRC/include/onnxruntime/core/common/logging/logging.h; \
    grep -q "define ORT_USE_CXX20_STD_CHRONO 0" $ONNX_SRC/include/onnxruntime/core/common/logging/logging.h; \
    mlas=$ONNX_SRC/onnxruntime/core/mlas/lib; \
    for f in activate_fp16 cast_kernel_neon dwconv eltwise_kernel_neon_fp16 \
             halfgemm_kernel_neon_fp16 hqnbitgemm_kernel_neon_fp16 pooling_fp16 \
             rotary_embedding_kernel_neon_fp16 softmax_kernel_neon_fp16; do \
      [ -f "$mlas/$f.cpp" ] && sed -i '1i #pragma GCC target("arch=armv8.2-a+fp16")' "$mlas/$f.cpp"; \
    done; \
    for f in sbconv_kernel_neon sbgemm_kernel_neon; do \
      [ -f "$mlas/$f.cpp" ] && sed -i '1i #pragma GCC target("arch=armv8.2-a+bf16")' "$mlas/$f.cpp"; \
    done; \
    sed -i '1i #pragma GCC target("arch=armv8.2-a+i8mm")' $mlas/sqnbitgemm_kernel_neon_int8_i8mm.cpp; \
    sed -i '1i #pragma GCC target("arch=armv8.2-a+dotprod")' $mlas/sqnbitgemm_kernel_neon_int8.cpp; \
    head -1 $mlas/sqnbitgemm_kernel_neon_int8_i8mm.cpp | grep -q i8mm; \
    head -1 $mlas/sbgemm_kernel_neon.cpp | grep -q bf16

WORKDIR $ONNX_SRC

# execinfo.h (backtrace) isn't available under musl.
RUN find . -type f -print0 | xargs -0 -r sed -i "/#include <execinfo\.h>/d"

RUN mkdir -p build
WORKDIR $ONNX_SRC/build

RUN set -eux; \
    cmake ../cmake \
      -B $ONNX_DIR \
      -DCMAKE_SYSTEM_NAME=Linux \
      -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
      -DCMAKE_CXX_STANDARD=20 \
      -DCMAKE_CXX_STANDARD_REQUIRED=ON \
      -DCMAKE_C_FLAGS="-include /usr/local/include/musl_locale_compat.h" \
      -DCMAKE_CXX_FLAGS="-std=c++17 -include /usr/local/include/musl_locale_compat.h" \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_C_COMPILER=$CC \
      -DCMAKE_CXX_COMPILER=$CXX \
      -DCMAKE_LINKER=$LD \
      -DCMAKE_COMPILE_WARNING_AS_ERROR=OFF \
      -Donnxruntime_BUILD_UNIT_TESTS=OFF \
      -Donnxruntime_ENABLE_EXTERNAL_CUSTOM_OP_SCHEMAS=OFF \
      -Donnxruntime_RUN_ONNX_TESTS=OFF \
      -DPython_EXECUTABLE=/usr/bin/python3 \
      -Donnxruntime_USE_VCPKG=OFF \
      -Donnxruntime_USE_MIMALLOC=OFF \
      -Donnxruntime_ENABLE_PYTHON=OFF \
      -Donnxruntime_BUILD_CSHARP=OFF \
      -Donnxruntime_BUILD_JAVA=OFF \
      -Donnxruntime_BUILD_NODEJS=OFF \
      -Donnxruntime_BUILD_OBJC=OFF \
      -Donnxruntime_BUILD_SHARED_LIB=OFF \
      -Donnxruntime_BUILD_APPLE_FRAMEWORK=OFF \
      -Donnxruntime_USE_DNNL=OFF \
      -Donnxruntime_USE_NNAPI_BUILTIN=OFF \
      -Donnxruntime_USE_VSINPU=OFF \
      -Donnxruntime_USE_RKNPU=OFF \
      -Donnxruntime_USE_VITISAI=OFF \
      -Donnxruntime_USE_TENSORRT=OFF \
      -Donnxruntime_USE_NV=OFF \
      -Donnxruntime_USE_TENSORRT_BUILTIN_PARSER=ON \
      -Donnxruntime_USE_TENSORRT_INTERFACE=OFF \
      -Donnxruntime_USE_CUDA_INTERFACE=OFF \
      -Donnxruntime_USE_NV_INTERFACE=OFF \
      -Donnxruntime_USE_OPENVINO_INTERFACE=OFF \
      -Donnxruntime_USE_VITISAI_INTERFACE=OFF \
      -Donnxruntime_USE_QNN_INTERFACE=OFF \
      -Donnxruntime_USE_MIGRAPHX_INTERFACE=OFF \
      -Donnxruntime_USE_MIGRAPHX=OFF \
      -Donnxruntime_DISABLE_RTTI=OFF \
      -Donnxruntime_DISABLE_EXCEPTIONS=OFF \
      -Donnxruntime_MINIMAL_BUILD=OFF \
      -Donnxruntime_ENABLE_LTO=OFF \
      -Donnxruntime_USE_ACL=OFF \
      -Donnxruntime_USE_ARMNN=OFF \
      -Donnxruntime_USE_JSEP=OFF \
      -Donnxruntime_USE_WEBGPU=OFF \
      -Donnxruntime_USE_EXTERNAL_DAWN=OFF \
      -Donnxruntime_WGSL_TEMPLATE=static \
      -Donnxruntime_ENABLE_TRAINING=OFF \
      -Donnxruntime_ENABLE_TRAINING_OPS=OFF \
      -Donnxruntime_ENABLE_TRAINING_APIS=OFF \
      -Donnxruntime_ENABLE_CPU_FP16_OPS=OFF \
      -Donnxruntime_USE_NCCL=OFF \
      -Donnxruntime_BUILD_BENCHMARKS=OFF \
      -Donnxruntime_USE_XNNPACK=OFF \
      -Donnxruntime_USE_WEBNN=OFF \
      -Donnxruntime_USE_CANN=OFF \
      -Donnxruntime_USE_CUDA=OFF \
      -Donnxruntime_USE_KLEIDIAI=OFF \
      -DCMAKE_INSTALL_PREFIX=$ONNX_DIR \
      -DCMAKE_BUILD_TYPE=Release \
      -Donnxruntime_USE_SYSTEM_PROTOBUF=ON \
      -DProtobuf_INCLUDE_DIR=/usr/local/include \
      -DProtobuf_LIBRARIES=/usr/local/lib/libprotobuf.a \
      -DProtobuf_PROTOC_EXECUTABLE=/opt/host-protoc/bin/protoc; \
    cmake --build $ONNX_DIR --config Release

ENV ORT_STRATEGY=system
ENV ORT_LIB_LOCATION=$ONNX_DIR

WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux arm64 build (Docker image, cross-compiled on linux/amd64) =="
  if docker image inspect "$img" >/dev/null 2>&1; then
    echo "Docker image '$img' already exists. Skipping build."
  else
    docker build "${build_args[@]}" --platform=linux/amd64 \
      --build-arg CACHE_BUST="${CACHE_BUST}" \
      -f "$df" -t "$img" "$tmp"
  fi

  echo "== Linux arm64 cargo builds (cpu=${WITH_CPU} vulkan=${WITH_VULKAN}) =="
  docker run --rm --platform=linux/amd64 \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -v "${HOST_K_CACHE}:${CONT_K_CACHE}" \
    -v "${HOST_WHISPER_MODELS}:${CONT_WHISPER_MODELS}" \
    -e WITH_CPU="${WITH_CPU}" \
    -e WITH_VULKAN="${WITH_VULKAN}" \
    -e CMAKE_SKIP_RPATH=ON \
    -e CMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
    "$img" \
    bash -lc '
      set -euo pipefail

      ARCH=arm64
      target=aarch64-unknown-linux-musl

      # rust-toolchain.toml pins the toolchain, but the image added the musl
      # target to whatever was default at image build time (stable). Cargo
      # switches to the pinned toolchain here and then finds no musl std.
      # Re-add the target from inside /work so rustup reads rust-toolchain.toml.
      cd /work
      rustup target add "$target"


      if [ ! -f /usr/local/lib/libasound.a ]; then
        echo "--- Building ALSA static library for musl ---"
        apt-get update -qq && apt-get install -y --no-install-recommends autoconf automake libtool
        curl -sL -o /tmp/alsa.tar.gz \
          "https://github.com/alsa-project/alsa-lib/archive/refs/tags/v1.2.12.tar.gz"
        tar xzf /tmp/alsa.tar.gz -C /tmp
        mv /tmp/alsa-lib-1.2.12 /tmp/alsa-lib
        cd /tmp/alsa-lib
        autoreconf -fi
        ./configure \
          --host=aarch64-linux-musl \
          --build=x86_64-linux-gnu \
          --prefix=/usr/local \
          --enable-shared=no \
          --enable-static=yes \
          --with-pkg-config-plugindir=/usr/local/lib/pkgconfig \
          CC=aarch64-linux-musl-gcc \
          CFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl -O3" \
          LDFLAGS="-L/opt/aarch64-linux-musl-cross/aarch64-linux-musl/lib"
        make -j$(nproc)
        make install
        cd /
        rm -rf /tmp/alsa-lib /tmp/alsa.tar.gz
      else
        echo "--- ALSA already built, skipping ---"
      fi

      if [ ! -f /usr/local/lib/libre2.a ]; then
        echo "--- Building abseil + re2 static libraries for musl ---"
        curl -sL -o /tmp/re2.tar.gz \
          "https://github.com/google/re2/archive/refs/tags/2024-07-02.tar.gz"
        tar xzf /tmp/re2.tar.gz -C /tmp
        mv /tmp/re2-2024-07-02 /tmp/re2

        curl -sL -o /tmp/absl.tar.gz \
          "https://github.com/abseil/abseil-cpp/archive/refs/tags/20240722.0.tar.gz"
        tar xzf /tmp/absl.tar.gz -C /tmp
        mv /tmp/abseil-cpp-20240722.0 /tmp/absl
        cmake -S /tmp/absl -B /tmp/absl-build \
          -DCMAKE_INSTALL_PREFIX=/usr/local \
          -DCMAKE_BUILD_TYPE=Release \
          -DBUILD_SHARED_LIBS=OFF \
          -DCMAKE_SYSTEM_NAME=Linux \
          -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
          -DCMAKE_C_COMPILER=aarch64-linux-musl-gcc \
          -DCMAKE_CXX_COMPILER=aarch64-linux-musl-g++ \
          -DCMAKE_CXX_FLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl" \
          -DCMAKE_C_FLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl" \
          -DABSL_BUILD_TESTING=OFF
        cmake --build /tmp/absl-build --parallel $(nproc)
        cmake --install /tmp/absl-build --prefix /usr/local

        make -C /tmp/re2 -j$(nproc) \
          CXX=aarch64-linux-musl-g++ \
          CXXFLAGS="--sysroot=/opt/aarch64-linux-musl-cross/aarch64-linux-musl -O3 -static -I/usr/local/include" \
          LDFLAGS="-static -L/usr/local/lib" \
          AR=aarch64-linux-musl-ar \
          static
        cp /tmp/re2/obj/libre2.a /usr/local/lib/
        mkdir -p $ONNX_DIR/_deps/re2-build/
        cp /usr/local/lib/libre2.a $ONNX_DIR/_deps/re2-build/
        rm -rf /tmp/re2 /tmp/re2.tar.gz /tmp/absl /tmp/absl.tar.gz /tmp/absl-build
      else
        echo "--- abseil/re2 already built, skipping ---"
      fi

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-${ARCH}-${variant}"

        echo "---- Building linux/${ARCH} [$variant] features: $feats"
        export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
        export CARGO_PROFILE_RELEASE_DEBUG=false
        export CARGO_PROFILE_RELEASE_STRIP=symbols
        export CARGO_PROFILE_RELEASE_INCREMENTAL=false
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-musl-g++
        export RUSTC_LINKER=aarch64-linux-musl-g++

        ABSL_LIBS=""
        for f in /usr/local/lib/libabsl_*.a; do
          ABSL_LIBS="$ABSL_LIBS -C link-arg=$f"
        done

        export RUSTFLAGS="-C target-feature=+crt-static -C codegen-units=1 -C opt-level=3 -C link-arg=-L/opt/aarch64-linux-musl-cross/aarch64-linux-musl/lib -C link-arg=-Wl,--start-group -C link-arg=/usr/local/lib/libopenblas.a -C link-arg=/usr/local/lib/libprotobuf.a -C link-arg=/usr/local/lib/libomp.a ${ABSL_LIBS} -C link-arg=-Wl,--end-group -C link-arg=-lm -C link-arg=-lc -C link-arg=-lgfortran -C link-arg=-lpthread -C link-arg=-lgcc"

        cd /work
        CARGO_TARGET_DIR="$ctd" \
        cargo build --release --target "$target" --features "$feats"
      }

      # Scope the cross toolchain to the target triple for the cargo build and
      # take it out of the generic environment. The image-wide CC/CFLAGS are
      # needed by the C library builds above, but cargo also compiles build
      # scripts and their deps for the HOST: with a global CC=aarch64-... ,
      # openssl-sys emitted aarch64 objects into the host artifact dir and the
      # host link failed ("Relocations in generic ELF (EM: 183) ... file in
      # wrong format"); a global CFLAGS handed host gcc the aarch64 sysroot so
      # it could not find /usr/include (ring: "no include path ... stdint.h").
      # Target builds still resolve CC_<target>/CFLAGS_<target>.
      t_="$(echo "$target" | tr - _)"
      T_="$(echo "$target" | tr "a-z-" "A-Z_")"
      export CFLAGS_${t_}="$CFLAGS"
      export CXXFLAGS_${t_}="$CXXFLAGS"
      export LDFLAGS_${t_}="$LDFLAGS"
      # OPENSSL_DIR/PKG_CONFIG_PATH point at the aarch64 OpenSSL in /usr/local.
      # openssl-sys reads them for HOST builds too, linking aarch64 libssl.a
      # into a host rlib -> "Relocations in generic ELF (EM: 183) ... file in
      # wrong format" when ort-sys built its build script. Scope them to the
      # target; the host then uses the system OpenSSL from libssl-dev.
      export ${T_}_OPENSSL_DIR="$OPENSSL_DIR"
      export ${T_}_OPENSSL_LIB_DIR="$OPENSSL_LIB_DIR"
      export ${T_}_OPENSSL_STATIC="$OPENSSL_STATIC"
      export PKG_CONFIG_PATH_${t_}="$PKG_CONFIG_PATH"
      export PKG_CONFIG_ALLOW_CROSS=1
      unset CFLAGS CXXFLAGS LDFLAGS CC CXX LD
      unset OPENSSL_DIR OPENSSL_LIB_DIR OPENSSL_STATIC PKG_CONFIG_PATH

      if [ "${WITH_CPU}" = "1" ]; then
        build_variant cpu "'"${FEATURES_CPU}"'"
      fi

      if [ "${WITH_VULKAN}" = "1" ]; then
        build_variant vulkan "'"${FEATURES_VULKAN}"'"
      fi
    '

  [[ "${WITH_CPU}" == "1" ]] && linux_copy_out "arm64" "aarch64-unknown-linux-musl" "cpu"
  [[ "${WITH_VULKAN}" == "1" ]] && linux_copy_out "arm64" "aarch64-unknown-linux-musl" "vulkan"
  true

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

# ==========================================================
# GLIBC variants (vulkan, cuda) - amd64 and arm64
#
# These CANNOT be musl. The Vulkan loader, its ICD drivers and NVIDIA's
# libcuda.so.1 are glibc binaries on every mainstream distro, and a musl
# process cannot load them: statically it has no dlopen at all
# ("Dynamic loading not supported"), and dynamically the glibc .so fails to
# relocate ("__strncpy_chk: symbol not found"). So the GPU variants are built
# against glibc, with everything of ours still linked statically - only libc,
# the GPU loaders, and ONNX Runtime stay dynamic.
#
# ONNX Runtime comes from Microsoft's prebuilt package rather than source:
# it removes a multi-hour build, and for CUDA it is the only practical option
# (the from-source CUDA EP build does not fit CI time limits at all).
# ==========================================================
build_linux_glibc_variant() {
  local arch="$1" variant="$2"
  local tmp df img target ort_url ort_dir

  local plat multiarch
  case "${arch}" in
    amd64) target="x86_64-unknown-linux-gnu";  plat="linux/amd64"
           multiarch="x86_64-linux-gnu" ;;
    # No cross toolchain here: this leg runs on a native arm64 runner, so the
    # image and the build are both aarch64.
    arm64) target="aarch64-unknown-linux-gnu"; plat="linux/arm64"
           multiarch="aarch64-linux-gnu" ;;
    *) echo "ERROR: unsupported glibc arch ${arch}"; return 1 ;;
  esac

  # CUDA 12 to match Ubuntu's nvidia-cuda-toolkit, so the ORT package and the
  # ggml/whisper CUDA code agree on a major version.
  case "${arch}-${variant}" in
    amd64-vulkan) ort_url="https://github.com/microsoft/onnxruntime/releases/download/v1.24.1/onnxruntime-linux-x64-1.24.1.tgz";           ort_dir="onnxruntime-linux-x64-1.24.1" ;;
    arm64-vulkan) ort_url="https://github.com/microsoft/onnxruntime/releases/download/v1.24.1/onnxruntime-linux-aarch64-1.24.1.tgz";       ort_dir="onnxruntime-linux-aarch64-1.24.1" ;;
    amd64-cuda)   ort_url="https://github.com/microsoft/onnxruntime/releases/download/v1.24.1/onnxruntime-linux-x64-gpu-1.24.1.tgz";       ort_dir="onnxruntime-linux-x64-gpu-1.24.1" ;;
    *) echo "ERROR: no prebuilt ONNX Runtime for ${arch}-${variant}"; return 1 ;;
  esac

  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.glibc.${arch}.${variant}"
  img="local/${BIN_NAME}-linux-glibc-${arch}-${variant}:cache"

  cat > "$df" <<'DOCKERFILE'
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
ARG ARCH
ARG VARIANT
ARG ORT_URL
ARG ORT_DIR

# noble, not jammy: ggml's Vulkan backend needs glslc to compile its shaders
# and 22.04 ships only glslang-tools. The cost is a glibc 2.39 floor.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl wget ca-certificates git \
      cmake python3 gfortran \
      libssl-dev zlib1g-dev libasound2-dev \
      libopenblas-dev libclang-dev clang \
      libvulkan-dev glslc \
 && rm -rf /var/lib/apt/lists/*

RUN if [ "$VARIANT" = "cuda" ]; then \
      apt-get update && apt-get install -y --no-install-recommends nvidia-cuda-toolkit && \
      rm -rf /var/lib/apt/lists/* ; \
    fi

# cuDNN for the ONNX Runtime CUDA execution provider - not part of the toolkit.
ARG CUDNN_VERSION=9.16.0.29
RUN if [ "$VARIANT" = "cuda" ]; then \
      set -eux; \
      f=cudnn-linux-x86_64-${CUDNN_VERSION}_cuda12-archive; \
      wget -nv --timeout=60 --tries=3 -O /tmp/cudnn.tar.xz \
        "https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/linux-x86_64/$f.tar.xz"; \
      mkdir -p /usr/local/cudnn; \
      tar -xJf /tmp/cudnn.tar.xz -C /tmp; \
      cp -a /tmp/$f/include /usr/local/cudnn/; \
      cp -a /tmp/$f/lib /usr/local/cudnn/; \
      rm -rf /tmp/cudnn.tar.xz /tmp/$f; \
      test -f /usr/local/cudnn/include/cudnn.h; \
    fi
ENV CUDNN_PATH=/usr/local/cudnn

# Prebuilt ONNX Runtime (shared). Its .so files ship beside the binary.
RUN set -eux; \
    wget -nv --timeout=60 --tries=3 -O /tmp/ort.tgz "$ORT_URL"; \
    mkdir -p /opt; \
    tar xzf /tmp/ort.tgz -C /opt; \
    rm -f /tmp/ort.tgz; \
    ln -sfn "/opt/${ORT_DIR}" /opt/onnxruntime; \
    test -f /opt/onnxruntime/lib/libonnxruntime.so
ENV ORT_STRATEGY=system
ENV ORT_LIB_LOCATION=/opt/onnxruntime/lib
ENV ORT_PREFER_DYNAMIC_LINK=1
ENV ONNXRUNTIME_INCLUDE_DIR=/opt/onnxruntime/include
ENV ONNXRUNTIME_LIB_DIR=/opt/onnxruntime/lib

# whisper-rs-sys requires BLAS_INCLUDE_DIRS when built with OpenBLAS, and the
# Debian layout puts the headers in the multiarch directory.
#
# The static archive is copied somewhere that holds no .so: apt ships both
# libopenblas.a and libopenblas.so in the same directory, and the plain
# "-lopenblas" that whisper-rs-sys emits would otherwise resolve to the shared
# one. A -L path is searched before the default directories, so this keeps
# OpenBLAS statically linked.
ARG MULTIARCH
RUN set -eux; \
    mkdir -p /opt/blas-static; \
    cp "/usr/lib/${MULTIARCH}/libopenblas.a" /opt/blas-static/; \
    test -f /usr/include/${MULTIARCH}/cblas.h
ENV BLAS_INCLUDE_DIRS=/usr/include/${MULTIARCH}
ENV BLAS_LIBRARIES=/opt/blas-static/libopenblas.a
ENV OPENBLAS_PATH=/usr

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH=/root/.cargo/bin:$PATH

RUN d="$(dirname "$(find /usr/lib -name 'libclang.so*' 2>/dev/null | head -1)")"; \
    test -n "$d" -a -d "$d"; \
    ln -sfn "$d" /usr/local/libclang
ENV LIBCLANG_PATH=/usr/local/libclang

WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux ${arch} ${variant} (glibc) image =="
  docker build "${build_args[@]}" --platform="${plat}" \
    --build-arg ARCH="${arch}" \
    --build-arg VARIANT="${variant}" \
    --build-arg ORT_URL="${ort_url}" \
    --build-arg ORT_DIR="${ort_dir}" \
    --build-arg MULTIARCH="${multiarch}" \
    -f "$df" -t "$img" "$tmp"

  local feats
  case "${variant}" in
    vulkan) feats="${FEATURES_VULKAN}" ;;
    cuda)   feats="${FEATURES_CUDA}" ;;
  esac

  echo "== Linux ${arch} ${variant} (glibc) cargo build =="
  docker run --rm --platform="${plat}" \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -v "${HOST_K_CACHE}:${CONT_K_CACHE}" \
    -v "${HOST_WHISPER_MODELS}:${CONT_WHISPER_MODELS}" \
    -e TARGET="${target}" \
    -e FEATS="${feats}" \
    -e ARCH="${arch}" \
    -e VARIANT="${variant}" \
    "$img" \
    bash -lc '
      set -euo pipefail
      cd /work
      rustup target add "$TARGET"

      ctd="/work/target-cross/linux-${ARCH}-${VARIANT}"

      # Not +crt-static: this binary must be able to dlopen the Vulkan/CUDA
      # loaders the user installs. Our own libraries still link statically;
      # $ORIGIN lets it find the ONNX Runtime .so shipped alongside it.
      export RUSTFLAGS="-C codegen-units=1 -C opt-level=3 \
        -C link-arg=-Wl,-rpath,\$ORIGIN \
        -C link-arg=-L/opt/onnxruntime/lib \
        -L native=/opt/blas-static \
        -C link-arg=-lgfortran"

      export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
      export CARGO_PROFILE_RELEASE_DEBUG=false
      export CARGO_PROFILE_RELEASE_STRIP=symbols
      export CARGO_PROFILE_RELEASE_INCREMENTAL=false

      CARGO_TARGET_DIR="$ctd" cargo build --release --target "$TARGET" --features "$FEATS"
    '

  # binary + the ONNX Runtime shared libraries it needs at run time
  local src_dir="${PROJECT_ROOT}/target-cross/linux-${arch}-${variant}/${target}/release"
  local out_dir="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-${arch}-${variant}-glibc"
  if [[ -f "${src_dir}/${BIN_NAME}" ]]; then
    rm -rf "${out_dir}"; mkdir -p "${out_dir}"
    cp "${src_dir}/${BIN_NAME}" "${out_dir}/"
    chmod +x "${out_dir}/${BIN_NAME}" || true
    docker run --rm --platform="${plat}" -v "${out_dir}:/out" "$img" \
      bash -lc 'cp -a /opt/onnxruntime/lib/libonnxruntime*.so* /out/ 2>/dev/null || true'
    add_artifact "${out_dir}"
    echo "✔ Built: ${out_dir}"
  else
    echo "ERROR: ${src_dir}/${BIN_NAME} not produced"
    return 1
  fi

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
}

# -----------------------------
# Run builds
# -----------------------------
ensure_espeak_data_archive

# musl covers cpu only. vulkan/cuda cannot be musl - the GPU loaders the user
# installs are glibc - so they go through build_linux_glibc_variant.
_wv="${WITH_VULKAN}"; _wc="${WITH_CUDA}"
WITH_VULKAN=0; WITH_CUDA=0
if [[ "${WITH_CPU}" == "1" ]]; then
  if want_arch amd64; then build_linux_amd64_variants; fi
  if want_arch arm64; then build_linux_arm64_variants; fi
fi
WITH_VULKAN="${_wv}"; WITH_CUDA="${_wc}"

if [[ "${WITH_VULKAN}" == "1" ]]; then
  want_arch amd64 && build_linux_glibc_variant amd64 vulkan
  want_arch arm64 && build_linux_glibc_variant arm64 vulkan
fi
if [[ "${WITH_CUDA}" == "1" ]]; then
  want_arch amd64 && build_linux_glibc_variant amd64 cuda
fi
true

# -----------------------------
# Check static build
# -----------------------------
for f in dist/${BIN_NAME}-*-linux-*; do
  [[ -f "$f" ]] || continue
  echo "Checking $f"

  if ldd "$f" 2>&1 | grep -q "not a dynamic"; then
    echo "✔ Statically linked (ldd says not a dynamic ELF)"
  else
    echo "ldd output (libvulkan.so.1 is expected/allowed on vulkan builds):"
    ldd "$f" || true
  fi

  if nm "$f" 2>/dev/null | grep -q "openblas"; then
    echo "✔ OpenBLAS symbols found (static link confirmed)"
  else
    echo "⚠ No OpenBLAS symbols found in $f"
  fi

  echo "---------------------------------"
done

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
