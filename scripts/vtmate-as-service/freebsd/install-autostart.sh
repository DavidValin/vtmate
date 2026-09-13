#!/bin/sh
# Installs vtmate-autostart.desktop (see that file for what it does and why,
# including the FreeBSD-specific caveats) as an XDG autostart entry.
set -eu

dir="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/.config/autostart"
cp "$dir/vtmate-autostart.desktop" "$HOME/.config/autostart/vtmate.desktop"

echo "Installed. vtmate starts next time you log in, or right now: vtmate --daemon"
echo "Uninstall with: ./uninstall-autostart.sh"
