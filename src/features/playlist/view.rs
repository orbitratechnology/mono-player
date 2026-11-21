//! The queue side panel: address entry, queue options and the virtualized list.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{
    Disableable as _, Icon, Selectable as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::{Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div};

use crate::actions::{OpenFiles, PLAYER_CONTEXT};
use crate::components::media_row::MediaRow;
use crate::features::player::view::{PlayerView, note};
use crate::features::playlist::model::RepeatMode;
use crate::theme::metrics::{EDGE, HIT_TARGET, TARGET_GAP};
use crate::theme::tokens;

impl PlayerView {
    pub(crate) fn render_queue_panel(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens();
        let repeat = self.playlist.repeat();
        let (repeat_icon, repeat_label) = match repeat {
            RepeatMode::One => (IconName::Repeat1, "Repeat one"),
            RepeatMode::All => (IconName::Repeat, "Repeat all"),
            RepeatMode::Off => (IconName::Repeat, "Repeat off"),
        };

        let address = h_flex()
            .px(EDGE)
            .pt_3()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.url_input).h(HIT_TARGET)),
            )
            .child(
                Button::new("address-add")
                    .outline()
                    .label("Add")
                    .h(HIT_TARGET)
                    .px_4()
                    .on_click(cx.listener(|this, _, window, cx| this.add_address(window, cx))),
            );

        let options = h_flex()
            .px(EDGE)
            .py_2()
            .gap(TARGET_GAP)
            .items_center()
            .child(
                Button::new("queue-add-files")
                    .ghost()
                    .icon(Icon::new(IconName::FolderOpen))
                    .label("Add files")
                    .h(HIT_TARGET)
                    .tooltip_with_action("Add files", &OpenFiles, Some(PLAYER_CONTEXT))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.prompt_open_files(window, cx)),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("queue-repeat")
                    .ghost()
                    .icon(Icon::new(repeat_icon))
                    .w(HIT_TARGET)
                    .h(HIT_TARGET)
                    .selected(repeat != RepeatMode::Off)
                    .accessibility_label(repeat_label)
                    .tooltip(repeat_label)
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_repeat(cx))),
            )
            .child({
                let shuffle = self.playlist.shuffle();
                let label = if shuffle { "Shuffle on" } else { "Shuffle off" };
                Button::new("queue-shuffle")
                    .ghost()
                    .icon(Icon::new(IconName::Shuffle))
                    .w(HIT_TARGET)
                    .h(HIT_TARGET)
                    .selected(shuffle)
                    .accessibility_label(label)
                    .tooltip(label)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_shuffle(cx)))
            })
            .child(
                Button::new("queue-clear")
                    .ghost()
                    .icon(Icon::new(IconName::Delete))
                    .w(HIT_TARGET)
                    .h(HIT_TARGET)
                    .accessibility_label("Clear queue")
                    .tooltip("Clear queue")
                    .disabled(self.playlist.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.clear_queue(cx))),
            );

        let list = if self.playlist.is_empty() {
            v_flex()
                .flex_1()
                .child(note("The queue is empty. Drop files here, add them with Ctrl/Cmd+O, or paste an address above."))
                .into_any_element()
        } else {
            let header = div()
                .px(EDGE)
                .pb_1()
                .text_xs()
                .text_color(t.foreground_muted)
                .child(SharedString::from(format!(
                    "{} item{}",
                    self.playlist.len(),
                    if self.playlist.len() == 1 { "" } else { "s" }
                )));
            v_flex()
                .flex_1()
                .min_h_0()
                .child(header)
                .child(
                    v_virtual_list(
                        cx.entity(),
                        "queue-list",
                        self.playlist_row_sizes.clone(),
                        |view, range, _, cx| {
                            let current = view.playlist.current_id();
                            let len = view.playlist.len();
                            range
                                .filter_map(|index| {
                                    let item = view.playlist.items().get(index)?.clone();
                                    let id = item.id;
                                    let entity = cx.entity().downgrade();
                                    let activate = entity.clone();
                                    let up = entity.clone();
                                    let down = entity.clone();
                                    let remove = entity;
                                    Some(MediaRow {
                                        id: id.0,
                                        index,
                                        title: item.title.into(),
                                        is_current: current == Some(id),
                                        failed: item.failed,
                                        can_move_up: index > 0,
                                        can_move_down: index + 1 < len,
                                        on_activate: Rc::new(move |_, window, cx| {
                                            let _ = activate.update(cx, |view, cx| {
                                                view.play_item(id, false, window, cx)
                                            });
                                        }),
                                        on_move_up: Rc::new(move |_, _, cx| {
                                            let _ = up
                                                .update(cx, |view, cx| view.move_item(id, -1, cx));
                                        }),
                                        on_move_down: Rc::new(move |_, _, cx| {
                                            let _ = down
                                                .update(cx, |view, cx| view.move_item(id, 1, cx));
                                        }),
                                        on_remove: Rc::new(move |_, _, cx| {
                                            let _ = remove
                                                .update(cx, |view, cx| view.remove_item(id, cx));
                                        }),
                                    })
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    .flex_1(),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(address)
            .child(options)
            .child(list)
    }
}
