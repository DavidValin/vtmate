#!/usr/bin/env bash
# Check a Windows .exe for dynamic-library imports, from Linux.
# Mirrors the dumpbin gate in build_windows.ps1 so an artifact can be
# verified without a Windows box.
#
#   ./scripts/check-static.sh path/to/vtmate-cpu.exe
set -uo pipefail

exe=${1:?usage: check-static.sh <file.exe>}
[[ -f $exe ]] || { echo "no such file: $exe" >&2; exit 2; }

command -v objdump >/dev/null || { echo "objdump not found (install binutils)" >&2; exit 2; }
objdump -i 2>/dev/null | grep -q pei-x86-64 || \
  echo "warning: this objdump may lack PE support; results could be empty" >&2

deps=$(objdump -p "$exe" 2>/dev/null | sed -n 's/^\tDLL Name: //p' | sort -fu)
[[ -n $deps ]] || { echo "no import table found - not a PE file?" >&2; exit 2; }

echo "=== Imports for $(basename "$exe") ==="
printf '  %s\n' $deps

# Same denylist as the CI gate: dynamic CRT, MSVC OpenMP, vendored libs.
# CUDA/Vulkan loaders and plain Win32 DLLs are allowed.
bad=$(printf '%s\n' $deps | grep -iE \
  '^(vcruntime[0-9]*\.dll|msvcp[0-9]*\.dll|msvcr[0-9]*\.dll|ucrtbased?\.dll|api-ms-win-crt-.*\.dll|vcomp[0-9]*\.dll|libopenblas\.dll|openblas\.dll|onnxruntime.*\.dll|espeak-ng\.dll|whisper\.dll|ggml.*\.dll)$' || true)

echo
if [[ -n $bad ]]; then
  echo "NOT STATIC - forbidden imports:"
  printf '  %s\n' $bad
  exit 1
fi
echo "OK: no dynamic CRT / OpenMP / vendored-library imports."
