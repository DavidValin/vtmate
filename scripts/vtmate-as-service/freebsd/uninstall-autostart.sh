#!/bin/sh
# Removes what install-autostart.sh installed.
set -eu

rm -f "$HOME/.config/autostart/vtmate.desktop"

echo "Removed."
