//! Playback actor integration tests against the real LibVLC runtime.
//!
//! Media is generated on the fly (a short PCM WAV), so no binary fixtures are committed. The
//! tests do not need an audio device: LibVLC keeps its clock running when audio output fails.
//! Set `MONO_PLAYER_SKIP_LIBVLC_TESTS=1` to skip them on machines without LibVLC plugins.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use mono_player::playback::state::PlaybackState;
use mono_player::playback::{
    Command, Event, MediaSource, PlaybackError, PlaybackHandle, RequestId,
};

const TIMEOUT: Duration = Duration::from_secs(15);

fn skip() -> bool {
    std::env::var_os("MONO_PLAYER_SKIP_LIBVLC_TESTS").is_some()
}

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("mono-player-it-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// A mono 16-bit 8 kHz sine WAV of `seconds`, with a non-ASCII name containing spaces.
    fn wav(&self, seconds: u32) -> PathBuf {
        let path = self.dir.join(format!("tóne {seconds}s ♪.wav"));
        write_wav(&path, seconds);
        path
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_wav(path: &Path, seconds: u32) {
    let rate = 8_000u32;
    let samples = rate * seconds;
    let mut data = Vec::with_capacity(44 + samples as usize * 2);
    let data_len = samples * 2;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + data_len).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes()); // PCM
    data.extend_from_slice(&1u16.to_le_bytes()); // mono
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&(rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        let t = i as f32 / rate as f32;
        let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 8_000.0) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
}

struct Harness {
    handle: PlaybackHandle,
    events: UnboundedReceiver<Event>,
    next: u64,
}

impl Harness {
    fn new() -> Self {
        let (tx, events) = unbounded();
        let handle = PlaybackHandle::spawn(tx).expect("spawn playback thread");
        Self {
            handle,
            events,
            next: 0,
        }
    }

    fn send(&self, command: Command) {
        self.handle.send(command).expect("actor alive");
    }

    fn open(&mut self, path: &Path) -> RequestId {
        self.next += 1;
        let request = RequestId(self.next);
        self.send(Command::Open {
            request,
            source: MediaSource::File(path.to_path_buf()),
            start_ms: None,
        });
        request
    }

    /// Collect events until `done` matches one; panics on timeout.
    fn until(&mut self, what: &str, mut done: impl FnMut(&Event) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + TIMEOUT;
        let mut seen = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    let matched = done(&event);
                    seen.push(event);
                    if matched {
                        return seen;
                    }
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => panic!("timed out waiting for {what}; saw {seen:#?}"),
            }
        }
    }

    fn state(&mut self, request: RequestId, wanted: PlaybackState) -> Vec<Event> {
        self.until(&format!("{wanted:?}"), |event| {
            matches!(event, Event::StateChanged { request: r, state } if *r == request && *state == wanted)
        })
    }

    fn error(&mut self) -> PlaybackError {
        let events = self.until("an error", |event| matches!(event, Event::Error { .. }));
        match events.last() {
            Some(Event::Error { error, .. }) => error.clone(),
            _ => unreachable!(),
        }
    }

    fn shutdown(mut self) {
        let done = self.handle.shutdown();
        let started = Instant::now();
        self.until("ShutdownComplete", |e| matches!(e, Event::ShutdownComplete));
        futures::executor::block_on(done).expect("shutdown signalled");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "teardown took {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn plays_seeks_ends_and_restarts() {
    if skip() {
        return;
    }
    let fixture = Fixture::new("play");
    let wav = fixture.wav(3);
    let mut player = Harness::new();

    let request = player.open(&wav);
    let events = player.state(request, PlaybackState::Playing);
    assert!(events.iter().any(|e| matches!(e, Event::RuntimeStarting)));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::RuntimeReady { version } if version.starts_with('3')))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::MediaOpening { request: r } if *r == request))
    );

    // Length arrives and is about three seconds.
    let events = player.until("length", |e| {
        matches!(
            e,
            Event::LengthChanged {
                length_ms: Some(_),
                ..
            }
        ) || matches!(
            e,
            Event::Progress {
                length_ms: Some(_),
                ..
            }
        )
    });
    let length = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::LengthChanged { length_ms, .. } | Event::Progress { length_ms, .. } => {
                *length_ms
            }
            _ => None,
        })
        .unwrap();
    assert!((2_500..=3_500).contains(&length), "length {length}");

    // Progress cadence is capped (PRD: 4–10 updates per second).
    let window_start = Instant::now();
    let mut progress = 0;
    while window_start.elapsed() < Duration::from_millis(1_000) {
        if let Ok(Event::Progress { .. }) = player.events.try_recv() {
            progress += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(progress <= 10, "{progress} progress events in one second");

    // Seek near the end; a progress event reflects it immediately.
    player.send(Command::SeekTo(2_000));
    player.until(
        "seek progress",
        |e| matches!(e, Event::Progress { time_ms, .. } if *time_ms >= 2_000),
    );

    player.until(
        "end",
        |e| matches!(e, Event::EndReached { request: r } if *r == request),
    );

    // Play after the end restarts the item.
    player.send(Command::Play);
    player.state(request, PlaybackState::Playing);

    player.shutdown();
}

#[test]
fn pause_resume_volume_and_rate() {
    if skip() {
        return;
    }
    let fixture = Fixture::new("controls");
    let wav = fixture.wav(5);
    let mut player = Harness::new();
    let request = player.open(&wav);
    player.state(request, PlaybackState::Playing);

    player.send(Command::Pause);
    player.state(request, PlaybackState::Paused);
    player.send(Command::TogglePlayback);
    player.state(request, PlaybackState::Playing);

    player.send(Command::SetVolume(30));
    player.until("volume", |e| {
        matches!(e, Event::VolumeChanged { volume: 30, .. })
    });
    player.send(Command::SetMuted(true));
    player.until("mute", |e| {
        matches!(e, Event::VolumeChanged { muted: true, .. })
    });
    player.send(Command::SetRate(1.4));
    player.until(
        "rate",
        |e| matches!(e, Event::RateChanged { rate } if *rate == 1.5),
    );

    player.send(Command::SetSubtitleDelay(500));
    player.until("subtitle delay", |e| {
        matches!(
            e,
            Event::DelaysChanged {
                subtitle_ms: 500,
                audio_ms: 0,
                ..
            }
        )
    });
    player.send(Command::SetAudioDelay(-99_000));
    player.until("clamped audio delay", |e| {
        matches!(
            e,
            Event::DelaysChanged {
                audio_ms: -30_000,
                ..
            }
        )
    });

    player.send(Command::Stop);
    player.state(request, PlaybackState::Stopped);
    player.shutdown();
}

#[test]
fn errors_are_typed_and_recoverable() {
    if skip() {
        return;
    }
    let fixture = Fixture::new("errors");
    let mut player = Harness::new();

    player.open(&fixture.dir.join("missing file.mp4"));
    assert!(matches!(player.error(), PlaybackError::FileNotFound(_)));

    player.open(&fixture.dir);
    assert!(matches!(player.error(), PlaybackError::NotAFile(_)));

    let corrupt = fixture.file("corrupt.mkv", b"definitely not matroska");
    player.open(&corrupt);
    assert!(matches!(
        player.error(),
        PlaybackError::PlaybackFailed {
            is_stream: false,
            ..
        }
    ));

    // A bad subtitle path is reported without ending playback.
    let wav = fixture.wav(3);
    let request = player.open(&wav);
    player.state(request, PlaybackState::Playing);
    player.send(Command::LoadExternalSubtitle(fixture.dir.join("nope.srt")));
    let error = player.error();
    assert!(matches!(error, PlaybackError::SubtitleLoadFailed(_)));
    assert!(!error.ends_item());

    // Tracks are reported for the playing item.
    player.until("tracks", |e| {
        matches!(e, Event::TracksChanged { request: r, tracks } if *r == request && !tracks.audio.is_empty())
    });

    player.shutdown();
}

#[test]
fn rapid_switching_settles_on_the_last_request() {
    if skip() {
        return;
    }
    let fixture = Fixture::new("switch");
    let wav = fixture.wav(4);
    let mut player = Harness::new();

    let mut last = RequestId(0);
    for _ in 0..15 {
        last = player.open(&wav);
    }
    player.state(last, PlaybackState::Playing);

    // Once the last item plays, nothing from replaced items may arrive.
    let quiet_until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < quiet_until {
        if let Ok(event) = player.events.try_recv() {
            let request = match &event {
                Event::StateChanged { request, .. }
                | Event::Progress { request, .. }
                | Event::LengthChanged { request, .. }
                | Event::SeekableChanged { request, .. }
                | Event::VideoOutputChanged { request, .. }
                | Event::TracksChanged { request, .. }
                | Event::Buffering { request, .. }
                | Event::EndReached { request }
                | Event::MediaOpening { request } => Some(*request),
                _ => None,
            };
            assert!(
                request.is_none_or(|r| r == last),
                "stale event after switching: {event:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    player.shutdown();
}

#[test]
fn shutdown_while_playing_and_when_idle() {
    if skip() {
        return;
    }
    // Idle: LibVLC was never started; shutdown is immediate.
    let player = Harness::new();
    player.shutdown();

    // Playing.
    let fixture = Fixture::new("shutdown");
    let wav = fixture.wav(10);
    let mut player = Harness::new();
    let request = player.open(&wav);
    player.state(request, PlaybackState::Playing);
    player.shutdown();
}
