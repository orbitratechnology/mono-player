//! Reusable monochrome controls built from GPUI Kit components.
//!
//! Every control keeps at least a 44×44 logical-pixel hit area (48×48 for play/pause), has an
//! accessible label, and uses a tooltip only as supplementary guidance. Controls invoke a
//! [`CommandHandler`] rather than dispatching window actions, so they behave the same in the
//! player window and in the floating controls window.

pub mod media_row;
pub mod surface;
pub mod timeline;
pub mod transport_controls;
pub mod volume_control;

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::{Action, App, ElementId, Pixels, Styled as _, Window, px};

use crate::actions::PLAYER_CONTEXT;
use crate::features::player::commands::UiCommand;
use crate::theme::metrics::HIT_TARGET;

/// Runs a [`UiCommand`] against the player, from whichever window the control lives in.
pub type CommandHandler = Rc<dyn Fn(UiCommand, &mut Window, &mut App)>;

// Icons the app uses beyond GPUI Kit's default component set. Only these SVGs are embedded.
gpui_kit::assets::icon_assets!(
    pub AppIcons,
    [
        AudioLines,
        AudioWaveform,
        Camera,
        Captions,
        ChevronsLeft,
        ChevronsRight,
        Film,
        Gauge,
        Headphones,
        Keyboard,
        Link,
        ListMusic,
        ListVideo,
        Music,
        Repeat,
        Repeat1,
        Shuffle,
        SkipBack,
        SkipForward,
        SlidersHorizontal,
        Sparkles,
        StepForward,
        Volume2,
        VolumeX,
        X,
    ]
);

/// A ghost icon button with a full-size hit area, accessible name and tooltip.
pub fn icon_button(id: impl Into<ElementId>, icon: IconName, label: &'static str) -> Button {
    sized_icon_button(id, icon, label, HIT_TARGET)
}

/// Like [`icon_button`] with an explicit square size (never below [`HIT_TARGET`]).
pub fn sized_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: &'static str,
    size: Pixels,
) -> Button {
    let size = size.max(HIT_TARGET);
    Button::new(id)
        .ghost()
        .icon(Icon::new(icon))
        .accessibility_label(label)
        .tooltip(label)
        .rounded(px(14.))
        .w(size)
        .h(size)
}

/// An icon button that runs `command`; its tooltip shows the shortcut bound to `action`.
pub fn command_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: &'static str,
    action: &dyn Action,
    command: UiCommand,
    on_command: &CommandHandler,
) -> Button {
    let on_command = on_command.clone();
    icon_button(id, icon, label)
        .tooltip_with_action(label, action, Some(PLAYER_CONTEXT))
        .on_click(move |_, window, cx| on_command(command, window, cx))
}
