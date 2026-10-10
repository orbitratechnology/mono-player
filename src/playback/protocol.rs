//! The typed command/event protocol between the UI and the playback actor.

use std::fmt;
use std::path::{Path, PathBuf};

use super::error::{PlaybackError, display_name};
use super::state::{PlaybackState, Tracks};
use crate::video_output::VideoTarget;

/// Identifies one `Open` request. Every media-scoped event echoes it so the UI can drop events
/// that belong to media it has already replaced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// Where media comes from. Constructed only through validation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaSource {
    File(PathBuf),
    Url(String),
}

/// URL schemes passed to LibVLC. Everything else is rejected before reaching the engine.
pub const ALLOWED_SCHEMES: &[&str] = &[
    "http", "https", "rtsp", "rtsps", "rtp", "udp", "mms", "mmsh", "ftp", "sftp", "smb", "file",
];

/// Extensions treated as external subtitle files when dropped or opened.
pub const SUBTITLE_EXTENSIONS: &[&str] = &[
    "srt", "ass", "ssa", "vtt", "sub", "idx", "smi", "sami", "txt", "usf", "ttml",
];

impl MediaSource {
    /// Interpret user text (an address bar entry) as a URL or a path. Pure: performs no I/O.
    pub fn parse(input: &str) -> Result<Self, PlaybackError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(PlaybackError::InvalidUrl(String::new()));
        }
        match trimmed.split_once("://") {
            Some(_) => Self::url(trimmed),
            None => Ok(Self::File(PathBuf::from(trimmed))),
        }
    }

    /// Validate a URL: allowed scheme, a non-empty remainder, no whitespace/control characters.
    pub fn url(input: &str) -> Result<Self, PlaybackError> {
        let Some((scheme, rest)) = input.split_once("://") else {
            return Err(PlaybackError::InvalidUrl(input.into()));
        };
        let valid_scheme = !scheme.is_empty()
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if !valid_scheme {
            return Err(PlaybackError::InvalidUrl(input.into()));
        }
        let scheme = scheme.to_ascii_lowercase();
        if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
            return Err(PlaybackError::UnsupportedScheme(scheme));
        }
        if rest.is_empty() || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(PlaybackError::InvalidUrl(input.into()));
        }
        if scheme != "file" && rest.starts_with('/') {
            // `http:///path` has no host.
            return Err(PlaybackError::InvalidUrl(input.into()));
        }
        Ok(Self::Url(format!("{scheme}://{rest}")))
    }

    /// A short name for lists and messages.
    pub fn display_name(&self) -> String {
        match self {
            Self::File(path) => display_name(path),
            Self::Url(url) => {
                let without_query = url.split(['?', '#']).next().unwrap_or(url);
                let tail = without_query
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty() && !s.contains(':'));
                tail.map(percent_decode_lossy)
                    .unwrap_or_else(|| url.clone())
            }
        }
    }

    /// A stable string for recents; paths are stored lossily as UTF-8 only for display/matching.
    pub fn location(&self) -> String {
        match self {
            Self::File(path) => path.to_string_lossy().into_owned(),
            Self::Url(url) => url.clone(),
        }
    }

    pub fn is_stream(&self) -> bool {
        matches!(self, Self::Url(url) if !url.starts_with("file://"))
    }

    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Url(_) => None,
        }
    }
}

impl fmt::Display for MediaSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

/// Whether a path looks like an external subtitle file (by extension; no I/O).
pub fn is_subtitle_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            SUBTITLE_EXTENSIONS
                .iter()
                .any(|s| s.eq_ignore_ascii_case(ext))
        })
}

/// Convert an absolute path to a `file://` URI with RFC 3986 percent-encoding.
///
/// LibVLC's slave API takes URIs; this keeps spaces, `#`, `%` and non-ASCII names intact.
/// On Unix the raw path bytes are encoded, so names that are not valid UTF-8 still round-trip.
pub fn path_to_file_uri(path: &Path) -> String {
    let bytes = path_bytes(path);
    let normalized: Vec<u8> = bytes
        .iter()
        .map(|&b| if cfg!(windows) && b == b'\\' { b'/' } else { b })
        .collect();
    let mut uri = String::from("file://");
    if normalized.first() != Some(&b'/') {
        // Windows drive path: file:///C:/...
        uri.push('/');
    }
    for &byte in &normalized {
        let keep =
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~' | b':');
        if keep {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

fn percent_decode_lossy(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Commands from the UI to the playback actor. Shutdown is separate: see
/// [`super::PlaybackHandle::shutdown`].
#[derive(Debug)]
pub enum Command {
    /// Open and start playing a source. `start_ms` resumes at a position.
    Open {
        request: RequestId,
        source: MediaSource,
        start_ms: Option<i64>,
    },
    Play,
    Pause,
    TogglePlayback,
    Stop,
    SeekTo(i64),
    /// Relative seek, resolved against the engine's current time on the playback thread.
    SeekBy(i64),
    SetVolume(u8),
    SetMuted(bool),
    SetRate(f32),
    SelectAudioTrack(i32),
    /// `-1` disables subtitles.
    SelectSubtitleTrack(i32),
    LoadExternalSubtitle(PathBuf),
    /// Audio delay in milliseconds (positive = later). Applies to the current item.
    SetAudioDelay(i64),
    /// Subtitle delay in milliseconds (positive = later). Applies to the current item.
    SetSubtitleDelay(i64),
    /// Attach (or detach with `None`) the native drawable LibVLC renders into.
    SetVideoTarget(Option<VideoTarget>),
}

/// Events from the playback actor to the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// LibVLC is initializing (first media request only).
    RuntimeStarting,
    /// LibVLC is ready; carries its version string.
    RuntimeReady {
        version: String,
    },
    MediaOpening {
        request: RequestId,
    },
    StateChanged {
        request: RequestId,
        state: PlaybackState,
    },
    /// Capped at [`super::state::PROGRESS_INTERVAL`].
    Progress {
        request: RequestId,
        time_ms: i64,
        length_ms: Option<i64>,
    },
    LengthChanged {
        request: RequestId,
        length_ms: Option<i64>,
    },
    SeekableChanged {
        request: RequestId,
        seekable: bool,
    },
    /// Whether the media currently has a video output (false for audio-only media).
    VideoOutputChanged {
        request: RequestId,
        has_video: bool,
    },
    TracksChanged {
        request: RequestId,
        tracks: Tracks,
    },
    Buffering {
        request: RequestId,
        percent: f32,
    },
    VolumeChanged {
        volume: u8,
        muted: bool,
    },
    RateChanged {
        rate: f32,
    },
    DelaysChanged {
        request: RequestId,
        audio_ms: i64,
        subtitle_ms: i64,
    },
    EndReached {
        request: RequestId,
    },
    /// `request` is `None` for errors not tied to a media item (runtime start, shutdown).
    Error {
        request: Option<RequestId>,
        error: PlaybackError,
    },
    ShutdownComplete,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paths_and_urls() {
        assert_eq!(
            MediaSource::parse("  /tmp/a b.mkv "),
            Ok(MediaSource::File(PathBuf::from("/tmp/a b.mkv")))
        );
        assert_eq!(
            MediaSource::parse("HTTPS://example.com/live.m3u8"),
            Ok(MediaSource::Url("https://example.com/live.m3u8".into()))
        );
    }

    #[test]
    fn rejects_bad_urls() {
        assert!(matches!(
            MediaSource::parse("javascript://alert(1)"),
            Err(PlaybackError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            MediaSource::parse("http://exa mple.com"),
            Err(PlaybackError::InvalidUrl(_))
        ));
        assert!(matches!(
            MediaSource::parse("http://"),
            Err(PlaybackError::InvalidUrl(_))
        ));
        assert!(matches!(
            MediaSource::parse("http:///nohost"),
            Err(PlaybackError::InvalidUrl(_))
        ));
        assert!(matches!(
            MediaSource::parse("1http://x"),
            Err(PlaybackError::InvalidUrl(_))
        ));
        assert!(matches!(
            MediaSource::parse("http://a\u{0}b"),
            Err(PlaybackError::InvalidUrl(_))
        ));
        assert!(MediaSource::parse("   ").is_err());
    }

    #[test]
    fn display_names() {
        assert_eq!(
            MediaSource::File(PathBuf::from("/x/日本語 file.mp4")).display_name(),
            "日本語 file.mp4"
        );
        assert_eq!(
            MediaSource::Url("https://h.example/a/My%20Song.mp3?x=1".into()).display_name(),
            "My Song.mp3"
        );
        assert_eq!(
            MediaSource::Url("rtsp://camera.local".into()).display_name(),
            "camera.local"
        );
        assert!(MediaSource::Url("https://h/x".into()).is_stream());
        assert!(!MediaSource::Url("file:///x".into()).is_stream());
    }

    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(
            path_to_file_uri(Path::new("/home/me/My Subs #1/é.srt")),
            "file:///home/me/My%20Subs%20%231/%C3%A9.srt"
        );
        #[cfg(windows)]
        assert_eq!(
            path_to_file_uri(Path::new("C:\\Videos\\a b.srt")),
            "file:///C:/Videos/a%20b.srt"
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            // A backslash is an ordinary filename byte on Unix, and names need not be UTF-8.
            assert_eq!(path_to_file_uri(Path::new("/a\\b")), "file:///a%5Cb");
            let raw = std::ffi::OsStr::from_bytes(b"/x/\xff.mkv");
            assert_eq!(path_to_file_uri(Path::new(raw)), "file:///x/%FF.mkv");
        }
    }

    #[test]
    fn detects_subtitles() {
        assert!(is_subtitle_path(Path::new("/a/b.SRT")));
        assert!(is_subtitle_path(Path::new("movie.en.ass")));
        assert!(!is_subtitle_path(Path::new("movie.mkv")));
        assert!(!is_subtitle_path(Path::new("noext")));
    }

    #[test]
    fn percent_decoding_tolerates_garbage() {
        assert_eq!(percent_decode_lossy("a%2"), "a%2");
        assert_eq!(percent_decode_lossy("%zz"), "%zz");
        assert_eq!(percent_decode_lossy("%41"), "A");
    }
}
