//! The Effects panel: LibVLC's equalizer, stereo mode, picture adjustments, geometry,
//! deinterlacing, frame stepping and snapshots.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};

use super::commands::UiCommand;
use super::view::PlayerView;
use crate::components::surface::{
    Segment, card, note_row, section, segmented, setting_row, slider_row,
};
use crate::playback::Command;
use crate::playback::features::{
    ASPECT_RATIOS, CROP_RATIOS, DEINTERLACE_MODES, StereoMode, ZOOM_LEVELS,
};
use crate::theme::metrics::{EDGE, HIT_TARGET, INSET};

fn band_label(hz: f32) -> String {
    if hz >= 1000.0 {
        let khz = hz / 1000.0;
        if khz.fract() == 0.0 {
            format!("{khz:.0} kHz")
        } else {
            format!("{khz:.1} kHz")
        }
    } else {
        format!("{hz:.0} Hz")
    }
}

fn db(value: f32) -> String {
    if value.abs() < 0.05 {
        "0 dB".into()
    } else {
        format!("{value:+.1} dB")
    }
}

impl PlayerView {
    /// A segmented control over an option table, sending `make(option)` on selection.
    fn option_segments(
        &self,
        id: &'static str,
        options: &'static [(&'static str, Option<&'static str>)],
        selected: usize,
        apply: fn(&mut PlayerView, usize),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        segmented(
            id,
            &self.glass,
            options.iter().enumerate().map(|(index, (label, _))| {
                let view = cx.entity().downgrade();
                Segment::new(*label, selected == index, move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        apply(this, index);
                        cx.notify();
                    });
                })
            }),
        )
        .into_any_element()
    }

    pub(crate) fn render_effects_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let glass = self.glass;
        let eq = self.settings.equalizer.clone();
        let info = self.model.equalizer_info.clone();
        let has_video = self.model.has_video && self.model.state.has_media();
        let mut content = v_flex().w_full().px(EDGE).pb_4();

        // Equalizer.
        let enable = Switch::new("eq-enabled")
            .checked(eq.enabled)
            .accessibility_label("Equalizer")
            .on_change(cx.listener(|this, on: &bool, _, cx| this.set_equalizer_enabled(*on, cx)));
        let mut eq_rows = vec![setting_row(
            "Equalizer",
            Some("LibVLC 10-band equalizer".into()),
            enable,
        )];
        if info.presets.is_empty() {
            eq_rows.push(note_row("Presets appear once playback has started."));
        } else {
            let mut presets = h_flex().w_full().px(INSET).py_2().gap_1().flex_wrap();
            for (index, preset) in info.presets.iter().enumerate() {
                presets = presets.child(
                    Button::new(gpui_kit::ElementId::NamedInteger(
                        "eq-preset".into(),
                        index as u64,
                    ))
                    .label(preset.name.clone())
                    .rounded_full()
                    .h(HIT_TARGET)
                    .px_3()
                    .map_variant(eq.preset == Some(index))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_equalizer_preset(Some(index), window, cx)
                    })),
                );
            }
            eq_rows.push(presets.into_any_element());
        }
        eq_rows.push(slider_row(
            "Preamp",
            db(eq.preamp),
            &self.eq_sliders[0],
            eq.enabled,
        ));
        for (band, slider) in self.eq_sliders.iter().skip(1).enumerate() {
            let hz = info.frequencies.get(band).copied().unwrap_or_default();
            let label = if hz > 0.0 {
                band_label(hz)
            } else {
                format!("Band {}", band + 1)
            };
            let value = eq.bands.get(band).copied().unwrap_or_default();
            eq_rows.push(slider_row(label, db(value), slider, eq.enabled));
        }
        content = content.child(section("Sound")).child(card(&glass, eq_rows));

        // Stereo mode.
        let stereo = segmented(
            "stereo",
            &glass,
            StereoMode::ALL.into_iter().map(|mode| {
                let view = cx.entity().downgrade();
                Segment::new(mode.label(), self.stereo_mode == mode, move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.stereo_mode = mode;
                        this.send(Command::SetStereoMode(mode));
                        cx.notify();
                    });
                })
            }),
        );
        content = content
            .child(section("Channels"))
            .child(div().w_full().child(stereo));

        // Picture adjustments.
        let adjust = self.picture.adjust;
        let names = ["Brightness", "Contrast", "Saturation", "Hue", "Gamma"];
        let values = [
            format!("{:.0}%", adjust.brightness * 100.0),
            format!("{:.0}%", adjust.contrast * 100.0),
            format!("{:.0}%", adjust.saturation * 100.0),
            format!("{:+.0}°", adjust.hue),
            format!("{:.2}", adjust.gamma),
        ];
        let mut picture_rows: Vec<AnyElement> = names
            .iter()
            .zip(values)
            .zip(self.adjust_sliders.iter())
            .map(|((name, value), slider)| slider_row(*name, value, slider, true))
            .collect();
        picture_rows.push(
            h_flex()
                .px(INSET)
                .py_2()
                .child(
                    Button::new("adjust-reset")
                        .ghost()
                        .label("Reset picture")
                        .rounded_full()
                        .h(HIT_TARGET)
                        .disabled(adjust.is_neutral())
                        .on_click(cx.listener(|this, _, window, cx| this.reset_adjust(window, cx))),
                )
                .into_any_element(),
        );
        content = content
            .child(section("Picture"))
            .child(card(&glass, picture_rows));

        // Geometry and deinterlacing.
        let picture = self.picture;
        content = content
            .child(section("Aspect ratio"))
            .child(self.option_segments(
                "aspect",
                ASPECT_RATIOS,
                picture.aspect,
                |this, index| {
                    this.picture.aspect = index;
                    let ratio = ASPECT_RATIOS[index].1.map(str::to_owned);
                    this.send(Command::SetAspectRatio(ratio));
                },
                cx,
            ))
            .child(section("Crop"))
            .child(self.option_segments(
                "crop",
                CROP_RATIOS,
                picture.crop,
                |this, index| {
                    this.picture.crop = index;
                    let crop = CROP_RATIOS[index].1.map(str::to_owned);
                    this.send(Command::SetCrop(crop));
                },
                cx,
            ))
            .child(section("Zoom"))
            .child(segmented(
                "zoom",
                &glass,
                ZOOM_LEVELS
                    .iter()
                    .enumerate()
                    .map(|(index, (label, factor))| {
                        let view = cx.entity().downgrade();
                        let factor = *factor;
                        Segment::new(*label, picture.zoom == index, move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.picture.zoom = index;
                                this.send(Command::SetZoom(factor));
                                cx.notify();
                            });
                        })
                    }),
            ))
            .child(section("Deinterlace"))
            .child(self.option_segments(
                "deinterlace",
                DEINTERLACE_MODES,
                picture.deinterlace,
                |this, index| {
                    this.picture.deinterlace = index;
                    let mode = DEINTERLACE_MODES[index].1.map(str::to_owned);
                    this.send(Command::SetDeinterlace(mode));
                },
                cx,
            ));

        // Frame tools.
        let handler = Self::local_handler(cx);
        let tool = |id: &'static str, icon: IconName, label: &'static str, command: UiCommand| {
            let handler = handler.clone();
            Button::new(id)
                .ghost()
                .icon(Icon::new(icon))
                .label(label)
                .rounded_full()
                .h(HIT_TARGET)
                .px_4()
                .disabled(!has_video)
                .on_click(move |_, window, cx| handler(command, window, cx))
        };
        content = content.child(section("Frame")).child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(tool(
                    "snapshot",
                    IconName::Camera,
                    "Snapshot",
                    UiCommand::Snapshot,
                ))
                .child(tool(
                    "next-frame",
                    IconName::StepForward,
                    "Next frame",
                    UiCommand::NextFrame,
                )),
        );

        div()
            .id("effects-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(content)
    }
}

/// Selected chips are primary (white), others ghost.
trait MapVariant {
    fn map_variant(self, selected: bool) -> Self;
}

impl MapVariant for Button {
    fn map_variant(self, selected: bool) -> Self {
        if selected {
            self.primary()
        } else {
            self.ghost()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_and_gain_labels() {
        assert_eq!(band_label(60.0), "60 Hz");
        assert_eq!(band_label(1000.0), "1 kHz");
        assert_eq!(band_label(3500.0), "3.5 kHz");
        assert_eq!(db(0.01), "0 dB");
        assert_eq!(db(4.0), "+4.0 dB");
        assert_eq!(db(-2.5), "-2.5 dB");
    }
}
