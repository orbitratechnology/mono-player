#!/usr/bin/env bash
# Assemble "Mono Player.app" around a built binary and the LibVLC runtime from an official
# VLC.app (libvlc, libvlccore and the plugins folder), then ad-hoc sign it.
#
# Usage: scripts/package-macos.sh <path/to/VLC.app> <path/to/mono-player binary> <output dir>
set -euo pipefail

VLC_APP=$1
BINARY=$2
OUT=$3

APP="$OUT/Mono Player.app"
CONTENTS="$APP/Contents"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Frameworks" "$CONTENTS/Resources"

cp "$BINARY" "$CONTENTS/MacOS/mono-player"
# The whole lib folder, symlinks preserved (libvlc.dylib -> libvlc.5.dylib, …).
ditto "$VLC_APP/Contents/MacOS/lib" "$CONTENTS/Frameworks"
# src/main.rs points VLC_PLUGIN_PATH at Contents/Frameworks/plugins.
ditto "$VLC_APP/Contents/MacOS/plugins" "$CONTENTS/Frameworks/plugins"

# The build adds the CI machine's VLC lib folder as an rpath for running tests; drop every
# absolute rpath so the shipped binary only looks inside its own bundle.
while read -r rpath; do
  case "$rpath" in
    @*) ;;
    *) install_name_tool -delete_rpath "$rpath" "$CONTENTS/MacOS/mono-player" ;;
  esac
done < <(otool -l "$CONTENTS/MacOS/mono-player" | awk '/LC_RPATH/{getline; getline; print $2}')

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Mono Player</string>
  <key>CFBundleDisplayName</key><string>Mono Player</string>
  <key>CFBundleIdentifier</key><string>app.monoplayer.MonoPlayer</string>
  <key>CFBundleExecutable</key><string>mono-player</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.entertainment</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "--- linkage"
otool -L "$CONTENTS/MacOS/mono-player" | grep -i vlc
otool -l "$CONTENTS/MacOS/mono-player" | awk '/LC_RPATH/{getline; getline; print "rpath: " $2}'

# Ad-hoc signature: required on Apple silicon. Not notarized.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
echo "Packaged $APP"
