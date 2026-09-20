#!/usr/bin/env bash
# Regenerates LICENSE.third_party: the license text of every Rust crate
# that goes into the released binaries (see scripts/about.toml and scripts/about.hbs), followed
# by the hand-written native libraries and models section.
# Needs cargo-about:  cargo install cargo-about --locked --features cli
set -euo pipefail
cd "$(dirname "$0")/.."

# vtmate's own LICENSE is a custom one, which cargo-about cannot classify, so
# it is clarified against the file's current hash (a changed LICENSE never
# trips the check).
cfg="$(mktemp)"
trap 'rm -f "$cfg"' EXIT
{
  cat scripts/about.toml
  cat <<EOF

[vtmate.clarify]
license = "LicenseRef-vtmate"

[[vtmate.clarify.files]]
path = "LICENSE"
checksum = "$(sha256sum LICENSE | cut -d' ' -f1)"
EOF
} > "$cfg"

# The Rust crates are generated. The C/C++ libraries and the models are not
# crates, so their licenses are kept by hand at the end of the same file, after
# the marker line, and carried over here.
out=LICENSE.third_party
marker='<<< everything below this line is maintained by hand'
manual="$(mktemp)"
generated="$(mktemp)"
trap 'rm -f "$cfg" "$manual" "$generated"' EXIT
grep -qF "$marker" "$out" || { echo "marker line missing from $out" >&2; exit 1; }
awk -v m="$marker" 'index($0, m) { f = 1 } f' "$out" > "$manual"

cargo about generate --config "$cfg" --fail scripts/about.hbs > "$generated"
{ cat "$generated"; echo; cat "$manual"; } > "$out"
echo "wrote LICENSE.third_party ($(wc -c < LICENSE.third_party) bytes)"
