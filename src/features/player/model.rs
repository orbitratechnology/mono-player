//! UI-facing playback snapshot, updated only from actor events (never by polling the engine).

use crate::playback::state::{PlaybackState, Tracks};
use crate::playback::{Event, PlaybackError, RequestId};

/// LibVLC runtime lifecycle as the UI sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// Not initialized yet (no media requested this session).
    #[default]
    NotStarted,
    Starting,
    Ready(String),
}

/// What the view should do after applying an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Stale or no-op event: nothing to redraw.
    Unchanged,
    Changed,
    /// The current item finished; advance the queue.
    EndReached(RequestId),
    /// The current item cannot be played; mark it and continue.
    ItemFailed(RequestId),
    ShutdownComplete,
}

#[derive(Clone, Debug)]
pub struct PlayerModel {
    pub request: Option<RequestId>,
    pub title: String,
    pub state: PlaybackState,
    pub time_ms: i64,
    pub length_ms: Option<i64>,
    pub seekable: bool,
    pub has_video: bool,
    /// Buffer fill while below 100%.
    pub buffering: Option<f32>,
    pub volume: u8,
    pub muted: bool,
    pub rate: f32,
    pub tracks: Tracks,
    /// The most recent error to show (dismissable).
    pub error: Option<PlaybackError>,
    pub runtime: RuntimeStatus,
}

impl Default for PlayerModel {
    fn default() -> Self {
        Self {
            request: None,
            title: String::new(),
            state: PlaybackState::Idle,
            time_ms: 0,
            length_ms: None,
            seekable: false,
            has_video: false,
            buffering: None,
            volume: 80,
            muted: false,
            rate: 1.0,
            tracks: Tracks::default(),
            error: None,
            runtime: RuntimeStatus::NotStarted,
        }
    }
}

impl PlayerModel {
    /// A new item was requested: reset everything item-scoped.
    pub fn begin(&mut self, request: RequestId, title: String, start_ms: Option<i64>) {
        self.request = Some(request);
        self.title = title;
        self.state = PlaybackState::Opening;
        self.time_ms = start_ms.unwrap_or(0).max(0);
        self.length_ms = None;
        self.seekable = false;
        self.has_video = false;
        self.buffering = None;
        self.tracks = Tracks::default();
        self.error = None;
    }

    /// Forget the current item (queue cleared or current item removed while stopped).
    pub fn clear(&mut self) {
        let keep = (self.volume, self.muted, self.rate, self.runtime.clone());
        *self = Self::default();
        (self.volume, self.muted, self.rate, self.runtime) = keep;
    }

    /// Fraction of the media played, for the timeline.
    pub fn progress(&self) -> f32 {
        match self.length_ms {
            Some(length) if length > 0 => (self.time_ms as f32 / length as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn can_seek(&self) -> bool {
        self.seekable && self.length_ms.is_some() && self.state.has_media()
            || matches!(self.state, PlaybackState::Ended | PlaybackState::Stopped)
                && self.length_ms.is_some()
    }

    fn is_current(&self, request: RequestId) -> bool {
        self.request == Some(request)
    }

    /// Apply one actor event. Events for replaced media are ignored.
    pub fn apply(&mut self, event: Event) -> Outcome {
        match event {
            Event::RuntimeStarting => {
                self.runtime = RuntimeStatus::Starting;
                Outcome::Changed
            }
            Event::RuntimeReady { version } => {
                self.runtime = RuntimeStatus::Ready(version);
                Outcome::Changed
            }
            Event::MediaOpening { request } if self.is_current(request) => {
                self.state = PlaybackState::Opening;
                Outcome::Changed
            }
            Event::StateChanged { request, state } if self.is_current(request) => {
                if self.state == state {
                    return Outcome::Unchanged;
                }
                self.state = state;
                if matches!(state, PlaybackState::Playing | PlaybackState::Paused) {
                    self.buffering = None;
                }
                Outcome::Changed
            }
            Event::Progress {
                request,
                time_ms,
                length_ms,
            } if self.is_current(request) => {
                let length_ms = length_ms.or(self.length_ms);
                if self.time_ms == time_ms && self.length_ms == length_ms {
                    return Outcome::Unchanged;
                }
                self.time_ms = time_ms;
                self.length_ms = length_ms;
                Outcome::Changed
            }
            Event::LengthChanged { request, length_ms } if self.is_current(request) => {
                self.length_ms = length_ms;
                Outcome::Changed
            }
            Event::SeekableChanged { request, seekable } if self.is_current(request) => {
                self.seekable = seekable;
                Outcome::Changed
            }
            Event::VideoOutputChanged { request, has_video } if self.is_current(request) => {
                if self.has_video == has_video {
                    return Outcome::Unchanged;
                }
                self.has_video = has_video;
                Outcome::Changed
            }
            Event::TracksChanged { request, tracks } if self.is_current(request) => {
                self.tracks = tracks;
                Outcome::Changed
            }
            Event::Buffering { request, percent } if self.is_current(request) => {
                self.buffering = (percent < 100.0).then_some(percent);
                Outcome::Changed
            }
            Event::VolumeChanged { volume, muted } => {
                self.volume = volume;
                self.muted = muted;
                Outcome::Changed
            }
            Event::RateChanged { rate } => {
                self.rate = rate;
                Outcome::Changed
            }
            Event::EndReached { request } if self.is_current(request) => {
                self.state = PlaybackState::Ended;
                Outcome::EndReached(request)
            }
            Event::Error {
                request: Some(request),
                error,
            } => {
                if !self.is_current(request) {
                    return Outcome::Unchanged;
                }
                let ends = error.ends_item();
                self.error = Some(error);
                if ends {
                    self.state = PlaybackState::Error;
                    self.buffering = None;
                    Outcome::ItemFailed(request)
                } else {
                    Outcome::Changed
                }
            }
            Event::Error {
                request: None,
                error,
            } => {
                if matches!(
                    error,
                    PlaybackError::RuntimeUnavailable | PlaybackError::PlayerUnavailable
                ) {
                    self.runtime = RuntimeStatus::NotStarted;
                    if self.state.is_loading() {
                        self.state = PlaybackState::Error;
                    }
                }
                if error == PlaybackError::PlaybackThreadFailed {
                    self.state = PlaybackState::Error;
                }
                self.error = Some(error);
                Outcome::Changed
            }
            Event::ShutdownComplete => Outcome::ShutdownComplete,
            // Media-scoped events for a request that is no longer current.
            _ => Outcome::Unchanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(request: u64) -> PlayerModel {
        let mut model = PlayerModel::default();
        model.begin(RequestId(request), "a.mkv".into(), None);
        model
    }

    #[test]
    fn stale_events_are_ignored() {
        let mut model = opened(2);
        let outcome = model.apply(Event::Progress {
            request: RequestId(1),
            time_ms: 999,
            length_ms: Some(5_000),
        });
        assert_eq!(outcome, Outcome::Unchanged);
        assert_eq!(model.time_ms, 0);
        assert_eq!(
            model.apply(Event::EndReached {
                request: RequestId(1)
            }),
            Outcome::Unchanged
        );
    }

    #[test]
    fn lifecycle_transitions() {
        let mut model = opened(1);
        let r = RequestId(1);
        assert_eq!(model.state, PlaybackState::Opening);
        model.apply(Event::Buffering {
            request: r,
            percent: 40.0,
        });
        assert_eq!(model.buffering, Some(40.0));
        model.apply(Event::StateChanged {
            request: r,
            state: PlaybackState::Playing,
        });
        assert_eq!(model.buffering, None);
        model.apply(Event::Progress {
            request: r,
            time_ms: 2_500,
            length_ms: Some(10_000),
        });
        assert_eq!(model.progress(), 0.25);
        model.apply(Event::SeekableChanged {
            request: r,
            seekable: true,
        });
        assert!(model.can_seek());
        assert_eq!(
            model.apply(Event::EndReached { request: r }),
            Outcome::EndReached(r)
        );
        assert_eq!(model.state, PlaybackState::Ended);
    }

    #[test]
    fn duplicate_progress_does_not_redraw() {
        let mut model = opened(1);
        let event = Event::Progress {
            request: RequestId(1),
            time_ms: 100,
            length_ms: None,
        };
        assert_eq!(model.apply(event.clone()), Outcome::Changed);
        assert_eq!(model.apply(event), Outcome::Unchanged);
    }

    #[test]
    fn errors_split_into_fatal_and_recoverable() {
        let mut model = opened(1);
        let r = RequestId(1);
        assert_eq!(
            model.apply(Event::Error {
                request: Some(r),
                error: PlaybackError::SeekUnsupported
            }),
            Outcome::Changed
        );
        assert_ne!(model.state, PlaybackState::Error);
        assert_eq!(
            model.apply(Event::Error {
                request: Some(r),
                error: PlaybackError::PlaybackFailed {
                    name: "a".into(),
                    is_stream: false
                }
            }),
            Outcome::ItemFailed(r)
        );
        assert_eq!(model.state, PlaybackState::Error);
    }

    #[test]
    fn runtime_failure_is_retryable() {
        let mut model = opened(1);
        model.apply(Event::RuntimeStarting);
        assert_eq!(model.runtime, RuntimeStatus::Starting);
        model.apply(Event::Error {
            request: None,
            error: PlaybackError::RuntimeUnavailable,
        });
        assert_eq!(model.runtime, RuntimeStatus::NotStarted);
        assert_eq!(model.state, PlaybackState::Error);
        assert!(model.error.is_some());
    }

    #[test]
    fn clear_keeps_preferences() {
        let mut model = opened(1);
        model.apply(Event::VolumeChanged {
            volume: 30,
            muted: true,
        });
        model.clear();
        assert_eq!(model.request, None);
        assert_eq!((model.volume, model.muted), (30, true));
    }
}
