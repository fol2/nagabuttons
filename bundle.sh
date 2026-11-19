#!/usr/bin/env bash
# Builds dist/NagaButtons.app (release binary, icon, Info.plist, signature).
# SIGN_ID: codesigning identity; "-" (ad-hoc) by default. A stable identity
# keeps the macOS permissions across rebuilds; ad-hoc needs them granted again.
set -euo pipefail
cd "$(dirname "$0")"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP=dist/NagaButtons.app

cargo build --release
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/nagabuttons "$APP/Contents/MacOS/nagabuttons"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
swift icon.swift "$WORK/icon.png"
mkdir "$WORK/AppIcon.iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s "$WORK/icon.png" --out "$WORK/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$WORK/icon.png" --out "$WORK/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/AppIcon.iconset" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.fol2.nagabuttons</string>
<key>CFBundleName</key><string>NagaButtons</string>
<key>CFBundleDisplayName</key><string>NagaButtons</string>
<key>CFBundleExecutable</key><string>nagabuttons</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>LSUIElement</key><true/>
<key>NSHumanReadableCopyright</key><string>MIT licence</string>
</dict></plist>
EOF
codesign --force --sign "${SIGN_ID:--}" --identifier io.github.fol2.nagabuttons "$APP"
codesign --verify --strict "$APP"
echo "Built $APP ($VERSION)"
