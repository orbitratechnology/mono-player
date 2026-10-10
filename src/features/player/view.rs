//! The root player view: video area, control bar and side panel.
//!
//! State changes flow one way: user input → [`PlaybackController`] command → actor → event →
//! [`PlayerModel::apply`] → render. Nothing here calls LibVLC, polls the engine or touches the
//! filesystem on the UI thread (settings are written on the background executor).

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, MouseMoveEvent, ParentElement as _,
    PathPromptOptions, Pixels, Render, SharedString, Size, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, TestSupportExt as _, Window, canvas, div,
    prelude::FluentBuilder as _, px, size,
};

use super::commands::{RESTART_THRESHOLD_MS, classify_paths, seek_offset_ms};
use super::controller::PlaybackController;
use super::model::{Outcome, PlayerModel, RuntimeStatus};
use crate::actions::*;
use crate::components::timeline::Timeline;
use crate::components::transport_controls::TransportControls;
use crate::components::volume_control::VolumeControl;
use crate::components::{action_button, icon_button};
use crate::diagnostics as log;
use crate::features::playlist::model::{Advance, Playlist, QueueItemId};
use crate::persistence::settings::{self, Settings, WindowGeometry};
use crate::playback::state::{
    DELAY_STEP_MS, MAX_VOLUME, PlaybackState, VOLUME_STEP, clamp_volume, format_delay, format_rate,
    format_time, step_rate,
};
use crate::playback::{Command, Event, MediaSource, PlaybackError, RequestId};
use crate::theme::metrics::{EDGE, HIT_TARGET, PANEL_WIDTH, ROW_HEIGHT, TARGET_GAP};
use crate::theme::tokens;
use crate::video_output::VideoSurface;

/// Fullscreen controls hide after this much inactivity during playback.
const CONTROLS_HIDE_DELAY: Duration = Duration::from_millis(2_500);
/// Settings writes are debounced so dragging a slider does not write the file repeatedly.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);
/// Upper bound on waiting for LibVLC teardown when quitting.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// Timeline slider resolution (slider value is a fraction of the length).
const TIMELINE_MAX: f32 = 1.0;

/// Which side panel is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Queue,
    Tracks,
    Settings,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::Tracks => "Tracks",
            Self::Settings => "Settings",
        }
    }
}

/// The queue entry currently loaded in the engine.
#[derive(Clone, Copy, Debug)]
struct PlayingItem {
    request: RequestId,
    item: QueueItemId,
    /// Started by auto-advance (so a failure may skip to the next entry).
    automatic: bool,
}

pub struct PlayerView {
    focus_handle: FocusHandle,
    controller: PlaybackController,
    pub(crate) model: PlayerModel,
    pub(crate) playlist: Playlist,
    playing: Option<PlayingItem>,
    pub(crate) settings: Settings,
    settings_path: Option<PathBuf>,
    save_task: Option<Task<()>>,
    timeline: Entity<SliderState>,
    volume_slider: Entity<SliderState>,
    /// Local preview while the user drags the timeline; progress events don't fight the drag.
    seek_preview: Option<i64>,
    pub(crate) url_input: Entity<InputState>,
    pub(crate) panel: Option<Panel>,
    video: Option<VideoSurface>,
    pub(crate) video_error: Option<String>,
    fullscreen: bool,
    controls_visible: bool,
    hide_controls_task: Option<Task<()>>,
    last_activity: Option<Instant>,
    shutting_down: bool,
    pub(crate) playlist_row_sizes: Rc<Vec<Size<Pixels>>>,
    _event_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl PlayerView {
    pub fn new(
        settings: Settings,
        settings_path: Option<PathBuf>,
        initial_paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let (controller, mut events) = PlaybackController::new(settings.volume, settings.muted);

        let timeline = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(TIMELINE_MAX)
                .step(0.0001)
                .default_value(0.)
        });
        let volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(f32::from(MAX_VOLUME))
                .step(1.)
                .default_value(f32::from(settings.volume))
        });
        let url_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("Add a file path or stream address"));

        let mut subscriptions = vec![
            cx.subscribe_in(&timeline, window, Self::on_timeline_event),
            cx.subscribe_in(&volume_slider, window, Self::on_volume_event),
            cx.subscribe_in(
                &url_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.add_address(window, cx);
                    }
                },
            ),
            cx.observe_window_bounds(window, |this, window, cx| {
                this.on_window_bounds_changed(window, cx)
            }),
        ];
        subscriptions.push(cx.on_app_quit(|this, _| {
            // Covers quits that don't go through `begin_shutdown` (e.g. the OS).
            this.shutting_down = true;
            let done = this.controller.shutdown();
            async move {
                let _ = done.await;
            }
        }));

        // Pump actor events into the view. Batches are drained so a backlog of progress
        // updates becomes a single redraw.
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this
                    .update_in(cx, |view, window, cx| view.apply_events(batch, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        // The native child surface LibVLC renders into. Creating it is cheap; LibVLC itself
        // starts only when media is first opened.
        let (video, video_error) = match VideoSurface::create(window) {
            Ok(surface) => (Some(surface), None),
            Err(error) => {
                log::warn(&format!("video surface unavailable: {error}"));
                (None, Some(error))
            }
        };

        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            // Close only after LibVLC has let go of the video surface.
            entity
                .update(cx, |view, cx| view.begin_shutdown(window, cx))
                .is_err()
        });

        let mut view = Self {
            focus_handle,
            controller,
            model: PlayerModel {
                volume: settings.volume,
                muted: settings.muted,
                ..PlayerModel::default()
            },
            playlist: Playlist::new(),
            playing: None,
            settings,
            settings_path,
            save_task: None,
            timeline,
            volume_slider,
            seek_preview: None,
            url_input,
            panel: None,
            video,
            video_error,
            fullscreen: window.is_fullscreen(),
            controls_visible: true,
            hide_controls_task: None,
            last_activity: None,
            shutting_down: false,
            playlist_row_sizes: Rc::new(Vec::new()),
            _event_task: event_task,
            _subscriptions: subscriptions,
        };
        view.playlist.set_repeat(view.settings.repeat);
        if let Some(surface) = &view.video {
            view.controller
                .send(Command::SetVideoTarget(Some(surface.target())));
        }
        view.focus_handle.focus(window, cx);
        if !initial_paths.is_empty() {
            view.open_paths(initial_paths, true, window, cx);
        }
        view
    }

    /// The current playback snapshot.
    pub fn model(&self) -> &PlayerModel {
        &self.model
    }

    /// The session queue.
    pub fn playlist(&self) -> &Playlist {
        &self.playlist
    }

    /// The open side panel, if any.
    pub fn panel(&self) -> Option<Panel> {
        self.panel
    }

    /// The address field's state (for focus management and tests).
    pub fn address_input(&self) -> &Entity<InputState> {
        &self.url_input
    }

    // ----- Events from the playback actor -------------------------------------------------

    fn apply_events(&mut self, batch: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        for event in batch {
            match self.model.apply(event) {
                Outcome::Unchanged | Outcome::ShutdownComplete => {}
                Outcome::Changed => changed = true,
                Outcome::EndReached(request) => {
                    changed = true;
                    self.on_end_reached(request, window, cx);
                }
                Outcome::ItemFailed(request) => {
                    changed = true;
                    self.on_item_failed(request, window, cx);
                }
            }
        }
        if changed {
            self.sync_sliders(window, cx);
            self.sync_video_visibility();
            self.update_window_title(window);
            cx.notify();
        }
    }

    fn on_end_reached(&mut self, request: RequestId, window: &mut Window, cx: &mut Context<Self>) {
        if self.playing.map(|p| p.request) != Some(request) {
            return;
        }
        self.remember_position();
        if let Some(next) = self.playlist.advance(Advance::Automatic) {
            self.play_item(next, true, window, cx);
        } else {
            self.show_controls(window, cx);
        }
    }

    fn on_item_failed(&mut self, request: RequestId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(playing) = self.playing.filter(|p| p.request == request) else {
            return;
        };
        self.playlist.mark_failed(playing.item, true);
        // Keep the queue moving when the failure came from auto-advance; a file the user just
        // picked stays selected with its error shown.
        if playing.automatic
            && let Some(next) = self.playlist.peek_next(Advance::User)
            && next != playing.item
        {
            self.playlist.advance(Advance::User);
            self.play_item(next, true, window, cx);
        }
    }

    // ----- Opening media -------------------------------------------------------------------

    /// Queue paths from the picker, a drop, or the command line. Subtitle files are attached to
    /// the current item instead of being queued.
    pub fn open_paths(
        &mut self,
        paths: Vec<PathBuf>,
        play_now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opened = classify_paths(paths);
        if !opened.media.is_empty() {
            self.enqueue(opened.media, play_now, window, cx);
        }
        // After queueing, so `movie.mkv movie.srt` opened together attaches the subtitle to
        // the movie (the actor applies commands in order).
        for subtitle in opened.subtitles {
            if self.model.state.has_media() {
                self.controller
                    .send(Command::LoadExternalSubtitle(subtitle));
            } else {
                self.model.error = Some(PlaybackError::SubtitleLoadFailed(subtitle));
            }
        }
        cx.notify();
    }

    fn enqueue(
        &mut self,
        sources: Vec<MediaSource>,
        play_now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let idle = !self.model.state.has_media();
        let ids = self.playlist.add(sources);
        self.refresh_row_sizes();
        if let Some(first) = ids.first().copied()
            && (play_now || idle)
        {
            self.play_item(first, false, window, cx);
        }
    }

    pub(crate) fn play_item(
        &mut self,
        id: QueueItemId,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.playlist.get(id).cloned() else {
            return;
        };
        self.remember_position();
        self.playlist.set_current(id);
        self.playlist.mark_failed(id, false);
        let location = item.source.location();
        let start = self.settings.resume_position(&location);
        self.settings
            .touch_recent(&location, matches!(item.source, MediaSource::Url(_)));
        let request = self.controller.open(item.source.clone(), start);
        self.playing = Some(PlayingItem {
            request,
            item: id,
            automatic,
        });
        self.model.begin(request, item.title.clone(), start);
        self.seek_preview = None;
        self.sync_sliders(window, cx);
        self.sync_video_visibility();
        self.update_window_title(window);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Open the text in the address field (URL or local path).
    pub(crate) fn add_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.url_input.read(cx).value().to_string();
        match MediaSource::parse(&text) {
            Ok(source) => {
                self.url_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.enqueue(vec![source], true, window, cx);
            }
            Err(error) => self.model.error = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn prompt_open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_paths(false, window, cx);
    }

    pub(crate) fn prompt_subtitle_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_paths(true, window, cx);
    }

    fn prompt_paths(&mut self, subtitles: bool, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: !subtitles,
            prompt: Some(if subtitles { "Load subtitle" } else { "Open" }.into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = receiver.await;
            let _ = this.update_in(cx, |view, window, cx| match result {
                Ok(Ok(Some(paths))) if subtitles => {
                    for path in paths {
                        view.controller.send(Command::LoadExternalSubtitle(path));
                    }
                }
                Ok(Ok(Some(paths))) => view.open_paths(paths, true, window, cx),
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(error)) => {
                    log::warn(&format!("file picker failed: {error}"));
                    view.model.error = Some(PlaybackError::OpenFailed {
                        name: "the file picker".into(),
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ----- Commands ------------------------------------------------------------------------

    fn toggle_playback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.request.is_some() {
            self.controller.send(Command::TogglePlayback);
        } else if let Some(id) = self
            .playlist
            .current_id()
            .or_else(|| self.playlist.items().first().map(|i| i.id))
        {
            self.play_item(id, false, window, cx);
        } else {
            self.prompt_open_files(window, cx);
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.remember_position();
        self.controller.send(Command::Stop);
        self.schedule_save(cx);
    }

    fn seek_by(&mut self, forward: bool, large: bool) {
        if self.model.request.is_some() {
            self.controller
                .send(Command::SeekBy(seek_offset_ms(forward, large)));
        }
    }

    fn previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.state.has_media() && self.model.time_ms > RESTART_THRESHOLD_MS {
            self.controller.send(Command::SeekTo(0));
        } else if let Some(previous) = self.playlist.go_previous() {
            self.play_item(previous, false, window, cx);
        }
    }

    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.playlist.advance(Advance::User) {
            self.play_item(next, false, window, cx);
        }
    }

    fn set_volume(&mut self, volume: u8, window: &mut Window, cx: &mut Context<Self>) {
        let volume = clamp_volume(i32::from(volume));
        if volume == self.settings.volume && volume == self.model.volume {
            return;
        }
        self.settings.volume = volume;
        self.model.volume = volume;
        self.controller.send(Command::SetVolume(volume));
        self.volume_slider.update(cx, |slider, cx| {
            slider.set_value(f32::from(volume), window, cx)
        });
        self.schedule_save(cx);
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.model.muted;
        self.model.muted = muted;
        self.settings.muted = muted;
        self.controller.send(Command::SetMuted(muted));
        self.schedule_save(cx);
        cx.notify();
    }

    pub(crate) fn set_rate(&mut self, rate: f32, cx: &mut Context<Self>) {
        self.controller.send(Command::SetRate(rate));
        self.model.rate = rate;
        cx.notify();
    }

    pub(crate) fn select_audio_track(&mut self, id: i32) {
        self.controller.send(Command::SelectAudioTrack(id));
    }

    pub(crate) fn select_subtitle_track(&mut self, id: i32) {
        self.controller.send(Command::SelectSubtitleTrack(id));
    }

    pub(crate) fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
        cx.notify();
    }

    pub(crate) fn remove_item(&mut self, id: QueueItemId, cx: &mut Context<Self>) {
        if self.playing.is_some_and(|p| p.item == id) {
            self.remember_position();
            self.controller.send(Command::Stop);
            self.playing = None;
            self.model.clear();
            self.sync_video_visibility();
        }
        self.playlist.remove(id);
        self.refresh_row_sizes();
        cx.notify();
    }

    pub(crate) fn move_item(&mut self, id: QueueItemId, delta: isize, cx: &mut Context<Self>) {
        if let Some(index) = self.playlist.index_of(id) {
            let target = index.saturating_add_signed(delta);
            self.playlist.move_item(id, target);
            cx.notify();
        }
    }

    pub(crate) fn clear_queue(&mut self, cx: &mut Context<Self>) {
        if self.playing.is_some() {
            self.remember_position();
            self.controller.send(Command::Stop);
        }
        self.playing = None;
        self.playlist.clear();
        self.model.clear();
        self.refresh_row_sizes();
        self.sync_video_visibility();
        cx.notify();
    }

    pub(crate) fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        let repeat = self.playlist.repeat().cycle();
        self.playlist.set_repeat(repeat);
        self.settings.repeat = repeat;
        self.schedule_save(cx);
        cx.notify();
    }

    pub(crate) fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        let shuffle = !self.playlist.shuffle();
        self.playlist.set_shuffle(shuffle);
        cx.notify();
    }

    pub(crate) fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.model.error = None;
        cx.notify();
    }

    pub(crate) fn settings_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_save(cx);
        cx.notify();
    }

    fn refresh_row_sizes(&mut self) {
        self.playlist_row_sizes = Rc::new(vec![size(PANEL_WIDTH, ROW_HEIGHT); self.playlist.len()]);
    }

    // ----- Sliders -------------------------------------------------------------------------

    fn on_timeline_event(
        &mut self,
        _: &Entity<SliderState>,
        event: &SliderEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(length) = self.model.length_ms else {
            return;
        };
        match event {
            SliderEvent::Change(SliderValue::Single(fraction)) => {
                // Local preview only: one engine seek happens on release.
                self.seek_preview = Some((f64::from(*fraction) * length as f64) as i64);
                cx.notify();
            }
            SliderEvent::Release(SliderValue::Single(fraction)) => {
                let target = (f64::from(*fraction) * length as f64) as i64;
                self.seek_preview = None;
                self.model.time_ms = target;
                self.controller.send(Command::SeekTo(target));
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_volume_event(
        &mut self,
        _: &Entity<SliderState>,
        event: &SliderEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (SliderEvent::Change(SliderValue::Single(value))
        | SliderEvent::Release(SliderValue::Single(value))) = event
        else {
            return;
        };
        let volume = clamp_volume(value.round() as i32);
        if volume != self.model.volume {
            self.model.volume = volume;
            self.settings.volume = volume;
            self.controller.send(Command::SetVolume(volume));
            cx.notify();
        }
        if matches!(event, SliderEvent::Release(_)) {
            self.schedule_save(cx);
        }
    }

    fn sync_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.seek_preview.is_none() {
            let progress = self.model.progress();
            self.timeline.update(cx, |slider, cx| {
                if slider.value() != SliderValue::Single(progress) {
                    slider.set_value(progress, window, cx);
                }
            });
        }
        let volume = f32::from(self.model.volume);
        self.volume_slider.update(cx, |slider, cx| {
            if slider.value() != SliderValue::Single(volume) {
                slider.set_value(volume, window, cx);
            }
        });
    }

    // ----- Video surface -------------------------------------------------------------------

    fn should_show_video(&self) -> bool {
        !self.shutting_down
            && self.model.has_video
            && matches!(
                self.model.state,
                PlaybackState::Playing | PlaybackState::Paused | PlaybackState::Buffering
            )
    }

    fn sync_video_visibility(&mut self) {
        let show = self.should_show_video();
        if let Some(surface) = &mut self.video {
            surface.set_visible(show);
        }
    }

    /// Called from the video area's prepaint with its laid-out bounds.
    fn place_video(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        let show = self.should_show_video();
        if let Some(surface) = &mut self.video {
            surface.set_bounds(bounds, scale);
            surface.set_visible(show);
        }
    }

    // ----- Fullscreen controls -------------------------------------------------------------

    fn on_window_bounds_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fullscreen = window.is_fullscreen();
        if fullscreen != self.fullscreen {
            self.fullscreen = fullscreen;
            self.show_controls(window, cx);
        }
        if !fullscreen {
            let bounds = window.bounds();
            self.settings.window = Some(WindowGeometry {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            });
            self.schedule_save(cx);
        }
    }

    /// Reveal controls; in fullscreen playback, schedule them to hide again. Throttled so
    /// pointer movement does not restart timers on every event.
    fn show_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let recently = self
            .last_activity
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(250));
        if self.controls_visible && recently {
            return;
        }
        self.last_activity = Some(now);
        if !self.controls_visible {
            self.controls_visible = true;
            cx.notify();
        }
        // Only video benefits from an unobstructed screen; audio keeps its controls.
        let auto_hide =
            self.fullscreen && self.model.has_video && self.model.state == PlaybackState::Playing;
        self.hide_controls_task = auto_hide.then(|| {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(CONTROLS_HIDE_DELAY).await;
                let _ = this.update(cx, |view, cx| {
                    if view.fullscreen
                        && view.model.has_video
                        && view.model.state == PlaybackState::Playing
                    {
                        view.controls_visible = false;
                        cx.notify();
                    }
                });
            })
        });
    }

    // ----- Persistence ---------------------------------------------------------------------

    fn remember_position(&mut self) {
        let Some(playing) = self.playing else { return };
        let Some(item) = self.playlist.get(playing.item) else {
            return;
        };
        if self.model.request == Some(playing.request) && self.model.time_ms > 0 {
            let location = item.source.location();
            self.settings
                .record_position(&location, self.model.time_ms, self.model.length_ms);
        }
    }

    /// Debounced write on the background executor.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok(snapshot) = this.update(cx, |view, _| view.settings.clone()) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { settings::save(&path, &snapshot) })
                .await;
            if let Err(error) = result {
                log::warn(&format!("could not save settings: {error}"));
            }
        }));
    }

    // ----- Shutdown ------------------------------------------------------------------------

    /// Ordered shutdown: hide the surface, stop the actor (which releases LibVLC), save
    /// settings, then quit. Returns `false` so the window stays until LibVLC has let go of it.
    fn begin_shutdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.shutting_down {
            return false;
        }
        self.shutting_down = true;
        self.remember_position();
        self.sync_video_visibility();
        let done = self.controller.shutdown();
        let snapshot = self.settings.clone();
        let path = self.settings_path.clone();
        self.save_task = None;
        cx.spawn_in(window, async move |_, cx| {
            let timer = cx.background_executor().timer(SHUTDOWN_GRACE);
            futures::future::select(done, Box::pin(timer)).await;
            if let Some(path) = path {
                let _ = cx
                    .background_executor()
                    .spawn(async move { settings::save(&path, &snapshot) })
                    .await;
            }
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
        false
    }

    fn update_window_title(&self, window: &mut Window) {
        let title = if self.model.title.is_empty() {
            "Mono Player".to_owned()
        } else {
            format!("{} — Mono Player", self.model.title)
        };
        window.set_window_title(&title);
    }

    // ----- Rendering -----------------------------------------------------------------------

    fn render_video_area(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens();
        let view = cx.entity().downgrade();
        let surface_shown = self.video.as_ref().is_some_and(VideoSurface::is_visible)
            || self.should_show_video() && self.video.is_some();
        let state = self.model.state;

        let placeholder = if surface_shown {
            None
        } else if self.model.request.is_none() {
            Some(self.render_empty_state(cx).into_any_element())
        } else if state.is_loading() {
            let label = match (&self.model.runtime, self.model.buffering) {
                (RuntimeStatus::Starting, _) => "Starting playback engine…".to_owned(),
                (_, Some(percent)) => format!("Buffering {percent:.0}%"),
                _ => "Opening…".to_owned(),
            };
            Some(
                v_flex()
                    .items_center()
                    .gap_3()
                    .child(Spinner::new().large().color(t.foreground_muted))
                    .child(div().text_color(t.foreground_muted).child(label))
                    .into_any_element(),
            )
        } else if state == PlaybackState::Error {
            Some(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::CircleAlert).large())
                    .child(
                        div().text_color(t.foreground_muted).child(
                            self.model
                                .error
                                .as_ref()
                                .map(|e| e.title())
                                .unwrap_or("Can't play media"),
                        ),
                    )
                    .into_any_element(),
            )
        } else {
            // Audio-only media (or video between frames): a quiet now-playing card.
            let icon = if self.model.has_video {
                IconName::Film
            } else {
                IconName::Music
            };
            Some(
                v_flex()
                    .items_center()
                    .gap_3()
                    .max_w(px(560.))
                    .px_6()
                    .child(Icon::new(icon).size(px(48.)).text_color(t.foreground_muted))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_center()
                            .child(SharedString::from(self.model.title.clone())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.foreground_muted)
                            .child(state.label()),
                    )
                    .into_any_element(),
            )
        };

        let _ = window;
        div()
            .id("video-area")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(t.background)
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let scale = window.scale_factor();
                        let _ = view.update(cx, |view, _| view.place_video(bounds, scale));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when_some(placeholder, |area, content| {
                area.child(
                    v_flex()
                        .absolute()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(content),
                )
            })
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        v_flex()
            .items_center()
            .gap_4()
            .child(
                Icon::new(IconName::AudioLines)
                    .size(px(48.))
                    .text_color(t.foreground_disabled),
            )
            .child(
                Button::new("empty-open")
                    .primary()
                    .icon(Icon::new(IconName::FolderOpen))
                    .label("Open file")
                    .h(HIT_TARGET)
                    .px_5()
                    .tooltip_with_action("Open file", &OpenFiles, Some(PLAYER_CONTEXT))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.prompt_open_files(window, cx)),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(t.foreground_muted)
                    .child("or drop audio and video files here"),
            )
    }

    fn render_error_banner(
        &self,
        error: &PlaybackError,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens();
        h_flex()
            .id("error-banner")
            .w_full()
            .px(EDGE)
            .py_1()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .bg(t.surface_hover)
            .child(Icon::new(IconName::CircleAlert))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(error.title()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.foreground_muted)
                            .child(SharedString::from(error.message())),
                    ),
            )
            .child(
                icon_button("error-dismiss", IconName::X, "Dismiss")
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_error(cx))),
            )
            .test_support()
    }

    fn render_control_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens();
        let model = &self.model;
        let shown_time = self.seek_preview.unwrap_or(model.time_ms);
        let total = model.length_ms.map(format_time).unwrap_or_else(|| {
            if model.state.has_media() {
                "--:--".into()
            } else {
                "0:00".into()
            }
        });

        let mut status = vec![model.state.label().to_owned()];
        if let Some(percent) = model.buffering.filter(|_| model.state.has_media()) {
            status.push(format!("buffering {percent:.0}%"));
        }
        if model.rate != 1.0 {
            status.push(format_rate(model.rate));
        }
        if self.playlist.len() > 1
            && let Some(index) = self.playing.and_then(|p| self.playlist.index_of(p.item))
        {
            status.push(format!("{} of {}", index + 1, self.playlist.len()));
        }
        let title: SharedString = if model.title.is_empty() {
            "Nothing playing".into()
        } else {
            model.title.clone().into()
        };

        let panel_button = |id: &'static str,
                            icon: IconName,
                            label: &'static str,
                            action: &dyn gpui_kit::Action,
                            panel: Panel| {
            action_button(id, icon, label, action).selected(self.panel == Some(panel))
        };
        let fullscreen_icon = if self.fullscreen {
            IconName::Minimize
        } else {
            IconName::Maximize
        };
        let fullscreen_label = if self.fullscreen {
            "Exit fullscreen"
        } else {
            "Fullscreen"
        };
        let _ = window;

        v_flex()
            .id("control-bar")
            .w_full()
            .flex_shrink_0()
            .bg(t.surface)
            .border_t_1()
            .border_color(t.border)
            .pb_2()
            .when_some(self.model.error.clone(), |bar, error| {
                bar.child(self.render_error_banner(&error, cx))
            })
            .child(Timeline {
                slider: self.timeline.clone(),
                elapsed: format_time(shown_time).into(),
                total: total.into(),
                enabled: self.model.can_seek(),
            })
            .child(
                h_flex()
                    .w_full()
                    .px(EDGE)
                    .gap(TARGET_GAP)
                    .items_center()
                    .child(TransportControls {
                        playing: self.model.state.is_playing_or_pending(),
                        has_media: self.model.request.is_some() || !self.playlist.is_empty(),
                        has_previous: self.playlist.peek_previous().is_some()
                            || self.model.time_ms > RESTART_THRESHOLD_MS,
                        has_next: self.playlist.peek_next(Advance::User).is_some(),
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_sm()
                                    .text_color(t.foreground_muted)
                                    .child(SharedString::from(status.join(" · "))),
                            ),
                    )
                    .child(VolumeControl {
                        slider: self.volume_slider.clone(),
                        muted: self.model.muted,
                    })
                    .child(panel_button(
                        "panel-tracks",
                        IconName::Captions,
                        "Audio, subtitles & speed",
                        &ShowTracks,
                        Panel::Tracks,
                    ))
                    .child(panel_button(
                        "panel-queue",
                        IconName::ListMusic,
                        "Playlist",
                        &TogglePlaylist,
                        Panel::Queue,
                    ))
                    .child(panel_button(
                        "panel-settings",
                        IconName::Settings,
                        "Settings",
                        &ShowSettings,
                        Panel::Settings,
                    ))
                    .child(action_button(
                        "toggle-fullscreen",
                        fullscreen_icon,
                        fullscreen_label,
                        &ToggleFullscreen,
                    )),
            )
    }

    fn render_side_panel(
        &mut self,
        panel: Panel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens();
        let tab = |id: &'static str, target: Panel, cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .label(target.title())
                .h(HIT_TARGET)
                .px_3()
                .selected(panel == target)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel = Some(target);
                    cx.notify();
                }))
        };
        let body = match panel {
            Panel::Queue => self.render_queue_panel(window, cx).into_any_element(),
            Panel::Tracks => self.render_tracks_panel(cx).into_any_element(),
            Panel::Settings => self.render_settings_panel(cx).into_any_element(),
        };
        v_flex()
            .id("side-panel")
            .w(PANEL_WIDTH)
            .h_full()
            .flex_shrink_0()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.border)
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(t.border)
                    .child(tab("tab-queue", Panel::Queue, cx))
                    .child(tab("tab-tracks", Panel::Tracks, cx))
                    .child(tab("tab-settings", Panel::Settings, cx))
                    .child(div().flex_1())
                    .child(
                        icon_button("panel-close", IconName::X, "Close panel").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.panel = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(v_flex().flex_1().min_h_0().child(body))
    }

    fn render_tracks_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let tracks = &self.model.tracks;
        let has_media = self.model.state.has_media();

        let section = |title: &'static str| {
            div()
                .px(EDGE)
                .pt_4()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.foreground_muted)
                .child(title.to_uppercase())
        };
        let choice = |id: SharedString, label: String, selected: bool| {
            Button::new(id)
                .ghost()
                .w_full()
                .h(HIT_TARGET)
                .justify_start()
                .px(EDGE)
                .selected(selected)
                .accessibility_label(if selected {
                    format!("{label}, selected")
                } else {
                    label.clone()
                })
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .w_5()
                                .when(selected, |d| d.child(Icon::new(IconName::Check))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(label),
                        ),
                )
        };

        let mut content = v_flex().w_full().pb_4();

        content = content.child(section("Audio"));
        let audio: Vec<_> = tracks.real_audio().cloned().collect();
        if audio.is_empty() {
            content = content.child(note(if has_media {
                "No selectable audio tracks"
            } else {
                "Open media to choose an audio track"
            }));
        }
        for track in audio {
            let id = track.id;
            content = content.child(
                choice(
                    format!("audio-{id}").into(),
                    track.name.clone(),
                    tracks.current_audio == Some(id),
                )
                .on_click(cx.listener(move |this, _, _, _| this.select_audio_track(id))),
            );
        }

        content = content.child(section("Subtitles")).child(
            choice(
                "subtitle-off".into(),
                "Off".into(),
                has_media && !tracks.subtitles_enabled(),
            )
            .disabled(!has_media)
            .on_click(cx.listener(|this, _, _, _| this.select_subtitle_track(-1))),
        );
        for track in tracks.real_subtitles().cloned().collect::<Vec<_>>() {
            let id = track.id;
            content = content.child(
                choice(
                    format!("subtitle-{id}").into(),
                    track.name.clone(),
                    tracks.current_subtitle == Some(id),
                )
                .on_click(cx.listener(move |this, _, _, _| this.select_subtitle_track(id))),
            );
        }
        content = content.child(
            h_flex().px(EDGE).pt_2().child(
                Button::new("load-subtitle")
                    .outline()
                    .icon(Icon::new(IconName::Plus))
                    .label("Load subtitle file…")
                    .h(HIT_TARGET)
                    .disabled(!has_media)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.prompt_subtitle_file(window, cx)),
                    ),
            ),
        );

        content = content.child(section("Sync"));
        let delay_row = |id: &'static str,
                         label: &'static str,
                         value: i64,
                         on_change: fn(i64) -> Command,
                         cx: &mut Context<Self>| {
            let step = |delta: i64, icon: IconName, name: &'static str, cx: &mut Context<Self>| {
                icon_button(SharedString::from(format!("{id}-{name}")), icon, name)
                    .disabled(!has_media)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.controller.send(on_change(value + delta));
                    }))
            };
            h_flex()
                .px(EDGE)
                .gap_2()
                .items_center()
                .child(div().flex_1().text_sm().child(label))
                .child(step(-DELAY_STEP_MS, IconName::Minus, "Earlier", cx))
                .child(
                    div()
                        .min_w(px(64.))
                        .text_center()
                        .text_sm()
                        .child(SharedString::from(format_delay(value))),
                )
                .child(step(DELAY_STEP_MS, IconName::Plus, "Later", cx))
        };
        content = content
            .child(delay_row(
                "audio-delay",
                "Audio delay",
                self.model.audio_delay_ms,
                Command::SetAudioDelay,
                cx,
            ))
            .child(delay_row(
                "subtitle-delay",
                "Subtitle delay",
                self.model.subtitle_delay_ms,
                Command::SetSubtitleDelay,
                cx,
            ));

        content = content.child(section("Speed"));
        let mut rates = h_flex().px(EDGE).flex_wrap().gap(TARGET_GAP);
        for rate in crate::playback::state::PLAYBACK_RATES {
            let selected = (self.model.rate - rate).abs() < f32::EPSILON;
            rates = rates.child(
                Button::new(SharedString::from(format!("rate-{rate}")))
                    .ghost()
                    .label(format_rate(rate))
                    .min_w(HIT_TARGET)
                    .h(HIT_TARGET)
                    .selected(selected)
                    .accessibility_label(format!(
                        "Speed {}{}",
                        format_rate(rate),
                        if selected { ", selected" } else { "" }
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_rate(rate, cx))),
            );
        }
        content = content.child(rates);

        div()
            .id("tracks-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(content)
    }

    fn handle_mouse_move(
        &mut self,
        _: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fullscreen {
            self.show_controls(window, cx);
        }
    }
}

/// Muted helper text inside panels.
pub(crate) fn note(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px(EDGE)
        .py_2()
        .text_sm()
        .text_color(tokens().foreground_muted)
        .child(text.into())
}

impl Focusable for PlayerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let show_chrome = self.controls_visible || !self.fullscreen;
        let panel = self.panel.filter(|_| show_chrome);

        v_flex()
            .id("player-root")
            .key_context(PLAYER_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(t.background)
            .text_color(t.foreground)
            .on_action(cx.listener(|this, _: &TogglePlayback, window, cx| {
                this.show_controls(window, cx);
                this.toggle_playback(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Stop, _, cx| this.stop(cx)))
            .on_action(
                cx.listener(|this, _: &OpenFiles, window, cx| this.prompt_open_files(window, cx)),
            )
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &ExitFullscreen, window, cx| {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                } else if this.panel.is_some() {
                    this.panel = None;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, window, cx| {
                this.show_controls(window, cx);
                this.seek_by(false, false);
            }))
            .on_action(cx.listener(|this, _: &SeekForward, window, cx| {
                this.show_controls(window, cx);
                this.seek_by(true, false);
            }))
            .on_action(cx.listener(|this, _: &SeekBackwardLarge, window, cx| {
                this.show_controls(window, cx);
                this.seek_by(false, true);
            }))
            .on_action(cx.listener(|this, _: &SeekForwardLarge, window, cx| {
                this.show_controls(window, cx);
                this.seek_by(true, true);
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, window, cx| {
                let volume = this
                    .model
                    .volume
                    .saturating_add(VOLUME_STEP)
                    .min(MAX_VOLUME);
                this.set_volume(volume, window, cx);
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, window, cx| {
                let volume = this.model.volume.saturating_sub(VOLUME_STEP);
                this.set_volume(volume, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| this.toggle_mute(cx)))
            .on_action(cx.listener(|this, _: &PreviousItem, window, cx| this.previous(window, cx)))
            .on_action(cx.listener(|this, _: &NextItem, window, cx| this.next(window, cx)))
            .on_action(
                cx.listener(|this, _: &TogglePlaylist, _, cx| this.toggle_panel(Panel::Queue, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowTracks, _, cx| this.toggle_panel(Panel::Tracks, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowSettings, _, cx| this.toggle_panel(Panel::Settings, cx)),
            )
            .on_action(cx.listener(|this, _: &SpeedUp, _, cx| {
                let rate = step_rate(this.model.rate, true);
                this.set_rate(rate, cx);
            }))
            .on_action(cx.listener(|this, _: &SpeedDown, _, cx| {
                let rate = step_rate(this.model.rate, false);
                this.set_rate(rate, cx);
            }))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.begin_shutdown(window, cx);
            }))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.open_paths(paths.paths().to_vec(), true, window, cx);
            }))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(tokens().surface_active))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    // Children fill the row's height (h_flex centers by default).
                    .items_stretch()
                    .child(self.render_video_area(window, cx))
                    .when_some(panel, |row, panel| {
                        row.child(self.render_side_panel(panel, window, cx))
                    }),
            )
            .when(show_chrome, |root| {
                root.child(self.render_control_bar(window, cx))
            })
    }
}
