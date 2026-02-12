#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="ai-mate"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${PROJECT_ROOT}/dist"
PKG_DIR="${DIST_DIR}/packages"
ASSETS_DIR="${PROJECT_ROOT}/assets"
ESPEAK_ARCHIVE="${ASSETS_DIR}/espeak-ng-data.tar.gz"

DO_PACKAGE=1

# macOS optional
MAC_WITH_OPENBLAS="${MAC_WITH_OPENBLAS:-0}"

usage() {
  cat <<'USAGE'
Usage:
  ./build_macos.sh [--skip-package]

Env:
  MAC_WITH_OPENBLAS=0|1 (macos) default 0
Notes:
  - macOS builds always enable Metal.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-package) DO_PACKAGE=0; shift ;;
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

mkdir -p "${DIST_DIR}" "${PKG_DIR}" "${PROJECT_ROOT}/target-cross" "${ASSETS_DIR}"

echo "Version: ${VERSION}"
echo "macOS: Metal always, MAC_WITH_OPENBLAS=${MAC_WITH_OPENBLAS}"

FEATURES_COMMON="whisper-logs"
FEATURES_MACOS_METAL="${FEATURES_COMMON},whisper-metal"
FEATURES_MACOS_METAL_OPENBLAS="${FEATURES_COMMON},whisper-metal,whisper-openblas"

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

# Embedded eSpeak asset generation (via Docker if missing)
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

ARTIFACTS=()
add_artifact() { [[ -f "$1" ]] && ARTIFACTS+=("$1"); }

arch="$(uname -m)"

echo "== macOS build [metal] features: ${FEATURES_MACOS_METAL} =="
cargo build --release --no-default-features --features "${FEATURES_MACOS_METAL}"
out="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-${arch}-metal"
cp "${PROJECT_ROOT}/target/release/${BIN_NAME}" "$out"
chmod +x "$out" || true
add_artifact "$out"
echo "✔ Built: $out"

if [[ "${MAC_WITH_OPENBLAS}" == "1" ]]; then
  echo "== macOS build [metal-openblas] features: ${FEATURES_MACOS_METAL_OPENBLAS} =="
  CARGO_TARGET_DIR="${PROJECT_ROOT}/target-cross/macos-${arch}-metal-openblas" \
    cargo build --release --no-default-features --features "${FEATURES_MACOS_METAL_OPENBLAS}"
  out="${DIST_DIR}/${BIN_NAME}-${VERSION}-macos-${arch}-metal-openblas"
  cp "${PROJECT_ROOT}/target-cross/macos-${arch}-metal-openblas/release/${BIN_NAME}" "$out"
  chmod +x "$out" || true
  add_artifact "$out"
  echo "✔ Built: $out"
fi

if [[ "${DO_PACKAGE}" -eq 1 ]]; then
  echo "== Packaging tar.gz + SHA256 =="
  for f in "${ARTIFACTS[@]}"; do
    package_one "$f"
  done
else
  echo "Skipping packaging (--skip-package)"
fi

echo "✔ macOS build complete"
