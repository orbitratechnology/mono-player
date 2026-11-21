# Mono Player

A fast, low-overhead desktop media player with a strictly monochrome, soft interface.
Rust + [GPUI Kit](https://gpui-kit.com) 0.10 for the UI, LibVLC (through
[`vlc-rs`](https://crates.io/crates/vlc-rs)) as the playback engine, rendering video
directly into a native child surface — no per-frame CPU copies — with the controls floating
over the picture in their own window.

![Mono Player playing a 1080p test video with the floating controls](docs/images/playing.png)

| Equalizer & picture | Settings | Tracks, chapters & sync |
|---|---|---|
| ![Effects panel](docs/images/effects.png) | ![Settings panel](docs/images/settings.png) | ![Tracks panel](docs/images/tracks.png) |

| Queue | Now playing (audio) | Empty state |
|---|---|---|
| ![Queue panel](docs/images/queue.png) | ![Audio with metadata](docs/images/audio.png) | ![Empty state](docs/images/empty.png) |

## Status

| Platform | Video output | Floating controls | Status |
|---|---|---|---|
| Linux, X11 | X11 child window + InputOnly input window | Child window (reparented, Shape-rounded, opaque) | **Verified** (see [spike report](docs/SPIKE-REPORT.md)) |
| Linux, Wayland session | XWayland (same X11 path) | Same | Implemented; needs validation on a real compositor |
| Linux, native Wayland (`MONO_PLAYER_WAYLAND=1`) | LibVLC's own window (PRD fallback 2) | — | Fallback only |
| Windows | Child `HWND` | Owned, non-topmost popup (translucent) | Implemented and type-checked; **not yet run on Windows** |
| macOS | `NSView` subview | Child `NSPanel` (translucent) | App launches and plays in CI; **video picture not yet confirmed on a real Mac** |

The PRD's Phase 0 gate passes on Linux/X11. Windows and macOS adapters exist behind the same
interface and compile for their targets, but must pass the same spike checklist on real hardware
before release.

## Features

- **Floating controls** over the video: a soft rounded pill with title/now playing, timeline,
  transport, volume, speed and panel shortcuts. They hide after 2.5 s of untouched video
  playback, stay while hovered or paused, and return on any pointer movement or key.
- **Translucent window** (macOS vibrancy / Windows Mica-style blur by default; opt-in on
  Linux) with soft glass panels and cards — still strictly black, white and grays.
- Open files (picker, drag and drop, command line), stream URLs (http/https/rtsp/…), Unicode
  and space-containing paths.
- Play/pause/stop, seek (one engine seek on slider release), ±5 s / ±30 s keyboard seeking,
  volume and mute, speed 0.5×–2×, fullscreen.
- **From LibVLC**:
  - 10-band equalizer with LibVLC's 18 built-in presets and a preamp, remembered across
    sessions; stereo / reverse / left / right / Dolby channel modes; output device choice.
  - Picture: brightness, contrast, saturation, hue and gamma; aspect ratio, crop and zoom;
    deinterlacing (blend, bob, linear, yadif, yadif 2×).
  - Audio, subtitle and video tracks; external subtitles; audio and subtitle delay.
  - DVD/Blu-ray titles and chapters with times, chapter stepping, frame stepping, snapshots
    (to `Pictures/Mono Player`).
  - Metadata: title, artist, album and stream "now playing" (ICY), with cover art.
  - Hardware decoding toggle (VA-API, VideoToolbox, D3D11) and network buffer length.
- Session queue: add, remove, reorder, previous/next, repeat off/all/one, shuffle, virtualized
  list, failed items marked in place.
- Recent media, resume where playback stopped, remembered volume, equalizer and window
  geometry — in one small versioned JSON file, with switches to disable and clear history.
- Plain-language errors without modals: missing file, permission, folder, invalid/unsupported
  address, unsupported or corrupt media, unreachable stream, engine start failure.

## Building

Requirements: a recent stable Rust toolchain (developed with 1.97) and the LibVLC 3 SDK.

```sh
# Debian/Ubuntu
sudo apt install libvlc-dev vlc-plugin-base \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
  libvulkan-dev libfontconfig-dev libfreetype-dev libxcb1-dev

cargo run --release -- path/to/video.mkv path/to/video.srt
```

### macOS

LibVLC comes from VLC itself. With [VLC](https://www.videolan.org/vlc/) installed in
`/Applications`, `cargo run --release -- video.mp4` works directly: `build.rs` finds
`VLC.app` and links against its `libvlc.dylib`. Otherwise set `VLC_LIB_DIR` to the folder
containing `libvlc.dylib`.

To build a self-contained `Mono Player.app` (LibVLC and plugins copied inside; downloads VLC
first if it isn't installed):

```sh
scripts/build-macos-app.sh
open "dist/Mono Player.app"
```

### Windows

Set `VLC_LIB_DIR` to the VLC SDK's `sdk/lib` folder (containing `libvlc.lib`) and ship
`libvlc.dll`, `libvlccore.dll` and `plugins\` beside the executable; see
[docs/PACKAGING.md](docs/PACKAGING.md).

Debug builds compile all dependencies with `opt-level = 2` (GPUI Kit's guidance) so the UI is
representative while our own crate stays fast to rebuild.

## Keyboard shortcuts

| Key | Action | Key | Action |
|---|---|---|---|
| Space | Play / pause | ↑ / ↓ | Volume up / down |
| S | Stop | M | Mute / unmute |
| Ctrl/Cmd+O | Open files | P / N | Previous / next item |
| F | Toggle fullscreen | L | Show / hide queue |
| Esc | Exit fullscreen (or close panel) | T | Tracks, sync & speed |
| ← / → | Seek 5 s | [ / ] | Slower / faster |
| Shift+← / → | Seek 30 s | Ctrl/Cmd+, | Settings |
| Shift+N / Shift+P | Next / previous chapter | Ctrl/Cmd+E | Equalizer & picture |
| E | Next frame | Shift+S | Snapshot |
| Ctrl/Cmd+Q | Quit | | |

Shortcuts are defined once in `src/actions.rs`; the in-app list (Settings panel) is generated
from the same table. Player keys never fire while a text field has focus.

## Testing

```sh
cargo test                          # everything
cargo test --lib                    # pure logic: state, playlist, settings, theme contrast…
cargo test --test playback_adapter  # the playback actor against the installed LibVLC
cargo test --test ui                # the real view in a headless GPUI Kit window
cargo clippy --all-targets -- -D warnings
```

The LibVLC tests generate their own WAV media and do not need an audio device. Set
`MONO_PLAYER_SKIP_LIBVLC_TESTS=1` on machines without LibVLC plugins.

## Configuration

| Variable | Effect |
|---|---|
| `MONO_PLAYER_CONFIG_DIR` | Settings directory (default: platform config dir) |
| `MONO_PLAYER_LOG=1` | Lifecycle tracing on stderr (LibVLC start time, teardown) |
| `MONO_PLAYER_WAYLAND=1` | Use native Wayland instead of XWayland (video then opens in LibVLC's own window) |
| `VLC_PLUGIN_PATH` | Override the LibVLC plugin directory |

Nothing is sent over the network except the media you open. There is no telemetry.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — layers, threading, event flow, lifecycle.
- [Spike report](docs/SPIKE-REPORT.md) — Phase 0 video-surface validation, measurements,
  findings, and what remains to verify per platform.
- [Packaging and licensing](docs/PACKAGING.md) — bundling LibVLC and plugins, notices.
