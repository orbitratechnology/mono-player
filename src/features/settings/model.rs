//! Preference toggles exposed in the settings panel.

use crate::persistence::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    RememberRecents,
    ResumePlayback,
}

impl Toggle {
    pub fn label(self) -> &'static str {
        match self {
            Self::RememberRecents => "Remember recent media",
            Self::ResumePlayback => "Resume where playback stopped",
        }
    }

    pub fn value(self, settings: &Settings) -> bool {
        match self {
            Self::RememberRecents => settings.remember_recents,
            Self::ResumePlayback => settings.resume_playback,
        }
    }

    /// Apply a toggle. Turning a history feature off also discards what it stored.
    pub fn apply(self, settings: &mut Settings, on: bool) {
        match self {
            Self::RememberRecents => {
                settings.remember_recents = on;
                if !on {
                    settings.clear_history();
                }
            }
            Self::ResumePlayback => {
                settings.resume_playback = on;
                if !on {
                    for recent in &mut settings.recents {
                        recent.position_ms = None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_history_clears_it() {
        let mut settings = Settings::default();
        settings.touch_recent("/m/a", false);
        settings.record_position("/m/a", 60_000, None);
        Toggle::ResumePlayback.apply(&mut settings, false);
        assert_eq!(settings.recents[0].position_ms, None);
        Toggle::RememberRecents.apply(&mut settings, false);
        assert!(settings.recents.is_empty());
        assert!(!Toggle::RememberRecents.value(&settings));
    }
}
