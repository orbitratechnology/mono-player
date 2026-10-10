//! The settings side panel: privacy toggles, recent media, engine info and shortcuts.

use std::path::PathBuf;

use gpui_kit::component::button::Button;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};

use super::model::Toggle;
use crate::actions::SHORTCUTS;
use crate::features::player::model::RuntimeStatus;
use crate::features::player::view::{PlayerView, note};
use crate::playback::MediaSource;
use crate::theme::metrics::{EDGE, HIT_TARGET};
use crate::theme::tokens;

impl PlayerView {
    pub(crate) fn render_settings_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let section = |title: &'static str| {
            div()
                .px(EDGE)
                .pt_4()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.foreground_muted)
                .child(title.to_uppercase())
        };

        let toggle_row = |toggle: Toggle, cx: &mut Context<Self>| {
            let id = match toggle {
                Toggle::RememberRecents => "setting-recents",
                Toggle::ResumePlayback => "setting-resume",
            };
            h_flex().px(EDGE).h(HIT_TARGET).items_center().child(
                Switch::new(id)
                    .checked(toggle.value(&self.settings))
                    .label(toggle.label())
                    .accessibility_label(toggle.label())
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        toggle.apply(&mut this.settings, *on);
                        this.settings_changed(cx);
                    })),
            )
        };

        let mut content = v_flex()
            .w_full()
            .pb_4()
            .child(section("History"))
            .child(toggle_row(Toggle::RememberRecents, cx))
            .child(toggle_row(Toggle::ResumePlayback, cx))
            .child(
                h_flex().px(EDGE).pt_1().child(
                    Button::new("clear-history")
                        .outline()
                        .label("Clear history")
                        .h(HIT_TARGET)
                        .disabled(self.settings.recents.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.clear_history();
                            this.settings_changed(cx);
                        })),
                ),
            )
            .child(section("Recent"));

        if self.settings.recents.is_empty() {
            content = content.child(note(if self.settings.remember_recents {
                "Nothing yet."
            } else {
                "Recent media is not being remembered."
            }));
        }
        for (index, recent) in self.settings.recents.iter().enumerate().take(10) {
            let source = if recent.is_url {
                MediaSource::Url(recent.location.clone())
            } else {
                MediaSource::File(PathBuf::from(&recent.location))
            };
            let name: SharedString = source.display_name().into();
            let detail: SharedString = recent.location.clone().into();
            content = content.child(
                v_flex()
                    .id(("recent", index))
                    .px(EDGE)
                    .min_h(HIT_TARGET)
                    .justify_center()
                    .hover(|row| row.bg(t.surface_hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let source = source.clone();
                        match &source {
                            MediaSource::File(path) => {
                                this.open_paths(vec![path.clone()], true, window, cx)
                            }
                            MediaSource::Url(_) => {
                                let ids = this.playlist.add([source]);
                                if let Some(id) = ids.first() {
                                    this.play_item(*id, false, window, cx);
                                }
                            }
                        }
                    }))
                    .child(
                        div()
                            .text_sm()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.foreground_muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(detail),
                    ),
            );
        }

        let engine = match &self.model.runtime {
            RuntimeStatus::NotStarted => "LibVLC starts when media is opened".to_owned(),
            RuntimeStatus::Starting => "LibVLC starting…".to_owned(),
            RuntimeStatus::Ready(version) => format!("LibVLC {version}"),
        };
        let surface = match &self.video_error {
            None => "Native video surface".to_owned(),
            Some(reason) => format!("Video surface unavailable: {reason}"),
        };
        content = content
            .child(section("Engine"))
            .child(note(engine))
            .child(note(surface))
            .child(section("Keyboard shortcuts"));

        for shortcut in SHORTCUTS {
            content = content.child(
                h_flex()
                    .px(EDGE)
                    .py_1()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .min_w_24()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(t.border)
                            .text_xs()
                            .text_center()
                            .child(shortcut.display),
                    )
                    .child(div().text_sm().child(shortcut.description)),
            );
        }

        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(content)
    }
}
