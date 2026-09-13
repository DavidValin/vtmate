#!/bin/sh
# Installs com.vtmate.daemon.plist (see that file for what it does and why)
# as a per-user launchd agent and starts it now.
set -eu

dir="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/Library/LaunchAgents"
cp "$dir/com.vtmate.daemon.plist" "$HOME/Library/LaunchAgents/com.vtmate.daemon.plist"
# bootstrap errors if the label is already loaded (unlike systemctl enable,
# this isn't idempotent on its own) - bootout first, ignoring failure, so
# re-running this script to pick up an edited plist just works.
launchctl bootout "gui/$(id -u)/com.vtmate.daemon" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.vtmate.daemon.plist"

echo "Installed. Check it with: launchctl print gui/$(id -u)/com.vtmate.daemon"
echo "Uninstall with: ./uninstall-launchd.sh"
