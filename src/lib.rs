//! Mono Player: a thin, monochrome GPUI Kit shell around the LibVLC playback engine.
//!
//! Layering (see `docs/ARCHITECTURE.md`):
//!
//! - [`features`] and [`components`]: GPUI views. They own presentation state only and talk to
//!   playback through typed [`playback::Command`]s and [`playback::Event`]s.
//! - [`playback`]: the playback actor thread. It is the only code that owns LibVLC objects; the
//!   binding itself is confined to [`playback::vlc_adapter`].
//! - [`video_output`]: the native child surface (X11 window / HWND / NSView) LibVLC draws into.
//! - [`persistence`]: the small versioned settings file.

pub mod actions;
pub mod app;
pub mod components;
pub mod diagnostics;
pub mod features;
pub mod persistence;
pub mod playback;
pub mod theme;
pub mod video_output;
