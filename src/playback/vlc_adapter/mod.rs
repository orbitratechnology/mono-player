//! The only module that touches `vlc-rs` or raw LibVLC symbols.
//!
//! Everything here is `!Send` and must stay on the playback thread. The rest of the application
//! sees typed commands, events and errors only.

mod events;
mod instance;
mod media_player;

pub use events::{EngineEvent, EngineSink};
pub use instance::Runtime;
pub use media_player::Player;
