#!/bin/sh
set -e

REPO="DavidValin/vtmate"
APP="vtmate"

# -------------------------
# Get latest version
# -------------------------
if command -v curl >/dev/null 2>&1; then
  VERSION=$(curl -s https://api.github.com/repos/$REPO/releases/latest \
    | grep '"tag_name":' | cut -d '"' -f 4)
elif command -v wget >/dev/null 2>&1; then
  VERSION=$(wget -qO- https://api.github.com/repos/$REPO/releases/latest \
    | grep '"tag_name":' | cut -d '"' -f 4)
else
  echo "Need curl or wget"
  exit 1
fi

[ -z "$VERSION" ] && { echo "Failed to fetch version"; exit 1; }

echo "Version: $VERSION"

BASE_URL="https://github.com/$REPO/releases/download/$VERSION"

# -------------------------
# OS / ARCH
# -------------------------
OS="$(uname -s 2>/dev/null || echo unknown)"
ARCH="$(uname -m 2>/dev/null || echo unknown)"

case "$OS" in
  Linux*) OS_NAME="linux" ;;
  Darwin*) OS_NAME="macos" ;;
  MINGW*|MSYS*|CYGWIN*) OS_NAME="windows" ;;
  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

# Release assets use the target-triple spelling: x86_64 / aarch64.
case "$ARCH" in
  x86_64|amd64) ARCH_NAME="x86_64" ;;
  arm64|aarch64) ARCH_NAME="aarch64" ;;
  *) echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

echo "OS=$OS_NAME ARCH=$ARCH_NAME"

# -------------------------
# GPU detection (HARDENED VERSION)
# -------------------------
CUDA=0
VULKAN=0

detect_cuda() {

  # 1. Standard PATH (Linux / WSL / Git Bash)
  if command -v nvidia-smi >/dev/null 2>&1; then
    return 0
  fi

  # 2. Windows known install paths
  if [ -f "/c/Program Files/NVIDIA Corporation/NVSMI/nvidia-smi.exe" ]; then
    return 0
  fi

  if [ -f "/cygdrive/c/Program Files/NVIDIA Corporation/NVSMI/nvidia-smi.exe" ]; then
    return 0
  fi

  # 3. Windows registry (strong signal)
  if command -v reg >/dev/null 2>&1; then
    reg query "HKLM\SOFTWARE\NVIDIA Corporation\Global\NVTweak" >/dev/null 2>&1 && return 0
  fi

  # 4. Execution probe fallback
  if command -v nvidia-smi.exe >/dev/null 2>&1; then
    nvidia-smi.exe -L >/dev/null 2>&1 && return 0
  fi

  return 1
}

detect_vulkan() {

  # 1. PATH
  if command -v vulkaninfo >/dev/null 2>&1; then
    return 0
  fi

  # 2. Windows system install
  if [ -f "/c/Windows/System32/vulkaninfo.exe" ]; then
    return 0
  fi

  # 3. Vulkan SDK install (Windows)
  if ls "/c/Program Files/Vulkan SDK/"*/Bin/vulkaninfo.exe >/dev/null 2>&1; then
    return 0
  fi

  # 4. Execution probe
  if command -v vulkaninfo.exe >/dev/null 2>&1; then
    vulkaninfo.exe >/dev/null 2>&1 && return 0
  fi

  return 1
}

detect_cuda && CUDA=1
detect_vulkan && VULKAN=1

echo "CUDA=$CUDA VULKAN=$VULKAN"

# -------------------------
# Candidate selection
# -------------------------
# Assets are named ${APP}-<version>-<os>-<arch>[-<variant>].<tgz|zip> and
# hold the binary (plus any bundled DLLs on Windows) at top level.
CANDIDATES=""
PREFIX="${APP}-${VERSION}-${OS_NAME}-${ARCH_NAME}"

if [ "$OS_NAME" = "macos" ]; then
  CANDIDATES="${PREFIX}.tgz"
fi

# -------------------------
# Linux (x86_64 / aarch64): cuda -> vulkan -> cpu, best available first
# -------------------------
if [ "$OS_NAME" = "linux" ]; then

  if [ "$CUDA" -eq 1 ]; then
    CANDIDATES="${PREFIX}-cuda.tgz"
  fi

  if [ "$VULKAN" -eq 1 ]; then
    CANDIDATES="${CANDIDATES} ${PREFIX}-vulkan.tgz"
  fi

  CANDIDATES="${CANDIDATES} ${PREFIX}-cpu.tgz"
fi

# -------------------------
# Windows x86_64
# -------------------------
if [ "$OS_NAME" = "windows" ] && [ "$ARCH_NAME" = "x86_64" ]; then

  if [ "$CUDA" -eq 1 ]; then
    CANDIDATES="${PREFIX}-cuda.zip"
  fi

  if [ "$VULKAN" -eq 1 ]; then
    CANDIDATES="${CANDIDATES} ${PREFIX}-vulkan.zip"
  fi

  CANDIDATES="${CANDIDATES} ${PREFIX}-cpu.zip"
fi

[ -z "$CANDIDATES" ] && { echo "No candidates built"; exit 1; }

echo "Candidates: $CANDIDATES"

# -------------------------
# Find first valid binary
# -------------------------
FOUND_URL=""
FOUND_BIN=""

for B in $CANDIDATES; do
  URL="$BASE_URL/$B"

  if command -v curl >/dev/null 2>&1; then
    CODE=$(curl -s -o /dev/null -w "%{http_code}" "$URL")
    if [ "$CODE" = "200" ]; then
      FOUND_URL="$URL"
      FOUND_BIN="$B"
      break
    fi
  else
    wget --spider -q "$URL" && {
      FOUND_URL="$URL"
      FOUND_BIN="$B"
      break
    }
  fi
done

if [ -z "$FOUND_URL" ]; then
  echo "❌ No valid binary found"
  exit 1
fi

echo "Selected: $FOUND_BIN"

# -------------------------
# Download
# -------------------------
TMP_FILE="/tmp/$FOUND_BIN"

if command -v curl >/dev/null 2>&1; then
  curl -fL -o "$TMP_FILE" "$FOUND_URL"
else
  wget -O "$TMP_FILE" "$FOUND_URL"
fi

# -------------------------
# Validate download sanity
# -------------------------
MIN_SIZE=100000  # 100 KB minimum (adjust if your binaries are smaller)

FILE_SIZE=$(wc -c < "$TMP_FILE" 2>/dev/null || echo 0)

if [ "$FILE_SIZE" -lt "$MIN_SIZE" ]; then
  echo "❌ Download too small or invalid ($FILE_SIZE bytes)"
  exit 1
fi

# Detect HTML error pages (GitHub fallback / proxy / 404 HTML)
if head -c 20 "$TMP_FILE" 2>/dev/null | grep -qi "<html"; then
  echo "❌ Download appears to be an HTML error page"
  exit 1
fi

echo "Download sanity check passed ($FILE_SIZE bytes)"

# -------------------------
# Install
# -------------------------
# The asset is an archive with the binary at top level (Windows: vtmate.exe
# plus the DLLs the CUDA/ORT variants need beside it), so unpack it into a
# scratch directory first.
EXTRACT_DIR="/tmp/${APP}-extract.$$"
rm -rf "$EXTRACT_DIR"
mkdir -p "$EXTRACT_DIR"

case "$FOUND_BIN" in
  *.tgz)
    tar -xzf "$TMP_FILE" -C "$EXTRACT_DIR"
    ;;
  *.zip)
    if command -v unzip >/dev/null 2>&1; then
      unzip -q -o "$TMP_FILE" -d "$EXTRACT_DIR"
    elif command -v powershell.exe >/dev/null 2>&1; then
      powershell.exe -NoProfile -Command \
        "Expand-Archive -Force -LiteralPath '$(cygpath -w "$TMP_FILE" 2>/dev/null || echo "$TMP_FILE")' -DestinationPath '$(cygpath -w "$EXTRACT_DIR" 2>/dev/null || echo "$EXTRACT_DIR")'"
    else
      echo "❌ Need unzip or powershell to extract $FOUND_BIN"
      exit 1
    fi
    ;;
  *)
    echo "❌ Unknown archive type: $FOUND_BIN"
    exit 1
    ;;
esac

case "$OS_NAME" in
  windows)
    INSTALL_DIR="$HOME/bin"
    mkdir -p "$INSTALL_DIR"
    [ -f "$EXTRACT_DIR/$APP.exe" ] || { echo "❌ $APP.exe not found in archive"; exit 1; }
    # Everything in the archive: the exe and the DLLs it must sit next to.
    cp "$EXTRACT_DIR"/* "$INSTALL_DIR/"
    ;;
  *)
    INSTALL_DIR="/usr/local/bin"
    [ -w "$INSTALL_DIR" ] || INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"

    [ -f "$EXTRACT_DIR/$APP" ] || { echo "❌ $APP not found in archive"; exit 1; }
    cp "$EXTRACT_DIR/$APP" "$INSTALL_DIR/$APP"
    chmod +x "$INSTALL_DIR/$APP"
    ;;
esac

rm -rf "$EXTRACT_DIR" "$TMP_FILE"

echo "Installed to: $INSTALL_DIR"
echo "Done."