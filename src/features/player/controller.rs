//! Owns the playback actor handle: request ids, command dispatch, and recovery if the
//! playback thread dies. Exposes a binding-independent interface to the view.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;

use crate::diagnostics as log;
use crate::playback::{Command, Event, MediaSource, PlaybackHandle, RequestId};
use crate::video_output::VideoTarget;

pub struct PlaybackController {
    handle: Option<PlaybackHandle>,
    events: UnboundedSender<Event>,
    next_request: u64,
    // Replayed into a respawned actor so a recovered thread matches the UI.
    video_target: Option<VideoTarget>,
    volume: u8,
    muted: bool,
    rate: f32,
    /// `Some` for a recording controller: commands are kept instead of sent to a thread.
    recorded: Option<Vec<Command>>,
}

impl PlaybackController {
    /// Spawn the (idle) playback thread. LibVLC is not initialized until the first `open`.
    pub fn new(volume: u8, muted: bool) -> (Self, UnboundedReceiver<Event>) {
        let (events, receiver) = unbounded();
        let mut controller = Self {
            handle: None,
            events,
            next_request: 0,
            video_target: None,
            volume,
            muted,
            rate: 1.0,
            recorded: None,
        };
        controller.respawn();
        (controller, receiver)
    }

    /// A controller without a playback thread, for UI tests: commands are recorded and the
    /// caller drives the view by sending events into the returned sender (on its own thread,
    /// which keeps GPUI's deterministic test scheduler happy).
    pub fn recording() -> (Self, UnboundedReceiver<Event>) {
        let (events, receiver) = unbounded();
        let controller = Self {
            handle: None,
            events,
            next_request: 0,
            video_target: None,
            volume: 80,
            muted: false,
            rate: 1.0,
            recorded: Some(Vec::new()),
        };
        (controller, receiver)
    }

    /// Commands sent so far by a recording controller.
    pub fn recorded(&self) -> &[Command] {
        self.recorded.as_deref().unwrap_or_default()
    }

    /// The sender events arrive on (used by tests to play the actor's role).
    pub fn event_sender(&self) -> UnboundedSender<Event> {
        self.events.clone()
    }

    fn respawn(&mut self) {
        match PlaybackHandle::spawn(self.events.clone()) {
            Ok(handle) => {
                let _ = handle.send(Command::SetVolume(self.volume));
                let _ = handle.send(Command::SetMuted(self.muted));
                let _ = handle.send(Command::SetRate(self.rate));
                let _ = handle.send(Command::SetVideoTarget(self.video_target));
                self.handle = Some(handle);
            }
            Err(error) => {
                log::warn(&format!("could not start playback thread: {error}"));
                self.handle = None;
            }
        }
    }

    /// Queue a command; never blocks. Restarts the playback thread once if it has died.
    pub fn send(&mut self, command: Command) {
        match &command {
            Command::SetVideoTarget(target) => self.video_target = *target,
            Command::SetVolume(volume) => self.volume = *volume,
            Command::SetMuted(muted) => self.muted = *muted,
            Command::SetRate(rate) => self.rate = *rate,
            _ => {}
        }
        if let Some(recorded) = &mut self.recorded {
            recorded.push(command);
            return;
        }
        let command = match &self.handle {
            Some(handle) => match handle.send(command) {
                Ok(()) => return,
                Err(command) => command,
            },
            None => command,
        };
        log::warn("playback thread unavailable; restarting it");
        self.respawn();
        if let Some(handle) = &self.handle {
            let _ = handle.send(command);
        }
    }

    /// Open a source and return the id its events will carry.
    pub fn open(&mut self, source: MediaSource, start_ms: Option<i64>) -> RequestId {
        self.next_request += 1;
        let request = RequestId(self.next_request);
        self.send(Command::Open {
            request,
            source,
            start_ms,
        });
        request
    }

    /// Begin ordered shutdown. Completes when LibVLC has been released.
    pub fn shutdown(&mut self) -> oneshot::Receiver<()> {
        match self.handle.as_mut() {
            Some(handle) if handle.is_alive() => handle.shutdown(),
            _ => {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(());
                rx
            }
        }
    }
}
