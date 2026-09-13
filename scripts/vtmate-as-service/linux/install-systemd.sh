#!/bin/sh
# Installs vtmate.service (see that file for what it does and why) as a
# systemd --user unit and starts it now.
set -eu

dir="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/.config/systemd/user"
cp "$dir/vtmate.service" "$HOME/.config/systemd/user/vtmate.service"
systemctl --user daemon-reload
systemctl --user enable --now vtmate

echo "Installed. Check it with: systemctl --user status vtmate"
echo "Uninstall with: ./uninstall-systemd.sh"
