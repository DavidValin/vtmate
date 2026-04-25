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

  local tmp df img CACHE_BUST
  tmp="$(mktemp -d)"
  df="${tmp}/Dockerfile.linux.amd64"
  FIXED_IMG="local/${BIN_NAME}-linux-amd64:cache"
  img="$FIXED_IMG"
  # img="local/${BIN_NAME}-linux-amd64:${VERSION}"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<'DOCKERFILE'
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST

# Variant config
# ----------------------------------------------------------
ARG WITH_BLAS=1
ARG WITH_CUDA=0
ARG WITH_ROCM=0
ARG WITH_VULKAN=0

ENV WITH_BLAS=${WITH_BLAS}
ENV WITH_CUDA=${WITH_CUDA}
ENV WITH_ROCM=${WITH_ROCM}
ENV WITH_VULKAN=${WITH_VULKAN}

# ----------------------------------------------------------
# Build dependencies
# ----------------------------------------------------------
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev musl-tools gcc-x86-64-linux-gnu g++-x86-64-linux-gnu \
  libclang-dev llvm-dev clang llvm \
  curl wget ca-certificates git \
  gfortran \
  zlib1g-dev libbz2-dev liblzma-dev \
  cmake \
  libasound2-dev \
  protobuf-compiler libprotobuf-dev \
  python3 python3-pip \
  perl \
  glslc \
&& rm -rf /var/lib/apt/lists/*

RUN if [ "$WITH_CUDA" = "1" ]; then \
    apt-get update && apt-get install -y --no-install-recommends nvidia-cuda-toolkit && \
    rm -rf /var/lib/apt/lists/* ; \
  fi

RUN if [ "$WITH_ROCM" = "1" ]; then \
    apt-get update && apt-get install -y --no-install-recommends rocm-hip-sdk hipblas rocblas && \
    rm -rf /var/lib/apt/lists/* ; \
  fi

# install musl g++ linker
# ----------------------------------------------------------
RUN wget https://musl.cc/x86_64-linux-musl-cross.tgz
RUN tar xvf x86_64-linux-musl-cross.tgz -C /opt/
ENV PATH=/opt/x86_64-linux-musl-cross/bin:$PATH

# Install Rust and add MUSL target
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
ENV CMAKE_SYSTEM_NAME=Linux
ENV CMAKE_C_COMPILER=/opt/x86_64-linux-musl-cross/bin/x86_64-linux-musl-gcc
ENV CMAKE_CXX_COMPILER=/opt/x86_64-linux-musl-cross/bin/x86_64-linux-musl-g++
ENV CMAKE_FIND_LIBRARY_SUFFIXES=".a"
ENV CMAKE_EXE_LINKER_FLAGS=-static
ENV BINDGEN_EXTRA_CLANG_ARGS="-I/opt/x86_64-linux-musl-cross/x86_64-linux-musl/include"

# ----------------------------------------------------------
# Build openssl for musl (amd64)
# ----------------------------------------------------------
RUN set -eux; \
    curl -LO https://www.openssl.org/source/openssl-3.1.3.tar.gz \
    && tar xvf openssl-3.1.3.tar.gz \
    && cd openssl-3.1.3 \
    && ./Configure linux-x86_64 no-shared no-tests no-async no-secure-memory no-engine --openssldir=/usr/local/ssl --libdir=/usr/local/lib --prefix=/usr/local \
    && make -j$(nproc) \
    && make install

ENV OPENSSL_STATIC=1
ENV OPENSSL_DIR=/usr/local
ENV OPENSSL_LIB_DIR=/usr/local/lib
ENV OPENSSL_INCLUDE_DIR=/usr/local/include

# ----------------------------------------------------------
# Build OpenMP for musl (amd64)
# ----------------------------------------------------------
ENV OPENMP_DIR=/openmp
ENV OPENMP_PREFIX=/usr/local

ENV CFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -fopenmp"
ENV CXXFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -fopenmp"

ENV LLVM_SRC_URL="https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.0/llvm-project-22.1.0.src.tar.xz"

WORKDIR $OPENMP_DIR
RUN wget -O llvm-project-22.1.0.src.tar.xz "$LLVM_SRC_URL" \
 && tar xf llvm-project-22.1.0.src.tar.xz

# Set correct OpenMP source paths
WORKDIR $OPENMP_DIR/llvm-project-22.1.0.src/openmp

RUN mkdir -p /openmp/build

RUN cmake -S $OPENMP_DIR/llvm-project-22.1.0.src/openmp \
  -B /openmp/build \
  -DCMAKE_INSTALL_PREFIX=$OPENMP_PREFIX \
  -DLIBOMP_ENABLE_SHARED=OFF \
  -DLIBOMP_ENABLE_STATIC=ON \
  -DCMAKE_BUILD_TYPE=Release

RUN cmake --build /openmp/build --parallel $(nproc) --target install

RUN echo "✔ OpenMP static library built!" \
 && echo "Library: $OPENMP_PREFIX/lib/libomp.a" \
 && echo "Headers: $OPENMP_PREFIX/include/"

# ----------------------------------------------------------
# Build static OpenBLAS for musl (amd64)
# ----------------------------------------------------------
RUN git clone --depth 1 https://github.com/xianyi/OpenBLAS.git /openblas

RUN cd /openblas && \
  set -eux; \
  make -j$(nproc) \
    CFLAGS="--sysroot=/opt/x86_64-linux-musl-cross/x86_64-linux-musl -fopenmp" \
    LDFLAGS="-lgfortran -lm -lpthread -lquadmath -lgomp -lpthread" \
    USE_STATIC=1 \
    STATIC_ONLY=1 \
    NO_SHARED=1 \
    USE_OPENMP=1 \
    USE_THREAD=1 \
    TARGET=GENERIC \
    NO_AVX=1 \
    VERBOSE=1 \
    2>&1 | tee /tmp/openblas_build.log

RUN set -eux; \
  cd /openblas && \
  make install PREFIX=/usr/local STATIC_ONLY=1 NO_SHARED=1 && \
  cd / && rm -rf /openblas

ENV OPENBLAS_PATH=/usr/local
ENV BLAS_LIBRARIES=/usr/local/lib/libopenblas.a
ENV BLAS_INCLUDE_DIRS=/usr/local/include

# ----------------------------------------------------------
# Build espeak-ng musl version (AMD64)
# ----------------------------------------------------------
RUN set -eux; \
    git clone --depth 1 https://github.com/espeak-ng/espeak-ng.git /espeak-ng; \
    cd /espeak-ng; \
    mkdir build && cd build

RUN set -eux; \
    cmake -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_CXX_FLAGS="-std=c++17" \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
      -DCMAKE_SKIP_RPATH=ON \
      -DCMAKE_INSTALL_RPATH="" \
      -DCMAKE_INSTALL_RPATH_USE_LINK_PATH=OFF \
      -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER \
      -DCMAKE_FIND_ROOT_PATH_MODE_LIBRARY=ONLY \
      -DCMAKE_FIND_ROOT_PATH_MODE_INCLUDE=ONLY \
      -DCMAKE_FIND_ROOT_PATH_MODE_PACKAGE=ONLY \
      -DCMAKE_INSTALL_PREFIX=/usr/local

RUN set -eux; \
    make -j$(nproc) && \
    make install

ENV ESPEAK_NG_DIR="/usr/local/lib"

# --------------------------------------------------
# Build static whisper.cpp + ggml (linked to OpenBLAS)
# --------------------------------------------------

RUN git clone --depth 1 https://github.com/ggerganov/whisper.cpp.git /whisper.cpp

RUN set -eux; \
    mkdir -p /whisper.cpp/build; \
    cd /whisper.cpp/build; \
    cmake .. \
        -DBUILD_SHARED_LIBS=OFF \
        -DGGML_OPENMP=ON \
        -DGGML_BLAS=ON \
        -DGGML_BLAS_STATIC=ON \
        -DGGML_BLAS_VENDOR=OpenBLAS \
        -DBLAS_LIBRARIES=$BLAS_LIBRARIES \
        -DBLAS_INCLUDE_DIRS=$BLAS_INCLUDE_DIRS \
        -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
        -DCMAKE_BUILD_TYPE=Release; \
    cmake --build . --config Release; \
    cmake --install . --prefix /usr/local

ENV WHISPER_PREBUILT_LIB=/usr/local/lib

# --------------------------------------------------
# Build protobuf musl version (AMD64 musl)
# --------------------------------------------------
RUN mkdir -p /protoc
WORKDIR /protoc

RUN set -eux; \
    git clone -b v3.21.12 https://github.com/protocolbuffers/protobuf.git; \
    cd protobuf; \
    mkdir build && cd build; \
    cmake .. \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_INSTALL_PREFIX=/usr/local \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DCMAKE_BUILD_TYPE=Release \
      -Dprotobuf_BUILD_TESTS=OFF; \
    make -j$(nproc); \
    make install


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


# -----------------------------
# Build ONNX Runtime for this variant (AMD64 musl)
# -----------------------------

ENV ONNX_DIR=/onnxruntime
ENV ONNX_SRC=/onnxruntime-src

RUN set -eux; \
    mkdir -p "$ONNX_DIR"; \
    git clone --depth 1 https://github.com/microsoft/onnxruntime.git $ONNX_SRC

WORKDIR $ONNX_SRC

# HACK: patch ocurrences of #include <execinfo.h> since is only used for backtrace and unsupported in musl
RUN set -eux; \
    find . -type f \( -name "*" \) -print0 | xargs -0 -r sed -i "/#include <execinfo\.h>/d"

RUN mkdir -p build
WORKDIR $ONNX_SRC/build

RUN set -eux; \
    cmake ../cmake \
      -B $ONNX_DIR \
      -DCMAKE_SYSTEM_PROCESSOR=AMD64 \
      -DCMAKE_CXX_STANDARD=17 \
      -DCMAKE_CXX_STANDARD_REQUIRED=ON \
      -DCMAKE_C_FLAGS="-march=x86-64 -include /usr/local/include/musl_locale_compat.h" \
      -DCMAKE_CXX_FLAGS="-march=x86-64 -std=c++17 -include /usr/local/include/musl_locale_compat.h" \
      -DCMAKE_EXE_LINKER_FLAGS="-static" \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_C_COMPILER=$CC \
      -DCMAKE_CXX_COMPILER=$CXX \
      -DCMAKE_LINKER=$LD \
      -DCMAKE_LDFLAGS="-lgfortran -lm -lpthread -lquadmath -lgomp -lpthread" \
      -DCMAKE_COMPILE_WARNING_AS_ERROR=OFF \
      -Donnxruntime_BUILD_UNIT_TESTS=OFF \
      -Donnxruntime_ENABLE_EXTERNAL_CUSTOM_OP_SCHEMAS=OFF \
      -Donnxruntime_RUN_ONNX_TESTS=OFF \
      -Donnxruntime_GENERATE_TEST_REPORTS=ON \
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
      -Donnxruntime_ENABLE_MICROSOFT_INTERNAL=OFF \
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
      -Donnxruntime_DISABLE_CONTRIB_OPS=OFF \
      -Donnxruntime_DISABLE_ML_OPS=OFF \
      -Donnxruntime_DISABLE_GENERATION_OPS=OFF \
      -Donnxruntime_DISABLE_RTTI=OFF \
      -Donnxruntime_DISABLE_EXCEPTIONS=OFF \
      -Donnxruntime_MINIMAL_BUILD=OFF \
      -Donnxruntime_EXTENDED_MINIMAL_BUILD=OFF \
      -Donnxruntime_MINIMAL_BUILD_CUSTOM_OPS=OFF \
      -Donnxruntime_REDUCED_OPS_BUILD=OFF \
      -Donnxruntime_CLIENT_PACKAGE_BUILD=OFF \
      -Donnxruntime_BUILD_MS_EXPERIMENTAL_OPS=OFF \
      -Donnxruntime_ENABLE_LTO=OFF \
      -Donnxruntime_USE_ACL=OFF \
      -Donnxruntime_USE_ARMNN=OFF \
      -Donnxruntime_ARMNN_RELU_USE_CPU=ON \
      -Donnxruntime_ARMNN_BN_USE_CPU=ON \
      -Donnxruntime_USE_JSEP=OFF \
      -Donnxruntime_USE_WEBGPU=OFF \
      -Donnxruntime_USE_EXTERNAL_DAWN=OFF \
      -Donnxruntime_WGSL_TEMPLATE=static \
      -Donnxruntime_ENABLE_NVTX_PROFILE=OFF \
      -Donnxruntime_ENABLE_TRAINING=OFF \
      -Donnxruntime_ENABLE_TRAINING_OPS=OFF \
      -Donnxruntime_ENABLE_TRAINING_APIS=OFF \
      -Donnxruntime_ENABLE_CPU_FP16_OPS=OFF \
      -Donnxruntime_USE_NCCL=OFF \
      -Donnxruntime_BUILD_BENCHMARKS=OFF \
      -Donnxruntime_GCOV_COVERAGE=OFF \
      -Donnxruntime_ENABLE_MEMORY_PROFILE=OFF \
      -Donnxruntime_ENABLE_CUDA_LINE_NUMBER_INFO=OFF \
      -Donnxruntime_USE_CUDA_NHWC_OPS=OFF \
      -Donnxruntime_BUILD_WEBASSEMBLY_STATIC_LIB=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_EXCEPTION_CATCHING=ON \
      -Donnxruntime_ENABLE_WEBASSEMBLY_API_EXCEPTION_CATCHING=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_EXCEPTION_THROWING=ON \
      -Donnxruntime_WEBASSEMBLY_RUN_TESTS_IN_BROWSER=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_JSPI=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_THREADS=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_DEBUG_INFO=OFF \
      -Donnxruntime_ENABLE_WEBASSEMBLY_PROFILING=OFF \
      -Donnxruntime_ENABLE_LAZY_TENSOR=OFF \
      -Donnxruntime_ENABLE_CUDA_PROFILING=OFF \
      -Donnxruntime_USE_XNNPACK=OFF \
      -Donnxruntime_USE_WEBNN=OFF \
      -Donnxruntime_USE_CANN=OFF \
      -Donnxruntime_DISABLE_FLOAT8_TYPES=OFF \
      -Donnxruntime_DISABLE_FLOAT4_TYPES=OFF \
      -Donnxruntime_DISABLE_SPARSE_TENSORS=OFF \
      -Donnxruntime_DISABLE_OPTIONAL_TYPE=OFF \
      -Donnxruntime_DISABLE_STRING_TYPE=OFF \
      -Donnxruntime_CUDA_MINIMAL=OFF \
      -Donnxruntime_USE_CUDA=$WITH_CUDA \
      -Donnxruntime_USE_KLEIDIAI=ON \
      -DCMAKE_INSTALL_PREFIX=$ONNX_DIR \
      -DCMAKE_BUILD_TYPE=Release \
      -Donnxruntime_USE_SYSTEM_PROTOBUF=ON \
      -DProtobuf_INCLUDE_DIR=/usr/local/include \
      -DProtobuf_LIBRARIES=/usr/local/lib/libprotobuf.a \
      -DProtobuf_PROTOC_EXECUTABLE=/usr/local/bin/protoc; \
    cmake --build $ONNX_DIR --config Release

ENV ORT_STRATEGY=system
ENV ORT_LIB_LOCATION=$ONNX_DIR
ENV ORT_DEBUG=1

WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux amd64 build (Docker image) =="

  if docker image inspect "$img" >/dev/null 2>&1; then
    echo "Docker image '$img' already exists. Skipping build."
  else
    local build_args=(--pull)
    [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

    echo "== Linux amd64 build (Docker image) =="
    docker build "${build_args[@]}" --platform=linux/amd64 \
        --build-arg WITH_CUDA="${WITH_CUDA}" \
        --build-arg WITH_ROCM="${WITH_ROCM}" \
        --build-arg CACHE_BUST="${CACHE_BUST}" \
        -f "$df" -t "$img" "$tmp"
   fi

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
      # set -euo pipefail

      ARCH=amd64
      target=x86_64-unknown-linux-musl

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

        export RUSTFLAGS="-C target-feature=+crt-static -C target-cpu=native -C codegen-units=1 -C opt-level=3 -C link-arg=-lm -C link-arg=-lc -C link-arg=-lgfortran -C link-arg=-lpthread -C link-arg=/usr/local/lib/libopenblas.a"
        export RUSTC_LINKER=x86_64-linux-musl-g++
        export LIB_DIR=/usr/local/lib

        echo "--- Libs: ------------------------------------------------------- "
        ls -ltha $LIB_DIR/*.a
        echo "------------------------------------------------------------------ "

        cd /work
        CARGO_TARGET_DIR="$ctd" \
        cargo build --release --target "$target" --features "$feats" -v
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

  linux_copy_out "amd64" "x86_64-unknown-linux-musl" "cpu"
  if [[ "${WITH_VULKAN}" == "1" ]] && [[ -f "${PROJECT_ROOT}/target-cross/linux-amd64-vulkan/x86_64-unknown-linux-musl/release/${BIN_NAME}" ]]; then
    linux_copy_out "amd64" "x86_64-unknown-linux-musl" "vulkan"
  fi
  [[ "${WITH_CUDA}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-musl" "cuda"
  [[ "${WITH_ROCM}" == "1" ]] && linux_copy_out "amd64" "x86_64-unknown-linux-musl" "rocm"

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
  img="local/${BIN_NAME}-linux-arm64:${VERSION}"
  CACHE_BUST="$(date +%s)"

  cat > "$df" <<'DOCKERFILE'
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
ARG CACHE_BUST

# System deps + Vulkan
 RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev:arm64 \
    musl-dev \
    musl-tools \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    curl \
    wget \
    ca-certificates \
    git \
    clang-20 \
    llvm-20-dev \
    libclang-20-dev \
    zlib1g-dev \
    libbz2-dev \
    liblzma-dev \
    cmake \
    libasound2-dev \
 && rm -rf /var/lib/apt/lists/*

# Make espeak-ng find clib
ENV LIBCLANG_PATH=/usr/lib/llvm-20/lib
ENV LD_LIBRARY_PATH=/usr/lib/llvm-20/lib

# Install Rust and add MUSL target
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup update stable
RUN rustup target add aarch64-unknown-linux-musl

# Optional: tell cc-rs where the MUSL compiler is
ENV CC_aarch64_unknown_linux_musl=/usr/bin/aarch64-linux-musl-gcc
ENV CXX=/usr/bin/aarch64-linux-musl-g++
ENV CC=/usr/bin/aarch64-linux-musl-gcc
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=/usr/bin/aarch64-linux-musl-gcc

# Install glslc
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends glslc || true; \
    rm -rf /var/lib/apt/lists/*; \
    (command -v glslc >/dev/null 2>&1 && echo "glslc installed") || echo "glslc not available"


# -----------------------------
# Build static OpenBLAS (arm64)
# -----------------------------
RUN git clone --depth 1 https://github.com/xianyi/OpenBLAS.git /tmp/openblas

# Build OpenBLAS
RUN cd /tmp/openblas && \
    set -eux; \
    make -j$(nproc) \
        STATIC_ONLY=1 \
        NO_SHARED=1 \
        USE_OPENMP=1 \
        USE_THREAD=1 \
        TARGET=ARMV8 \
        VERBOSE=1 \
        2>&1 | tee /tmp/openblas_build.log

# Install OpenBLAS
RUN cd /tmp/openblas && \
    make install PREFIX=/usr/local STATIC_ONLY=1 NO_SHARED=1 && \
    cd / && rm -rf /tmp/openblas


# -----------------------------
# Build ONNX Runtime for this variant (ARM64 musl)
# -----------------------------
ENV ONNX_DIR=/work/deps/onnxruntime
RUN set -eux; \
    mkdir -p "$ONNX_DIR"; \
    git clone --depth 1 https://github.com/microsoft/onnxruntime.git /tmp/onnxruntime; \
    cd /tmp/onnxruntime; \
    mkdir -p build && cd build


ENV USE_BLAS=ON
ENV USE_CUDA=OFF
ENV USE_VULKAN=OFF
ENV USE_ROCM=OFF

RUN set -eux; \
    cmake ../cmake \
      -D CMAKE_BUILD_TYPE=Release \
      -D CMAKE_POSITION_INDEPENDENT_CODE=ON \
      -D CMAKE_INSTALL_PREFIX="$ONNX_DIR" \
      -D USE_SHARED_LIBS=OFF \
      -D BUILD_SHARED_LIBS=OFF \
      -D USE_OPENMP=OFF \
      -D ORT_CPU_ENABLE_AVX=OFF \
      -D ORT_CPU_ENABLE_AVX2=OFF \
      -D ORT_CPU_ENABLE_AVX512=OFF \
      -D ORT_CPU_ENABLE_FMA=OFF \
      -D ORT_CPU_ENABLE_MF16C=OFF \
      -D ORT_CPU_ENABLE_BFLOAT16=OFF \
      -D ORT_CPU_ENABLE_VNNI=OFF \
      -D ORT_CPU_ENABLE_AMX=OFF \
      -D USE_MKL=OFF \
      -D onnxruntime_USE_CUDA=${USE_CUDA} \
      -D USE_ROCM=$USE_ROCM \
      -D USE_VULKAN=$USE_VULKAN \
      -D USE_TENSORRT=OFF \
      -D USE_EIGEN=ON \
      -D USE_BLAS=$USE_BLAS \
      -D CMAKE_C_COMPILER=/usr/bin/aarch64-linux-musl-gcc \
      -D CMAKE_CXX_COMPILER=/usr/bin/aarch64-linux-musl-g++ \
      -D BLAS_LIBRARIES=/usr/local/lib/libopenblas.a \
      -D BLAS_INCLUDE_DIRS=/usr/local/include \
      -D OPENSSL_ROOT_DIR=/usr/local/musl-openssl; \
    cmake --build $ONNX_BUILD --config Release

# Rust + musl target
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup update stable
RUN rustup target add aarch64-unknown-linux-musl
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
      target=aarch64-unknown-linux-musl

      build_variant() {
        local variant="$1"
        local feats="$2"
        local ctd="/work/target-cross/linux-${ARCH}-${variant}"

        echo "---- Building linux/${ARCH} [$variant] features: $feats"

        export OPENBLAS_STATIC=1
        export GGML_BLAS=ON
        export GGML_BLAS_VENDOR=OpenBLAS
        export BLAS_INCLUDE_DIRS=/usr/local/include
        export BLAS_LIBRARIES=/usr/local/lib/libopenblas.a

        export RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static -C link-arg=-lstdc++ -C link-arg=-lsupc++ -C link-arg=-lm -C link-arg=-lpthread -C link-arg=/usr/local/lib/libopenblas.a -C target-cpu=native -C codegen-units=1 -C opt-level=3"

        export CARGO_PROFILE_RELEASE_LTO=false
        export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
        export CARGO_PROFILE_RELEASE_DEBUG=false
        export CARGO_PROFILE_RELEASE_STRIP=symbols
        export CARGO_PROFILE_RELEASE_INCREMENTAL=false
        export OPENSSL_DIR=/usr/local
        export OPENSSL_LIB_DIR=/usr/local/lib
        export OPENSSL_INCLUDE_DIR=/usr/local/include
        export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig

        export ESPEAK_NG_DIR="/work/deps/espeak-ng-install"
        
        

        # Export for Rust build
        export ORT_DIR="$ONNX_DIR"
        export GGML_BLAS_VENDOR="OpenBLAS"

        GGML_CMAKE_ARGS="-DGGML_BLAS=ON \
          -DGGML_BLAS_VENDOR=OpenBLAS \
          -DOPENBLAS_STATIC=ON \
          -DBLAS_LIBRARIES=/usr/local/lib/libopenblas.a \
          -DBLAS_INCLUDE_DIRS=/usr/local/include \
          -DCMAKE_PREFIX_PATH=/usr/include:/usr/lib/aarch64-linux-gnu" \
        CARGO_TARGET_DIR="$ctd" \
        cargo build --release --target "$target" --features "$feats"
      }

      # Always build CPU variant statically
      build_variant cpu "'"${FEATURES_CPU}"'"

      # Vulkan dynamic variant
      if [ "${WITH_VULKAN}" = "1" ] && command -v glslc >/dev/null 2>&1; then
        build_variant vulkan "'"${FEATURES_VULKAN}"'"
      fi
    '

  # Copy out artifacts
  linux_copy_out "arm64" "aarch64-unknown-linux-musl" "cpu"
  if [[ "${WITH_VULKAN}" == "1" ]] && [[ -f "${PROJECT_ROOT}/target-cross/linux-arm64-vulkan/aarch64-unknown-linux-musl/release/${BIN_NAME}" ]]; then
    linux_copy_out "arm64" "aarch64-unknown-linux-musl" "vulkan"
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
# Check static build
# -----------------------------
for f in dist/ai-mate-*-linux-*/ai-mate; do
  echo "Checking $f"

  if ldd "$f" 2>&1 | grep -q "not a dynamic"; then
    echo "✔ Statically linked (ldd says not a dynamic ELF)"
  else
    echo "ldd output:"
    ldd "$f" || true
  fi

  # Fallback: check for OpenBLAS symbols
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
