# Packaging and licensing

Installed builds must carry the exact LibVLC runtime and plugin set they were tested with and
must never depend on a system-wide VLC.

## Runtime layout

`main.rs` sets `VLC_PLUGIN_PATH` before anything else runs (single-threaded), using the first
directory that exists next to the executable:

| Layout | Plugin directory | LibVLC libraries |
|---|---|---|
| Windows (installer or portable zip) | `<app>\plugins\` | `libvlc.dll`, `libvlccore.dll` beside `mono-player.exe` |
| Linux package | `<prefix>/lib/mono-player/plugins/` | `libvlc.so.5`, `libvlccore.so.9` in `<prefix>/lib/mono-player/` with the binary's `RUNPATH` set to `$ORIGIN/../lib/mono-player` |
| macOS `.app` | `Mono Player.app/Contents/Frameworks/plugins/` | `libvlc.dylib`, `libvlccore.dylib` in `Contents/Frameworks/`, binary `rpath` `@executable_path/../Frameworks` |

An explicit `VLC_PLUGIN_PATH` in the environment wins (useful for troubleshooting). Developer
builds without a bundle fall back to the system LibVLC.

LibVLC is initialized lazily on the playback thread when media is first opened. If the plugin
directory is missing or incompatible, `libvlc_new` fails and the UI shows "Playback engine
unavailable" with a retry on the next open; the app itself keeps running. Because `vlc-rs` links
LibVLC at build time, a completely missing `libvlc` shared library is reported by the OS loader
at startup instead — installers must verify both libraries are present.

### Linux

- Build against the LibVLC 3.0.x SDK matching the bundled runtime.
- Bundle `libvlc`, `libvlccore` and the plugin set (start from `vlc-plugin-base` plus
  `vlc-plugin-video-output`; measure before trimming), then run `vlc-cache-gen` on the plugin
  directory at install time so first start does not scan every plugin.
- The app prefers X11 (XWayland under Wayland sessions) because LibVLC 3 embeds video through
  X11 window ids. Document `MONO_PLAYER_WAYLAND=1` as unsupported for embedded video.
- Distribution-native packages (deb/rpm) may instead depend on the distro's `libvlc5` and
  `vlc-plugin-base`; in that case test against each supported distribution release.

### Windows

- Link against `libvlc.lib` from the VLC 3.0.x SDK (`sdk/lib`), e.g. by setting
  `RUSTFLAGS="-L <vlc>/sdk/lib"`; ship `libvlc.dll`, `libvlccore.dll` and `plugins\` from the
  same VLC build and architecture.
- Sign `mono-player.exe` and the bundled DLLs.

### macOS

- Take LibVLC from the official VLC 3.0.x macOS app (`Contents/MacOS/lib/` and
  `Contents/MacOS/plugins/`) and copy it into `Contents/Frameworks/`.
- Fix install names (`install_name_tool`) to `@rpath/…`, sign every dylib and plugin, and
  notarize the bundle.

## Clean-machine checklist

1. Install on a machine without VLC; open audio, 1080p H.264 video, a subtitle file and a
   network stream.
2. Rename the plugin directory: the app must start and report "Playback engine unavailable".
3. Upgrade over a previous version; uninstall and confirm no files remain except the settings
   file (`settings.json`), which the uninstaller may offer to remove.

## Licensing review (required before distribution)

Review the licenses of the exact binaries you ship; obtain legal review for commercial
distribution. Starting points as of this build:

| Component | License (verify for the shipped build) |
|---|---|
| LibVLC (`libvlc`, `libvlccore`) | LGPL-2.1-or-later — permits dynamic linking; ship notices, offer the LibVLC source, allow relinking |
| VLC plugins | Mostly LGPL-2.1-or-later; **some modules are GPL** — audit the bundled set |
| Codec libraries inside plugins (FFmpeg, libdvbpsi, …) | Depends on their build configuration; patent-encumbered codecs may need separate review |
| `vlc-rs` | MIT |
| GPUI Kit, GPUI Base, GPUI Component, `gpui-pre-*` | Apache-2.0 |
| Lucide icons (via `gpui-kit-assets`) | ISC (Feather-derived icons MIT) |
| Other Rust dependencies | Run `cargo about` / `cargo deny check licenses` and include the generated notices |

The application's own license has not been chosen yet; `Cargo.toml` sets `publish = false`
until it is.
