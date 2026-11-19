#!/usr/bin/env bash
# Builds NagaButtons, copies it to Applications and opens it. On first launch
# it adds itself to Login Items; "Start at Login" in its menu turns that off.
# APP_DIR: install folder (/Applications by default). SIGN_ID: see bundle.sh.
set -euo pipefail
cd "$(dirname "$0")"
APP_DIR="${APP_DIR:-/Applications}"

./bundle.sh
osascript -e 'quit app "NagaButtons"' 2>/dev/null || true
# Version 0.1.0 used a LaunchAgent. launchd starts the binary outside
# LaunchServices, and macOS then may not match it to its privacy grants.
launchctl bootout "gui/$(id -u)/io.github.fol2.nagabuttons" 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/io.github.fol2.nagabuttons.plist"
rm -rf "$APP_DIR/NagaButtons.app"
ditto dist/NagaButtons.app "$APP_DIR/NagaButtons.app"
[[ "$APP_DIR" == /Applications ]] || ln -sfn "$APP_DIR/NagaButtons.app" /Applications/NagaButtons.app
open "$APP_DIR/NagaButtons.app"
echo "Installed and opened $APP_DIR/NagaButtons.app"
