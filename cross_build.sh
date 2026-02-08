#!/usr/bin/env bash
set -euo pipefail

########################################
# Configuration
########################################
BIN_NAME="ai-mate"
PROJECT_ROOT="$(pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
PKG_DIR="${DIST_DIR}/packages"

VERSION="$(
  awk -F\" '
    $1 ~ /^[[:space:]]*version[[:space:]]*=[[:space:]]*/ { print $2; exit }
  ' Cargo.toml
)"
if [[ -z "${VERSION}" ]]; then
  echo "Failed to read version from Cargo.toml"
  exit 1
fi

########################################
# Temp workspace (repo stays clean)
########################################
TMPDIR="$(mktemp -d)"
DF_LINUX="${TMPDIR}/Dockerfile.linux"

IMG_LINUX="local/${BIN_NAME}-linux-build:${VERSION}-$$"

cleanup() {
  set +e
  echo ""
  echo "== Cleanup =="
  docker image rm -f "${IMG_LINUX}" >/dev/null 2>&1 || true
  rm -rf "${TMPDIR}" >/dev/null 2>&1 || true
  echo "✔ Removed temporary Docker image + temp files"
}
trap cleanup EXIT

########################################
# Preflight
########################################
command -v docker >/dev/null || { echo "Docker required"; exit 1; }
command -v cargo  >/dev/null || { echo "cargo required"; exit 1; }
command -v shasum >/dev/null || { echo "shasum required"; exit 1; }
command -v tar    >/dev/null || { echo "tar required"; exit 1; }

mkdir -p "${DIST_DIR}" "${PKG_DIR}"

HOST_ARCH="$(uname -m)"
DOCKER_SERVER_ARCH="$(docker version --format '{{.Server.Arch}}' 2>/dev/null || true)"
DOCKER_SERVER_OS="$(docker version --format '{{.Server.Os}}' 2>/dev/null || true)"

echo "== Platform preflight =="
echo "Host arch:   ${HOST_ARCH}"
echo "Docker srv:  ${DOCKER_SERVER_OS}/${DOCKER_SERVER_ARCH}"

# Helper: can we run linux/amd64 containers?
can_run_amd64() {
  docker run --rm --platform=linux/amd64 alpine:3.19 uname -m >/dev/null 2>&1
}

# If we're on ARM Linux, attempt to enable amd64 emulation (best-effort).
# On macOS Apple Silicon, user must enable Rosetta in Docker Desktop manually.
if [[ "${HOST_ARCH}" =~ ^(arm64|aarch64)$ ]]; then
  echo "ARM host detected."
  if [[ "${DOCKER_SERVER_OS}" == "linux" ]]; then
    if ! can_run_amd64; then
      echo "amd64 containers currently FAIL to run. Attempting to install binfmt for amd64 (requires privileged Docker)..."
      docker run --rm --privileged tonistiigi/binfmt --install amd64 >/dev/null 2>&1 || true
    fi
  fi
fi

FORCE_AMD64_DOCKER=0
if can_run_amd64; then
  FORCE_AMD64_DOCKER=1
  echo "amd64 containers: ✅ runnable"
else
  echo "amd64 containers: ❌ NOT runnable"
  echo "If you need amd64 containers on ARM:"
  echo "  - Linux: run 'docker run --rm --privileged tonistiigi/binfmt --install amd64'"
  echo "  - macOS (Apple Silicon): enable Rosetta/x86_64 emulation in Docker Desktop settings"
fi

# Helper: append --platform=linux/amd64 only when supported
platform_amd64_args() {
  if [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]]; then
    echo "--platform=linux/amd64"
  else
    echo ""
  fi
}

########################################
# macOS build (native)
########################################
echo "== macOS build =="
cargo build --release
MAC_BIN="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-arm64"
cp "target/release/${BIN_NAME}" "${MAC_BIN}"
chmod +x "${MAC_BIN}" || true

########################################
# Debian bookworm Linux builder (stable mirrors + multiarch + MinGW)
# - Builds x86_64-unknown-linux-gnu (when amd64 containers runnable)
# - Builds aarch64-unknown-linux-gnu using aarch64 cross toolchain + arm64 ALSA libs
# - Builds i686-pc-windows-gnu using mingw-w64 toolchain (no cross-rs images)
########################################
cat > "${DF_LINUX}" <<'DOCKERFILE'
FROM debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive

# Multiarch must be added BEFORE the update/install that includes :arm64 packages
RUN dpkg --add-architecture arm64 \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git xz-utils \
      build-essential pkg-config \
      # ALSA dev for native (amd64) build
      libasound2-dev \
      # Cross toolchain for aarch64
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
      # ARM64 ALSA runtime+dev so -lasound resolves for aarch64 link
      libasound2:arm64 libasound2-dev:arm64 \
      # Common cross runtime/dev pieces (Debian names)
      libc6-dev-arm64-cross \
      # Windows cross toolchains (GNU)
      mingw-w64 \
 && rm -rf /var/lib/apt/lists/*

# Install Rust via rustup
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Pre-install targets (cached in the image)
RUN rustup target add \
      x86_64-unknown-linux-gnu \
      aarch64-unknown-linux-gnu \
      i686-pc-windows-gnu

WORKDIR /work
DOCKERFILE

########################################
# Build Linux builder image
########################################
echo "== Build Linux builder image (Debian bookworm) =="

# If we can run amd64 containers, build amd64 builder (original design).
# Otherwise, build native builder for current platform.
BUILD_PLATFORM_ARGS=()
if [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]]; then
  BUILD_PLATFORM_ARGS+=(--platform=linux/amd64)
fi

docker build --pull --no-cache "${BUILD_PLATFORM_ARGS[@]}" \
  -f "${DF_LINUX}" -t "${IMG_LINUX}" "${TMPDIR}"

########################################
# Linux amd64 build (Docker)
########################################
echo "== Linux amd64 build (Docker) =="

if [[ "${FORCE_AMD64_DOCKER}" -ne 1 ]]; then
  echo "Skipping Linux amd64 build because amd64 containers are not runnable on this host."
  echo "Enable amd64 emulation (Rosetta/binfmt) to build this target here, or run on an amd64 machine."
else
  docker run --rm --platform=linux/amd64 \
    -v "${PROJECT_ROOT}:/work" \
    -w /work \
    "${IMG_LINUX}" \
    bash -lc "cargo build --release --target x86_64-unknown-linux-gnu"

  LINUX_AMD64_BIN="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-amd64"
  cp "target/x86_64-unknown-linux-gnu/release/${BIN_NAME}" "${LINUX_AMD64_BIN}"
  chmod +x "${LINUX_AMD64_BIN}" || true
fi

########################################
# Linux arm64 build (Docker, cross toolchain)
########################################
echo "== Linux arm64 build (Docker) =="

docker run --rm $(platform_amd64_args) \
  -v "${PROJECT_ROOT}:/work" \
  -w /work \
  -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  -e CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
  -e CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
  -e PKG_CONFIG_ALLOW_CROSS=1 \
  -e PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig:/usr/lib/pkgconfig \
  "${IMG_LINUX}" \
  bash -lc "cargo build --release --target aarch64-unknown-linux-gnu"

LINUX_ARM64_BIN="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-arm64"
cp "target/aarch64-unknown-linux-gnu/release/${BIN_NAME}" "${LINUX_ARM64_BIN}"
chmod +x "${LINUX_ARM64_BIN}" || true

########################################
# Windows x86 (32-bit) build (MinGW in Docker)
########################################
echo "== Windows x86 (32-bit) build (MinGW in Docker) =="

docker run --rm $(platform_amd64_args) \
  -v "${PROJECT_ROOT}:/work" \
  -w /work \
  -e CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc \
  -e CC_i686_pc_windows_gnu=i686-w64-mingw32-gcc \
  -e CXX_i686_pc_windows_gnu=i686-w64-mingw32-g++ \
  "${IMG_LINUX}" \
  bash -lc "rustup target add i686-pc-windows-gnu && cargo build --release --target i686-pc-windows-gnu"

WIN_X86_BIN="${DIST_DIR}/${BIN_NAME}-${VERSION}-windows-x86.exe"
cp "target/i686-pc-windows-gnu/release/${BIN_NAME}.exe" "${WIN_X86_BIN}"

########################################
# Packaging: tar.gz + SHA256
########################################
echo "== Packaging tar.gz + SHA256 =="

package_one() {
  local src="$1"
  local base
  base="$(basename "$src")"
  local tgz="${PKG_DIR}/${base}.tar.gz"
  tar -C "$(dirname "$src")" -czf "${tgz}" "${base}"
  (cd "${PKG_DIR}" && shasum -a 256 "$(basename "${tgz}")" > "$(basename "${tgz}").sha256")
}

package_one "${MAC_BIN}"
if [[ "${FORCE_AMD64_DOCKER}" -eq 1 ]]; then
  package_one "${LINUX_AMD64_BIN}"
fi
package_one "${LINUX_ARM64_BIN}"
package_one "${WIN_X86_BIN}"

echo ""
echo "✔ Build complete"
echo "Artifacts (raw): ${DIST_DIR}"
ls -lh "${DIST_DIR}" | sed 's/^/  /'
echo ""
echo "Packages: ${PKG_DIR}"
ls -lh "${PKG_DIR}" | sed 's/^/  /'
