# Phase 0 spike report: GPUI + LibVLC video output

PRD §7 requires proving, per platform, that LibVLC can render efficiently inside the GPUI window
before the full UI is committed. This report records what was verified, how, and what remains.

## Locked combination

| Component | Version |
|---|---|
| GPUI Kit | 0.7.1 (`gpui-pre-*` 0.3.8, resolved by GPUI Kit; `Cargo.lock` committed) |
| `vlc-rs` | 0.3.0 (crates.io; last release of the garkimasera repository) |
| LibVLC | 3.0.20 "Vetinari" (Ubuntu 24.04 `libvlc5`, `vlc-plugin-base`, `vlc-plugin-video-output`) |
| Rust | 1.97.0 stable |

`vlc-rs` 0.3.0 covers `Instance`, `Media`, `MediaPlayer`, playback state, seeking, volume,
rate and the platform drawable setters. Gaps found and handled inside `playback::vlc_adapter`
through the binding's `sys` module or reviewed `extern "C"` declarations:

- No event detach and leaked callback closures → own attach/detach with an owned context.
- Track-description helpers drop the last entry → own list walk.
- No subtitle slave, spu selection, delays or length getters → `sys` calls.
- `Media::new_location` panics on interior NUL → URLs are validated (no control characters)
  before reaching it; Unix paths are passed as byte-exact percent-encoded `file://` URIs so
  non-UTF-8 names survive.

## Strategy chosen

Fallback order item 1: **direct native output into a validated child surface**. Zero
per-frame CPU copies in the application; LibVLC's video output draws into the child
window/view itself. GPUI controls are laid out beside the surface (never overlaid), which
removes z-order and hit-testing conflicts by construction.

## Linux / X11 — verified

Environment: Xvfb 1280×800×24, openbox (EWMH, for fullscreen), Mesa lavapipe (software Vulkan)
for GPUI and llvmpipe for LibVLC's OpenGL output. No GPU and no audio device, so absolute
CPU numbers below are worst-case software rendering and not representative of user hardware.

| Spike objective | Result |
|---|---|
| 1. GPUI window | ✅ |
| 2. Native video target | ✅ X11 `InputOutput` child + `InputOnly` input overlay |
| 3. Attach LibVLC | ✅ `set_xwindow` before `play`; title overlay and VLC input handling disabled |
| 4. Play local 1080p H.264 | ✅ 1920×1080 30 fps H.264/AAC, aspect preserved (letterboxed) |
| 5. Resize without corruption | ✅ window resized 1120×700 → 800×500 while playing; side panel open/close re-lays out the surface |
| 6. GPUI controls without stacking/hit-test problems | ✅ controls outside the video rect; pointer over the video reaches GPUI (see finding below) |
| 7. Fullscreen enter/exit | ✅ `F`/`Esc`; controls auto-hide after 2.5 s and reappear on pointer movement over the video |
| 8. Stop and reopen repeatedly | ✅ integration test opens 15 items back to back; no stale events after the last |
| 9. Clean exit | ✅ Ctrl+Q and window-manager close both release LibVLC first (≈35–140 ms), process exits in ≈200–300 ms |
| 10. Measurements | See below |

### Finding: LibVLC swallows pointer motion on its drawable

`xwininfo -events` showed LibVLC's X11 embedding selects `PointerMotion` on the window it is
given even with `libvlc_video_set_mouse_input(false)` (it uses it to auto-hide the cursor). X
stops event propagation at the first window where any client selected the event, so GPUI's
XInput2 motion selection on the top-level never saw movement over the video, and fullscreen
controls could not be revealed. Fix: an `InputOnly` sibling with the same geometry stacked
above the video window; it selects nothing, so pointer events propagate from it directly to the
GPUI window and never pass through LibVLC's window. Verified with `xdotool` in fullscreen.

### Finding: silent demuxer fallback

A text file named `.mkv` was "played" by LibVLC's fallback demuxer and ended instantly with no
`EncounteredError`. The actor now reports an item that ends with no length, no position, no
video output and no audio track as `PlaybackFailed`.

### Measurements (Linux, software rendering)

| Metric | Debug build | Release build |
|---|---|---|
| LibVLC initialized at startup | No (lazy; first media request) | No |
| LibVLC `libvlc_new` time on first open | 170–390 ms | 156 ms |
| Idle CPU, no media, 20 s average | 0.8 % of one core | 0.65 % of one core |
| Idle RSS, no media | 181 MB | 179 MB |
| 1080p30 H.264 playback CPU (software decode + llvmpipe GL + lavapipe UI) | 51–91 % of one core | 78 % of one core |
| 1080p30 playback RSS | 421 MB | 416 MB |
| Cold launch to visible window (5 runs) | — | 183–215 ms |
| Progress events per second while playing | ≤ 8 (asserted ≤ 10 in tests) | same |
| Teardown (stop + release LibVLC) | 35–140 ms | 66 ms |

The idle RSS includes Mesa's software Vulkan driver; the PRD's ≤ 100 MB idle target must be
measured on reference hardware with a real GPU driver before it is judged. Playback CPU here is
dominated by software decoding and software OpenGL; with VA-API/VDPAU available LibVLC selects
hardware decoding by default (we do not force a decoder).

## Linux / Wayland

GPUI supports native Wayland, but LibVLC 3 can only embed through X11 window ids. When `DISPLAY`
is set, Mono Player asks GPUI for its X11 backend (`WindowingRequest::Windowed` with the Wayland
display removed), so Wayland sessions run under XWayland with the verified X11 path.
`MONO_PLAYER_WAYLAND=1` keeps native Wayland; video then falls back to LibVLC's own window
(PRD fallback 2). **To validate:** GNOME and KDE Wayland sessions with XWayland, fractional
scaling, and fullscreen.

## Windows — implemented, not yet run

Child `HWND` (`WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN`) passed to `set_hwnd`; its window
procedure returns `HTTRANSPARENT` for `WM_NCHITTEST` and forwards stray mouse messages to the
parent. Type-checked and Clippy-clean for `x86_64-pc-windows-msvc`. **To validate:** that
GPUI's DirectX/DirectComposition swap chain does not draw over child windows, that pointer
motion over LibVLC's inner window reaches GPUI (LibVLC disables its window when mouse input is
off), DPI scaling, and the full objective list above.

## macOS — implemented, not yet run

`NSView` subclass added as a subview of GPUI's content view and passed to `set_nsobject`;
`hitTest:` returns nil so the subtree is transparent to the mouse; frames are flipped unless the
parent is flipped. Type-checked and Clippy-clean for `aarch64-apple-darwin`. **To validate:**
layering with GPUI's Metal layer-backed view, Retina scaling, native fullscreen spaces, and the
full objective list above.

## Gate status

| Platform | Gate |
|---|---|
| Linux X11 / XWayland | **Pass** (software rendering; repeat on reference hardware for the performance budget) |
| Windows | Open — requires a run on Windows hardware |
| macOS | Open — requires a run on macOS hardware |
