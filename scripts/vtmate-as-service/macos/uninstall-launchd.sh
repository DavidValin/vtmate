#!/bin/sh
# Removes what install-launchd.sh installed.
set -eu

launchctl bootout "gui/$(id -u)/com.vtmate.daemon" 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/com.vtmate.daemon.plist"

echo "Removed."
