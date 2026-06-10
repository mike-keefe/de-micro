#!/bin/sh
# Builds de_micro.app from the release binary, then zips it for sharing.
set -e
cd "$(dirname "$0")"

cargo build --release

APP="de_micro.app"
rm -rf "$APP" de_micro.app.zip
mkdir -p "$APP/Contents/MacOS"

cp target/release/de-micro "$APP/Contents/MacOS/de_micro"

cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>de_micro</string>
    <key>CFBundleDisplayName</key>
    <string>de_micro</string>
    <key>CFBundleIdentifier</key>
    <string>dev.mikekeefe.de-micro</string>
    <key>CFBundleExecutable</key>
    <string>de_micro</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
</dict>
</plist>
EOF

codesign --force --sign - "$APP"
zip -qry de_micro.app.zip "$APP"

echo "built: $APP and de_micro.app.zip"
echo "receiver may need: xattr -dr com.apple.quarantine de_micro.app (or right-click > Open)"
