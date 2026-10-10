//! The monochrome design system: one set of semantic color tokens and layout metrics.
//!
//! Every component color in GPUI Kit's theme is derived from these tokens, and any color the
//! component library defines that we do not map explicitly is desaturated, so no chromatic
//! accent can appear in the UI.

use std::sync::LazyLock;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Pixels, px, rgb};

/// Semantic color tokens (PRD §11).
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub foreground: Hsla,
    pub foreground_muted: Hsla,
    pub foreground_disabled: Hsla,
    pub border: Hsla,
    pub track_background: Hsla,
    pub track_foreground: Hsla,
    pub focus_ring: Hsla,
}

impl Tokens {
    pub fn dark() -> Self {
        Self {
            background: rgb(0x000000).into(),
            surface: rgb(0x0A0A0A).into(),
            surface_hover: rgb(0x1A1A1A).into(),
            surface_active: rgb(0x262626).into(),
            foreground: rgb(0xFFFFFF).into(),
            foreground_muted: rgb(0xB3B3B3).into(),
            foreground_disabled: rgb(0x737373).into(),
            border: rgb(0x333333).into(),
            track_background: rgb(0x4A4A4A).into(),
            track_foreground: rgb(0xFFFFFF).into(),
            focus_ring: rgb(0xFFFFFF).into(),
        }
    }

    /// Every token, for contrast and saturation checks.
    pub fn all(&self) -> [(&'static str, Hsla); 11] {
        [
            ("background", self.background),
            ("surface", self.surface),
            ("surface_hover", self.surface_hover),
            ("surface_active", self.surface_active),
            ("foreground", self.foreground),
            ("foreground_muted", self.foreground_muted),
            ("foreground_disabled", self.foreground_disabled),
            ("border", self.border),
            ("track_background", self.track_background),
            ("track_foreground", self.track_foreground),
            ("focus_ring", self.focus_ring),
        ]
    }
}

static TOKENS: LazyLock<Tokens> = LazyLock::new(Tokens::dark);

/// The active token set.
pub fn tokens() -> &'static Tokens {
    &TOKENS
}

/// Layout metrics (PRD §10.4). Hit areas never shrink below these.
pub mod metrics {
    use super::*;

    /// Minimum interactive hit area.
    pub const HIT_TARGET: Pixels = px(44.);
    /// Primary play/pause control.
    pub const PRIMARY_TARGET: Pixels = px(48.);
    /// Minimum spacing between neighboring targets.
    pub const TARGET_GAP: Pixels = px(8.);
    /// Height of one playlist/track row (≥ HIT_TARGET).
    pub const ROW_HEIGHT: Pixels = px(48.);
    /// Side panel width.
    pub const PANEL_WIDTH: Pixels = px(340.);
    /// Volume slider width.
    pub const VOLUME_WIDTH: Pixels = px(112.);
    /// Edge padding of bars and panels.
    pub const EDGE: Pixels = px(12.);
}

/// Contrast ratio between two colors (WCAG 2.x relative luminance).
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    fn luminance(color: Hsla) -> f32 {
        let rgba = color.to_rgb();
        let channel = |c: f32| {
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgba.r) + 0.7152 * channel(rgba.g) + 0.0722 * channel(rgba.b)
    }
    let (la, lb) = (luminance(a), luminance(b));
    let (light, dark) = if la > lb { (la, lb) } else { (lb, la) };
    (light + 0.05) / (dark + 0.05)
}

/// Remove hue and saturation, keeping lightness and alpha.
pub fn desaturate(color: Hsla) -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        ..color
    }
}

/// Install the monochrome theme into GPUI Kit. Call once after `gpui_kit::init`.
pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let t = *tokens();
    Theme::update(cx, |theme| {
        // 1. Neutralize every color the component library defines.
        if let Ok(serde_json::Value::Object(mut map)) = serde_json::to_value(&theme.colors) {
            for value in map.values_mut() {
                if let Ok(color) = serde_json::from_value::<Hsla>(value.clone())
                    && let Ok(gray) = serde_json::to_value(desaturate(color))
                {
                    *value = gray;
                }
            }
            if let Ok(colors) = serde_json::from_value(serde_json::Value::Object(map)) {
                theme.colors = colors;
            }
        }

        // 2. Map the semantic tokens onto the component roles we use.
        let c = &mut theme.colors;
        c.background = t.background;
        c.foreground = t.foreground;
        c.border = t.border;
        c.input = t.border;
        c.ring = t.focus_ring;
        c.muted = t.surface_hover;
        c.muted_foreground = t.foreground_muted;
        c.accent = t.surface_hover;
        c.accent_foreground = t.foreground;
        c.selection = t.track_background;
        c.caret = t.foreground;

        c.primary = t.foreground;
        c.primary_foreground = t.background;
        c.primary_hover = rgb(0xE6E6E6).into();
        c.primary_active = rgb(0xCCCCCC).into();
        c.secondary = t.surface;
        c.secondary_foreground = t.foreground;
        c.secondary_hover = t.surface_hover;
        c.secondary_active = t.surface_active;

        c.button = t.surface;
        c.button_foreground = t.foreground;
        c.button_hover = t.surface_hover;
        c.button_active = t.surface_active;
        c.button_primary = t.foreground;
        c.button_primary_foreground = t.background;
        c.button_primary_hover = rgb(0xE6E6E6).into();
        c.button_primary_active = rgb(0xCCCCCC).into();
        c.button_secondary = t.surface;
        c.button_secondary_foreground = t.foreground;
        c.button_secondary_hover = t.surface_hover;
        c.button_secondary_active = t.surface_active;

        c.slider_bar = t.track_foreground;
        c.slider_thumb = t.track_foreground;
        c.progress_bar = t.track_foreground;
        // Off: dark thumb on gray. On: dark thumb on white. The thumb position carries the
        // state; the fill change is secondary.
        c.switch = t.track_background;
        c.switch_thumb = t.background;

        c.popover = t.surface;
        c.popover_foreground = t.foreground;
        c.list = t.background;
        c.list_hover = t.surface_hover;
        c.list_active = t.surface_active;
        c.list_active_border = t.foreground;
        c.scrollbar = t.background;
        c.scrollbar_thumb = t.track_background;
        c.scrollbar_thumb_hover = t.foreground_disabled;
        c.title_bar = t.surface;
        c.title_bar_border = t.border;
        c.drag_border = t.foreground;
        c.drop_target = t.surface_active;
        c.overlay = Hsla {
            a: 0.6,
            ..t.background
        };

        theme.shadow = false;
        theme.focus_ring = true;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_monochrome() {
        for (name, color) in Tokens::dark().all() {
            let rgba = color.to_rgb();
            assert!(
                (rgba.r - rgba.g).abs() < 0.002 && (rgba.g - rgba.b).abs() < 0.002,
                "{name} is not neutral"
            );
        }
    }

    #[test]
    fn text_tokens_meet_contrast() {
        let t = Tokens::dark();
        // WCAG AA for text (4.5:1) on every surface text appears on.
        for surface in [t.background, t.surface, t.surface_hover, t.surface_active] {
            assert!(contrast_ratio(t.foreground, surface) >= 4.5);
            assert!(contrast_ratio(t.foreground_muted, surface) >= 4.5);
        }
        // Disabled text is exempt from AA but must stay legible on panels.
        assert!(contrast_ratio(t.foreground_disabled, t.surface) >= 3.0);
        // Focus ring and played track: non-text 3:1 against their background.
        assert!(contrast_ratio(t.focus_ring, t.surface) >= 3.0);
        assert!(contrast_ratio(t.track_foreground, t.track_background) >= 3.0);
    }

    #[test]
    fn hit_targets_meet_minimums() {
        assert!(metrics::HIT_TARGET >= px(44.));
        assert!(metrics::PRIMARY_TARGET >= px(48.));
        assert!(metrics::ROW_HEIGHT >= metrics::HIT_TARGET);
        assert!(metrics::TARGET_GAP >= px(8.));
    }

    #[test]
    fn desaturate_keeps_lightness() {
        let red = Hsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let gray = desaturate(red);
        assert_eq!(gray.s, 0.0);
        assert_eq!(gray.l, 0.5);
    }
}
