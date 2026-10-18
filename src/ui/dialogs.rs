//! Native file pickers.
//!
//! Each dialog runs modally on the UI thread (as GPUI's own file prompt does), from a task
//! that doesn't hold the app borrowed, so its message loop keeps the window alive.

use std::path::{Path, PathBuf};

use gpui::{App, Task};
use player_core::i18n::Key;
use player_core::media_paths::AUDIO_EXTENSIONS;

use crate::platform::NativeWindow;
use crate::ui::t;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Several audio files.
    AudioFiles,
    /// One audio file (locating a moved track).
    AudioFile,
    /// One or more folders.
    Folders,
    /// One `.lrc` file.
    Lyrics,
}

/// Shows a picker and resolves to the chosen paths (empty if cancelled).
pub fn pick(
    kind: Pick,
    start_dir: Option<&Path>,
    native: Option<NativeWindow>,
    cx: &mut App,
) -> Task<Vec<PathBuf>> {
    let title = t(
        cx,
        match kind {
            Pick::AudioFiles => Key::OpenFiles,
            Pick::AudioFile => Key::LocateFile,
            Pick::Folders => Key::OpenFolder,
            Pick::Lyrics => Key::ChooseLyricsFile,
        },
    );
    let mut dialog = rfd::FileDialog::new().set_title(title);
    dialog = match kind {
        Pick::AudioFiles | Pick::AudioFile => dialog.add_filter(t(cx, Key::AudioFiles), AUDIO_EXTENSIONS),
        Pick::Lyrics => dialog.add_filter(t(cx, Key::LyricsFiles), &["lrc"]),
        Pick::Folders => dialog,
    };
    // Never pass GPUI's `Window` here; see `NativeWindow::dialog_parent`.
    if let Some(native) = native {
        dialog = dialog.set_parent(&native.dialog_parent());
    }
    let start = start_dir
        .filter(|dir| dir.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| directories::UserDirs::new().and_then(|dirs| dirs.audio_dir().map(Into::into)));
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    cx.spawn(async move |_| {
        match kind {
            Pick::AudioFiles => dialog.pick_files(),
            Pick::AudioFile | Pick::Lyrics => dialog.pick_file().map(|file| vec![file]),
            Pick::Folders => dialog.pick_folders(),
        }
        .unwrap_or_default()
    })
}
