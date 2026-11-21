//! Translating user intent (opened/dropped paths, key presses) into playback requests.
//! Pure functions: no I/O happens here (existence checks run on the playback thread).

use std::path::PathBuf;

use crate::playback::MediaSource;
use crate::playback::protocol::is_subtitle_path;
use crate::playback::state::{SEEK_STEP, SEEK_STEP_LARGE};

/// Paths from the file picker or a drop, split by role.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OpenedPaths {
    pub media: Vec<MediaSource>,
    pub subtitles: Vec<PathBuf>,
}

/// Split paths into media to queue and subtitle files to attach to the current item.
pub fn classify_paths(paths: impl IntoIterator<Item = PathBuf>) -> OpenedPaths {
    let mut opened = OpenedPaths::default();
    for path in paths {
        if is_subtitle_path(&path) {
            opened.subtitles.push(path);
        } else {
            opened.media.push(MediaSource::File(path));
        }
    }
    opened
}

/// Signed seek offset for an arrow-key step.
pub fn seek_offset_ms(forward: bool, large: bool) -> i64 {
    let step = if large { SEEK_STEP_LARGE } else { SEEK_STEP };
    let ms = step.as_millis() as i64;
    if forward { ms } else { -ms }
}

/// The side panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Queue,
    Tracks,
    Effects,
    Settings,
}

impl Panel {
    pub const ALL: [Self; 4] = [Self::Queue, Self::Tracks, Self::Effects, Self::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::Tracks => "Tracks",
            Self::Effects => "Effects",
            Self::Settings => "Settings",
        }
    }
}

/// Everything the user can ask the player to do, from a key binding or from the floating
/// controls (which live in their own window and call `PlayerView::perform`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiCommand {
    TogglePlayback,
    Stop,
    Previous,
    Next,
    Seek {
        forward: bool,
        large: bool,
    },
    VolumeUp,
    VolumeDown,
    ToggleMute,
    SpeedUp,
    SpeedDown,
    TogglePanel(Panel),
    ClosePanel,
    ToggleFullscreen,
    ExitFullscreen,
    OpenFiles,
    NextChapter,
    PreviousChapter,
    NextFrame,
    Snapshot,
    DismissError,
    Quit,
    /// Pointer or keyboard activity: reveal the controls.
    Activity,
}

/// "Previous" restarts the current item when more than this far into it.
pub const RESTART_THRESHOLD_MS: i64 = 3_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_media_and_subtitles() {
        let opened = classify_paths([
            PathBuf::from("/m/film.mkv"),
            PathBuf::from("/m/film.en.srt"),
            PathBuf::from("/m/Ünïcode song.flac"),
        ]);
        assert_eq!(opened.media.len(), 2);
        assert_eq!(opened.subtitles, vec![PathBuf::from("/m/film.en.srt")]);
    }

    #[test]
    fn seek_offsets() {
        assert_eq!(seek_offset_ms(true, false), 5_000);
        assert_eq!(seek_offset_ms(false, true), -30_000);
    }
}
