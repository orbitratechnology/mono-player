//! Previous / play-pause / next.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{Disableable as _, Icon, h_flex};
use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window};

use super::{CommandHandler, command_button, sized_icon_button};
use crate::actions::{NextItem, PLAYER_CONTEXT, PreviousItem, TogglePlayback};
use crate::features::player::commands::UiCommand;
use crate::theme::metrics::{PRIMARY_TARGET, TARGET_GAP};

#[derive(IntoElement)]
pub struct TransportControls {
    /// Show "pause" instead of "play".
    pub playing: bool,
    pub has_media: bool,
    pub has_previous: bool,
    pub has_next: bool,
    pub on_command: CommandHandler,
}

impl RenderOnce for TransportControls {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (icon, label) = if self.playing {
            (IconName::Pause, "Pause")
        } else {
            (IconName::Play, "Play")
        };
        let on_command = self.on_command.clone();
        let play_pause = sized_icon_button("transport-play-pause", icon, label, PRIMARY_TARGET)
            .primary()
            .icon(Icon::new(icon))
            .rounded_full()
            .tooltip_with_action(label, &TogglePlayback, Some(PLAYER_CONTEXT))
            .disabled(!self.has_media)
            .on_click(move |_, window, cx| on_command(UiCommand::TogglePlayback, window, cx));

        h_flex()
            .gap(TARGET_GAP)
            .items_center()
            .flex_shrink_0()
            .child(
                command_button(
                    "transport-previous",
                    IconName::SkipBack,
                    "Previous",
                    &PreviousItem,
                    UiCommand::Previous,
                    &self.on_command,
                )
                .disabled(!self.has_previous),
            )
            .child(play_pause)
            .child(
                command_button(
                    "transport-next",
                    IconName::SkipForward,
                    "Next",
                    &NextItem,
                    UiCommand::Next,
                    &self.on_command,
                )
                .disabled(!self.has_next),
            )
    }
}
