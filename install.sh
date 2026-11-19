#!/usr/bin/env bash
# Builds NagaButtons, copies it to Applications and starts it at login with a
# LaunchAgent that restarts it after a crash (Quit from the menu stays quit).
# APP_DIR: install folder (/Applications by default). SIGN_ID: see bundle.sh.
set -euo pipefail
cd "$(dirname "$0")"
APP_DIR="${APP_DIR:-/Applications}"
LABEL=io.github.fol2.nagabuttons
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

./bundle.sh
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
rm -rf "$APP_DIR/NagaButtons.app"
ditto dist/NagaButtons.app "$APP_DIR/NagaButtons.app"
[[ "$APP_DIR" == /Applications ]] || ln -sfn "$APP_DIR/NagaButtons.app" /Applications/NagaButtons.app

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$LABEL</string>
<key>ProgramArguments</key><array><string>$APP_DIR/NagaButtons.app/Contents/MacOS/nagabuttons</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>ThrottleInterval</key><integer>10</integer>
<key>ProcessType</key><string>Interactive</string>
<key>StandardErrorPath</key><string>$HOME/Library/Logs/NagaButtons.log</string>
</dict></plist>
EOF
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "Installed $APP_DIR/NagaButtons.app; log: ~/Library/Logs/NagaButtons.log"
