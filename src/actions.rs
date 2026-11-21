//! Keyboard actions and their bindings: one canonical table drives both `bind_keys` and the
//! shortcuts list in the UI, so help text can never drift from behavior.

use gpui_kit::{Action, App, KeyBinding, actions};

actions!(
    mono_player,
    [
        TogglePlayback,
        Stop,
        OpenFiles,
        ToggleFullscreen,
        ExitFullscreen,
        SeekBackward,
        SeekForward,
        SeekBackwardLarge,
        SeekForwardLarge,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        PreviousItem,
        NextItem,
        TogglePlaylist,
        ShowTracks,
        ShowSettings,
        SpeedUp,
        SpeedDown,
        Quit,
    ]
);

/// The key context of the player root.
pub const PLAYER_CONTEXT: &str = "MonoPlayer";

/// Bindings that must not fire while a text field or other key-consuming control is focused.
/// GPUI Kit's text input uses the `Input` context.
const PLAYER_ONLY: &str = "MonoPlayer && !Input";

/// One documented shortcut.
pub struct Shortcut {
    /// GPUI keystroke syntax; `secondary` is Cmd on macOS and Ctrl elsewhere.
    pub keys: &'static str,
    /// What the user sees in the shortcuts list.
    pub display: &'static str,
    pub description: &'static str,
    action: fn() -> Box<dyn Action>,
    binding: fn(&'static str, &'static str) -> KeyBinding,
    context: &'static str,
}

impl Shortcut {
    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }

    fn key_binding(&self) -> KeyBinding {
        (self.binding)(self.keys, self.context)
    }
}

macro_rules! shortcut {
    ($keys:literal, $display:literal, $description:literal, $action:ident, $context:expr) => {
        Shortcut {
            keys: $keys,
            display: $display,
            description: $description,
            action: || Box::new($action),
            binding: |keys, context| KeyBinding::new(keys, $action, Some(context)),
            context: $context,
        }
    };
}

/// The canonical shortcut table (PRD §10.5).
pub const SHORTCUTS: &[Shortcut] = &[
    shortcut!(
        "space",
        "Space",
        "Play / pause",
        TogglePlayback,
        PLAYER_ONLY
    ),
    shortcut!("s", "S", "Stop", Stop, PLAYER_ONLY),
    shortcut!(
        "secondary-o",
        "Ctrl/Cmd+O",
        "Open files",
        OpenFiles,
        PLAYER_CONTEXT
    ),
    shortcut!("f", "F", "Toggle fullscreen", ToggleFullscreen, PLAYER_ONLY),
    shortcut!(
        "escape",
        "Esc",
        "Exit fullscreen",
        ExitFullscreen,
        PLAYER_ONLY
    ),
    shortcut!("left", "←", "Seek back 5 s", SeekBackward, PLAYER_ONLY),
    shortcut!("right", "→", "Seek forward 5 s", SeekForward, PLAYER_ONLY),
    shortcut!(
        "shift-left",
        "Shift+←",
        "Seek back 30 s",
        SeekBackwardLarge,
        PLAYER_ONLY
    ),
    shortcut!(
        "shift-right",
        "Shift+→",
        "Seek forward 30 s",
        SeekForwardLarge,
        PLAYER_ONLY
    ),
    shortcut!("up", "↑", "Volume up", VolumeUp, PLAYER_ONLY),
    shortcut!("down", "↓", "Volume down", VolumeDown, PLAYER_ONLY),
    shortcut!("m", "M", "Mute / unmute", ToggleMute, PLAYER_ONLY),
    shortcut!("p", "P", "Previous item", PreviousItem, PLAYER_ONLY),
    shortcut!("n", "N", "Next item", NextItem, PLAYER_ONLY),
    shortcut!(
        "l",
        "L",
        "Show / hide playlist",
        TogglePlaylist,
        PLAYER_ONLY
    ),
    shortcut!(
        "t",
        "T",
        "Audio, subtitles & speed",
        ShowTracks,
        PLAYER_ONLY
    ),
    shortcut!("]", "]", "Faster", SpeedUp, PLAYER_ONLY),
    shortcut!("[", "[", "Slower", SpeedDown, PLAYER_ONLY),
    shortcut!(
        "secondary-,",
        "Ctrl/Cmd+,",
        "Settings",
        ShowSettings,
        PLAYER_CONTEXT
    ),
    shortcut!("secondary-q", "Ctrl/Cmd+Q", "Quit", Quit, PLAYER_CONTEXT),
];

/// Register every binding from [`SHORTCUTS`].
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(SHORTCUTS.iter().map(Shortcut::key_binding));
}
