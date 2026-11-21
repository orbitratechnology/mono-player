#!/usr/bin/env bash
# Build "Mono Player.app" on a Mac in one step.
#
# Uses the VLC.app in /Applications (or $VLC_APP) as the LibVLC source; if there is none, it
# downloads the official VLC release into target/vlc first. The finished, self-contained app
# is written to dist/ — LibVLC and its plugins are copied inside it.
#
# Usage: scripts/build-macos-app.sh
set -euo pipefail
cd "$(dirname "$0")/.."

VLC_VERSION=3.0.21

if [[ -z "${VLC_APP:-}" ]]; then
  for candidate in "/Applications/VLC.app" "$HOME/Applications/VLC.app" "target/vlc/VLC.app"; do
    if [[ -f "$candidate/Contents/MacOS/lib/libvlc.dylib" ]]; then
      VLC_APP=$candidate
      break
    fi
  done
fi

if [[ -z "${VLC_APP:-}" ]]; then
  echo "No VLC.app found; downloading VLC ${VLC_VERSION} (used only as the LibVLC source)…"
  mkdir -p target/vlc
  dmg=target/vlc/vlc.dmg
  curl -fL --progress-bar -o "$dmg" \
    "https://download.videolan.org/pub/videolan/vlc/${VLC_VERSION}/macosx/vlc-${VLC_VERSION}-universal.dmg"
  mount=$(mktemp -d)
  hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" >/dev/null
  ditto "$mount/VLC.app" target/vlc/VLC.app
  hdiutil detach "$mount" >/dev/null
  VLC_APP=target/vlc/VLC.app
fi

VLC_APP=$(cd "$(dirname "$VLC_APP")" && pwd)/$(basename "$VLC_APP")
echo "Using LibVLC from $VLC_APP"
export VLC_LIB_DIR="$VLC_APP/Contents/MacOS/lib"

cargo build --release --locked
scripts/package-macos.sh "$VLC_APP" target/release/mono-player dist

echo
echo "Done: dist/Mono Player.app"
echo "Run it with:  open \"dist/Mono Player.app\""
