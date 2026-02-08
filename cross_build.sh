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
command -v tar >/dev/null || { echo "tar required"; exit 1; }

mkdir -p "${DIST_DIR}" "${PKG_DIR}"

########################################
# macOS build (native)
########################################
echo "== macOS build =="
cargo build --release
MAC_BIN="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-arm64"
cp "target/release/${BIN_NAME}" "${MAC_BIN}"
chmod +x "${MAC_BIN}" || true

########################################
# Debian bookworm Linux builder (stable mirrors + multiarch)
# - Builds x86_64-unknown-linux-gnu
# - Builds aarch64-unknown-linux-gnu using aarch64 cross toolchain + arm64 ALSA libs
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
 && rm -rf /var/lib/apt/lists/*

# Install Rust via rustup
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

RUN rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu

WORKDIR /work
DOCKERFILE

########################################
# Build Linux builder image
########################################
echo "== Build Linux builder image (Debian bookworm) =="
docker build --pull --no-cache --platform=linux/amd64 -f "${DF_LINUX}" -t "${IMG_LINUX}" "${TMPDIR}"

########################################
# Linux amd64 build (Docker)
########################################
echo "== Linux amd64 build (Docker) =="
docker run --rm --platform=linux/amd64 \
  -v "${PROJECT_ROOT}:/work" \
  -w /work \
  "${IMG_LINUX}" \
  bash -lc "cargo build --release --target x86_64-unknown-linux-gnu"

LINUX_AMD64_BIN="${DIST_DIR}/${BIN_NAME}-${VERSION}-linux-amd64"
cp "target/x86_64-unknown-linux-gnu/release/${BIN_NAME}" "${LINUX_AMD64_BIN}"
chmod +x "${LINUX_AMD64_BIN}" || true

########################################
# Linux arm64 build (Docker, cross toolchain)
########################################
echo "== Linux arm64 build (Docker) =="
docker run --rm --platform=linux/amd64 \
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
# Windows x86 (32-bit) build (cross) — NO amd64
########################################
echo "== Windows x86 (32-bit) build (cross) =="
command -v cross >/dev/null 2>&1 || cargo install --locked cross
rustup target add i686-pc-windows-gnu

CROSS_CONTAINER_OPTS="--platform=linux/amd64" \
cross build --release --target i686-pc-windows-gnu

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
package_one "${LINUX_AMD64_BIN}"
package_one "${LINUX_ARM64_BIN}"
package_one "${WIN_AMD64_BIN}"

echo ""
echo "✔ Build complete"
echo "Artifacts (raw): ${DIST_DIR}"
ls -lh "${DIST_DIR}" | sed 's/^/  /'
echo ""
echo "Packages: ${PKG_DIR}"
ls -lh "${PKG_DIR}" | sed 's/^/  /'
