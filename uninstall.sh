#!/usr/bin/env bash
# Quits NagaButtons, removes it and gives the side buttons back their keys.
# Remove it from Privacy & Security and Login Items yourself.
set -euo pipefail
APP_DIR="${APP_DIR:-/Applications}"
osascript -e 'quit app "NagaButtons"' 2>/dev/null || true
launchctl bootout "gui/$(id -u)/io.github.fol2.nagabuttons" 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/io.github.fol2.nagabuttons.plist"
rm -rf "$APP_DIR/NagaButtons.app" /Applications/NagaButtons.app
hidutil property --matching '{"VendorID":5426,"ProductID":180}' --set '{"UserKeyMapping":[]}' >/dev/null
echo "NagaButtons removed."
