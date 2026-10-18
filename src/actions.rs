//! Keyboard shortcuts.

use gpui::{App, KeyBinding, actions};

/// Key context set on the main view; shortcuts only apply inside it.
pub const KEY_CONTEXT: &str = "Mi";

actions!(
    mi,
    [
        TogglePlay,
        Previous,
        Next,
        SeekBackward,
        SeekForward,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        TogglePlaylist,
        ToggleLibrary,
        ToggleLyrics,
        ToggleEq,
        CycleRepeat,
        ToggleShuffle,
        ToggleFavorite,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, context),
        KeyBinding::new("left", Previous, context),
        KeyBinding::new("right", Next, context),
        KeyBinding::new("shift-left", SeekBackward, context),
        KeyBinding::new("shift-right", SeekForward, context),
        KeyBinding::new("up", VolumeUp, context),
        KeyBinding::new("down", VolumeDown, context),
        KeyBinding::new("m", ToggleMute, context),
        KeyBinding::new("l", TogglePlaylist, context),
        KeyBinding::new("b", ToggleLibrary, context),
        KeyBinding::new("y", ToggleLyrics, context),
        KeyBinding::new("e", ToggleEq, context),
        KeyBinding::new("r", CycleRepeat, context),
        KeyBinding::new("s", ToggleShuffle, context),
        KeyBinding::new("f", ToggleFavorite, context),
    ]);
    crate::ui::widgets::text_input::bind_keys(cx);
    crate::ui::views::bind_keys(cx);
}
