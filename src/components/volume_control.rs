//! Mute toggle plus volume slider.

use gpui_kit::assets::IconName;
use gpui_kit::component::h_flex;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::{App, Entity, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window};

use super::action_button;
use crate::actions::ToggleMute;
use crate::theme::metrics::{HIT_TARGET, VOLUME_WIDTH};

#[derive(IntoElement)]
pub struct VolumeControl {
    pub slider: Entity<SliderState>,
    pub muted: bool,
}

impl RenderOnce for VolumeControl {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        // Muted state is carried by the icon and the accessible name, not by color.
        let (icon, label) = if self.muted {
            (IconName::VolumeX, "Unmute")
        } else {
            (IconName::Volume2, "Mute")
        };
        h_flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .child(action_button("volume-mute", icon, label, &ToggleMute))
            .child(
                h_flex()
                    .w(VOLUME_WIDTH)
                    .h(HIT_TARGET)
                    .items_center()
                    .child(Slider::new(&self.slider).w_full()),
            )
    }
}
