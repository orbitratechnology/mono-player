//! Reusable monochrome controls built from GPUI Kit components.
//!
//! Every control here keeps at least a 44×44 logical-pixel hit area (48×48 for play/pause),
//! has an accessible label, and uses a tooltip only as supplementary guidance.

pub mod media_row;
pub mod timeline;
pub mod transport_controls;
pub mod volume_control;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::{Action, ElementId, Pixels, Styled as _};

use crate::actions::PLAYER_CONTEXT;
use crate::theme::metrics::HIT_TARGET;

// Icons the app uses beyond GPUI Kit's default component set. Only these SVGs are embedded.
gpui_kit::assets::icon_assets!(
    pub AppIcons,
    [
        AudioLines,
        Captions,
        Film,
        Gauge,
        Keyboard,
        Link,
        ListMusic,
        Music,
        Repeat,
        Repeat1,
        Shuffle,
        SkipBack,
        SkipForward,
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
        .w(size)
        .h(size)
}

/// An icon button that dispatches `action`; its tooltip shows the bound shortcut.
pub fn action_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: &'static str,
    action: &dyn Action,
) -> Button {
    let dispatched = action.boxed_clone();
    icon_button(id, icon, label)
        .tooltip_with_action(label, action, Some(PLAYER_CONTEXT))
        .on_click(move |_, window, cx| window.dispatch_action(dispatched.boxed_clone(), cx))
}
