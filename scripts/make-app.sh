#!/usr/bin/env bash
# Builds "Gemini API.app" from the release binary (run on macOS).
set -euo pipefail

APP_NAME="Gemini API.app"
ZIP_NAME="Gemini-API-macOS.zip"
BIN="target/release/gemini-api"
VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"

if [[ ! -f "$BIN" ]]; then
  echo "error: $BIN not found — run 'cargo build --release' first" >&2
  exit 1
fi

rm -rf "$APP_NAME" "$ZIP_NAME"
mkdir -p "$APP_NAME/Contents/MacOS" "$APP_NAME/Contents/Resources"

cp "$BIN" "$APP_NAME/Contents/MacOS/gemini-api"

cat > "$APP_NAME/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Gemini API</string>
    <key>CFBundleDisplayName</key>
    <string>Gemini API</string>
    <key>CFBundleIdentifier</key>
    <string>dev.gemcli.gemini-api</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>gemini-api</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Ad-hoc signature (enough for personal use; Gatekeeper note in the README)
codesign --force --sign - "$APP_NAME" >/dev/null 2>&1 || true

# Zip preserving the bundle structure and executable bits
ditto -c -k --keepParent "$APP_NAME" "$ZIP_NAME"

echo "built: $APP_NAME ($ZIP_NAME, v$VERSION)"
