//! Elapsed time, seek slider, total time.

use gpui_kit::component::h_flex;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::{
    App, Entity, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
    div, px,
};

use crate::theme::metrics::{EDGE, HIT_TARGET};
use crate::theme::tokens;

#[derive(IntoElement)]
pub struct Timeline {
    pub slider: Entity<SliderState>,
    pub elapsed: SharedString,
    pub total: SharedString,
    pub enabled: bool,
}

impl RenderOnce for Timeline {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let t = tokens();
        let time_label = |text: SharedString| {
            div()
                .min_w(px(56.))
                .flex_shrink_0()
                .text_sm()
                .text_color(t.foreground_muted)
                .child(text)
        };
        h_flex()
            .w_full()
            .px(EDGE)
            .gap_3()
            .items_center()
            .child(time_label(self.elapsed))
            .child(
                // The slider's own track is thin; this wrapper gives it a full touch-height hit
                // area (pointer events land on the slider element inside).
                h_flex()
                    .flex_1()
                    .h(HIT_TARGET)
                    .items_center()
                    .child(Slider::new(&self.slider).disabled(!self.enabled).w_full()),
            )
            .child(time_label(self.total).text_right())
    }
}
