//! Typed playback errors with plain-language messages.

use std::fmt;
use std::path::PathBuf;

/// Everything that can go wrong between a user's request and audible/visible playback.
///
/// The UI shows [`PlaybackError::title`] and [`PlaybackError::message`]; technical detail goes to
/// the local log only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackError {
    /// `libvlc_new` failed: the runtime or its plugin directory is missing or incompatible.
    RuntimeUnavailable,
    /// The runtime started but could not create a media player.
    PlayerUnavailable,
    FileNotFound(PathBuf),
    AccessDenied(PathBuf),
    /// A folder was given where a media file was expected (folders are never scanned).
    NotAFile(PathBuf),
    InvalidUrl(String),
    UnsupportedScheme(String),
    /// LibVLC rejected the media while creating it.
    OpenFailed {
        name: String,
    },
    /// LibVLC reported `EncounteredError` while opening or playing: unsupported codec, corrupt
    /// data, missing plugin, unreachable stream, or audio output failure.
    PlaybackFailed {
        name: String,
        is_stream: bool,
    },
    SeekUnsupported,
    SubtitleLoadFailed(PathBuf),
    TrackUnavailable,
    /// The native video surface could not be created or attached.
    VideoSurfaceUnavailable(String),
    /// The playback thread stopped unexpectedly.
    PlaybackThreadFailed,
}

impl PlaybackError {
    /// A short heading.
    pub fn title(&self) -> &'static str {
        match self {
            Self::RuntimeUnavailable | Self::PlayerUnavailable => "Playback engine unavailable",
            Self::FileNotFound(_) => "File not found",
            Self::AccessDenied(_) => "Access denied",
            Self::NotAFile(_) => "Not a media file",
            Self::InvalidUrl(_) | Self::UnsupportedScheme(_) => "Invalid address",
            Self::OpenFailed { .. } => "Can't open media",
            Self::PlaybackFailed {
                is_stream: true, ..
            } => "Stream unavailable",
            Self::PlaybackFailed { .. } => "Can't play media",
            Self::SeekUnsupported => "Seeking unavailable",
            Self::SubtitleLoadFailed(_) => "Subtitle not loaded",
            Self::TrackUnavailable => "Track unavailable",
            Self::VideoSurfaceUnavailable(_) => "Video output unavailable",
            Self::PlaybackThreadFailed => "Playback stopped",
        }
    }

    /// The explanation plus the next useful action.
    pub fn message(&self) -> String {
        match self {
            Self::RuntimeUnavailable => "LibVLC could not start. Reinstall Mono Player, or check \
                that its bundled VLC plugins folder is present. Then try again."
                .into(),
            Self::PlayerUnavailable => {
                "LibVLC started but could not create a player. Try again.".into()
            }
            Self::FileNotFound(path) => format!(
                "“{}” no longer exists. It may have been moved or deleted.",
                display_name(path)
            ),
            Self::AccessDenied(path) => format!(
                "You don't have permission to read “{}”.",
                display_name(path)
            ),
            Self::NotAFile(path) => format!(
                "“{}” is a folder. Folders aren't scanned — open or drop the files instead.",
                display_name(path)
            ),
            Self::InvalidUrl(url) => format!("“{url}” isn't a valid media address."),
            Self::UnsupportedScheme(scheme) => {
                format!("“{scheme}” addresses aren't supported. Use http, https, rtsp, or a file.")
            }
            Self::OpenFailed { name } => format!("LibVLC couldn't open “{name}”."),
            Self::PlaybackFailed {
                name,
                is_stream: true,
            } => format!(
                "“{name}” couldn't be reached or stopped responding. Check the address and your \
                 connection."
            ),
            Self::PlaybackFailed { name, .. } => format!(
                "“{name}” is unsupported or damaged, or a required codec is missing. Other items \
                 in the queue are unaffected."
            ),
            Self::SeekUnsupported => "This media doesn't support seeking.".into(),
            Self::SubtitleLoadFailed(path) => format!(
                "“{}” couldn't be loaded as a subtitle. Playback continues.",
                display_name(path)
            ),
            Self::TrackUnavailable => "That track couldn't be selected. Playback continues.".into(),
            Self::VideoSurfaceUnavailable(reason) => {
                format!("Video can't be shown in this window ({reason}). Audio still plays.")
            }
            Self::PlaybackThreadFailed => {
                "The playback engine stopped unexpectedly. Open the media again to retry.".into()
            }
        }
    }

    /// Whether this error ends playback of the current item (as opposed to a recoverable notice).
    pub fn ends_item(&self) -> bool {
        !matches!(
            self,
            Self::SeekUnsupported
                | Self::SubtitleLoadFailed(_)
                | Self::TrackUnavailable
                | Self::VideoSurfaceUnavailable(_)
        )
    }
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.title(), self.message())
    }
}

impl std::error::Error for PlaybackError {}

/// The last path component, for messages; falls back to the whole path.
pub fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_plain_and_name_the_file() {
        let error = PlaybackError::FileNotFound(PathBuf::from("/media/Ünïcode film.mkv"));
        assert_eq!(error.title(), "File not found");
        assert!(error.message().contains("Ünïcode film.mkv"));
        assert!(!error.message().contains("/media"));
    }

    #[test]
    fn recoverable_errors_do_not_end_item() {
        assert!(!PlaybackError::SeekUnsupported.ends_item());
        assert!(!PlaybackError::SubtitleLoadFailed(PathBuf::from("a.srt")).ends_item());
        assert!(
            PlaybackError::PlaybackFailed {
                name: "x".into(),
                is_stream: false
            }
            .ends_item()
        );
    }

    #[test]
    fn stream_failures_have_their_own_title() {
        let error = PlaybackError::PlaybackFailed {
            name: "radio".into(),
            is_stream: true,
        };
        assert_eq!(error.title(), "Stream unavailable");
    }
}
