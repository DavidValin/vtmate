#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="ai-mate"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
PKG_DIR="${DIST_DIR}/packages"

DO_PACKAGE=1
DOCKER_NO_CACHE=1

SEL_OS="all"     # macos,linux,windows,all
SEL_ARCH="all"   # amd64,arm64,all

usage() {
  cat <<'USAGE'
Usage:
  ./cross_build.sh [--os <list>] [--arch <list>] [--skip-package] [--cache|--no-cache]

--os   comma-separated: macos,linux,windows,all
--arch comma-separated: amd64,arm64,all

Notes:
- Windows build is MSVC and runs only on Windows (requires cl.exe). On macOS/Linux it is skipped.
- macOS build is native and runs only on macOS. On Windows/Linux it is skipped.
- Linux builds use Docker:
  - linux/amd64 uses Ubuntu 24.04 (noble) to satisfy newer glibc requirements (ort-sys / ORT)
  - linux/arm64 uses Ubuntu 24.04 (noble) on linux/arm64 (native on Apple Silicon)
- Docker builds write to target-cross/ to avoid cache/glibc conflicts.
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

want_os() {
  [[ "${SEL_OS}" == "all" ]] && return 0
  list_has "${SEL_OS}" "$1"
}

want_arch() {
  [[ "${SEL_ARCH}" == "all" ]] && return 0
  list_has "${SEL_ARCH}" "$1"
}

host_os() {
  local u; u="$(uname -s | tr '[:upper:]' '[:lower:]')"
  case "$u" in
    darwin*) echo "macos" ;;
    linux*) echo "linux" ;;
    mingw*|msys*|cygwin*) echo "windows" ;;
    *) echo "unknown" ;;
  esac
}

HOST_OS="$(host_os)"

# Args
while [[ $# -gt 0 ]]; do
  case "$1" in
    --os) SEL_OS="$(normalize_list "${2-}")"; shift 2 ;;
    --arch) SEL_ARCH="$(normalize_list "${2-}")"; shift 2 ;;
    --skip-package) DO_PACKAGE=0; shift ;;
    --cache) DOCKER_NO_CACHE=0; shift ;;
    --no-cache) DOCKER_NO_CACHE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown arg: $1"; usage; exit 1 ;;
  esac
done

# Version
VERSION="$(
  awk -F\" '
    $1 ~ /^[[:space:]]*version[[:space:]]*=[[:space:]]*/ { print $2; exit }
  ' "${PROJECT_ROOT}/Cargo.toml"
)"
[[ -n "${VERSION}" ]] || { echo "Failed to read version from Cargo.toml"; exit 1; }

mkdir -p "${DIST_DIR}" "${PKG_DIR}" "${PROJECT_ROOT}/target-cross"

echo "cross_build.sh started: $(date) args: --os ${SEL_OS} --arch ${SEL_ARCH}"
echo "Host OS: ${HOST_OS}"
echo "Project: ${PROJECT_ROOT}"
echo "Version: ${VERSION}"

########################################
# Packaging helpers
########################################
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

make_tgz() {
  local src="$1" tgz="$2"
  command -v tar >/dev/null 2>&1 || { echo "ERROR: tar not found"; exit 1; }
  tar -C "$(dirname "$src")" -czf "$tgz" "$(basename "$src")"
}

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

########################################
# Docker helpers
########################################
docker_ok=0
if command -v docker >/dev/null 2>&1; then docker_ok=1; fi

can_run_amd64() {
  docker run --rm --platform=linux/amd64 alpine:3.19 uname -m >/dev/null 2>&1
}

FORCE_AMD64_DOCKER=0
if [[ "$docker_ok" -eq 1 ]] && can_run_amd64; then FORCE_AMD64_DOCKER=1; fi

ARTIFACTS=()

########################################
# macOS native build
########################################
build_macos_native() {
  if [[ "${HOST_OS}" != "macos" ]]; then
    echo "Skipping macOS build: host is ${HOST_OS} (macOS native build requires macOS)."
    return 0
  fi
  echo "== macOS build (native) =="
  command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found"; exit 1; }
  cargo build --release

  local arch; arch="$(uname -m)"
  local out="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-${arch}"
  cp "${PROJECT_ROOT}/target/release/${BIN_NAME}" "$out"
  chmod +x "$out" || true
  ARTIFACTS+=("$out")
  echo "✔ Built: $out"
}

########################################
# Windows MSVC build (skipped on macOS/Linux)
########################################
build_windows_msvc_amd64() {
  if [[ "${HOST_OS}" != "windows" ]]; then
    echo "Skipping Windows MSVC build: host is ${HOST_OS} (requires Windows + MSVC toolchain)."
    return 0
  fi
  echo "== Windows x86_64 MSVC build (local) =="
  if ! command -v cl.exe >/dev/null 2>&1; then
    echo "Skipping Windows build: cl.exe not found (run from VS x64 Native Tools prompt)."
    return 0
  fi
  rustup target add x86_64-pc-windows-msvc >/dev/null
  cargo build --release --target x86_64-pc-windows-msvc
  local out="${DIST_DIR}/${BIN_NAME}-${VERSION}-windows-msvc-amd64.exe"
  cp "${PROJECT_ROOT}/target/x86_64-pc-windows-msvc/release/${BIN_NAME}.exe" "$out"
  ARTIFACTS+=("$out")
  echo "✔ Built: $out"
}

########################################
# Linux amd64 build (Docker, Ubuntu noble for newer glibc)
########################################
build_linux_amd64_docker() {
  if [[ "$docker_ok" -ne 1 ]]; then
    echo "Skipping linux-amd64: docker not found."
    return 0
  fi
  if [[ "${FORCE_AMD64_DOCKER}" -ne 1 ]]; then
    echo "Skipping linux-amd64: linux/amd64 containers not runnable (enable Rosetta/QEMU)."
    return 0
  fi

  local tmp; tmp="$(mktemp -d)"
  local df="${tmp}/Dockerfile.linux.amd64"
  local img="local/${BIN_NAME}-linux-amd64:${VERSION}-$$"

  cat > "$df" <<'DOCKERFILE'
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git xz-utils \
    build-essential pkg-config \
    cmake ninja-build \
    clang libclang-dev llvm-dev \
    perl \
    libssl-dev \
    libasound2-dev \
 && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add x86_64-unknown-linux-gnu
WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux amd64 build (Docker) =="
  docker build "${build_args[@]}" --platform=linux/amd64 -f "$df" -t "$img" "$tmp"

  docker run --rm --platform=linux/amd64 \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -e CARGO_TARGET_DIR=/work/target-cross/linux-amd64 \
    "$img" \
    bash -lc "cargo build --release --target x86_64-unknown-linux-gnu"

  local out="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-amd64"
  cp "${PROJECT_ROOT}/target-cross/linux-amd64/x86_64-unknown-linux-gnu/release/${BIN_NAME}" "$out"
  chmod +x "$out" || true
  ARTIFACTS+=("$out")

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
  echo "✔ Built: $out"
}

########################################
# Linux arm64 build (Docker, native linux/arm64 on Apple Silicon)
########################################
build_linux_arm64_docker() {
  if [[ "$docker_ok" -ne 1 ]]; then
    echo "Skipping linux-arm64: docker not found."
    return 0
  fi

  local tmp; tmp="$(mktemp -d)"
  local df="${tmp}/Dockerfile.linux.arm64"
  local img="local/${BIN_NAME}-linux-arm64:${VERSION}-$$"

  cat > "$df" <<'DOCKERFILE'
FROM ubuntu:noble
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git xz-utils \
    build-essential pkg-config \
    cmake ninja-build \
    clang libclang-dev llvm-dev \
    perl \
    libssl-dev \
    libasound2-dev \
 && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add aarch64-unknown-linux-gnu
WORKDIR /work
DOCKERFILE

  local build_args=(--pull)
  [[ "${DOCKER_NO_CACHE}" -eq 1 ]] && build_args+=(--no-cache)

  echo "== Linux arm64 build (Docker, native arm64) =="
  docker build "${build_args[@]}" --platform=linux/arm64 -f "$df" -t "$img" "$tmp"

  docker run --rm --platform=linux/arm64 \
    -v "${PROJECT_ROOT}:/work" -w /work \
    -e CARGO_TARGET_DIR=/work/target-cross/linux-arm64 \
    "$img" \
    bash -lc "cargo build --release --target aarch64-unknown-linux-gnu"

  local out="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-arm64"
  cp "${PROJECT_ROOT}/target-cross/linux-arm64/aarch64-unknown-linux-gnu/release/${BIN_NAME}" "$out"
  chmod +x "$out" || true
  ARTIFACTS+=("$out")

  docker image rm -f "$img" >/dev/null 2>&1 || true
  rm -rf "$tmp" >/dev/null 2>&1 || true
  echo "✔ Built: $out"
}

########################################
# Run selected builds
########################################
if want_os macos; then build_macos_native; fi
if want_os windows && want_arch amd64; then build_windows_msvc_amd64; fi
if want_os linux; then
  if want_arch amd64; then build_linux_amd64_docker; fi
  if want_arch arm64; then build_linux_arm64_docker; fi
fi

########################################
# Packaging
########################################
if [[ "${DO_PACKAGE}" -eq 1 ]]; then
  echo "== Packaging tar.gz + SHA256 =="
  for f in "${ARTIFACTS[@]}"; do
    package_one "$f"
  done
else
  echo "Skipping packaging (--skip-package)"
fi

echo ""
echo "✔ Build complete"
echo "Artifacts (raw): ${DIST_DIR}"
ls -lh "${DIST_DIR}" | sed 's/^/  /' || true
echo ""
echo "Packages: ${PKG_DIR}"
ls -lh "${PKG_DIR}" | sed 's/^/  /' || true
