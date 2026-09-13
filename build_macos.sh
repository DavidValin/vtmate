#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="vtmate"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
ASSETS_DIR="${PROJECT_ROOT}/assets"

usage() {
  cat <<'USAGE'
Usage:
  ./build_macos.sh [--arch arm64|x86_64]

Notes:
  - macOS build
  - Metal enabled
  - No OpenBLAS
  - Produces a single binary
USAGE
}

ARCH_SEL=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH_SEL="${2-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown arg: $1"; usage; exit 1 ;;
  esac
done

HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
if [[ "${HOST_OS}" != "darwin" ]]; then
  echo "ERROR: build_macos.sh must be run on macOS."
  exit 1
fi

VERSION="$(
  awk -F\" '
    $1 ~ /^[[:space:]]*version[[:space:]]*=[[:space:]]*/ { print $2; exit }
  ' "${PROJECT_ROOT}/Cargo.toml"
)"
[[ -n "${VERSION}" ]] || { echo "Failed to read version from Cargo.toml"; exit 1; }

mkdir -p "${DIST_DIR}" "${ASSETS_DIR}"

echo "Version: ${VERSION}"

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found"; exit 1; }

arch="${ARCH_SEL:-$(uname -m)}"

case "${arch}" in
  arm64|aarch64) arch="arm64";  RUST_TARGET="aarch64-apple-darwin" ;;
  x86_64|amd64)  arch="x86_64"; RUST_TARGET="x86_64-apple-darwin"  ;;
  *) echo "ERROR: unsupported --arch '${arch}' (use arm64 or x86_64)"; exit 1 ;;
esac

# ggml's Metal backend targets Apple GPUs; on Intel Macs it is unsupported or
# slower than the CPU path, so Metal is gated on arm64.
if [[ "${arch}" == "arm64" ]]; then
  FEATURES="whisper-metal"
else
  FEATURES=""
fi

# Apple Silicon hosts can emit x86_64 directly - the SDK carries both slices -
# so the Intel build no longer needs an Intel runner.
rustup target add "${RUST_TARGET}" >/dev/null 2>&1 || true

echo "macOS build: ${arch} (${RUST_TARGET}, features: ${FEATURES:-none})"

export MACOSX_DEPLOYMENT_TARGET=11.0

export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_STRIP=symbols
export CARGO_PROFILE_RELEASE_INCREMENTAL=false
# Same x86 baseline as the Linux and Windows builds (x86-64-v3 = AVX2/FMA,
# every Intel Mac that runs macOS 11 has it); Apple silicon needs nothing.
if [ "${arch}" = "x86_64" ]; then
  export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=x86-64-v3"
fi

echo "== Building macOS (${arch}) with features: ${FEATURES:-none} =="

# A previous dependency held path buffers with a low fixed size limit that
# --target's triple could push past; kept short and outside the project tree
# since there is no upside to a longer path.
CARGO_TARGET_DIR="${TARGET_ROOT:-${HOME}/t/${arch}}"
mkdir -p "${CARGO_TARGET_DIR}"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR}" \
cargo build --release \
  --target "${RUST_TARGET}" \
  --features "${FEATURES}"

# Artifact naming: <bin>-<version>-macos-<arch>/vtmate, with the arch in
# target-triple spelling (aarch64, x86_64) like the Linux/Windows artifacts.
# The release workflow tars the directory contents, so the archive extracts
# to ./vtmate directly.
case "${arch}" in
  arm64) artifact_arch="aarch64" ;;
  *)     artifact_arch="${arch}" ;;
esac
out_dir="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-${artifact_arch}"
rm -rf "${out_dir}"; mkdir -p "${out_dir}"
cp "${CARGO_TARGET_DIR}/${RUST_TARGET}/release/${BIN_NAME}" "${out_dir}/"
chmod +x "${out_dir}/${BIN_NAME}" || true

echo "✔ Built: ${out_dir}/${BIN_NAME}"
echo "✔ macOS build complete"
