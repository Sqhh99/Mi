//! File-system conventions for audio files and their sidecar cover art and lyrics.

use std::path::{Path, PathBuf};

/// Extensions the audio engine can decode, offered in the open-file dialog.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "oga"];

const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
/// Folder-wide cover images, in order of preference (matched case-insensitively).
const FOLDER_COVER_STEMS: &[&str] = &["cover", "folder", "front", "album", "albumart"];
/// Subfolders that may hold `song.lrc` instead of the track's own folder.
const LYRICS_DIRS: &[&str] = &["lyrics", "Lyrics", "LRC", "lrc"];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext)))
}

pub fn is_lyrics_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("lrc"))
}

/// `song.jpg` / `song.png` / ... next to `song.mp3`, else a folder image such as
/// `cover.jpg` or `folder.png` in the same directory.
pub fn sidecar_cover(audio: &Path) -> Option<PathBuf> {
    COVER_EXTENSIONS
        .iter()
        .map(|ext| audio.with_extension(ext))
        .find(|candidate| candidate.is_file())
        .or_else(|| folder_cover(audio.parent()?))
}

/// The folder-wide cover image in `dir` (`cover.jpg`, `folder.png`, ...), if any.
pub fn folder_cover(dir: &Path) -> Option<PathBuf> {
    let images: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| COVER_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext)))
        })
        .collect();
    FOLDER_COVER_STEMS.iter().find_map(|stem| {
        images
            .iter()
            .find(|path| {
                path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case(stem))
            })
            .cloned()
    })
}

/// `song.lrc` next to `song.mp3`, else in a `lyrics/` subfolder.
pub fn sidecar_lyrics(audio: &Path) -> Option<PathBuf> {
    let beside = audio.with_extension("lrc");
    if beside.is_file() {
        return Some(beside);
    }
    let (dir, name) = (audio.parent()?, beside.file_name()?);
    LYRICS_DIRS.iter().map(|sub| dir.join(sub).join(name)).find(|candidate| candidate.is_file())
}

/// File name shown when a track has no title tag.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// File name without its extension, used as a title when a track has no title tag.
pub fn display_stem(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| display_name(path))
}

/// Name of the folder holding `path`, for subtitles when a track has no artist tag.
pub fn folder_name(path: &Path) -> Option<String> {
    path.parent()?.file_name().map(|name| name.to_string_lossy().into_owned())
}

/// Expands dropped or picked paths into audio files: files are kept if they are audio,
/// folders are scanned recursively (sorted by path within each folder).
pub fn collect_audio_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            collect_dir(path, &mut files, 0);
        } else if is_audio_file(path) {
            files.push(path.clone());
        }
    }
    files
}

/// Deep enough for any real music collection, and stops symlink loops.
const MAX_SCAN_DEPTH: usize = 32;

fn collect_dir(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> =
        entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    entries.sort_by_key(|path| path.to_string_lossy().to_lowercase());
    for path in entries {
        if path.is_dir() {
            collect_dir(&path, files, depth + 1);
        } else if is_audio_file(&path) {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_audio_extensions() {
        assert!(is_audio_file(Path::new("a/b.MP3")));
        assert!(is_audio_file(Path::new("song.flac")));
        assert!(!is_audio_file(Path::new("cover.jpg")));
        assert!(!is_audio_file(Path::new("noext")));
    }

    #[test]
    fn finds_bundled_sidecars() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let audio = examples.join("Bittersweet Chocolate.m4a");
        assert_eq!(sidecar_cover(&audio), Some(examples.join("Bittersweet Chocolate.jpg")));
        assert_eq!(sidecar_lyrics(&audio), Some(examples.join("Bittersweet Chocolate.lrc")));
        assert_eq!(sidecar_cover(&examples.join("missing.mp3")), None);
        assert_eq!(display_name(&audio), "Bittersweet Chocolate.m4a");
        assert_eq!(display_stem(&audio), "Bittersweet Chocolate");
        assert_eq!(folder_name(&audio).as_deref(), Some("examples"));
    }

    #[test]
    fn folder_covers_lyrics_dirs_and_recursive_collection() {
        let dir = std::env::temp_dir().join(format!("mi-media-paths-{}", std::process::id()));
        let album = dir.join("Album");
        std::fs::create_dir_all(album.join("Disc 2")).unwrap();
        std::fs::create_dir_all(album.join("lyrics")).unwrap();
        for file in ["b.mp3", "A.flac", "notes.txt", "Folder.JPG", "Disc 2/c.ogg"] {
            std::fs::write(album.join(file), b"").unwrap();
        }
        std::fs::write(album.join("lyrics/b.lrc"), b"").unwrap();

        assert_eq!(sidecar_cover(&album.join("b.mp3")), Some(album.join("Folder.JPG")));
        assert_eq!(folder_cover(&album.join("Disc 2")), None);
        assert_eq!(sidecar_lyrics(&album.join("b.mp3")), Some(album.join("lyrics").join("b.lrc")));
        assert!(is_lyrics_file(Path::new("x.LRC")));

        let files = collect_audio_files(&[album.clone(), album.join("b.mp3"), album.join("notes.txt")]);
        assert_eq!(
            files,
            vec![
                album.join("A.flac"),
                album.join("b.mp3"),
                album.join("Disc 2").join("c.ogg"),
                album.join("b.mp3")
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
