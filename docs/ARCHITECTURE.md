# Architecture

Mono Player is a thin application layer around LibVLC. The rules from the PRD that shape the
code: LibVLC is the only playback engine; the GPUI thread never waits on playback or file I/O;
video goes straight into a native surface; the UI reacts to engine events rather than polling.

```text
 GPUI thread                                    playback thread ("mono-playback")
┌───────────────────────────────┐   Command    ┌──────────────────────────────────┐
│ PlayerView (features/player)  │ ───────────▶ │ Actor (playback/actor.rs)        │
│  PlayerModel  ◀── apply(Event)│  std mpsc,   │  owns Runtime + Player (LibVLC)  │
│  Playlist, Settings           │  never blocks│  lazy LibVLC start               │
│  VideoSurface (native child)  │              │  throttled progress (8/s)        │
└──────────────▲────────────────┘              └──────────────┬───────────────────┘
               │ Event (futures mpsc; drained in batches,      │ EngineEvent tagged with
               │ one redraw per batch)                          │ the RequestId of its media
               └────────────────────────────────────────────────┤
                                                    LibVLC callback threads
                                                    (events.rs: normalize, never block,
                                                     never unwind, detach on unload)
```

## Modules

| Path | Role |
|---|---|
| `src/actions.rs` | Actions and the single shortcut table used for key bindings and help |
| `src/theme.rs` | Monochrome tokens, metrics (44/48 px targets), GPUI Kit theme mapping |
| `src/components/` | Stateless `RenderOnce` controls: transport, timeline, volume, queue row |
| `src/features/player/` | `PlayerView` (root view), `PlayerModel` (event reducer), `PlaybackController` (actor handle, request ids, respawn), `commands` (pure input mapping) |
| `src/features/playlist/` | `Playlist` model (stable ids, repeat, shuffle) and the queue panel |
| `src/features/settings/` | Preference toggles and the settings panel |
| `src/playback/` | Protocol (`Command`/`Event`), state helpers, typed errors, the actor |
| `src/playback/vlc_adapter/` | The only code touching `vlc-rs` or raw LibVLC symbols |
| `src/video_output/` | Native child surface per platform behind one `VideoSurface` API |
| `src/persistence/` | Versioned settings file with migration and atomic writes |

## Threading and ownership

- `vlc-rs` types are `!Send`. The actor creates, uses and drops the LibVLC `Instance`,
  `MediaPlayer` and `Media` on its own thread. Field order in `Actor` and `Player` encodes the
  release order: callbacks detached → media released → player released → instance released.
- The UI talks to the actor through `PlaybackController`, which never blocks: `send` enqueues on
  an unbounded std channel (commands are user-paced). If the playback thread has died, the
  controller restarts it and replays volume, mute, rate and the video target.
- LibVLC callbacks (`vlc_adapter/events.rs`) run on LibVLC threads. They read the event union,
  drop `TimeChanged` events closer than 50 ms, and push a small enum into the actor queue. They
  are wrapped in `catch_unwind` and never touch GPUI.
- `vlc-rs` 0.3 leaks callback closures and cannot detach them, so the adapter attaches with its
  own context and calls `libvlc_event_detach` before freeing it. A subscription is created per
  media item and detached when the item is unloaded, so no callback for a replaced item can run
  afterwards.

## Event flow and stale events

Every `Open` carries a `RequestId`. The actor tags engine events with the request of the media
that produced them and drops events for anything but the current request; `PlayerModel::apply`
filters again on the UI side. Tests open 15 items in rapid succession and assert that nothing
from earlier requests arrives once the last one plays.

Progress is capped at 8 updates per second in the actor (`PROGRESS_INTERVAL`). The UI drains all
queued events per wake-up and redraws once. While the user drags the timeline, progress updates
do not move the slider; a single `SeekTo` is sent on release.

No timers run while idle: the actor blocks on its queue (it uses a 150 ms timeout only while a
track-list refresh is pending), the UI has no polling loop, and the fullscreen auto-hide timer
exists only during fullscreen video playback after user activity.

## Video surface and layout

GPUI renders its UI on the GPU; LibVLC renders into a native child surface positioned over the
video rectangle. GPUI cannot draw over a native child, so the layout never overlays GPUI
content on the video: the control bar, error banner and side panel sit beside or below the video
rectangle, which shrinks to make room (also in fullscreen when the controls are revealed). The
surface is hidden whenever there is nothing to show (no media, opening, audio-only, error), and
GPUI paints the empty state, spinner or now-playing card in that area instead.

The video rectangle comes from a `canvas` element's prepaint; `VideoSurface::set_bounds` is a
no-op unless the rectangle changed, and the X11 implementation sends fire-and-forget requests,
so resizing never waits on the display server.

Platform specifics live in `src/video_output/{linux,windows,macos}.rs`:

- **X11** — a black `InputOutput` child for LibVLC plus an `InputOnly` sibling stacked above
  it. LibVLC's X11 embedding always selects `PointerMotion` on its drawable, and X stops
  propagating an event at the first window where a client selected it; the input-only sibling
  selects nothing, so pointer events reach the GPUI window (needed to reveal fullscreen
  controls). On Wayland sessions the app requests GPUI's X11 backend (XWayland).
- **Windows** — a child `HWND` whose window procedure answers `WM_NCHITTEST` with
  `HTTRANSPARENT` and forwards stray mouse messages to the parent.
- **macOS** — an `NSView` subclass whose `hitTest:` returns nil, making it and LibVLC's subview
  transparent to the mouse.

Lifetime: the surface is destroyed only after the actor has released the player. Closing the
window or pressing Ctrl/Cmd+Q hides the surface, asks the actor to shut down, waits for its
completion signal (bounded at 3 s), saves settings on the background executor, then quits.

## Errors

`PlaybackError` is a typed enum with a plain-language title and message (with the next useful
action). Errors that end an item mark it failed in the queue; when the failure came from
auto-advance, the queue moves on. Recoverable errors (seek unsupported, subtitle load failed,
track unavailable) leave playback running. Errors show in a dismissable banner, never a modal.

Two engine quirks are handled explicitly:

- LibVLC's demuxer fallback can "play" unrecognizable data and end immediately without an error.
  An item that ends with no length, no position, no video output and no audio track is reported
  as `PlaybackFailed`.
- `vlc-rs` 0.3's track-description helpers skip the last list entry; the adapter walks the list
  itself.

## Persistence

`settings.json` in the platform config directory (or `MONO_PLAYER_CONFIG_DIR`) holds volume,
mute, repeat mode, window geometry, privacy switches and up to 20 recent items with optional
resume positions. It is read once before the window opens, written atomically (temp file +
rename) on the background executor with a 600 ms debounce, and migrated by version. Corrupt
files fall back to defaults.
