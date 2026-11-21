//! The Tracks panel: audio, subtitle and video tracks, titles and chapters, sync and speed.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use super::view::PlayerView;
use crate::components::icon_button;
use crate::components::surface::{self, Segment, card, note_row, section, segmented, setting_row};
use crate::playback::Command;
use crate::playback::state::{
    DELAY_STEP_MS, PLAYBACK_RATES, format_delay, format_rate, format_time,
};
use crate::theme::metrics::{EDGE, HIT_TARGET, INSET};

/// How a track/chapter row looks.
struct Choice {
    label: SharedString,
    detail: Option<SharedString>,
    selected: bool,
    enabled: bool,
}

impl PlayerView {
    /// A selectable row that runs `on_click` on this view.
    fn choice_row(
        &self,
        id: impl Into<SharedString>,
        choice: Choice,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        surface::choice_row(
            gpui_kit::ElementId::Name(id.into()),
            choice.label,
            choice.detail,
            choice.selected,
            choice.enabled,
            cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
                cx.notify();
            }),
        )
    }

    pub(crate) fn render_tracks_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let glass = self.glass;
        let tracks = self.model.tracks.clone();
        let navigation = self.model.navigation.clone();
        let has_media = self.model.state.has_media();
        let mut content = v_flex().w_full().px(EDGE).pb_4();

        // Audio.
        let mut rows: Vec<AnyElement> = tracks
            .real_audio()
            .map(|track| {
                let id = track.id;
                self.choice_row(
                    format!("audio-{id}"),
                    Choice {
                        label: track.name.clone().into(),
                        detail: None,
                        selected: tracks.current_audio == Some(id),
                        enabled: true,
                    },
                    move |this, _| this.send(Command::SelectAudioTrack(id)),
                    cx,
                )
            })
            .collect();
        if rows.is_empty() {
            rows.push(note_row(if has_media {
                "No selectable audio tracks"
            } else {
                "Open media to choose an audio track"
            }));
        }
        content = content.child(section("Audio")).child(card(&glass, rows));

        // Subtitles.
        let mut rows = vec![self.choice_row(
            "subtitle-off",
            Choice {
                label: "Off".into(),
                detail: None,
                selected: has_media && !tracks.subtitles_enabled(),
                enabled: has_media,
            },
            |this, _| this.send(Command::SelectSubtitleTrack(-1)),
            cx,
        )];
        rows.extend(tracks.real_subtitles().map(|track| {
            let id = track.id;
            self.choice_row(
                format!("subtitle-{id}"),
                Choice {
                    label: track.name.clone().into(),
                    detail: None,
                    selected: tracks.current_subtitle == Some(id),
                    enabled: true,
                },
                move |this, _| this.send(Command::SelectSubtitleTrack(id)),
                cx,
            )
        }));
        rows.push(
            h_flex()
                .px(INSET)
                .py_2()
                .child(
                    Button::new("load-subtitle")
                        .ghost()
                        .icon(Icon::new(IconName::Plus))
                        .label("Load subtitle file…")
                        .rounded_full()
                        .h(HIT_TARGET)
                        .disabled(!has_media)
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.prompt_subtitle_file(window, cx)
                            }),
                        ),
                )
                .into_any_element(),
        );
        content = content
            .child(section("Subtitles"))
            .child(card(&glass, rows));

        // Video tracks (only when there is a choice).
        let video: Vec<_> = tracks.real_video().cloned().collect();
        if video.len() > 1 {
            let rows = video
                .into_iter()
                .map(|track| {
                    let id = track.id;
                    self.choice_row(
                        format!("video-{id}"),
                        Choice {
                            label: track.name.into(),
                            detail: None,
                            selected: tracks.current_video == Some(id),
                            enabled: true,
                        },
                        move |this, _| this.send(Command::SelectVideoTrack(id)),
                        cx,
                    )
                })
                .collect::<Vec<_>>();
            content = content.child(section("Video")).child(card(&glass, rows));
        }

        // Titles (discs) and chapters.
        let real_titles: Vec<_> = navigation
            .titles
            .iter()
            .filter(|title| !title.is_menu || navigation.titles.len() > 1)
            .cloned()
            .collect();
        if real_titles.len() > 1 {
            let rows = real_titles
                .into_iter()
                .map(|title| {
                    let index = title.index;
                    let detail = (title.duration_ms > 0)
                        .then(|| SharedString::from(format_time(title.duration_ms)));
                    self.choice_row(
                        format!("title-{index}"),
                        Choice {
                            label: (if title.is_menu {
                                format!("{} (menu)", title.name)
                            } else {
                                title.name
                            })
                            .into(),
                            detail,
                            selected: navigation.current_title == Some(index),
                            enabled: true,
                        },
                        move |this, _| this.send(Command::SetTitle(index)),
                        cx,
                    )
                })
                .collect::<Vec<_>>();
            content = content.child(section("Titles")).child(card(&glass, rows));
        }
        if !navigation.chapters.is_empty() {
            let current = navigation
                .current_chapter
                .or_else(|| navigation.chapter_at(self.model.time_ms).map(|c| c.index));
            let rows = navigation
                .chapters
                .iter()
                .map(|chapter| {
                    let index = chapter.index;
                    self.choice_row(
                        format!("chapter-{index}"),
                        Choice {
                            label: chapter.name.clone().into(),
                            detail: Some(format_time(chapter.offset_ms).into()),
                            selected: current == Some(index),
                            enabled: true,
                        },
                        move |this, _| this.send(Command::SetChapter(index)),
                        cx,
                    )
                })
                .collect::<Vec<_>>();
            content = content.child(section("Chapters")).child(card(&glass, rows));
        }

        // Sync.
        let delay_control =
            |id: &'static str, value: i64, make: fn(i64) -> Command, cx: &mut Context<Self>| {
                let step =
                    |delta: i64, icon: IconName, name: &'static str, cx: &mut Context<Self>| {
                        icon_button(SharedString::from(format!("{id}-{name}")), icon, name)
                            .disabled(!has_media)
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.send(make(value + delta));
                            }))
                    };
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(step(-DELAY_STEP_MS, IconName::Minus, "Earlier", cx))
                    .child(
                        div()
                            .min_w(px(56.))
                            .text_center()
                            .text_sm()
                            .child(SharedString::from(format_delay(value))),
                    )
                    .child(step(DELAY_STEP_MS, IconName::Plus, "Later", cx))
            };
        let audio_delay = delay_control(
            "audio-delay",
            self.model.audio_delay_ms,
            Command::SetAudioDelay,
            cx,
        );
        let subtitle_delay = delay_control(
            "subtitle-delay",
            self.model.subtitle_delay_ms,
            Command::SetSubtitleDelay,
            cx,
        );
        content = content.child(section("Sync")).child(card(
            &glass,
            [
                setting_row("Audio delay", None, audio_delay),
                setting_row("Subtitle delay", None, subtitle_delay),
            ],
        ));

        // Speed.
        let rate = self.model.rate;
        let speeds = segmented(
            "rate",
            &glass,
            PLAYBACK_RATES.iter().map(|&value| {
                let view = cx.entity().downgrade();
                Segment::new(
                    format_rate(value),
                    (rate - value).abs() < f32::EPSILON,
                    move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| this.set_rate(value, cx));
                    },
                )
            }),
        );
        content = content
            .child(section("Speed"))
            .child(div().w_full().child(speeds));

        div()
            .id("tracks-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(content)
    }
}
