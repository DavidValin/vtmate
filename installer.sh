#!/bin/sh
# vtmate installer - POSIX sh (bash/dash/zsh on Linux and macOS, Git Bash on Windows)
#
#   curl -fsSL https://raw.githubusercontent.com/DavidValin/vtmate/main/installer.sh | sh
#   sh installer.sh [--scope user|system] [--prefix DIR] [--variant cpu|vulkan|cuda12|cuda13]
#                   [--version TAG] [--yes] [--dry-run] [--uninstall] [--help]
#
# Layout (fixed locations, the binary is put on $PATH):
#   Linux/macOS  user:   ~/.local/bin/vtmate          ~/.local/lib/vtmate/*.so
#                system: /usr/local/bin/vtmate        /usr/local/lib/vtmate/*.so
#   Windows      user:   %LOCALAPPDATA%\Programs\vtmate\{bin,lib}
#                system: %ProgramFiles%\vtmate\{bin,lib}
# The Linux cuda binary finds its libraries through its rpath
# ($ORIGIN/../lib/vtmate); on Windows the lib directory is added to PATH.
set -e

REPO="DavidValin/vtmate"
APP="vtmate"
MARK="# added by vtmate installer"

# -------------------------
# Arguments
# -------------------------
SCOPE=""; PREFIX=""; VARIANT=""; VERSION=""; YES=0; DRY_RUN=0; UNINSTALL=0
usage() {
  sed -n '2,15p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
  cat <<EOF

Options:
  --scope user|system   install for this user (default) or system-wide (sudo / Administrator)
  --prefix DIR          custom prefix: DIR/bin and DIR/lib/vtmate (implies --scope user)
  --variant NAME        force cpu, vulkan, cuda12 or cuda13 instead of auto-detection
                        (cuda = whichever CUDA major this machine has a runtime for)
  --version TAG         install a specific release tag instead of the latest
  --yes                 answer yes to every question (reinstall, uninstall)
  --dry-run             detect, select and report; download and install nothing
  --uninstall           remove the program, its libraries, PATH entries and ~/.vtmate
  --help                this text
EOF
}
while [ $# -gt 0 ]; do
  case "$1" in
    --scope)     SCOPE="$2"; shift 2 ;;
    --scope=*)   SCOPE="${1#*=}"; shift ;;
    --prefix)    PREFIX="$2"; shift 2 ;;
    --prefix=*)  PREFIX="${1#*=}"; shift ;;
    --variant)   VARIANT="$2"; shift 2 ;;
    --variant=*) VARIANT="${1#*=}"; shift ;;
    --version)   VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#*=}"; shift ;;
    --yes|-y)    YES=1; shift ;;
    --dry-run)   DRY_RUN=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    --help|-h)   usage; exit 0 ;;
    *) echo "Unknown option: $1"; usage; exit 1 ;;
  esac
done
case "$SCOPE" in ""|user|system) ;; *) echo "❌ --scope must be user or system"; exit 1 ;; esac
case "$VARIANT" in ""|cpu|vulkan|cuda|cuda12|cuda13) ;; *) echo "❌ --variant must be cpu, vulkan, cuda12, cuda13 or cuda"; exit 1 ;; esac
[ -n "$PREFIX" ] && SCOPE="user"

# -------------------------
# Helpers
# -------------------------
say()  { printf '%s\n' "$*"; }
warn() { printf '⚠️  %s\n' "$*"; }
die()  { printf '❌ %s\n' "$*" >&2; exit 1; }

have_tty() { [ -r /dev/tty ] && [ -w /dev/tty ]; }

# ask "question" "default"  -> prints the answer (default when non-interactive)
ask() {
  if [ "$YES" -eq 1 ] || ! have_tty; then printf '%s' "$2"; return; fi
  printf '%s ' "$1" > /dev/tty
  read -r a < /dev/tty || a=""
  [ -z "$a" ] && a="$2"
  printf '%s' "$a"
}
# confirm "question" "y|n"
confirm() {
  [ "$YES" -eq 1 ] && return 0
  case "$(ask "$1 [y/N]" "$2")" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

fetch() { # url -> stdout
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1"
  elif command -v wget >/dev/null 2>&1; then wget -qO- "$1"
  else die "Need curl or wget"; fi
}
url_exists() { # HEAD only - a GET here would silently pull the whole asset
  if command -v curl >/dev/null 2>&1; then
    [ "$(curl -sIL -o /dev/null -w '%{http_code}' "$1")" = "200" ]
  else
    wget --spider -q "$1"
  fi
}
download() { # url dest
  if command -v curl >/dev/null 2>&1; then curl -fL --progress-bar -o "$2" "$1"; else wget -q --show-progress -O "$2" "$1"; fi
}

# -------------------------
# OS / ARCH
# -------------------------
OS="$(uname -s 2>/dev/null || echo unknown)"
ARCH="$(uname -m 2>/dev/null || echo unknown)"
case "$OS" in
  Linux*)  OS_NAME="linux" ;;
  Darwin*) OS_NAME="macos" ;;
  MINGW*|MSYS*|CYGWIN*) OS_NAME="windows" ;;
  *) die "Unsupported OS: $OS" ;;
esac
case "$ARCH" in
  x86_64|amd64)  ARCH_NAME="x86_64" ;;
  arm64|aarch64) ARCH_NAME="aarch64" ;;
  *) die "Unsupported arch: $ARCH" ;;
esac
WSL=0
if [ "$OS_NAME" = "linux" ] && grep -qi microsoft /proc/version 2>/dev/null; then WSL=1; fi

VTMATE_HOME="$HOME/.vtmate"
MANIFEST="$VTMATE_HOME/install-manifest"

# -------------------------
# Install locations
# -------------------------
winpath() { cygpath -u "$1" 2>/dev/null || printf '%s' "$1"; }

resolve_dirs() { # sets BIN_DIR LIB_DIR SUDO
  SUDO=""
  if [ -n "$PREFIX" ]; then
    BIN_DIR="$PREFIX/bin"; LIB_DIR="$PREFIX/lib/$APP"
  elif [ "$OS_NAME" = "windows" ]; then
    if [ "$SCOPE" = "system" ]; then
      root="$(winpath "${ProgramFiles:-C:\\Program Files}")/$APP"
    else
      root="$(winpath "${LOCALAPPDATA:-$HOME/AppData/Local}")/Programs/$APP"
    fi
    BIN_DIR="$root/bin"; LIB_DIR="$root/lib"
  else
    if [ "$SCOPE" = "system" ]; then
      BIN_DIR="/usr/local/bin"; LIB_DIR="/usr/local/lib/$APP"
    else
      BIN_DIR="$HOME/.local/bin"; LIB_DIR="$HOME/.local/lib/$APP"
    fi
  fi
  if [ "$SCOPE" = "system" ] && [ "$OS_NAME" != "windows" ] && [ "$(id -u)" -ne 0 ]; then
    command -v sudo >/dev/null 2>&1 || die "system-wide install needs root or sudo"
    SUDO="sudo"
  fi
}

# run a command with sudo when the target needs it
priv() { if [ -n "$SUDO" ]; then $SUDO "$@"; else "$@"; fi; }

# -------------------------
# PATH persistence
# -------------------------
path_has() { case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac; }

shell_rc() {
  case "$(basename "${SHELL:-sh}")" in
    zsh)  echo "$HOME/.zshrc" ;;
    fish) echo "$HOME/.config/fish/config.fish" ;;
    bash) if [ "$OS_NAME" = "macos" ]; then echo "$HOME/.bash_profile"; else echo "$HOME/.bashrc"; fi ;;
    *)    echo "$HOME/.profile" ;;
  esac
}

add_to_path() { # dir...
  if [ "$OS_NAME" = "windows" ]; then
    target="User"; [ "$SCOPE" = "system" ] && target="Machine"
    for d in "$@"; do
      w="$(cygpath -w "$d" 2>/dev/null || printf '%s' "$d")"
      powershell.exe -NoProfile -Command "
        \$p = [Environment]::GetEnvironmentVariable('Path', '$target');
        if ((\$p -split ';') -notcontains '$w') {
          [Environment]::SetEnvironmentVariable('Path', (\$p.TrimEnd(';') + ';$w'), '$target') }" \
        || warn "could not add $w to the $target PATH; add it yourself"
    done
    say "PATH updated for new terminals (open a new one)."
    return
  fi
  rc="$(shell_rc)"; mkdir -p "$(dirname "$rc")"
  for d in "$@"; do
    path_has "$d" && continue
    grep -s "$MARK" "$rc" 2>/dev/null | grep -qF "$d" && continue
    if [ "$(basename "$rc")" = "config.fish" ]; then
      printf '\nfish_add_path "%s" %s\n' "$d" "$MARK" >> "$rc"
    else
      printf '\nexport PATH="%s:$PATH" %s\n' "$d" "$MARK" >> "$rc"
    fi
    say "Added $d to PATH in $rc (open a new shell or: source $rc)"
  done
}

remove_from_path() { # dir...
  if [ "$OS_NAME" = "windows" ]; then
    for target in User Machine; do
      for d in "$@"; do
        w="$(cygpath -w "$d" 2>/dev/null || printf '%s' "$d")"
        powershell.exe -NoProfile -Command "
          \$p = [Environment]::GetEnvironmentVariable('Path', '$target');
          if (\$p) { \$n = ((\$p -split ';') | Where-Object { \$_ -and \$_ -ne '$w' }) -join ';';
            if (\$n -ne \$p) { [Environment]::SetEnvironmentVariable('Path', \$n, '$target') } }" 2>/dev/null || true
      done
    done
    return
  fi
  for rc in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.zshrc" "$HOME/.profile" "$HOME/.config/fish/config.fish"; do
    [ -f "$rc" ] && grep -qs "$MARK" "$rc" || continue
    grep -v "$MARK" "$rc" > "$rc.vtmate.tmp" && mv "$rc.vtmate.tmp" "$rc"
    say "Removed vtmate PATH entry from $rc"
  done
}

# -------------------------
# Existing installation
# -------------------------
installed_files() { [ -f "$MANIFEST" ] && cat "$MANIFEST"; command -v "$APP" 2>/dev/null; true; }

detect_existing() {
  FOUND=""
  [ -d "$VTMATE_HOME" ] && FOUND="$VTMATE_HOME"
  b="$(command -v "$APP" 2>/dev/null || true)"; [ -n "$b" ] && FOUND="$FOUND $b"
  [ -f "$MANIFEST" ] && FOUND="$FOUND (manifest: $MANIFEST)"
  [ -n "$FOUND" ]
}

remove_installed() { # from manifest + anything at the resolved locations
  if [ -f "$MANIFEST" ]; then
    while IFS= read -r f; do
      [ -n "$f" ] || continue
      if [ -e "$f" ]; then
        if [ -w "$(dirname "$f")" ]; then rm -f "$f"; else sudo rm -f "$f" 2>/dev/null || rm -f "$f"; fi
        say "Removed $f"
      fi
    done < "$MANIFEST"
  fi
  # Our own files at the resolved locations (older installs without a
  # manifest). Anything else in $LIB_DIR - e.g. CUDA libraries a user dropped
  # there - is left alone; --uninstall removes the directory.
  for f in "$BIN_DIR/$APP" "$BIN_DIR/$APP.exe" "$LIB_DIR"/libonnxruntime* "$LIB_DIR"/onnxruntime*.dll; do
    [ -e "$f" ] && { priv rm -f "$f"; say "Removed $f"; }
  done
  [ -d "$LIB_DIR" ] && priv rmdir "$LIB_DIR" 2>/dev/null || true
}

backup_settings() {
  [ -f "$VTMATE_HOME/settings" ] || return 0
  ts="$(date +%Y-%m-%d_%H-%M-%S)"
  cp -p "$VTMATE_HOME/settings" "$VTMATE_HOME/settings.backup.$ts"
  warn "Your settings were backed up to $VTMATE_HOME/settings.backup.$ts"
  warn "vtmate starts with fresh default settings; copy your agents, keys and choices back from the backup (or rename it to 'settings' to restore it as is)."
}

reset_vtmate_home() { # keep settings backups and read-files, drop everything else
  [ -d "$VTMATE_HOME" ] || return 0
  for e in "$VTMATE_HOME"/* "$VTMATE_HOME"/.[!.]*; do
    [ -e "$e" ] || continue
    case "$(basename "$e")" in settings.backup.*|read-files) continue ;; esac
    rm -rf "$e"
  done
}

# -------------------------
# Uninstall
# -------------------------
if [ "$UNINSTALL" -eq 1 ]; then
  [ -z "$SCOPE" ] && SCOPE="user"
  resolve_dirs
  say "This removes vtmate, its libraries, its PATH entries and the whole $VTMATE_HOME directory (settings, models, read-files)."
  if [ "$DRY_RUN" -eq 1 ]; then
    say "Would remove:"; installed_files | sed 's/^/  /'; say "  $BIN_DIR/$APP*"; say "  $LIB_DIR"; say "  $VTMATE_HOME"; exit 0
  fi
  confirm "Uninstall vtmate?" "n" || { say "Aborted."; exit 0; }
  remove_installed
  [ -d "$LIB_DIR" ] && { priv rm -rf "$LIB_DIR"; say "Removed $LIB_DIR"; }
  remove_from_path "$BIN_DIR" "$LIB_DIR"
  rm -rf "$VTMATE_HOME"
  say "✅ vtmate uninstalled."
  exit 0
fi

# -------------------------
# Version
# -------------------------
if [ -z "$VERSION" ]; then
  VERSION="$(fetch "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | cut -d '"' -f 4)"
  [ -n "$VERSION" ] || die "Failed to fetch the latest version"
fi
# VTMATE_BASE_URL overrides the download location (testing against a mirror).
BASE_URL="${VTMATE_BASE_URL:-https://github.com/$REPO/releases/download/$VERSION}"
say "vtmate $VERSION - $OS_NAME/$ARCH_NAME$([ "$WSL" -eq 1 ] && echo ' (WSL)')"
[ "$WSL" -eq 1 ] && warn "Running under WSL: this installs the Linux build inside WSL. For GPU use you need NVIDIA's WSL2 driver on the Windows side; the native Windows build is a separate download."

# -------------------------
# GPU detection: driver, then the runtime the cuda12 / cuda13 variant needs.
# The two cuda builds are the same program against CUDA 12.x / 13.x; the one
# to install is the one whose runtime libraries are already on this machine.
# -------------------------
detect_cuda_driver() {
  command -v nvidia-smi >/dev/null 2>&1 && return 0
  [ -f "/c/Program Files/NVIDIA Corporation/NVSMI/nvidia-smi.exe" ] && return 0
  [ -f "/c/Windows/System32/nvidia-smi.exe" ] && return 0
  command -v reg >/dev/null 2>&1 && reg query "HKLM\\SOFTWARE\\NVIDIA Corporation\\Global\\NVTweak" >/dev/null 2>&1 && return 0
  command -v nvidia-smi.exe >/dev/null 2>&1 && nvidia-smi.exe -L >/dev/null 2>&1 && return 0
  return 1
}
detect_vulkan() {
  command -v vulkaninfo >/dev/null 2>&1 && return 0
  [ -f "/c/Windows/System32/vulkaninfo.exe" ] && return 0
  ls "/c/Program Files/Vulkan SDK/"*/Bin/vulkaninfo.exe >/dev/null 2>&1 && return 0
  [ "$OS_NAME" = "linux" ] && ls /usr/lib/libvulkan.so.1 /usr/lib/*/libvulkan.so.1 /usr/lib64/libvulkan.so.1 >/dev/null 2>&1 && return 0
  command -v vulkaninfo.exe >/dev/null 2>&1 && vulkaninfo.exe >/dev/null 2>&1 && return 0
  return 1
}
# have_so libcudart.so.12 -> found through ldconfig, LD_LIBRARY_PATH or the usual CUDA dirs
have_so() {
  ldconfig -p 2>/dev/null | grep -q "$1" && return 0
  dirs="$(printf '%s' "${LD_LIBRARY_PATH:-}" | tr ':' ' ') /usr/local/cuda/lib64 /opt/cuda/lib64 /opt/cuda/targets/x86_64-linux/lib /usr/lib/x86_64-linux-gnu /usr/lib64 /usr/lib $LIB_DIR"
  for d in $dirs; do [ -e "$d/$1" ] && return 0; done
  return 1
}
have_dll() { # cudart64_*.dll on PATH or in the CUDA toolkit
  for d in $(printf '%s' "$PATH" | tr ':' ' ') "$(winpath "${CUDA_PATH:-}")/bin"; do
    ls "$d"/$1 >/dev/null 2>&1 && return 0
  done
  return 1
}
# cuda_runtime_libs MAJOR -> the runtime files the cudaMAJOR build needs here.
# Library names carry the CUDA major (cuFFT bumps its own: 11 under CUDA 12,
# 12 under CUDA 13); cuRAND 10 and cuDNN 9 keep theirs across both. On
# Windows the archive already bundles cuDNN and cuBLAS beside the exe, so
# only the rest is listed.
cuda_runtime_libs() {
  if [ "$OS_NAME" = "linux" ]; then
    case "$1" in
      12) echo "libcudart.so.12 libcublas.so.12 libcublasLt.so.12 libcufft.so.11 libcurand.so.10 libcudnn.so.9" ;;
      13) echo "libcudart.so.13 libcublas.so.13 libcublasLt.so.13 libcufft.so.12 libcurand.so.10 libcudnn.so.9" ;;
    esac
  elif [ "$OS_NAME" = "windows" ]; then
    case "$1" in
      12) echo "cudart64_12.dll cufft64_11.dll curand64_10.dll" ;;
      13) echo "cudart64_13.dll cufft64_12.dll curand64_10.dll" ;;
    esac
  fi
}
cuda_runtime_missing() { # MAJOR -> prints what is missing for the cudaMAJOR variant
  for l in $(cuda_runtime_libs "$1"); do
    if [ "$OS_NAME" = "linux" ]; then have_so "$l" || printf '%s ' "$l"
    else have_dll "$l" || printf '%s ' "$l"; fi
  done
}
# driver_cuda_max -> the newest CUDA major the driver can run (nvidia-smi's
# header line); empty when there is no nvidia-smi to ask. A CUDA 13 runtime
# on a driver that stops at 12.x fails at load, so that major is skipped.
driver_cuda_max() {
  for c in nvidia-smi nvidia-smi.exe; do
    command -v "$c" >/dev/null 2>&1 || continue
    "$c" 2>/dev/null | sed -n 's/.*CUDA Version: *\([0-9][0-9]*\)\..*/\1/p' | head -1
    return 0
  done
  return 0
}
# cuda_major_available -> the highest CUDA major (13, then 12) whose runtime
# is complete on this machine and within the driver's reach; empty if none.
cuda_major_available() {
  drv="$(driver_cuda_max)"
  for m in 13 12; do
    [ -n "$drv" ] && [ "$drv" -lt "$m" ] && continue
    [ -z "$(cuda_runtime_missing "$m")" ] && { echo "$m"; return 0; }
  done
  return 0
}

CUDA=0; VULKAN=0
detect_cuda_driver && CUDA=1
detect_vulkan && VULKAN=1
say "Detected: NVIDIA driver=$([ $CUDA -eq 1 ] && echo yes || echo no)  Vulkan=$([ $VULKAN -eq 1 ] && echo yes || echo no)"
# Say which CUDA major this machine can actually run, not just that a driver
# exists: with two cuda builds to choose between, "why that one?" is the first
# question a failing GPU install raises.
if [ "$CUDA" -eq 1 ]; then
  drv="$(driver_cuda_max)"
  say "  NVIDIA driver reaches CUDA ${drv:-unknown}.x"
  for m in 13 12; do
    miss="$(cuda_runtime_missing "$m")"
    if [ -n "$miss" ]; then
      say "  cuda$m runtime: incomplete - missing $miss"
    elif [ -n "$drv" ] && [ "$drv" -lt "$m" ]; then
      say "  cuda$m runtime: present, but out of the driver's reach"
    else
      say "  cuda$m runtime: complete"
    fi
  done
fi

# -------------------------
# Scope (ask unless given)
# -------------------------
if [ -z "$SCOPE" ]; then
  if [ "$OS_NAME" = "windows" ]; then
    hint="1) this user (%LOCALAPPDATA%\\Programs\\vtmate)  2) all users (%ProgramFiles%\\vtmate, run as Administrator)"
  else
    hint="1) this user (~/.local)  2) system-wide (/usr/local, needs sudo)"
  fi
  case "$(ask "Install for: $hint  [1]" "1")" in 2|system) SCOPE="system" ;; *) SCOPE="user" ;; esac
fi
resolve_dirs
say "Binary:    $BIN_DIR/$APP$([ "$OS_NAME" = windows ] && echo .exe)"
say "Libraries: $LIB_DIR"

# -------------------------
# Candidates: cuda13/cuda12 (whichever runtime is here) -> vulkan -> cpu
# -------------------------
PREFIX_NAME="${APP}-${VERSION}-${OS_NAME}-${ARCH_NAME}"
EXT="tgz"; [ "$OS_NAME" = "windows" ] && EXT="zip"
CANDIDATES=""
if [ "$OS_NAME" = "macos" ]; then
  CANDIDATES="${PREFIX_NAME}.${EXT}"
elif [ "$VARIANT" = "cuda" ]; then
  major="$(cuda_major_available)"
  [ -n "$major" ] || die "--variant cuda: no complete CUDA 12 or 13 runtime found on this machine; use --variant cuda12 or --variant cuda13 to force one"
  CANDIDATES="${PREFIX_NAME}-cuda${major}.${EXT}"
elif [ -n "$VARIANT" ]; then
  CANDIDATES="${PREFIX_NAME}-${VARIANT}.${EXT}"
else
  if [ "$CUDA" -eq 1 ]; then
    major="$(cuda_major_available)"
    if [ -n "$major" ]; then
      CANDIDATES="${PREFIX_NAME}-cuda${major}.${EXT}"
    else
      drv="$(driver_cuda_max)"
      for m in 13 12; do
        if [ -n "$drv" ] && [ "$drv" -lt "$m" ]; then
          warn "cuda$m: the NVIDIA driver goes up to CUDA $drv.x only"
        else
          warn "cuda$m: NVIDIA driver found, but the build also needs: $(cuda_runtime_missing "$m")"
        fi
      done
      warn "Install the CUDA Toolkit 12.x or 13.x$([ "$OS_NAME" = linux ] && echo ' and cuDNN 9') and rerun, or force one with --variant cuda12|cuda13. Falling back to vulkan/cpu."
    fi
  fi
  [ "$VULKAN" -eq 1 ] && CANDIDATES="$CANDIDATES ${PREFIX_NAME}-vulkan.${EXT}"
  CANDIDATES="$CANDIDATES ${PREFIX_NAME}-cpu.${EXT}"
fi
say "Candidates: $CANDIDATES"

# -------------------------
# Existing install -> reinstall?
# -------------------------
REINSTALL=0
if detect_existing; then
  say "vtmate is already installed:$FOUND"
  if [ "$DRY_RUN" -eq 0 ]; then
    if [ "$YES" -eq 0 ] && ! have_tty; then die "existing installation found; rerun with --yes to reinstall"; fi
    confirm "Reinstall? (settings are backed up, $VTMATE_HOME is reset, read-files are kept)" "n" || { say "Aborted."; exit 0; }
    REINSTALL=1
  fi
fi

if [ "$DRY_RUN" -eq 1 ]; then
  say "Dry run: nothing downloaded or installed."
  for B in $CANDIDATES; do url_exists "$BASE_URL/$B" && { say "Would install: $B"; break; }; done
  exit 0
fi

# -------------------------
# Install: try candidates in order, keep the first one that starts
# -------------------------
TMP_DIR="${TMPDIR:-/tmp}/${APP}-install.$$"
mkdir -p "$TMP_DIR"
trap 'rm -rf "$TMP_DIR"' EXIT

extract() { # archive dir
  case "$1" in
    *.tgz) tar -xzf "$1" -C "$2" ;;
    *.zip)
      if command -v unzip >/dev/null 2>&1; then unzip -q -o "$1" -d "$2"
      elif command -v powershell.exe >/dev/null 2>&1; then
        powershell.exe -NoProfile -Command "Expand-Archive -Force -LiteralPath '$(cygpath -w "$1")' -DestinationPath '$(cygpath -w "$2")'"
      else die "Need unzip or powershell to extract $1"; fi ;;
  esac
}

smoke_test() { # binary
  if [ "$OS_NAME" = "windows" ]; then
    PATH="$LIB_DIR:$PATH" "$1" --list-voices >/dev/null 2>&1
  else
    "$1" --list-voices >/dev/null 2>&1
  fi
}

install_from() { # extracted dir -> 0 on success (files recorded in manifest)
  src="$1"
  bin="$src/$APP"; [ "$OS_NAME" = "windows" ] && bin="$src/$APP.exe"
  [ -f "$bin" ] || { warn "$(basename "$bin") not found in archive"; return 1; }
  priv mkdir -p "$BIN_DIR" "$LIB_DIR"
  mkdir -p "$VTMATE_HOME"; : > "$MANIFEST"
  priv cp "$bin" "$BIN_DIR/"
  priv chmod +x "$BIN_DIR/$(basename "$bin")"
  echo "$BIN_DIR/$(basename "$bin")" >> "$MANIFEST"
  for f in "$src"/*.so "$src"/*.so.* "$src"/*.dll; do
    [ -e "$f" ] || continue
    priv cp -P "$f" "$LIB_DIR/"
    echo "$LIB_DIR/$(basename "$f")" >> "$MANIFEST"
  done
  if smoke_test "$BIN_DIR/$(basename "$bin")"; then return 0; fi
  warn "$(basename "$bin") from $CUR could not start on this machine"
  while IFS= read -r f; do priv rm -f "$f"; done < "$MANIFEST"
  rm -f "$MANIFEST"
  return 1
}

if [ "$REINSTALL" -eq 1 ]; then
  backup_settings
  remove_installed
  reset_vtmate_home
fi

INSTALLED=""
say "Looking for a matching build in $VERSION ..."
for CUR in $CANDIDATES; do
  url="$BASE_URL/$CUR"
  url_exists "$url" || { say "Not in this release: $CUR"; continue; }
  say "Downloading $CUR ..."
  download "$url" "$TMP_DIR/$CUR"
  size="$(wc -c < "$TMP_DIR/$CUR" 2>/dev/null || echo 0)"
  [ "$size" -ge 100000 ] || { warn "download too small ($size bytes), skipping"; continue; }
  head -c 20 "$TMP_DIR/$CUR" | grep -qi "<html" && { warn "download is an HTML error page, skipping"; continue; }
  rm -rf "$TMP_DIR/x"; mkdir -p "$TMP_DIR/x"
  extract "$TMP_DIR/$CUR" "$TMP_DIR/x"
  if install_from "$TMP_DIR/x"; then INSTALLED="$CUR"; break; fi
  rm -f "$TMP_DIR/$CUR"
done
[ -n "$INSTALLED" ] || die "No variant of vtmate $VERSION could be installed on this machine"

# -------------------------
# PATH + notes
# -------------------------
if [ "$OS_NAME" = "windows" ]; then add_to_path "$BIN_DIR" "$LIB_DIR"; else add_to_path "$BIN_DIR"; fi

case "$INSTALLED" in
  *-cuda12.*|*-cuda13.*)
    m="${INSTALLED##*-cuda}"; m="${m%%.*}"
    say "Installed the cuda$m build. It needs the CUDA $m runtime$([ "$OS_NAME" = linux ] && echo ' and cuDNN 9') on this machine (system-wide or copied into $LIB_DIR)." ;;
  *-vulkan.*)
    say "Installed the vulkan build. It needs the Vulkan loader (libvulkan.so.1 / vulkan-1.dll) from your GPU driver." ;;
esac
say "✅ Installed $INSTALLED to $BIN_DIR"
path_has "$BIN_DIR" || say "Open a new terminal, then run: $APP"
