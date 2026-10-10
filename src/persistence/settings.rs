//! The small, versioned, local settings file. No database; nothing leaves the machine.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::features::playlist::model::RepeatMode;
use crate::playback::state::{MAX_VOLUME, clamp_volume};

pub const CURRENT_VERSION: u32 = 1;
pub const MAX_RECENTS: usize = 20;
const FILE_NAME: &str = "settings.json";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowGeometry {
    /// Reject corrupt or absurd geometry rather than opening an invisible window.
    pub fn is_plausible(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && (320.0..=16_384.0).contains(&self.width)
            && (240.0..=16_384.0).contains(&self.height)
            && self.x.abs() < 32_768.0
            && self.y.abs() < 32_768.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentItem {
    /// A path or URL, as given by the user.
    pub location: String,
    #[serde(default)]
    pub is_url: bool,
    /// Last playback position, only stored when resume is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub volume: u8,
    pub muted: bool,
    pub repeat: RepeatMode,
    pub window: Option<WindowGeometry>,
    /// Keep a list of recently opened media.
    pub remember_recents: bool,
    /// Resume recently played files where they stopped.
    pub resume_playback: bool,
    pub recents: Vec<RecentItem>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            volume: 80,
            muted: false,
            repeat: RepeatMode::Off,
            window: None,
            remember_recents: true,
            resume_playback: true,
            recents: Vec::new(),
        }
    }
}

impl Settings {
    /// Parse settings JSON, migrating older versions and repairing out-of-range values.
    /// Anything unreadable falls back to defaults: a bad settings file never blocks startup.
    pub fn from_json(text: &str) -> Self {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::default();
        };
        migrate(&mut value);
        let mut settings: Self = serde_json::from_value(value).unwrap_or_default();
        settings.sanitize();
        settings
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    fn sanitize(&mut self) {
        self.version = CURRENT_VERSION;
        self.volume = clamp_volume(i32::from(self.volume)).min(MAX_VOLUME);
        if self.window.is_some_and(|w| !w.is_plausible()) {
            self.window = None;
        }
        if !self.remember_recents {
            self.recents.clear();
        }
        self.recents.retain(|r| !r.location.trim().is_empty());
        self.recents.truncate(MAX_RECENTS);
        if !self.resume_playback {
            for recent in &mut self.recents {
                recent.position_ms = None;
            }
        }
    }

    /// Record an opened item at the top of recents (no-op when recents are disabled).
    pub fn touch_recent(&mut self, location: &str, is_url: bool) {
        if !self.remember_recents {
            return;
        }
        let previous = self
            .recents
            .iter()
            .position(|r| r.location == location)
            .map(|index| self.recents.remove(index));
        let item = previous.unwrap_or(RecentItem {
            location: location.to_owned(),
            is_url,
            position_ms: None,
            length_ms: None,
        });
        self.recents.insert(0, item);
        self.recents.truncate(MAX_RECENTS);
    }

    /// Remember where playback stopped. Positions near the start or end are cleared so finished
    /// media restarts from the beginning.
    pub fn record_position(&mut self, location: &str, position_ms: i64, length_ms: Option<i64>) {
        if !self.remember_recents || !self.resume_playback {
            return;
        }
        let Some(recent) = self.recents.iter_mut().find(|r| r.location == location) else {
            return;
        };
        let near_end = length_ms.is_some_and(|len| len > 0 && len - position_ms < 15_000);
        recent.length_ms = length_ms;
        recent.position_ms = (position_ms > 10_000 && !near_end).then_some(position_ms);
    }

    /// The resume position for a location, if resuming is enabled.
    pub fn resume_position(&self, location: &str) -> Option<i64> {
        if !self.resume_playback {
            return None;
        }
        self.recents
            .iter()
            .find(|r| r.location == location)
            .and_then(|r| r.position_ms)
    }

    pub fn clear_history(&mut self) {
        self.recents.clear();
    }
}

/// Upgrade older documents in place. Version 0 (pre-release) had no `version` field and stored
/// volume as a 0.0–1.0 float under `volume_fraction`.
fn migrate(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let version = object.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    if version == 0 {
        if let Some(fraction) = object.remove("volume_fraction").and_then(|v| v.as_f64()) {
            let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u64;
            object.insert("volume".into(), percent.into());
        }
        object.insert("version".into(), CURRENT_VERSION.into());
    }
}

/// Where settings live: `$MONO_PLAYER_CONFIG_DIR`, else the platform config directory.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MONO_PLAYER_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Mono Player"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Mono Player")
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|dir| dir.join("mono-player"))
    }
}

pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(FILE_NAME))
}

/// Load settings. Missing or corrupt files yield defaults.
pub fn load(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(text) => Settings::from_json(&text),
        Err(_) => Settings::default(),
    }
}

/// Write settings atomically (temp file + rename) so a crash never leaves a torn file.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, settings.to_json())?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut settings = Settings {
            volume: 55,
            muted: true,
            repeat: RepeatMode::All,
            window: Some(WindowGeometry {
                x: 10.0,
                y: 20.0,
                width: 1280.0,
                height: 720.0,
            }),
            ..Default::default()
        };
        settings.touch_recent("/m/a.mkv", false);
        settings.record_position("/m/a.mkv", 60_000, Some(600_000));
        let parsed = Settings::from_json(&settings.to_json());
        assert_eq!(parsed, settings);
        assert_eq!(parsed.resume_position("/m/a.mkv"), Some(60_000));
    }

    #[test]
    fn migrates_version_zero() {
        let parsed = Settings::from_json(r#"{"volume_fraction": 0.42, "muted": true}"#);
        assert_eq!(parsed.version, CURRENT_VERSION);
        assert_eq!(parsed.volume, 42);
        assert!(parsed.muted);
    }

    #[test]
    fn repairs_bad_values() {
        let parsed = Settings::from_json(
            r#"{"version":1,"volume":250,"window":{"x":0,"y":0,"width":5,"height":5},
                "recents":[{"location":"  "},{"location":"/a"}]}"#,
        );
        assert_eq!(parsed.volume, MAX_VOLUME);
        assert_eq!(parsed.window, None);
        assert_eq!(parsed.recents.len(), 1);
        assert_eq!(Settings::from_json("not json"), Settings::default());
        assert_eq!(Settings::from_json("[]"), Settings::default());
    }

    #[test]
    fn recents_are_bounded_and_deduplicated() {
        let mut settings = Settings::default();
        for i in 0..30 {
            settings.touch_recent(&format!("/m/{i}"), false);
        }
        settings.touch_recent("/m/25", false);
        assert_eq!(settings.recents.len(), MAX_RECENTS);
        assert_eq!(settings.recents[0].location, "/m/25");
        assert_eq!(
            settings
                .recents
                .iter()
                .filter(|r| r.location == "/m/25")
                .count(),
            1
        );
    }

    #[test]
    fn privacy_switches_are_honored() {
        let mut settings = Settings {
            remember_recents: false,
            ..Default::default()
        };
        settings.touch_recent("/m/a", false);
        assert!(settings.recents.is_empty());

        let mut settings = Settings {
            resume_playback: false,
            ..Default::default()
        };
        settings.touch_recent("/m/a", false);
        settings.record_position("/m/a", 60_000, None);
        assert_eq!(settings.resume_position("/m/a"), None);
        settings.clear_history();
        assert!(settings.recents.is_empty());
    }

    #[test]
    fn positions_near_edges_are_not_resumed() {
        let mut settings = Settings::default();
        settings.touch_recent("/m/a", false);
        settings.record_position("/m/a", 5_000, Some(600_000));
        assert_eq!(settings.resume_position("/m/a"), None);
        settings.record_position("/m/a", 595_000, Some(600_000));
        assert_eq!(settings.resume_position("/m/a"), None);
    }

    #[test]
    fn saves_atomically() {
        let dir = std::env::temp_dir().join(format!("mono-player-settings-{}", std::process::id()));
        let path = dir.join("nested").join(FILE_NAME);
        let settings = Settings {
            volume: 33,
            ..Default::default()
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), settings);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(load(&path), Settings::default());
    }
}
