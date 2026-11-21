//! Previous / play-pause / next / stop.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{Disableable as _, Icon, h_flex};
use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window};

use super::{action_button, sized_icon_button};
use crate::actions::{NextItem, PLAYER_CONTEXT, PreviousItem, Stop, TogglePlayback};
use crate::theme::metrics::{PRIMARY_TARGET, TARGET_GAP};

#[derive(IntoElement)]
pub struct TransportControls {
    /// Show "pause" instead of "play".
    pub playing: bool,
    pub has_media: bool,
    pub has_previous: bool,
    pub has_next: bool,
}

impl RenderOnce for TransportControls {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (icon, label) = if self.playing {
            (IconName::Pause, "Pause")
        } else {
            (IconName::Play, "Play")
        };
        let play_pause = sized_icon_button("transport-play-pause", icon, label, PRIMARY_TARGET)
            .primary()
            .icon(Icon::new(icon))
            .rounded(PRIMARY_TARGET / 2.)
            .tooltip_with_action(label, &TogglePlayback, Some(PLAYER_CONTEXT))
            .disabled(!self.has_media)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(TogglePlayback), cx));

        h_flex()
            .gap(TARGET_GAP)
            .items_center()
            .flex_shrink_0()
            .child(
                action_button(
                    "transport-previous",
                    IconName::SkipBack,
                    "Previous",
                    &PreviousItem,
                )
                .disabled(!self.has_previous),
            )
            .child(play_pause)
            .child(
                action_button("transport-next", IconName::SkipForward, "Next", &NextItem)
                    .disabled(!self.has_next),
            )
            .child(
                action_button("transport-stop", IconName::Square, "Stop", &Stop)
                    .disabled(!self.has_media),
            )
    }
}
