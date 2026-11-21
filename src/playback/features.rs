//! Engine features beyond basic transport, as plain data: the audio equalizer, picture
//! adjustments, geometry (aspect ratio, crop, zoom), deinterlacing, stereo mode, audio output
//! devices, media metadata, titles/chapters and per-media decoding options.
//!
//! Everything here is engine-independent; `playback::vlc_adapter` maps it onto LibVLC.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// LibVLC's equalizer range for preamp and bands, in dB.
pub const EQ_MIN_DB: f32 = -20.0;
pub const EQ_MAX_DB: f32 = 20.0;

/// One built-in LibVLC equalizer preset.
#[derive(Clone, Debug, PartialEq)]
pub struct EqualizerPreset {
    pub name: String,
    pub preamp: f32,
    pub bands: Vec<f32>,
}

/// The equalizer LibVLC offers: band centre frequencies (Hz) and its presets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EqualizerInfo {
    pub frequencies: Vec<f32>,
    pub presets: Vec<EqualizerPreset>,
}

/// The user's equalizer, persisted in settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqualizerSettings {
    pub enabled: bool,
    /// The preset the values came from, if they were not edited since.
    pub preset: Option<usize>,
    pub preamp: f32,
    pub bands: Vec<f32>,
}

impl Default for EqualizerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: None,
            preamp: 0.0,
            bands: vec![0.0; 10],
        }
    }
}

impl EqualizerSettings {
    /// Clamp every gain into LibVLC's accepted range and fix the band count.
    pub fn sanitized(mut self, band_count: usize) -> Self {
        let clamp = |v: f32| {
            if v.is_finite() {
                v.clamp(EQ_MIN_DB, EQ_MAX_DB)
            } else {
                0.0
            }
        };
        self.preamp = clamp(self.preamp);
        self.bands.resize(band_count, 0.0);
        for band in &mut self.bands {
            *band = clamp(*band);
        }
        self
    }

    /// Adopt a preset's values.
    pub fn apply_preset(&mut self, index: usize, preset: &EqualizerPreset) {
        self.enabled = true;
        self.preset = Some(index);
        self.preamp = preset.preamp;
        self.bands = preset.bands.clone();
    }
}

/// Picture adjustments (LibVLC's "adjust" video filter).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoAdjust {
    /// 0.0–2.0, neutral 1.0.
    pub brightness: f32,
    /// 0.0–2.0, neutral 1.0.
    pub contrast: f32,
    /// 0.0–3.0, neutral 1.0.
    pub saturation: f32,
    /// −180–180 degrees, neutral 0.
    pub hue: f32,
    /// 0.01–10.0, neutral 1.0.
    pub gamma: f32,
}

impl Default for VideoAdjust {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            hue: 0.0,
            gamma: 1.0,
        }
    }
}

impl VideoAdjust {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    pub fn clamped(self) -> Self {
        let c = |v: f32, lo: f32, hi: f32, d: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        Self {
            brightness: c(self.brightness, 0.0, 2.0, 1.0),
            contrast: c(self.contrast, 0.0, 2.0, 1.0),
            saturation: c(self.saturation, 0.0, 3.0, 1.0),
            hue: c(self.hue, -180.0, 180.0, 0.0),
            gamma: c(self.gamma, 0.01, 10.0, 1.0),
        }
    }
}

/// Aspect ratios offered in the UI; `None` lets LibVLC use the source's.
pub const ASPECT_RATIOS: &[(&str, Option<&str>)] = &[
    ("Auto", None),
    ("16:9", Some("16:9")),
    ("4:3", Some("4:3")),
    ("21:9", Some("21:9")),
    ("16:10", Some("16:10")),
    ("1:1", Some("1:1")),
    ("2.35:1", Some("235:100")),
];

/// Crop presets (LibVLC crop geometry as an aspect ratio); `None` is no crop.
pub const CROP_RATIOS: &[(&str, Option<&str>)] = &[
    ("None", None),
    ("16:9", Some("16:9")),
    ("4:3", Some("4:3")),
    ("21:9", Some("21:9")),
    ("1:1", Some("1:1")),
];

/// Zoom factors; 0 means "fit to window".
pub const ZOOM_LEVELS: &[(&str, f32)] = &[("Fit", 0.0), ("½×", 0.5), ("1×", 1.0), ("2×", 2.0)];

/// Deinterlacing modes; `None` is off.
pub const DEINTERLACE_MODES: &[(&str, Option<&str>)] = &[
    ("Off", None),
    ("Blend", Some("blend")),
    ("Bob", Some("bob")),
    ("Linear", Some("linear")),
    ("Yadif", Some("yadif")),
    ("Yadif 2×", Some("yadif2x")),
];

/// Audio channel mapping (`libvlc_audio_output_channel_t`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StereoMode {
    #[default]
    Stereo,
    Reverse,
    Left,
    Right,
    Dolby,
}

impl StereoMode {
    pub const ALL: [Self; 5] = [
        Self::Stereo,
        Self::Reverse,
        Self::Left,
        Self::Right,
        Self::Dolby,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Stereo => "Stereo",
            Self::Reverse => "Reverse",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Dolby => "Dolby",
        }
    }

    /// The `libvlc_audio_output_channel_t` value.
    pub fn to_libvlc(self) -> i32 {
        match self {
            Self::Stereo => 1,
            Self::Reverse => 2,
            Self::Left => 3,
            Self::Right => 4,
            Self::Dolby => 5,
        }
    }

    pub fn from_libvlc(value: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.to_libvlc() == value)
    }
}

/// An audio output device of the current audio output module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

/// Descriptive metadata LibVLC extracted from the media or stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    /// For radio streams: the current song (ICY metadata).
    pub now_playing: Option<String>,
    /// Cover art LibVLC saved locally (only `file://` artwork is used).
    pub artwork: Option<PathBuf>,
}

impl MediaMeta {
    /// "Artist — Album", or whichever is known.
    pub fn subtitle(&self) -> Option<String> {
        match (&self.artist, &self.album) {
            (Some(artist), Some(album)) => Some(format!("{artist} — {album}")),
            (Some(one), None) | (None, Some(one)) => Some(one.clone()),
            (None, None) => None,
        }
    }
}

/// A title (DVD/Blu-ray title, or the single title of a file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleInfo {
    pub index: i32,
    pub name: String,
    pub duration_ms: i64,
    pub is_menu: bool,
}

/// A chapter of the current title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterInfo {
    pub index: i32,
    pub name: String,
    pub offset_ms: i64,
    pub duration_ms: i64,
}

/// Titles and chapters of the current media.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Navigation {
    pub titles: Vec<TitleInfo>,
    pub chapters: Vec<ChapterInfo>,
    pub current_title: Option<i32>,
    pub current_chapter: Option<i32>,
}

impl Navigation {
    /// The chapter containing `time_ms`, by offsets (works even before LibVLC reports one).
    pub fn chapter_at(&self, time_ms: i64) -> Option<&ChapterInfo> {
        self.chapters
            .iter()
            .rev()
            .find(|chapter| chapter.offset_ms <= time_ms)
    }
}

/// Network caching choices in milliseconds (LibVLC's default is 1000).
pub const NETWORK_CACHING_MS: &[u32] = &[300, 1000, 3000, 10_000];

/// Per-media decoding options applied when an item is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackOptions {
    /// Let LibVLC use VA-API, VDPAU, VideoToolbox, DXVA/D3D11 when available.
    pub hardware_decoding: bool,
    pub network_caching_ms: u32,
}

impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            hardware_decoding: true,
            network_caching_ms: 1000,
        }
    }
}

impl PlaybackOptions {
    /// LibVLC media options (`:name=value`) implementing these choices.
    pub fn media_options(&self) -> Vec<String> {
        let mut options = vec![format!(
            ":network-caching={}",
            self.network_caching_ms.clamp(100, 60_000)
        )];
        if !self.hardware_decoding {
            options.push(":avcodec-hw=none".into());
            options.push(":no-videotoolbox".into());
        }
        options
    }
}

/// Convert a `file://` URI to a path (percent-decoded). Other schemes yield `None`.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(value) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
    }
    #[cfg(not(unix))]
    {
        let text = String::from_utf8_lossy(&out).into_owned();
        // file:///C:/x → C:/x
        Some(PathBuf::from(text.trim_start_matches('/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equalizer_is_clamped_and_resized() {
        let settings = EqualizerSettings {
            enabled: true,
            preset: None,
            preamp: 99.0,
            bands: vec![-50.0, f32::NAN, 3.0],
        }
        .sanitized(10);
        assert_eq!(settings.preamp, EQ_MAX_DB);
        assert_eq!(settings.bands.len(), 10);
        assert_eq!(&settings.bands[..3], &[EQ_MIN_DB, 0.0, 3.0]);
    }

    #[test]
    fn presets_apply() {
        let preset = EqualizerPreset {
            name: "Rock".into(),
            preamp: 5.0,
            bands: vec![1.0; 10],
        };
        let mut settings = EqualizerSettings::default();
        settings.apply_preset(3, &preset);
        assert!(settings.enabled);
        assert_eq!(settings.preset, Some(3));
        assert_eq!(settings.bands, vec![1.0; 10]);
    }

    #[test]
    fn video_adjust_neutral_and_clamp() {
        assert!(VideoAdjust::default().is_neutral());
        let clamped = VideoAdjust {
            brightness: 5.0,
            hue: -400.0,
            gamma: 0.0,
            ..Default::default()
        }
        .clamped();
        assert_eq!(clamped.brightness, 2.0);
        assert_eq!(clamped.hue, -180.0);
        assert_eq!(clamped.gamma, 0.01);
    }

    #[test]
    fn stereo_modes_round_trip() {
        for mode in StereoMode::ALL {
            assert_eq!(StereoMode::from_libvlc(mode.to_libvlc()), Some(mode));
        }
        assert_eq!(StereoMode::from_libvlc(-1), None);
    }

    #[test]
    fn playback_options_map_to_media_options() {
        assert_eq!(
            PlaybackOptions::default().media_options(),
            vec![":network-caching=1000"]
        );
        let off = PlaybackOptions {
            hardware_decoding: false,
            network_caching_ms: 3000,
        };
        assert_eq!(
            off.media_options(),
            vec![
                ":network-caching=3000",
                ":avcodec-hw=none",
                ":no-videotoolbox"
            ]
        );
    }

    #[test]
    fn chapters_by_time() {
        let nav = Navigation {
            chapters: vec![
                ChapterInfo {
                    index: 0,
                    name: "Intro".into(),
                    offset_ms: 0,
                    duration_ms: 60_000,
                },
                ChapterInfo {
                    index: 1,
                    name: "Part 1".into(),
                    offset_ms: 60_000,
                    duration_ms: 60_000,
                },
            ],
            ..Default::default()
        };
        assert_eq!(nav.chapter_at(30_000).map(|c| c.index), Some(0));
        assert_eq!(nav.chapter_at(90_000).map(|c| c.index), Some(1));
    }

    #[test]
    fn meta_subtitle() {
        let meta = MediaMeta {
            artist: Some("A".into()),
            album: Some("B".into()),
            ..Default::default()
        };
        assert_eq!(meta.subtitle().as_deref(), Some("A — B"));
        assert_eq!(MediaMeta::default().subtitle(), None);
    }

    #[cfg(unix)]
    #[test]
    fn file_uris_decode() {
        assert_eq!(
            file_uri_to_path("file:///home/me/art%20cover.jpg"),
            Some(PathBuf::from("/home/me/art cover.jpg"))
        );
        assert_eq!(file_uri_to_path("attachment://cover.jpg"), None);
    }
}
