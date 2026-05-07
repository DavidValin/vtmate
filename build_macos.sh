#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="vtmate"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
ASSETS_DIR="${PROJECT_ROOT}/assets"
ESPEAK_ARCHIVE="${ASSETS_DIR}/espeak-ng-data.tar.gz"

usage() {
  cat <<'USAGE'
Usage:
  ./build_macos.sh

Notes:
  - macOS build
  - Metal enabled
  - No OpenBLAS
  - Produces a single binary
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
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
echo "macOS build: whisper-metal only"

FEATURES="whisper-metal"

# --- Embedded eSpeak asset generation ---

docker_ok=0
command -v docker >/dev/null 2>&1 && docker_ok=1

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

  docker build --pull --platform=linux/amd64 -f "$df" -t "$img" "$tmp"

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

ensure_espeak_data_archive

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found"; exit 1; }

arch="$(uname -m)"

export MACOSX_DEPLOYMENT_TARGET=11.0

export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_STRIP=symbols
export CARGO_PROFILE_RELEASE_INCREMENTAL=false

echo "== Building macOS (${arch}) with whisper-metal =="

export MACOSX_DEPLOYMENT_TARGET=11.0
export RUSTFLAGS="-C link-args=-mmacosx-version-min=10.13"

CARGO_TARGET_DIR="${PROJECT_ROOT}/target-cross/macos-${arch}" \
cargo build --release \
  --features "${FEATURES}"

out="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-${arch}"
cp "${PROJECT_ROOT}/target-cross/macos-${arch}/release/${BIN_NAME}" "$out"
chmod +x "$out" || true

echo "✔ Built: $out"
echo "✔ macOS build complete"
