//! One playlist row: title, current/failed state, and reorder/remove controls.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Disableable as _, Icon, h_flex};
use gpui_kit::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Window, div, prelude::FluentBuilder as _,
};

use super::icon_button;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use crate::theme::metrics::{EDGE, ROW_HEIGHT};
use crate::theme::tokens;

type Handler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct MediaRow {
    pub id: u64,
    pub index: usize,
    pub title: SharedString,
    pub is_current: bool,
    pub failed: bool,
    pub can_move_up: bool,
    pub can_move_down: bool,
    pub on_activate: Handler,
    pub on_move_up: Handler,
    pub on_move_down: Handler,
    pub on_remove: Handler,
}

impl RenderOnce for MediaRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let t = tokens();
        let row_id = ElementId::NamedInteger("media-row".into(), self.id);
        let activate = self.on_activate.clone();
        let (up, down, remove) = (self.on_move_up, self.on_move_down, self.on_remove);

        // The current item is marked by an icon, bold text, a fill and a left rule — never by
        // color alone.
        let marker = if self.failed {
            Icon::new(IconName::CircleAlert).into_any_element()
        } else if self.is_current {
            Icon::new(IconName::Play).into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(t.foreground_disabled)
                .child(SharedString::from(format!("{}", self.index + 1)))
                .into_any_element()
        };

        let label = if self.failed {
            format!("{}, failed to play", self.title)
        } else if self.is_current {
            format!("{}, current item", self.title)
        } else {
            format!("Play {}", self.title)
        };
        // The main area is a real button: focusable, keyboard-activatable, and named.
        let play = Button::new(ElementId::NamedInteger("media-row-play".into(), self.id))
            .ghost()
            .flex_1()
            .min_w_0()
            .h(ROW_HEIGHT)
            .px_0()
            .justify_start()
            .accessibility_label(label)
            .on_click(move |event, window, cx| activate(event, window, cx))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .w_6()
                            .flex_shrink_0()
                            .justify_center()
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .when(self.is_current, |d| d.font_weight(FontWeight::BOLD))
                            .text_color(if self.failed {
                                t.foreground_disabled
                            } else {
                                t.foreground
                            })
                            .when(self.failed, |d| d.line_through())
                            .child(self.title),
                    ),
            );

        h_flex()
            .id(row_id)
            .h(ROW_HEIGHT)
            .w_full()
            .pl(EDGE)
            .pr_1()
            .gap_1()
            .items_center()
            .border_l_2()
            .border_color(if self.is_current {
                t.foreground
            } else {
                t.background
            })
            .when(self.is_current, |row| row.bg(t.surface_active))
            .hover(|row| row.bg(t.surface_hover))
            .child(play)
            .child(
                icon_button(
                    ElementId::NamedInteger("row-up".into(), self.id),
                    IconName::ArrowUp,
                    "Move up",
                )
                .disabled(!self.can_move_up)
                .on_click(move |e, w, cx| up(e, w, cx)),
            )
            .child(
                icon_button(
                    ElementId::NamedInteger("row-down".into(), self.id),
                    IconName::ArrowDown,
                    "Move down",
                )
                .disabled(!self.can_move_down)
                .on_click(move |e, w, cx| down(e, w, cx)),
            )
            .child(
                icon_button(
                    ElementId::NamedInteger("row-remove".into(), self.id),
                    IconName::X,
                    "Remove from queue",
                )
                .on_click(move |e, w, cx| remove(e, w, cx)),
            )
            .test_support()
    }
}
