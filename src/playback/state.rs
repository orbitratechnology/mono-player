//! Engine-independent playback state and the pure helpers around it.

use std::time::{Duration, Instant};

/// Normalized playback state, independent of LibVLC's `libvlc_state_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackState {
    /// Nothing loaded.
    #[default]
    Idle,
    /// Media is being opened (file access, demuxer probing, network connect).
    Opening,
    /// The engine is filling its input buffer.
    Buffering,
    Playing,
    Paused,
    Stopped,
    /// The end of the media was reached.
    Ended,
    /// The current item failed; see the accompanying [`super::PlaybackError`].
    Error,
}

impl PlaybackState {
    /// A short, plain-language status label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "No media",
            Self::Opening => "Opening…",
            Self::Buffering => "Buffering…",
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
            Self::Ended => "Ended",
            Self::Error => "Error",
        }
    }

    /// Whether the transport should present a "pause" affordance.
    pub fn is_playing_or_pending(self) -> bool {
        matches!(self, Self::Playing | Self::Opening | Self::Buffering)
    }

    /// Whether a media item is loaded and can receive seek/track commands.
    pub fn has_media(self) -> bool {
        matches!(
            self,
            Self::Opening | Self::Buffering | Self::Playing | Self::Paused
        )
    }

    /// Whether the opening/buffering indicator should be shown.
    pub fn is_loading(self) -> bool {
        matches!(self, Self::Opening | Self::Buffering)
    }
}

/// One selectable elementary stream (audio or subtitle track).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    /// LibVLC track id. `-1` means "disabled" for subtitles.
    pub id: i32,
    pub name: String,
}

/// The audio and subtitle tracks of the current media.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracks {
    pub audio: Vec<TrackInfo>,
    pub subtitles: Vec<TrackInfo>,
    pub current_audio: Option<i32>,
    /// `None` or `Some(-1)` both mean subtitles are disabled.
    pub current_subtitle: Option<i32>,
}

impl Tracks {
    /// Subtitle tracks without LibVLC's synthetic "Disable" entry.
    pub fn real_subtitles(&self) -> impl Iterator<Item = &TrackInfo> {
        self.subtitles.iter().filter(|t| t.id >= 0)
    }

    /// Audio tracks without LibVLC's synthetic "Disable" entry.
    pub fn real_audio(&self) -> impl Iterator<Item = &TrackInfo> {
        self.audio.iter().filter(|t| t.id >= 0)
    }

    pub fn subtitles_enabled(&self) -> bool {
        self.current_subtitle.is_some_and(|id| id >= 0)
    }
}

/// Supported playback rates. Kept to the range LibVLC handles well with audio time-stretching.
pub const PLAYBACK_RATES: [f32; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Volume is exposed in percent. LibVLC accepts up to 200 (amplification), but the default UI
/// range stops at 100 so the slider never distorts audio by surprise.
pub const MAX_VOLUME: u8 = 100;

/// Small and large keyboard seek steps.
pub const SEEK_STEP: Duration = Duration::from_secs(5);
pub const SEEK_STEP_LARGE: Duration = Duration::from_secs(30);
pub const VOLUME_STEP: u8 = 5;

/// Progress events are capped to this cadence (8 updates/s, inside the PRD's 4–10 window).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(125);

/// Audio/subtitle synchronization step and range.
pub const DELAY_STEP_MS: i64 = 100;
pub const MAX_DELAY_MS: i64 = 30_000;

/// Clamp a delay to `±MAX_DELAY_MS`.
pub fn clamp_delay(ms: i64) -> i64 {
    ms.clamp(-MAX_DELAY_MS, MAX_DELAY_MS)
}

/// Display a delay as `0.0 s`, `+0.3 s` or `−1.2 s`.
pub fn format_delay(ms: i64) -> String {
    let seconds = ms as f64 / 1000.0;
    if ms == 0 {
        "0.0 s".into()
    } else if ms > 0 {
        format!("+{seconds:.1} s")
    } else {
        format!("−{:.1} s", -seconds)
    }
}

/// Clamp a volume request into `0..=MAX_VOLUME`.
pub fn clamp_volume(volume: i32) -> u8 {
    volume.clamp(0, MAX_VOLUME as i32) as u8
}

/// Clamp a seek target to the media. Unknown length only clamps the lower bound.
///
/// Seeking exactly to the end makes some demuxers stall, so a known length clamps to just before
/// it (the end-of-media event then arrives normally).
pub fn clamp_seek(target_ms: i64, length_ms: Option<i64>) -> i64 {
    let target = target_ms.max(0);
    match length_ms {
        Some(length) if length > 0 => target.min((length - 250).max(0)),
        _ => target,
    }
}

/// Snap an arbitrary rate to the nearest supported rate.
pub fn nearest_rate(rate: f32) -> f32 {
    if !rate.is_finite() {
        return 1.0;
    }
    PLAYBACK_RATES
        .iter()
        .copied()
        .min_by(|a, b| (a - rate).abs().total_cmp(&(b - rate).abs()))
        .unwrap_or(1.0)
}

/// The next faster/slower supported rate.
pub fn step_rate(rate: f32, faster: bool) -> f32 {
    let current = nearest_rate(rate);
    let index = PLAYBACK_RATES
        .iter()
        .position(|r| *r == current)
        .unwrap_or(2);
    let next = if faster {
        (index + 1).min(PLAYBACK_RATES.len() - 1)
    } else {
        index.saturating_sub(1)
    };
    PLAYBACK_RATES[next]
}

/// Display label for a rate: `1×`, `1.25×`, `0.5×`.
pub fn format_rate(rate: f32) -> String {
    let text = format!("{rate:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}×")
}

/// Format milliseconds as `m:ss` or `h:mm:ss`. Negative values clamp to zero.
pub fn format_time(ms: i64) -> String {
    let total_seconds = ms.max(0) / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Lets an event through at most once per `interval`; used to cap progress updates.
#[derive(Debug)]
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Returns `true` and records `now` if enough time has passed since the last accepted event.
    pub fn accept(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }

    /// Forget the last event so the next one passes immediately (after a seek or media change).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(-5_000), "0:00");
        assert_eq!(format_time(59_999), "0:59");
        assert_eq!(format_time(61_000), "1:01");
        assert_eq!(format_time(3_600_000), "1:00:00");
        assert_eq!(format_time(3_725_000), "1:02:05");
    }

    #[test]
    fn clamps_volume() {
        assert_eq!(clamp_volume(-3), 0);
        assert_eq!(clamp_volume(42), 42);
        assert_eq!(clamp_volume(250), MAX_VOLUME);
    }

    #[test]
    fn clamps_seek() {
        assert_eq!(clamp_seek(-100, Some(10_000)), 0);
        assert_eq!(clamp_seek(5_000, Some(10_000)), 5_000);
        assert_eq!(clamp_seek(20_000, Some(10_000)), 9_750);
        assert_eq!(clamp_seek(20_000, None), 20_000);
        assert_eq!(clamp_seek(100, Some(100)), 0);
    }

    #[test]
    fn rates() {
        assert_eq!(nearest_rate(1.1), 1.0);
        assert_eq!(nearest_rate(f32::NAN), 1.0);
        assert_eq!(step_rate(1.0, true), 1.25);
        assert_eq!(step_rate(2.0, true), 2.0);
        assert_eq!(step_rate(0.5, false), 0.5);
        assert_eq!(format_rate(1.0), "1×");
        assert_eq!(format_rate(1.25), "1.25×");
        assert_eq!(format_rate(0.5), "0.5×");
    }

    #[test]
    fn delays() {
        assert_eq!(clamp_delay(45_000), MAX_DELAY_MS);
        assert_eq!(clamp_delay(-45_000), -MAX_DELAY_MS);
        assert_eq!(format_delay(0), "0.0 s");
        assert_eq!(format_delay(300), "+0.3 s");
        assert_eq!(format_delay(-1_200), "−1.2 s");
    }

    #[test]
    fn throttle_caps_cadence() {
        let start = Instant::now();
        let mut throttle = Throttle::new(Duration::from_millis(100));
        assert!(throttle.accept(start));
        assert!(!throttle.accept(start + Duration::from_millis(50)));
        assert!(throttle.accept(start + Duration::from_millis(100)));
        throttle.reset();
        assert!(throttle.accept(start + Duration::from_millis(101)));
    }

    #[test]
    fn state_predicates() {
        assert!(PlaybackState::Opening.is_loading());
        assert!(PlaybackState::Paused.has_media());
        assert!(!PlaybackState::Ended.has_media());
        assert!(PlaybackState::Buffering.is_playing_or_pending());
    }

    #[test]
    fn tracks_filter_disable_entry() {
        let tracks = Tracks {
            subtitles: vec![
                TrackInfo {
                    id: -1,
                    name: "Disable".into(),
                },
                TrackInfo {
                    id: 3,
                    name: "English".into(),
                },
            ],
            current_subtitle: Some(-1),
            ..Default::default()
        };
        assert_eq!(tracks.real_subtitles().count(), 1);
        assert!(!tracks.subtitles_enabled());
    }
}
