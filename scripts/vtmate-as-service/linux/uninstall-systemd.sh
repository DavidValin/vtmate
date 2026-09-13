#!/bin/sh
# Removes what install-systemd.sh installed.
set -eu

systemctl --user disable --now vtmate 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/vtmate.service"
systemctl --user daemon-reload

echo "Removed."
