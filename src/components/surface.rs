//! Soft surfaces for panels: section labels, grouped cards, setting rows, segmented controls and
//! labelled sliders. Monochrome, generously rounded, separated by hairlines rather than boxes.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, FontWeight, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::theme::metrics::{CARD_RADIUS, HIT_TARGET, INSET};
use crate::theme::{Glass, tokens};

/// A small uppercase label above a group of settings.
pub fn section(title: impl Into<SharedString>) -> Div {
    let title: SharedString = title.into();
    div()
        .px_1()
        .pt_5()
        .pb_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tokens().foreground_muted)
        .child(SharedString::from(title.to_uppercase()))
}

/// A rounded group of rows separated by hairlines.
pub fn card(glass: &Glass, rows: impl IntoIterator<Item = AnyElement>) -> Div {
    let mut card = v_flex()
        .w_full()
        .rounded(CARD_RADIUS)
        .bg(glass.card)
        .border_1()
        .border_color(glass.hairline)
        .overflow_hidden();
    for (index, row) in rows.into_iter().enumerate() {
        card = card.child(
            div()
                .w_full()
                .when(index > 0, |d| d.border_t_1().border_color(glass.hairline))
                .child(row),
        );
    }
    card
}

/// A labelled row: title (and optional detail) on the left, a control on the right.
pub fn setting_row(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    h_flex()
        .w_full()
        .min_h(px(56.))
        .px(INSET)
        .py_2()
        .gap_3()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(title.into()))
                .when_some(detail, |col, detail| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(tokens().foreground_muted)
                            .child(detail),
                    )
                }),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

/// A full-width text row inside a card (for notes and empty states).
pub fn note_row(text: impl Into<SharedString>) -> AnyElement {
    div()
        .px(INSET)
        .py_3()
        .text_sm()
        .text_color(tokens().foreground_muted)
        .child(text.into())
        .into_any_element()
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One option of a [`segmented`] control.
pub struct Segment {
    pub label: SharedString,
    pub selected: bool,
    pub on_select: ClickHandler,
}

impl Segment {
    pub fn new(
        label: impl Into<SharedString>,
        selected: bool,
        on_select: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            selected,
            on_select: Rc::new(on_select),
        }
    }
}

/// A pill-shaped segmented control. The selected segment is a white chip with dark text
/// (state shown by fill *and* weight, not color alone); every segment keeps a 44 px height.
pub fn segmented(
    id: impl Into<SharedString>,
    glass: &Glass,
    segments: impl IntoIterator<Item = Segment>,
) -> Div {
    segmented_inner(id, glass, segments, false)
}

/// A segmented control whose segments share the full width equally (used for tab strips).
pub fn segmented_fill(
    id: impl Into<SharedString>,
    glass: &Glass,
    segments: impl IntoIterator<Item = Segment>,
) -> Div {
    segmented_inner(id, glass, segments, true).w_full()
}

fn segmented_inner(
    id: impl Into<SharedString>,
    glass: &Glass,
    segments: impl IntoIterator<Item = Segment>,
    fill: bool,
) -> Div {
    let id: SharedString = id.into();
    let mut row = h_flex()
        .p(px(3.))
        .gap(px(2.))
        .rounded_full()
        .bg(glass.chip)
        .when(!fill, |row| row.flex_wrap());
    for (index, segment) in segments.into_iter().enumerate() {
        let on_select = segment.on_select.clone();
        let label = segment.label.clone();
        let button = Button::new(ElementId::NamedInteger(id.clone(), index as u64))
            .label(segment.label)
            .rounded_full()
            .h(HIT_TARGET)
            .map(|b| {
                if fill {
                    b.flex_1().min_w_0().px_0p5().text_xs()
                } else {
                    b.px_3()
                }
            })
            .accessibility_label(if segment.selected {
                SharedString::from(format!("{label}, selected"))
            } else {
                label
            })
            .on_click(move |e, w, cx| on_select(e, w, cx));
        let button = if segment.selected {
            // Fill marks the selection; tab strips skip the bold weight so labels fit.
            button
                .primary()
                .when(!fill, |b| b.font_weight(FontWeight::SEMIBOLD))
        } else {
            button.ghost()
        };
        row = row.child(button);
    }
    row
}

/// A selectable, left-aligned row: a check mark when selected, the label, and an optional
/// trailing detail. A real button, so it is focusable and announces its selected state.
pub fn choice_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    detail: Option<SharedString>,
    selected: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    use gpui_kit::assets::IconName;
    use gpui_kit::component::{Disableable as _, Icon};

    let label: SharedString = label.into();
    Button::new(id)
        .ghost()
        .w_full()
        .h(px(48.))
        .rounded(px(0.))
        .px(INSET)
        .disabled(!enabled)
        .accessibility_label(if selected {
            SharedString::from(format!("{label}, selected"))
        } else {
            label.clone()
        })
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .w_5()
                        .flex_shrink_0()
                        .when(selected, |d| d.child(Icon::new(IconName::Check))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(label),
                )
                .when_some(detail, |row, detail| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(tokens().foreground_muted)
                            .child(detail),
                    )
                }),
        )
        .on_click(on_click)
        .into_any_element()
}

/// A labelled slider with its current value on the right.
pub fn slider_row(
    title: impl Into<SharedString>,
    value: impl Into<SharedString>,
    slider: &Entity<SliderState>,
    enabled: bool,
) -> AnyElement {
    v_flex()
        .w_full()
        .px(INSET)
        .pt_2()
        .child(
            h_flex()
                .w_full()
                .text_sm()
                .child(div().flex_1().child(title.into()))
                .child(
                    div()
                        .text_xs()
                        .text_color(tokens().foreground_muted)
                        .child(value.into()),
                ),
        )
        .child(
            h_flex()
                .h(HIT_TARGET)
                .items_center()
                .child(Slider::new(slider).disabled(!enabled).w_full()),
        )
        .into_any_element()
}
