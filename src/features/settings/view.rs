//! The Settings panel: playback engine options, audio output, appearance, history, shortcuts and
//! engine information — as soft grouped cards.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use super::model::Toggle;
use crate::actions::SHORTCUTS;
use crate::components::surface::{
    Segment, card, choice_row, note_row, section, segmented, setting_row,
};
use crate::features::player::model::RuntimeStatus;
use crate::features::player::view::PlayerView;
use crate::playback::features::NETWORK_CACHING_MS;
use crate::playback::{Command, MediaSource};
use crate::theme::metrics::{EDGE, HIT_TARGET, INSET};
use crate::theme::tokens;

fn caching_label(ms: u32) -> String {
    if ms >= 1000 {
        format!("{} s", ms / 1000)
    } else {
        format!("{ms} ms")
    }
}

impl PlayerView {
    pub(crate) fn render_settings_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let glass = self.glass;
        let mut content = v_flex().w_full().px(EDGE).pb_4();

        // Playback engine.
        let playback = self.settings.playback;
        let hardware = Switch::new("setting-hw")
            .checked(playback.hardware_decoding)
            .accessibility_label("Hardware decoding")
            .on_change(cx.listener(|this, on: &bool, _, cx| {
                this.settings.playback.hardware_decoding = *on;
                this.send(Command::SetPlaybackOptions(this.settings.playback));
                this.settings_changed(cx);
            }));
        let resume = toggle_switch(Toggle::ResumePlayback, &self.settings, cx);
        let caching = segmented(
            "caching",
            &glass,
            NETWORK_CACHING_MS.iter().map(|&ms| {
                let view = cx.entity().downgrade();
                Segment::new(
                    caching_label(ms),
                    playback.network_caching_ms == ms,
                    move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.settings.playback.network_caching_ms = ms;
                            this.send(Command::SetPlaybackOptions(this.settings.playback));
                            this.settings_changed(cx);
                        });
                    },
                )
            }),
        );
        content = content.child(section("Playback")).child(card(
            &glass,
            [
                setting_row(
                    "Hardware decoding",
                    Some("VA-API, VideoToolbox or D3D11 when available".into()),
                    hardware,
                ),
                setting_row(Toggle::ResumePlayback.label(), None, resume),
                v_flex()
                    .px(INSET)
                    .py_3()
                    .gap_2()
                    .child(div().text_sm().child("Network buffer"))
                    .child(div().text_xs().text_color(t.foreground_muted).child(
                        "Longer buffers ride out unreliable streams; applies to the next item.",
                    ))
                    .child(caching)
                    .into_any_element(),
            ],
        ));

        // Audio output.
        let current_device = self.settings.audio_device.clone();
        let mut device_rows: Vec<AnyElement> = Vec::new();
        let mut choices = vec![(None, "System default".to_owned())];
        choices.extend(
            self.model
                .audio_devices
                .iter()
                .map(|device| (Some(device.id.clone()), device.name.clone())),
        );
        for (index, (id, name)) in choices.into_iter().enumerate() {
            let selected = current_device == id;
            device_rows.push(choice_row(
                gpui_kit::ElementId::NamedInteger("audio-device".into(), index as u64),
                name,
                None,
                selected,
                true,
                cx.listener(move |this, _, _, cx| {
                    this.settings.audio_device = id.clone();
                    this.send(Command::SetAudioDevice(id.clone()));
                    this.settings_changed(cx);
                }),
            ));
        }
        if self.model.audio_devices.is_empty() {
            device_rows.push(note_row(
                "Devices are listed once the playback engine has started.",
            ));
        }
        content = content
            .child(section("Audio output"))
            .child(card(&glass, device_rows));

        // Appearance.
        let translucent = self.settings.translucent();
        let translucency = Switch::new("setting-translucent")
            .checked(translucent)
            .accessibility_label("Translucent window")
            .on_change(
                cx.listener(|this, on: &bool, window, cx| this.set_translucent(*on, window, cx)),
            );
        content = content.child(section("Appearance")).child(card(
            &glass,
            [setting_row(
                "Translucent window",
                Some("Let the desktop show softly through the window".into()),
                translucency,
            )],
        ));

        // History.
        let recents = toggle_switch(Toggle::RememberRecents, &self.settings, cx);
        let mut history_rows = vec![setting_row(Toggle::RememberRecents.label(), None, recents)];
        for (index, recent) in self.settings.recents.iter().enumerate().take(8) {
            let source = if recent.is_url {
                MediaSource::Url(recent.location.clone())
            } else {
                MediaSource::File(PathBuf::from(&recent.location))
            };
            let name: SharedString = source.display_name().into();
            let detail: SharedString = recent.location.clone().into();
            history_rows.push(
                v_flex()
                    .id(("recent", index))
                    .px(INSET)
                    .min_h(HIT_TARGET)
                    .py_2()
                    .justify_center()
                    .hover(|row| row.bg(glass.chip))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| match &source {
                        MediaSource::File(path) => {
                            this.open_paths(vec![path.clone()], true, window, cx)
                        }
                        MediaSource::Url(_) => {
                            let ids = this.playlist.add([source.clone()]);
                            if let Some(id) = ids.first() {
                                this.play_item(*id, false, window, cx);
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
                    )
                    .into_any_element(),
            );
        }
        history_rows.push(
            h_flex()
                .px(INSET)
                .py_2()
                .child(
                    Button::new("clear-history")
                        .ghost()
                        .label("Clear history")
                        .rounded_full()
                        .h(HIT_TARGET)
                        .disabled(self.settings.recents.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.clear_history();
                            this.settings_changed(cx);
                        })),
                )
                .into_any_element(),
        );
        content = content
            .child(section("History"))
            .child(card(&glass, history_rows));

        // Keyboard.
        let shortcut_rows = SHORTCUTS.iter().map(|shortcut| {
            h_flex()
                .px(INSET)
                .py_2()
                .gap_3()
                .items_center()
                .child(div().flex_1().text_sm().child(shortcut.description))
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded(px(8.))
                        .bg(glass.chip)
                        .text_xs()
                        .child(shortcut.display),
                )
                .into_any_element()
        });
        content = content
            .child(section("Keyboard"))
            .child(card(&glass, shortcut_rows));

        // About.
        let engine = match &self.model.runtime {
            RuntimeStatus::NotStarted => "Starts when media is opened".to_owned(),
            RuntimeStatus::Starting => "Starting…".to_owned(),
            RuntimeStatus::Ready(version) => format!("LibVLC {version}"),
        };
        let surface = match &self.video_error {
            None => "Native video surface".to_owned(),
            Some(reason) => format!("Unavailable: {reason}"),
        };
        content = content.child(section("About")).child(card(
            &glass,
            [
                setting_row("Engine", Some(engine.into()), div()),
                setting_row("Video output", Some(surface.into()), div()),
                setting_row(
                    "Mono Player",
                    Some(format!("Version {}", env!("CARGO_PKG_VERSION")).into()),
                    div(),
                ),
            ],
        ));

        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(content)
    }
}

fn toggle_switch(
    toggle: Toggle,
    settings: &crate::persistence::settings::Settings,
    cx: &mut Context<PlayerView>,
) -> Switch {
    let id = match toggle {
        Toggle::RememberRecents => "setting-recents",
        Toggle::ResumePlayback => "setting-resume",
    };
    Switch::new(id)
        .checked(toggle.value(settings))
        .accessibility_label(toggle.label())
        .on_change(cx.listener(move |this, on: &bool, _, cx| {
            toggle.apply(&mut this.settings, *on);
            this.settings_changed(cx);
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caching_labels() {
        assert_eq!(caching_label(300), "300 ms");
        assert_eq!(caching_label(10_000), "10 s");
    }
}
