//! The floating controls: a separate borderless window that floats over the bottom of the
//! video, shown on activity and hidden while video plays undisturbed.
//!
//! GPUI cannot paint over the native video surface inside the player window, so the controls
//! are their own GPUI window. `video_output::OverlayHost` ties it to the player window natively
//! (child window on X11 and macOS, owned window on Windows); this module renders it.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div,
    prelude::FluentBuilder as _, px, size,
};
use raw_window_handle::HasWindowHandle as _;

use super::commands::{Panel, RESTART_THRESHOLD_MS, UiCommand};
use super::view::{PlayerView, with_player_actions};
use crate::actions::*;
use crate::components::timeline::Timeline;
use crate::components::transport_controls::TransportControls;
use crate::components::volume_control::VolumeControl;
use crate::components::{CommandHandler, command_button, icon_button};
use crate::features::playlist::model::Advance;
use crate::playback::state::{format_rate, format_time};
use crate::theme::metrics::{
    HIT_TARGET, INSET, PILL_ERROR_HEIGHT, PILL_HEIGHT, PILL_MARGIN, PILL_MAX_WIDTH, PILL_RADIUS,
};
use crate::theme::{overlay_glass, overlay_translucent, tokens};
use crate::video_output::Frame;

/// Where the pill goes inside the video rectangle: bottom-centre, inset by [`PILL_MARGIN`],
/// at most [`PILL_MAX_WIDTH`] wide. `None` when the video area is too small to hold it.
pub fn pill_frame(video: Bounds<Pixels>, scale: f32, has_error: bool) -> Option<Frame> {
    let (vx, vy) = (f32::from(video.origin.x), f32::from(video.origin.y));
    let (vw, vh) = (f32::from(video.size.width), f32::from(video.size.height));
    let height = PILL_HEIGHT + if has_error { PILL_ERROR_HEIGHT } else { 0.0 };
    let margin = PILL_MARGIN.min(vw * 0.04).max(8.0);
    let width = (vw - margin * 2.0).min(PILL_MAX_WIDTH);
    if width < 240.0 || vh < height + margin {
        return None;
    }
    Some(Frame {
        x: vx + (vw - width) / 2.0,
        y: vy + vh - height - margin,
        width,
        height,
        scale,
    })
}

/// Open the controls window for `player` (whose window is `main`) and link the two.
pub fn open_controls(
    main: AnyWindowHandle,
    player: Entity<PlayerView>,
    cx: &mut App,
) -> gpui_kit::Result<AnyWindowHandle> {
    let options = WindowOptions {
        // Placed natively right after opening; this initial size just avoids a flash.
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(PILL_MAX_WIDTH), px(PILL_HEIGHT)),
            cx,
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: if overlay_translucent() {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        },
        app_id: Some("mono-player".into()),
        ..Default::default()
    };
    let mut native = None;
    let player_for_view = player.clone();
    let window = cx.open_window(options, |window, cx| {
        native = window.window_handle().ok().map(|h| h.as_raw());
        let view = cx.new(|cx| ControlsOverlay::new(player_for_view, main, cx));
        // The Root normally paints the theme background; the pill draws its own.
        cx.new(|cx| gpui_kit::base::Root::new(view, window, cx).bg(gpui_kit::transparent_black()))
    })?;
    let window: AnyWindowHandle = window.into();
    player.update(cx, |player, cx| player.attach_overlay(window, native, cx));
    Ok(window)
}

/// The view inside the controls window.
pub struct ControlsOverlay {
    player: Entity<PlayerView>,
    main: AnyWindowHandle,
    on_command: CommandHandler,
    focus_handle: FocusHandle,
    _observe: Subscription,
}

impl ControlsOverlay {
    fn new(player: Entity<PlayerView>, main: AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&player, |_, _, cx| cx.notify());
        let target = player.clone();
        // Commands run in the player window, where the player's state and surfaces live.
        let on_command: CommandHandler = Rc::new(move |command, _, cx| {
            let target = target.clone();
            let _ = main.update(cx, |_, window, cx| {
                target.update(cx, |player, cx| player.perform(command, window, cx))
            });
        });
        Self {
            player,
            main,
            on_command,
            focus_handle: cx.focus_handle(),
            _observe: observe,
        }
    }

    fn run(
        &self,
        command: UiCommand,
    ) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + use<> {
        let on_command = self.on_command.clone();
        move |_, window, cx| on_command(command, window, cx)
    }
}

impl Render for ControlsOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let glass = overlay_glass();
        let player = self.player.read(cx);
        let model = &player.model;
        let width = f32::from(window.viewport_size().width);
        // Drop secondary controls as the pill narrows, keeping hit targets intact.
        let roomy = width >= 640.0;
        let compact = width < 520.0;

        let shown_time = player.seek_preview.unwrap_or(model.time_ms);
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
        if let Some(chapter) = model.navigation.chapter_at(model.time_ms)
            && model.navigation.chapters.len() > 1
        {
            status.push(chapter.name.clone());
        }
        if let Some(subtitle) = model.meta.subtitle() {
            status.push(subtitle);
        }
        let status = player
            .notice
            .clone()
            .unwrap_or_else(|| SharedString::from(status.join("  ·  ")));
        let title: SharedString = if model.title.is_empty() {
            "Nothing playing".into()
        } else {
            model.display_title().to_owned().into()
        };

        let header = match &model.error {
            Some(error) => h_flex()
                .id("error-banner")
                .w_full()
                .min_h(px(44. + 8.))
                .gap_3()
                .items_center()
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
                                .text_xs()
                                .text_color(t.foreground_muted)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(SharedString::from(error.message())),
                        ),
                )
                .child(
                    icon_button("error-dismiss", IconName::X, "Dismiss")
                        .on_click(self.run(UiCommand::DismissError)),
                )
                .test_support()
                .into_any_element(),
            None => h_flex()
                .w_full()
                .h(px(20.))
                .gap_2()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(t.foreground_muted)
                        .child(status),
                )
                .into_any_element(),
        };

        let panel_button = |id: &'static str,
                            icon: IconName,
                            label: &'static str,
                            action: &dyn gpui_kit::Action,
                            panel: Panel| {
            let button = command_button(
                id,
                icon,
                label,
                action,
                UiCommand::TogglePanel(panel),
                &self.on_command,
            );
            if player.panel == Some(panel) {
                button.bg(glass.chip_active)
            } else {
                button
            }
        };
        let (fullscreen_icon, fullscreen_label) = if player.fullscreen {
            (IconName::Minimize, "Exit fullscreen")
        } else {
            (IconName::Maximize, "Fullscreen")
        };

        let controls = h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .child(TransportControls {
                playing: model.state.is_playing_or_pending(),
                has_media: model.request.is_some() || !player.playlist.is_empty(),
                has_previous: player.playlist.peek_previous().is_some()
                    || model.time_ms > RESTART_THRESHOLD_MS,
                has_next: player.playlist.peek_next(Advance::User).is_some(),
                on_command: self.on_command.clone(),
            })
            .child(div().flex_1())
            .when(!compact, |row| {
                row.child(VolumeControl {
                    slider: player.volume_slider.clone(),
                    muted: model.muted,
                    on_command: self.on_command.clone(),
                })
            })
            .when(compact, |row| {
                let (icon, label) = if model.muted {
                    (IconName::VolumeX, "Unmute")
                } else {
                    (IconName::Volume2, "Mute")
                };
                row.child(command_button(
                    "volume-mute",
                    icon,
                    label,
                    &ToggleMute,
                    UiCommand::ToggleMute,
                    &self.on_command,
                ))
            })
            .when(roomy, |row| {
                row.child(
                    Button::new("speed")
                        .ghost()
                        .label(format_rate(model.rate))
                        .rounded(px(14.))
                        .min_w(HIT_TARGET)
                        .h(HIT_TARGET)
                        .accessibility_label(format!("Speed {}", format_rate(model.rate)))
                        .tooltip_with_action("Faster", &SpeedUp, Some(PLAYER_CONTEXT))
                        .on_click(self.run(UiCommand::SpeedUp)),
                )
            })
            .child(panel_button(
                "panel-tracks",
                IconName::Captions,
                "Audio, subtitles & chapters",
                &ShowTracks,
                Panel::Tracks,
            ))
            .when(roomy, |row| {
                row.child(panel_button(
                    "panel-effects",
                    IconName::SlidersHorizontal,
                    "Equalizer & picture",
                    &ShowEffects,
                    Panel::Effects,
                ))
            })
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
            .child(command_button(
                "toggle-fullscreen",
                fullscreen_icon,
                fullscreen_label,
                &ToggleFullscreen,
                UiCommand::ToggleFullscreen,
                &self.on_command,
            ));

        let hover_target = self.player.clone();
        let main = self.main;
        let root = div()
            .id("controls-pill")
            .key_context(PLAYER_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .rounded(px(PILL_RADIUS))
            .bg(glass.pill)
            .border_1()
            .border_color(glass.hairline)
            .text_color(t.foreground)
            .px(INSET)
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .on_hover(move |hovered, _, cx| {
                let (hovered, target) = (*hovered, hover_target.clone());
                let _ = main.update(cx, |_, window, cx| {
                    target.update(cx, |player, cx| {
                        player.set_overlay_hovered(hovered, window, cx)
                    })
                });
            })
            .on_mouse_move({
                let on_command = self.on_command.clone();
                move |_, window, cx| on_command(UiCommand::Activity, window, cx)
            })
            // A click anywhere on the pill gives it keyboard focus, so shortcuts keep working
            // after using the controls.
            .capture_any_mouse_down({
                let focus = self.focus_handle.clone();
                move |_, window, cx| {
                    if !focus.contains_focused(window, cx) {
                        focus.focus(window, cx);
                    }
                }
            });
        // Keys pressed while the pill has focus act on the player like in its own window.
        with_player_actions(root, &self.on_command)
            .child(header)
            .child(Timeline {
                slider: player.timeline.clone(),
                elapsed: format_time(shown_time).into(),
                total: total.into(),
                enabled: model.can_seek(),
            })
            .child(controls)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{point, px, size};

    use super::*;

    fn video(w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(w), px(h)))
    }

    #[test]
    fn pill_sits_bottom_centre() {
        let frame = pill_frame(video(1280., 720.), 2.0, false).unwrap();
        assert_eq!(frame.width, PILL_MAX_WIDTH);
        assert_eq!(frame.x, (1280. - PILL_MAX_WIDTH) / 2.);
        assert_eq!(frame.y + frame.height, 720. - PILL_MARGIN);
        assert_eq!(frame.scale, 2.0);
    }

    #[test]
    fn pill_narrows_and_grows_for_errors() {
        let narrow = pill_frame(video(600., 400.), 1.0, false).unwrap();
        assert!(narrow.width < 600. && narrow.width > 500.);
        let error = pill_frame(video(600., 400.), 1.0, true).unwrap();
        assert_eq!(error.height, PILL_HEIGHT + PILL_ERROR_HEIGHT);
        assert!(pill_frame(video(200., 400.), 1.0, false).is_none());
        assert!(pill_frame(video(800., 100.), 1.0, false).is_none());
    }
}
