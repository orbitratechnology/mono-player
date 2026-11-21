//! The root player view: the video, a floating side panel, and the link to the floating
//! controls window.
//!
//! State changes flow one way: user input → [`UiCommand`] → [`PlaybackController`] command →
//! actor → event → [`PlayerModel::apply`] → render. Nothing here calls LibVLC, polls the engine
//! or touches the filesystem on the UI thread (settings are written on the background executor).
//!
//! The transport controls are not drawn in this window: GPUI cannot paint over the native video
//! surface, so they live in their own window (see [`super::overlay`]) that this view positions
//! over the bottom of the video and shows or hides with activity.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, ExternalPaths, FocusHandle,
    Focusable, FontWeight, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement as _,
    PathPromptOptions, Pixels, Render, SharedString, Size, Styled as _, Subscription, Task, Window,
    WindowBackgroundAppearance, canvas, div, img, prelude::FluentBuilder as _, px, size,
};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

use super::commands::{RESTART_THRESHOLD_MS, UiCommand, classify_paths, seek_offset_ms};
use super::controller::PlaybackController;
use super::model::{Outcome, PlayerModel, RuntimeStatus};
use super::overlay::pill_frame;
use crate::actions::*;
use crate::components::CommandHandler;
use crate::components::icon_button;
use crate::components::surface::{Segment, segmented_fill};
use crate::diagnostics as log;
use crate::features::playlist::model::{Advance, Playlist, QueueItemId};
use crate::persistence::settings::{self, Settings, WindowGeometry};
use crate::playback::features::{EQ_MAX_DB, EQ_MIN_DB, StereoMode, VideoAdjust};
use crate::playback::state::{MAX_VOLUME, PlaybackState, VOLUME_STEP, clamp_volume, step_rate};
use crate::playback::{Command, Event, MediaSource, PlaybackError, RequestId};
use crate::theme::metrics::{
    EDGE, HIT_TARGET, PANEL_MARGIN, PANEL_RADIUS, PANEL_WIDTH, PILL_RADIUS, ROW_HEIGHT,
};
use crate::theme::{Glass, tokens, window_glass};
use crate::video_output::{OverlayHost, VideoSurface};

pub use super::commands::Panel;

/// Controls hide after this much inactivity while video plays.
const CONTROLS_HIDE_DELAY: Duration = Duration::from_millis(2_500);
/// Settings writes are debounced so dragging a slider does not write the file repeatedly.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);
/// Upper bound on waiting for LibVLC teardown when quitting.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// How long transient notices ("Snapshot saved") stay in the controls.
const NOTICE_DURATION: Duration = Duration::from_secs(3);
/// Timeline slider resolution (slider value is a fraction of the length).
const TIMELINE_MAX: f32 = 1.0;

/// The queue entry currently loaded in the engine.
#[derive(Clone, Copy, Debug)]
struct PlayingItem {
    request: RequestId,
    item: QueueItemId,
    /// Started by auto-advance (so a failure may skip to the next entry).
    automatic: bool,
}

/// Session picture settings (indices into the option tables in `playback::features`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PictureState {
    pub adjust: VideoAdjust,
    pub aspect: usize,
    pub crop: usize,
    pub zoom: usize,
    pub deinterlace: usize,
}

/// The floating controls window and its native placement.
struct OverlayLink {
    window: AnyWindowHandle,
    host: Option<OverlayHost>,
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
    pub(crate) timeline: Entity<SliderState>,
    pub(crate) volume_slider: Entity<SliderState>,
    /// Preamp first, then one slider per equalizer band.
    pub(crate) eq_sliders: Vec<Entity<SliderState>>,
    /// Brightness, contrast, saturation, hue, gamma.
    pub(crate) adjust_sliders: [Entity<SliderState>; 5],
    pub(crate) picture: PictureState,
    pub(crate) stereo_mode: StereoMode,
    /// Local preview while the user drags the timeline; progress events don't fight the drag.
    pub(crate) seek_preview: Option<i64>,
    pub(crate) url_input: Entity<InputState>,
    pub(crate) panel: Option<Panel>,
    video: Option<VideoSurface>,
    pub(crate) video_error: Option<String>,
    pub(crate) fullscreen: bool,
    controls_visible: bool,
    overlay_hovered: bool,
    hide_controls_task: Option<Task<()>>,
    last_activity: Option<Instant>,
    overlay: Option<OverlayLink>,
    native_handle: Option<RawWindowHandle>,
    pub(crate) notice: Option<SharedString>,
    notice_task: Option<Task<()>>,
    pub(crate) glass: Glass,
    shutting_down: bool,
    pub(crate) playlist_row_sizes: Rc<Vec<Size<Pixels>>>,
    _event_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

fn slider(cx: &mut App, min: f32, max: f32, step: f32, value: f32) -> Entity<SliderState> {
    cx.new(|_| {
        SliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(value)
    })
}

impl PlayerView {
    pub fn new(
        settings: Settings,
        settings_path: Option<PathBuf>,
        initial_paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let backend = PlaybackController::new(settings.volume, settings.muted);
        Self::with_controller(backend, settings, settings_path, initial_paths, window, cx)
    }

    /// Build the view around a given controller (e.g. [`PlaybackController::recording`]).
    pub fn with_controller(
        (controller, mut events): (
            PlaybackController,
            futures::channel::mpsc::UnboundedReceiver<Event>,
        ),
        settings: Settings,
        settings_path: Option<PathBuf>,
        initial_paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let timeline = slider(cx, 0., TIMELINE_MAX, 0.0001, 0.);
        let volume_slider = slider(
            cx,
            0.,
            f32::from(MAX_VOLUME),
            1.,
            f32::from(settings.volume),
        );
        let eq = &settings.equalizer;
        let eq_sliders: Vec<_> = std::iter::once(eq.preamp)
            .chain(eq.bands.iter().copied())
            .map(|value| slider(cx, EQ_MIN_DB, EQ_MAX_DB, 0.5, value))
            .collect();
        let neutral = VideoAdjust::default();
        let adjust_sliders = [
            slider(cx, 0., 2., 0.01, neutral.brightness),
            slider(cx, 0., 2., 0.01, neutral.contrast),
            slider(cx, 0., 3., 0.01, neutral.saturation),
            slider(cx, -180., 180., 1., neutral.hue),
            slider(cx, 0.01, 4., 0.01, neutral.gamma),
        ];
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
        for (index, eq_slider) in eq_sliders.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                eq_slider,
                window,
                move |this, _, event: &SliderEvent, _, cx| {
                    let (SliderEvent::Change(SliderValue::Single(value))
                    | SliderEvent::Release(SliderValue::Single(value))) = event
                    else {
                        return;
                    };
                    this.set_equalizer_value(index, *value, cx);
                },
            ));
        }
        for (index, adjust_slider) in adjust_sliders.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                adjust_slider,
                window,
                move |this, _, event: &SliderEvent, _, cx| {
                    let (SliderEvent::Change(SliderValue::Single(value))
                    | SliderEvent::Release(SliderValue::Single(value))) = event
                    else {
                        return;
                    };
                    this.set_adjust_value(index, *value, cx);
                },
            ));
        }
        subscriptions.push(cx.on_app_quit(|this, _| {
            // Covers quits that don't go through `begin_shutdown` (e.g. the OS).
            log::info("application quit");
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
        let native_handle = window.window_handle().ok().map(|h| h.as_raw());

        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            log::info("player window asked to close");
            // Close only after LibVLC has let go of the video surface.
            entity
                .update(cx, |view, cx| view.begin_shutdown(window, cx))
                .is_err()
        });

        let glass = window_glass(settings.translucent());
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
            eq_sliders,
            adjust_sliders,
            picture: PictureState::default(),
            stereo_mode: StereoMode::Stereo,
            seek_preview: None,
            url_input,
            panel: None,
            video,
            video_error,
            fullscreen: window.is_fullscreen(),
            controls_visible: true,
            overlay_hovered: false,
            hide_controls_task: None,
            last_activity: None,
            overlay: None,
            native_handle,
            notice: None,
            notice_task: None,
            glass,
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
        view.controller
            .send(Command::SetPlaybackOptions(view.settings.playback));
        if view.settings.equalizer.enabled {
            view.controller
                .send(Command::SetEqualizer(Some(view.settings.equalizer.clone())));
        }
        if view.settings.audio_device.is_some() {
            view.controller
                .send(Command::SetAudioDevice(view.settings.audio_device.clone()));
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

    /// The playback controller (tests inspect recorded commands through it).
    pub fn controller(&self) -> &PlaybackController {
        &self.controller
    }

    /// The address field's state (for focus management and tests).
    pub fn address_input(&self) -> &Entity<InputState> {
        &self.url_input
    }

    /// Whether the floating controls are currently shown.
    pub fn controls_visible(&self) -> bool {
        self.controls_visible
    }

    /// The native handle of the player window (used to attach the controls window).
    pub fn native_handle(&self) -> Option<RawWindowHandle> {
        self.native_handle
    }

    /// A handler that runs commands from inside this (the player) window.
    pub(crate) fn local_handler(cx: &Context<Self>) -> CommandHandler {
        let view = cx.entity().downgrade();
        Rc::new(move |command, window, cx| {
            let _ = view.update(cx, |player, cx| player.perform(command, window, cx));
        })
    }

    // ----- The floating controls window ----------------------------------------------------

    /// Link the floating controls window; `overlay_native` is its native handle.
    pub fn attach_overlay(
        &mut self,
        window: AnyWindowHandle,
        overlay_native: Option<RawWindowHandle>,
        cx: &mut Context<Self>,
    ) {
        let host = match (self.native_handle, overlay_native) {
            (Some(main), Some(overlay)) => match OverlayHost::attach(main, overlay) {
                Ok(host) => Some(host),
                Err(error) => {
                    log::warn(&format!("floating controls not attached: {error}"));
                    None
                }
            },
            _ => None,
        };
        self.overlay = Some(OverlayLink { window, host });
        cx.notify();
    }

    pub(crate) fn set_overlay_hovered(
        &mut self,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_hovered != hovered {
            self.overlay_hovered = hovered;
            self.hide_controls_task = None;
            self.update_auto_hide(window, cx);
        }
    }

    /// Whether there is anything for the controls to control.
    fn has_content(&self) -> bool {
        self.model.request.is_some() || !self.playlist.is_empty()
    }

    fn sync_overlay(&mut self) {
        let visible = self.controls_visible && self.has_content() && !self.shutting_down;
        if let Some(host) = self.overlay.as_mut().and_then(|o| o.host.as_mut()) {
            host.set_visible(visible);
        }
    }

    fn place_overlay(&mut self, video: Bounds<Pixels>, scale: f32) {
        let has_error = self.model.error.is_some();
        if let Some(host) = self.overlay.as_mut().and_then(|o| o.host.as_mut())
            && let Some(frame) = pill_frame(video, scale, has_error)
        {
            host.set_frame(frame, PILL_RADIUS);
        }
        self.sync_overlay();
    }

    // ----- Events from the playback actor -------------------------------------------------

    fn apply_events(&mut self, batch: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        for event in batch {
            let snapshot = matches!(&event, Event::SnapshotSaved { .. });
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
            if snapshot && let Some(path) = &self.model.last_snapshot {
                let name = crate::playback::error::display_name(path);
                self.show_notice(format!("Snapshot saved: {name}"), window, cx);
            }
        }
        if changed {
            self.sync_sliders(window, cx);
            self.sync_video_visibility();
            self.update_window_title(window);
            self.update_auto_hide(window, cx);
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

    fn show_notice(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.show_controls(window, cx);
        self.notice_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(NOTICE_DURATION).await;
            let _ = this.update(cx, |view, cx| {
                view.notice = None;
                cx.notify();
            });
        }));
        cx.notify();
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
        self.show_controls(window, cx);
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

    /// The single entry point for user intent, from key bindings in either window and from the
    /// floating controls.
    pub fn perform(&mut self, command: UiCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.show_controls(window, cx);
        match command {
            UiCommand::TogglePlayback => self.toggle_playback(window, cx),
            UiCommand::Stop => self.stop(cx),
            UiCommand::Previous => self.previous(window, cx),
            UiCommand::Next => self.next(window, cx),
            UiCommand::Seek { forward, large } => self.seek_by(forward, large),
            UiCommand::VolumeUp => {
                let volume = self
                    .model
                    .volume
                    .saturating_add(VOLUME_STEP)
                    .min(MAX_VOLUME);
                self.set_volume(volume, window, cx);
            }
            UiCommand::VolumeDown => {
                let volume = self.model.volume.saturating_sub(VOLUME_STEP);
                self.set_volume(volume, window, cx);
            }
            UiCommand::ToggleMute => self.toggle_mute(cx),
            UiCommand::SpeedUp => self.set_rate(step_rate(self.model.rate, true), cx),
            UiCommand::SpeedDown => self.set_rate(step_rate(self.model.rate, false), cx),
            UiCommand::TogglePanel(panel) => self.toggle_panel(panel, cx),
            UiCommand::ClosePanel => {
                self.panel = None;
                cx.notify();
            }
            UiCommand::ToggleFullscreen => window.toggle_fullscreen(),
            UiCommand::ExitFullscreen => {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                } else if self.panel.is_some() {
                    self.panel = None;
                    cx.notify();
                }
            }
            UiCommand::OpenFiles => self.prompt_open_files(window, cx),
            UiCommand::NextChapter => self.controller.send(Command::StepChapter(1)),
            UiCommand::PreviousChapter => self.controller.send(Command::StepChapter(-1)),
            UiCommand::NextFrame => self.controller.send(Command::NextFrame),
            UiCommand::Snapshot => match settings::snapshot_dir() {
                Some(dir) => self.controller.send(Command::TakeSnapshot(dir)),
                None => self.model.error = Some(PlaybackError::SnapshotFailed),
            },
            UiCommand::DismissError => self.dismiss_error(cx),
            UiCommand::Quit => {
                log::info("quit command");
                self.begin_shutdown(window, cx);
            }
            UiCommand::Activity => {}
        }
    }

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

    pub(crate) fn send(&mut self, command: Command) {
        self.controller.send(command);
    }

    pub(crate) fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
        if self.panel == Some(Panel::Settings) {
            self.controller.send(Command::RefreshAudioDevices);
        }
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

    /// Switch window translucency and the panel surfaces with it.
    pub(crate) fn set_translucent(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.translucent_window = Some(on);
        self.glass = window_glass(on);
        window.set_background_appearance(window_background(on));
        self.settings_changed(cx);
    }

    // ----- Effects -------------------------------------------------------------------------

    fn set_equalizer_value(&mut self, index: usize, value: f32, cx: &mut Context<Self>) {
        let eq = &mut self.settings.equalizer;
        let slot = if index == 0 {
            &mut eq.preamp
        } else {
            match eq.bands.get_mut(index - 1) {
                Some(band) => band,
                None => return,
            }
        };
        if (*slot - value).abs() < f32::EPSILON {
            return;
        }
        *slot = value;
        eq.preset = None;
        eq.enabled = true;
        self.controller
            .send(Command::SetEqualizer(Some(self.settings.equalizer.clone())));
        self.settings_changed(cx);
    }

    /// Apply a LibVLC preset (or with `None`, flatten) and move the sliders to match.
    pub(crate) fn apply_equalizer_preset(
        &mut self,
        preset: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let info = &self.model.equalizer_info;
        match preset.and_then(|index| info.presets.get(index).map(|p| (index, p.clone()))) {
            Some((index, preset)) => self.settings.equalizer.apply_preset(index, &preset),
            None => {
                let enabled = self.settings.equalizer.enabled;
                self.settings.equalizer = Default::default();
                self.settings.equalizer.enabled = enabled;
            }
        }
        let values: Vec<f32> = std::iter::once(self.settings.equalizer.preamp)
            .chain(self.settings.equalizer.bands.iter().copied())
            .collect();
        for (slider, value) in self.eq_sliders.iter().zip(values) {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        self.controller
            .send(Command::SetEqualizer(Some(self.settings.equalizer.clone())));
        self.settings_changed(cx);
    }

    pub(crate) fn set_equalizer_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings.equalizer.enabled = on;
        self.controller
            .send(Command::SetEqualizer(Some(self.settings.equalizer.clone())));
        self.settings_changed(cx);
    }

    fn set_adjust_value(&mut self, index: usize, value: f32, cx: &mut Context<Self>) {
        let adjust = &mut self.picture.adjust;
        let slot = match index {
            0 => &mut adjust.brightness,
            1 => &mut adjust.contrast,
            2 => &mut adjust.saturation,
            3 => &mut adjust.hue,
            _ => &mut adjust.gamma,
        };
        if (*slot - value).abs() < f32::EPSILON {
            return;
        }
        *slot = value;
        self.controller
            .send(Command::SetVideoAdjust(self.picture.adjust));
        cx.notify();
    }

    pub(crate) fn reset_adjust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let neutral = VideoAdjust::default();
        self.picture.adjust = neutral;
        let values = [
            neutral.brightness,
            neutral.contrast,
            neutral.saturation,
            neutral.hue,
            neutral.gamma,
        ];
        for (slider, value) in self.adjust_sliders.iter().zip(values) {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        self.controller.send(Command::SetVideoAdjust(neutral));
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(length) = self.model.length_ms else {
            return;
        };
        self.show_controls(window, cx);
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
        self.sync_overlay();
    }

    /// Called from the video area's prepaint with its laid-out bounds.
    fn place_video(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        let show = self.should_show_video();
        if let Some(surface) = &mut self.video {
            surface.set_bounds(bounds, scale);
            surface.set_visible(show);
        }
        self.place_overlay(bounds, scale);
    }

    // ----- Controls visibility -------------------------------------------------------------

    fn on_window_bounds_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fullscreen = window.is_fullscreen();
        if fullscreen != self.fullscreen {
            self.fullscreen = fullscreen;
            self.show_controls(window, cx);
            cx.notify();
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

    /// Controls hide by themselves only while video plays (and the pointer is not on them).
    fn wants_auto_hide(&self) -> bool {
        self.model.has_video && self.model.state == PlaybackState::Playing && !self.overlay_hovered
    }

    /// Reveal the controls (user activity) and restart the auto-hide countdown. Throttled so
    /// pointer movement does not restart timers on every event.
    fn show_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let recently = self
            .last_activity
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(250));
        if self.controls_visible && recently && self.hide_controls_task.is_some() {
            return;
        }
        self.last_activity = Some(now);
        if !self.controls_visible {
            self.controls_visible = true;
            self.sync_overlay();
            cx.notify();
        }
        self.hide_controls_task = None;
        self.update_auto_hide(window, cx);
    }

    /// Keep the auto-hide countdown in step with the playback state: running while video plays
    /// untouched, cancelled (with the controls shown) otherwise. No timer runs when idle.
    fn update_auto_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wants_auto_hide() {
            self.hide_controls_task = None;
            if !self.controls_visible {
                self.controls_visible = true;
                self.sync_overlay();
                cx.notify();
            }
            return;
        }
        if !self.controls_visible || self.hide_controls_task.is_some() {
            return;
        }
        self.hide_controls_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(CONTROLS_HIDE_DELAY).await;
            let _ = this.update(cx, |view, cx| {
                view.hide_controls_task = None;
                if view.wants_auto_hide() {
                    view.controls_visible = false;
                    view.sync_overlay();
                    cx.notify();
                }
            });
        }));
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

    /// Ordered shutdown: hide the surfaces, close the controls window, stop the actor (which
    /// releases LibVLC), save settings, then quit. Returns `false` so the window stays until
    /// LibVLC has let go of it.
    fn begin_shutdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.shutting_down {
            return false;
        }
        log::info("shutdown requested (window close or quit command)");
        self.shutting_down = true;
        self.remember_position();
        self.sync_video_visibility();
        if let Some(link) = self.overlay.take() {
            // `link` (and with it the native host, which detaches the macOS child panel) drops
            // at the end of this block, before the deferred close runs.
            let overlay = link.window;
            window.defer(cx, move |_, cx| {
                let _ = overlay.update(cx, |_, window, _| window.remove_window());
            });
        }
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
        let title = self.model.display_title();
        let title = if title.is_empty() {
            "Mono Player".to_owned()
        } else {
            format!("{title} — Mono Player")
        };
        window.set_window_title(&title);
    }

    // ----- Rendering -----------------------------------------------------------------------

    fn render_video_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let view = cx.entity().downgrade();
        let surface_shown = self.video.as_ref().is_some_and(VideoSurface::is_visible)
            || self.should_show_video() && self.video.is_some();
        let state = self.model.state;
        // Black behind video; the translucent window shows through when nothing plays.
        let opaque = self.model.has_video || state.has_media();

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
            Some(self.render_now_playing().into_any_element())
        };

        div()
            .id("video-area")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(opaque, |d| d.bg(t.background))
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
                        // Keep clear of the floating controls.
                        .pb(px(140.))
                        .child(content),
                )
            })
    }

    /// Audio (or video between frames): artwork or a soft icon, title, artist — album.
    fn render_now_playing(&self) -> impl IntoElement {
        let t = tokens();
        let meta = &self.model.meta;
        let art = meta.artwork.clone().filter(|path| path.is_file());
        let icon = if self.model.has_video {
            IconName::Film
        } else {
            IconName::Music
        };
        v_flex()
            .items_center()
            .gap_4()
            .max_w(px(560.))
            .px_6()
            .child(match art {
                Some(path) => img(path).size(px(220.)).rounded(px(18.)).into_any_element(),
                None => div()
                    .size(px(120.))
                    .rounded_full()
                    .bg(self.glass.chip)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).size(px(44.)).text_color(t.foreground_muted))
                    .into_any_element(),
            })
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_center()
                    .child(SharedString::from(self.model.display_title().to_owned())),
            )
            .when_some(meta.subtitle(), |col, subtitle| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(t.foreground_muted)
                        .text_center()
                        .child(subtitle),
                )
            })
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        v_flex()
            .items_center()
            .gap_5()
            .child(
                div()
                    .size(px(112.))
                    .rounded_full()
                    .bg(self.glass.chip)
                    .border_1()
                    .border_color(self.glass.hairline)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::AudioLines)
                            .size(px(44.))
                            .text_color(t.foreground_muted),
                    ),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Drop media to play"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.foreground_muted)
                            .child("Video, music, subtitles and network streams"),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("empty-open")
                            .primary()
                            .icon(Icon::new(IconName::FolderOpen))
                            .label("Open file")
                            .rounded_full()
                            .h(HIT_TARGET)
                            .px_5()
                            .tooltip_with_action("Open file", &OpenFiles, Some(PLAYER_CONTEXT))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_open_files(window, cx)
                            })),
                    )
                    .child(
                        Button::new("empty-address")
                            .ghost()
                            .icon(Icon::new(IconName::Link))
                            .label("Open address")
                            .rounded_full()
                            .h(HIT_TARGET)
                            .px_5()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.panel = Some(Panel::Queue);
                                this.url_input
                                    .update(cx, |input, cx| input.focus(window, cx));
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_side_panel(
        &mut self,
        panel: Panel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let glass = self.glass;
        let tabs = segmented_fill(
            "panel-tab",
            &glass,
            Panel::ALL.into_iter().map(|target| {
                let view = cx.entity().downgrade();
                Segment::new(target.title(), panel == target, move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.panel = Some(target);
                        if target == Panel::Settings {
                            this.controller.send(Command::RefreshAudioDevices);
                        }
                        cx.notify();
                    });
                })
            }),
        );
        let body = match panel {
            Panel::Queue => self.render_queue_panel(window, cx).into_any_element(),
            Panel::Tracks => self.render_tracks_panel(cx).into_any_element(),
            Panel::Effects => self.render_effects_panel(cx).into_any_element(),
            Panel::Settings => self.render_settings_panel(cx).into_any_element(),
        };
        v_flex()
            .id("side-panel")
            .w(PANEL_WIDTH)
            .h_full()
            .flex_shrink_0()
            .rounded(PANEL_RADIUS)
            .bg(glass.panel)
            .border_1()
            .border_color(glass.hairline)
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(tabs))
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

    fn handle_mouse_move(
        &mut self,
        _: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Movement reported by the player window means the pointer is not on the controls
        // (their window takes the events while hovered). GPUI only clears the controls' hover
        // state on their next move, so clear it here when the pointer has left them.
        self.overlay_hovered = false;
        self.show_controls(window, cx);
    }
}

/// The window background for a translucency choice.
pub fn window_background(translucent: bool) -> WindowBackgroundAppearance {
    match (
        translucent,
        cfg!(any(target_os = "linux", target_os = "freebsd")),
    ) {
        (false, _) => WindowBackgroundAppearance::Opaque,
        // Linux compositors rarely blur X11 windows; plain alpha keeps it predictable.
        (true, true) => WindowBackgroundAppearance::Transparent,
        (true, false) => WindowBackgroundAppearance::Blurred,
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

/// Route every player action to a [`UiCommand`].
macro_rules! bind_commands {
    ($element:expr, $handler:expr, { $($action:ty => $command:expr),* $(,)? }) => {{
        let element = $element;
        $(
            let handler = $handler.clone();
            let element = element.on_action(move |_: &$action, window, cx| {
                handler($command, window, cx)
            });
        )*
        element
    }};
}

/// Attach the shared action → command bindings to an element.
pub(crate) fn with_player_actions<E: InteractiveElement>(
    element: E,
    handler: &CommandHandler,
) -> E {
    bind_commands!(element, handler, {
        TogglePlayback => UiCommand::TogglePlayback,
        Stop => UiCommand::Stop,
        OpenFiles => UiCommand::OpenFiles,
        ToggleFullscreen => UiCommand::ToggleFullscreen,
        ExitFullscreen => UiCommand::ExitFullscreen,
        SeekBackward => UiCommand::Seek { forward: false, large: false },
        SeekForward => UiCommand::Seek { forward: true, large: false },
        SeekBackwardLarge => UiCommand::Seek { forward: false, large: true },
        SeekForwardLarge => UiCommand::Seek { forward: true, large: true },
        VolumeUp => UiCommand::VolumeUp,
        VolumeDown => UiCommand::VolumeDown,
        ToggleMute => UiCommand::ToggleMute,
        PreviousItem => UiCommand::Previous,
        NextItem => UiCommand::Next,
        TogglePlaylist => UiCommand::TogglePanel(Panel::Queue),
        ShowTracks => UiCommand::TogglePanel(Panel::Tracks),
        ShowEffects => UiCommand::TogglePanel(Panel::Effects),
        ShowSettings => UiCommand::TogglePanel(Panel::Settings),
        SpeedUp => UiCommand::SpeedUp,
        SpeedDown => UiCommand::SpeedDown,
        TakeSnapshot => UiCommand::Snapshot,
        NextFrame => UiCommand::NextFrame,
        NextChapter => UiCommand::NextChapter,
        PreviousChapter => UiCommand::PreviousChapter,
        Quit => UiCommand::Quit,
    })
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let show_panel = self.controls_visible || !self.fullscreen;
        let panel = self.panel.filter(|_| show_panel);
        let handler = Self::local_handler(cx);
        let root_background = if self.settings.translucent() {
            self.glass.window
        } else {
            t.background
        };

        let root = div()
            .id("player-root")
            .key_context(PLAYER_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(root_background)
            .text_color(t.foreground);
        with_player_actions(root, &handler)
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.open_paths(paths.paths().to_vec(), true, window, cx);
            }))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(tokens().surface_active))
            .child(
                h_flex()
                    .size_full()
                    // Children fill the row's height (h_flex centers by default).
                    .items_stretch()
                    .child(self.render_video_area(cx))
                    .when_some(panel, |row, panel| {
                        row.child(
                            div()
                                .p(PANEL_MARGIN)
                                .flex_shrink_0()
                                .child(self.render_side_panel(panel, window, cx)),
                        )
                    }),
            )
    }
}
