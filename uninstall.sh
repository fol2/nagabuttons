#!/usr/bin/env bash
# Stops NagaButtons, removes the LaunchAgent and the app, and gives the side
# buttons back their keys. Remove it from Privacy & Security yourself.
set -euo pipefail
APP_DIR="${APP_DIR:-/Applications}"
LABEL=io.github.fol2.nagabuttons
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
osascript -e 'quit app "NagaButtons"' 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
rm -rf "$APP_DIR/NagaButtons.app" /Applications/NagaButtons.app
hidutil property --matching '{"VendorID":5426,"ProductID":180}' --set '{"UserKeyMapping":[]}' >/dev/null
echo "NagaButtons removed."
