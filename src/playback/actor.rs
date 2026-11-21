//! The playback actor: one thread that owns every LibVLC object.
//!
//! The UI sends [`Command`]s through a non-blocking channel; LibVLC callbacks feed normalized
//! [`EngineEvent`]s into the same queue. The actor applies both in order and reports
//! [`Event`]s back. It blocks on its queue when idle (no polling loop) and initializes LibVLC
//! only when media is first requested.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;

use super::error::PlaybackError;
use super::protocol::{Command, Event, MediaSource, RequestId};
use super::state::{
    MAX_VOLUME, PROGRESS_INTERVAL, PlaybackState, Throttle, clamp_delay, clamp_seek, clamp_volume,
    nearest_rate,
};
use super::vlc_adapter::{EngineEvent, Player, Runtime};
use crate::diagnostics as log;
use crate::video_output::VideoTarget;

/// How long track-change bursts are coalesced before the track list is re-read.
const TRACKS_SETTLE: Duration = Duration::from_millis(150);

enum Message {
    Command(Command),
    /// An engine event tagged with the request whose media produced it.
    Engine(RequestId, EngineEvent),
    Shutdown(Option<oneshot::Sender<()>>),
}

/// The UI's handle to the playback thread. Sending never blocks.
pub struct PlaybackHandle {
    tx: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl PlaybackHandle {
    /// Start the playback thread. LibVLC is *not* initialized here.
    pub fn spawn(events: UnboundedSender<Event>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let actor_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("mono-playback".into())
            .spawn(move || {
                let panic_events = events.clone();
                let result = catch_unwind(AssertUnwindSafe(move || {
                    Actor::new(rx, actor_tx, events).run();
                }));
                if result.is_err() {
                    log::warn("playback thread panicked");
                    let _ = panic_events.unbounded_send(Event::Error {
                        request: None,
                        error: PlaybackError::PlaybackThreadFailed,
                    });
                }
            })?;
        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Queue a command. Returns it back if the playback thread is gone.
    pub fn send(&self, command: Command) -> Result<(), Command> {
        self.tx
            .send(Message::Command(command))
            .map_err(|mpsc::SendError(message)| match message {
                Message::Command(command) => command,
                _ => unreachable!("only commands are sent here"),
            })
    }

    /// Whether the playback thread is still running.
    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Ask the actor to stop playback and release LibVLC in order. The receiver completes once
    /// teardown is done (or immediately if the thread already exited).
    pub fn shutdown(&mut self) -> oneshot::Receiver<()> {
        let (done_tx, done_rx) = oneshot::channel();
        if let Err(mpsc::SendError(Message::Shutdown(Some(done)))) =
            self.tx.send(Message::Shutdown(Some(done_tx)))
        {
            let _ = done.send(());
        }
        // Detach: the UI thread never joins (and never waits on) the playback thread.
        self.thread.take();
        done_rx
    }
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.tx.send(Message::Shutdown(None));
        }
    }
}

struct Current {
    request: RequestId,
    source: MediaSource,
    /// Volume/rate are re-applied once per item when its audio output exists.
    applied_output_settings: bool,
    /// A seek requested while the item had ended; applied once playback restarts.
    pending_seek: Option<i64>,
    last_buffering: i32,
}

/// Owns LibVLC. Field order matters for drop: the player is released before the runtime.
struct Actor {
    player: Option<Player>,
    runtime: Option<Runtime>,
    rx: Receiver<Message>,
    tx: Sender<Message>,
    events: UnboundedSender<Event>,
    current: Option<Current>,
    state: PlaybackState,
    video_target: Option<VideoTarget>,
    volume: u8,
    muted: bool,
    rate: f32,
    length_ms: Option<i64>,
    time_ms: i64,
    seekable: bool,
    progress: Throttle,
    tracks_dirty: bool,
    audio_delay_ms: i64,
    subtitle_delay_ms: i64,
}

impl Actor {
    fn new(rx: Receiver<Message>, tx: Sender<Message>, events: UnboundedSender<Event>) -> Self {
        Self {
            player: None,
            runtime: None,
            rx,
            tx,
            events,
            current: None,
            state: PlaybackState::Idle,
            video_target: None,
            volume: 80,
            muted: false,
            rate: 1.0,
            length_ms: None,
            time_ms: 0,
            seekable: false,
            progress: Throttle::new(PROGRESS_INTERVAL),
            tracks_dirty: false,
            audio_delay_ms: 0,
            subtitle_delay_ms: 0,
        }
    }

    fn run(mut self) {
        log::info("playback thread started");
        let done = loop {
            // Block indefinitely unless a track-list refresh is pending.
            let message = if self.tracks_dirty {
                match self.rx.recv_timeout(TRACKS_SETTLE) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        self.flush_tracks();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break None,
                }
            } else {
                match self.rx.recv() {
                    Ok(message) => message,
                    Err(_) => break None,
                }
            };
            match message {
                Message::Command(command) => self.handle_command(command),
                Message::Engine(request, event) => self.handle_engine(request, event),
                Message::Shutdown(done) => break done,
            }
        };
        self.teardown();
        if let Some(done) = done {
            let _ = done.send(());
        }
        log::info("playback thread finished");
    }

    /// Ordered release: stop playback, detach callbacks (inside `Player::drop`), release media
    /// and player, then the LibVLC instance.
    fn teardown(&mut self) {
        let started = Instant::now();
        self.player.take();
        self.runtime.take();
        self.emit(Event::ShutdownComplete);
        log::info(&format!("LibVLC released in {:?}", started.elapsed()));
    }

    fn emit(&self, event: Event) {
        // The UI may already be gone during shutdown; nothing to do then.
        let _ = self.events.unbounded_send(event);
    }

    fn current_request(&self) -> Option<RequestId> {
        self.current.as_ref().map(|c| c.request)
    }

    fn set_state(&mut self, state: PlaybackState) {
        if self.state == state {
            return;
        }
        self.state = state;
        if let Some(request) = self.current_request() {
            self.emit(Event::StateChanged { request, state });
        }
    }

    fn fail(&mut self, request: Option<RequestId>, error: PlaybackError) {
        log::warn(&format!("{error:?}"));
        if error.ends_item() && request.is_some() {
            self.set_state(PlaybackState::Error);
        }
        self.emit(Event::Error { request, error });
    }

    /// Start LibVLC on first use. Failure is reported and retried on the next request.
    fn ensure_player(&mut self) -> bool {
        if self.player.is_some() {
            return true;
        }
        if self.runtime.is_none() {
            self.emit(Event::RuntimeStarting);
            let started = Instant::now();
            match Runtime::start() {
                Ok(runtime) => {
                    let version = Runtime::version();
                    log::info(&format!(
                        "LibVLC {version} started in {:?}",
                        started.elapsed()
                    ));
                    self.emit(Event::RuntimeReady { version });
                    self.runtime = Some(runtime);
                }
                Err(error) => {
                    self.fail(None, error);
                    return false;
                }
            }
        }
        let runtime = self.runtime.as_ref().expect("runtime initialized above");
        match Player::new(runtime) {
            Ok(player) => {
                player.set_video_target(self.video_target);
                self.player = Some(player);
                true
            }
            Err(error) => {
                self.fail(None, error);
                false
            }
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Open {
                request,
                source,
                start_ms,
            } => self.open(request, source, start_ms),
            Command::Play => self.play(),
            Command::Pause => {
                if self.state.has_media()
                    && let Some(player) = &self.player
                {
                    player.set_pause(true);
                }
            }
            Command::TogglePlayback => {
                if matches!(
                    self.state,
                    PlaybackState::Playing | PlaybackState::Buffering
                ) {
                    if let Some(player) = &self.player {
                        player.set_pause(true);
                    }
                } else {
                    self.play();
                }
            }
            Command::Stop => {
                if let Some(player) = &self.player {
                    player.stop();
                }
                if self.current.is_some() {
                    self.time_ms = 0;
                    self.set_state(PlaybackState::Stopped);
                    self.emit_progress(true);
                }
            }
            Command::SeekTo(ms) => self.seek(ms),
            Command::SeekBy(delta) => {
                let base = self
                    .player
                    .as_ref()
                    .and_then(Player::time_ms)
                    .filter(|t| *t >= 0)
                    .unwrap_or(self.time_ms);
                self.seek(base.saturating_add(delta));
            }
            Command::SetVolume(volume) => {
                self.volume = clamp_volume(i32::from(volume)).min(MAX_VOLUME);
                if let Some(player) = &self.player {
                    player.set_volume(self.volume);
                }
                self.emit(Event::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
            Command::SetMuted(muted) => {
                self.muted = muted;
                if let Some(player) = &self.player {
                    player.set_muted(muted);
                }
                self.emit(Event::VolumeChanged {
                    volume: self.volume,
                    muted,
                });
            }
            Command::SetRate(rate) => {
                self.rate = nearest_rate(rate);
                if let Some(player) = &self.player {
                    player.set_rate(self.rate);
                }
                self.emit(Event::RateChanged { rate: self.rate });
            }
            Command::SelectAudioTrack(id) => {
                let ok = self
                    .player
                    .as_ref()
                    .is_some_and(|p| self.state.has_media() && p.select_audio_track(id));
                if !ok {
                    self.fail(self.current_request(), PlaybackError::TrackUnavailable);
                }
                self.flush_tracks();
            }
            Command::SelectSubtitleTrack(id) => {
                let ok = self
                    .player
                    .as_ref()
                    .is_some_and(|p| self.state.has_media() && p.select_subtitle_track(id));
                if !ok {
                    self.fail(self.current_request(), PlaybackError::TrackUnavailable);
                }
                self.flush_tracks();
            }
            Command::LoadExternalSubtitle(path) => self.load_subtitle(path),
            Command::SetAudioDelay(ms) => self.set_delays(Some(clamp_delay(ms)), None),
            Command::SetSubtitleDelay(ms) => self.set_delays(None, Some(clamp_delay(ms))),
            Command::SetVideoTarget(target) => {
                self.video_target = target;
                if let Some(player) = &self.player {
                    player.set_video_target(target);
                }
            }
        }
    }

    fn open(&mut self, request: RequestId, source: MediaSource, start_ms: Option<i64>) {
        // Retire the previous item first so none of its late events can be mistaken for ours.
        if let Some(player) = &mut self.player {
            player.unload();
        }
        self.current = Some(Current {
            request,
            source: source.clone(),
            applied_output_settings: false,
            pending_seek: None,
            last_buffering: -1,
        });
        self.state = PlaybackState::Idle;
        self.length_ms = None;
        self.time_ms = start_ms.unwrap_or(0).max(0);
        self.seekable = false;
        self.progress.reset();
        self.tracks_dirty = false;
        // LibVLC resets synchronization per input.
        self.audio_delay_ms = 0;
        self.subtitle_delay_ms = 0;
        self.emit(Event::MediaOpening { request });
        self.set_state(PlaybackState::Opening);

        let source = match source {
            MediaSource::File(path) => match check_file(&path) {
                Ok(path) => MediaSource::File(path),
                Err(error) => return self.fail(Some(request), error),
            },
            url => url,
        };

        if !self.ensure_player() {
            self.set_state(PlaybackState::Error);
            return;
        }
        let tx = self.tx.clone();
        let sink = Box::new(move |event| {
            // Non-blocking; fails only if the actor is gone, which is fine to ignore.
            let _ = tx.send(Message::Engine(request, event));
        });
        let runtime = self.runtime.as_ref().expect("player implies runtime");
        let player = self.player.as_mut().expect("ensured above");
        if let Err(error) = player.load(runtime, &source, start_ms, sink) {
            return self.fail(Some(request), error);
        }
        player.set_video_target(self.video_target);
        if !player.play() {
            let name = source.display_name();
            let is_stream = source.is_stream();
            self.fail(
                Some(request),
                PlaybackError::PlaybackFailed { name, is_stream },
            );
        }
    }

    fn play(&mut self) {
        let Some(player) = &self.player else { return };
        if self.current.is_none() {
            return;
        }
        if matches!(
            self.state,
            PlaybackState::Ended | PlaybackState::Stopped | PlaybackState::Error
        ) {
            // An ended/stopped input cannot resume; restart it from the beginning (or from a
            // seek requested meanwhile).
            player.stop();
            self.time_ms = 0;
        }
        if !player.play()
            && let Some(current) = &self.current
        {
            let error = PlaybackError::PlaybackFailed {
                name: current.source.display_name(),
                is_stream: current.source.is_stream(),
            };
            let request = current.request;
            self.fail(Some(request), error);
        }
    }

    fn seek(&mut self, target_ms: i64) {
        if self.current.is_none() {
            return;
        }
        let target = clamp_seek(target_ms, self.length_ms);
        if matches!(self.state, PlaybackState::Ended | PlaybackState::Stopped) {
            if let Some(current) = &mut self.current {
                current.pending_seek = Some(target);
            }
            self.time_ms = target;
            self.emit_progress(true);
            self.play();
            return;
        }
        if !self.state.has_media() {
            return;
        }
        if !self.seekable {
            return self.fail(self.current_request(), PlaybackError::SeekUnsupported);
        }
        if let Some(player) = &self.player {
            player.set_time(target);
        }
        self.time_ms = target;
        self.emit_progress(true);
    }

    fn set_delays(&mut self, audio_ms: Option<i64>, subtitle_ms: Option<i64>) {
        let (Some(player), Some(request)) = (&self.player, self.current_request()) else {
            return;
        };
        if !self.state.has_media() {
            return;
        }
        if let Some(ms) = audio_ms
            && player.set_audio_delay(ms)
        {
            self.audio_delay_ms = ms;
        }
        if let Some(ms) = subtitle_ms
            && player.set_subtitle_delay(ms)
        {
            self.subtitle_delay_ms = ms;
        }
        self.emit(Event::DelaysChanged {
            request,
            audio_ms: self.audio_delay_ms,
            subtitle_ms: self.subtitle_delay_ms,
        });
    }

    fn load_subtitle(&mut self, path: PathBuf) {
        let error = PlaybackError::SubtitleLoadFailed(path.clone());
        let Some(player) = self.player.as_ref().filter(|_| self.state.has_media()) else {
            return self.fail(self.current_request(), error);
        };
        let path = match check_file(&path) {
            Ok(path) => path,
            Err(_) => return self.fail(self.current_request(), error),
        };
        if !player.add_subtitle_file(&path) {
            return self.fail(self.current_request(), error);
        }
        self.tracks_dirty = true;
    }

    fn handle_engine(&mut self, request: RequestId, event: EngineEvent) {
        if self.current_request() != Some(request) {
            // Late event from media that has been replaced.
            return;
        }
        match event {
            EngineEvent::Opening => self.set_state(PlaybackState::Opening),
            EngineEvent::Buffering(percent) => {
                let rounded = percent.clamp(0.0, 100.0) as i32;
                let current = self.current.as_mut().expect("checked above");
                if rounded != current.last_buffering {
                    current.last_buffering = rounded;
                    self.emit(Event::Buffering {
                        request,
                        percent: rounded as f32,
                    });
                }
            }
            EngineEvent::Playing => {
                self.set_state(PlaybackState::Playing);
                self.on_playing();
            }
            EngineEvent::Paused => self.set_state(PlaybackState::Paused),
            EngineEvent::Stopped => {
                if !matches!(self.state, PlaybackState::Ended | PlaybackState::Error) {
                    self.set_state(PlaybackState::Stopped);
                }
            }
            EngineEvent::EndReached => {
                if self.ended_without_media() {
                    // LibVLC's demuxer fallback can "play" unrecognizable data and end at once
                    // without reporting an error; surface it as the failure it is.
                    return self.fail_current(request);
                }
                if let Some(length) = self.length_ms {
                    self.time_ms = length;
                }
                self.emit_progress(true);
                self.set_state(PlaybackState::Ended);
                self.emit(Event::EndReached { request });
            }
            EngineEvent::EncounteredError => self.fail_current(request),
            EngineEvent::TimeChanged(time) => {
                self.time_ms = time.max(0);
                self.emit_progress(false);
            }
            EngineEvent::LengthChanged(length) => {
                let length = (length > 0).then_some(length);
                if length != self.length_ms {
                    self.length_ms = length;
                    self.emit(Event::LengthChanged {
                        request,
                        length_ms: length,
                    });
                }
            }
            EngineEvent::SeekableChanged(seekable) => {
                if seekable != self.seekable {
                    self.seekable = seekable;
                    self.emit(Event::SeekableChanged { request, seekable });
                }
            }
            EngineEvent::Vout(count) => self.emit(Event::VideoOutputChanged {
                request,
                has_video: count > 0,
            }),
            EngineEvent::TracksChanged => self.tracks_dirty = true,
        }
    }

    fn fail_current(&mut self, request: RequestId) {
        let current = self.current.as_ref().expect("caller checked the request");
        let error = PlaybackError::PlaybackFailed {
            name: current.source.display_name(),
            is_stream: current.source.is_stream(),
        };
        self.fail(Some(request), error);
    }

    /// Whether the item reached its end without ever exposing a decodable stream: no length, no
    /// position, no video output and no audio track.
    fn ended_without_media(&self) -> bool {
        let Some(player) = &self.player else {
            return false;
        };
        self.length_ms.is_none()
            && self.time_ms <= 0
            && !player.has_video()
            && player.tracks().real_audio().next().is_none()
    }

    /// First `Playing` of an item: its audio output now exists, so volume/mute/rate stick.
    fn on_playing(&mut self) {
        let Some(player) = &self.player else { return };
        let Some(current) = &mut self.current else {
            return;
        };
        if !current.applied_output_settings {
            current.applied_output_settings = true;
            player.set_volume(self.volume);
            player.set_muted(self.muted);
            if self.rate != 1.0 {
                player.set_rate(self.rate);
            }
            self.seekable = player.is_seekable();
            let request = current.request;
            self.emit(Event::SeekableChanged {
                request,
                seekable: self.seekable,
            });
            self.emit(Event::VideoOutputChanged {
                request,
                has_video: player.has_video(),
            });
            if let Some(length) = player.length_ms() {
                self.length_ms = Some(length);
                self.emit(Event::LengthChanged {
                    request,
                    length_ms: Some(length),
                });
            }
            self.tracks_dirty = true;
        }
        if let Some(target) = self.current.as_mut().and_then(|c| c.pending_seek.take())
            && let Some(player) = &self.player
        {
            player.set_time(target);
        }
    }

    fn emit_progress(&mut self, force: bool) {
        let Some(request) = self.current_request() else {
            return;
        };
        if force {
            self.progress.reset();
        }
        if self.progress.accept(Instant::now()) {
            self.emit(Event::Progress {
                request,
                time_ms: self.time_ms,
                length_ms: self.length_ms,
            });
        }
    }

    fn flush_tracks(&mut self) {
        self.tracks_dirty = false;
        let (Some(player), Some(request)) = (&self.player, self.current_request()) else {
            return;
        };
        let tracks = player.tracks();
        self.emit(Event::TracksChanged { request, tracks });
    }
}

/// Validate a local file on the playback thread (never on the UI thread).
fn check_file(path: &Path) -> Result<PathBuf, PlaybackError> {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let metadata = std::fs::metadata(&path).map_err(|error| io_error(&path, &error))?;
    if metadata.is_dir() {
        return Err(PlaybackError::NotAFile(path));
    }
    // Opening proves readability (permissions, ACLs, sandboxing) without reading content.
    std::fs::File::open(&path).map_err(|error| io_error(&path, &error))?;
    Ok(path)
}

fn io_error(path: &Path, error: &std::io::Error) -> PlaybackError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => PlaybackError::AccessDenied(path.to_path_buf()),
        _ => PlaybackError::FileNotFound(path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_file_maps_errors() {
        let dir = std::env::temp_dir();
        assert!(matches!(check_file(&dir), Err(PlaybackError::NotAFile(_))));
        assert!(matches!(
            check_file(Path::new("/definitely/not/here.mkv")),
            Err(PlaybackError::FileNotFound(_))
        ));
        let file = dir.join(format!("mono-player-check-{}.bin", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(check_file(&file), Ok(file.clone()));
        std::fs::remove_file(file).unwrap();
    }
}
