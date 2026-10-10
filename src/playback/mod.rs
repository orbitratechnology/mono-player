//! Playback: the actor thread, its protocol, and the LibVLC adapter behind it.

mod actor;
pub mod error;
pub mod protocol;
pub mod state;
pub mod vlc_adapter;

pub use actor::PlaybackHandle;
pub use error::PlaybackError;
pub use protocol::{Command, Event, MediaSource, RequestId};
pub use state::{PlaybackState, TrackInfo, Tracks};
