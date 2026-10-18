//! Persistent user settings, stored as JSON.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::eq::EqLevels;
use crate::i18n::Lang;
use crate::queue::RepeatMode;
use crate::track::ReplayGainMode;

/// How many recently played tracks are remembered.
pub const RECENT_LIMIT: usize = 200;
/// Lyrics size range, in percent of the default.
pub const LYRICS_SCALE_RANGE: (u8, u8) = (70, 180);
/// Limit of the ReplayGain pre-amplification, in dB either way.
pub const MAX_PREAMP_DB: i8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Day,
    Night,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: ThemeMode,
    /// Opacity of the window surface, 0..=100.
    pub material_strength: u8,
    /// Opacity of button backgrounds, 0..=100.
    pub button_material_strength: u8,
    pub background_glow: bool,
    /// The faint kitten peeking from the window's bottom-right corner.
    pub show_kitten: bool,
    /// Turns off purely decorative motion (spectrum bars, shimmer, island wave).
    pub reduce_motion: bool,
    pub language: Lang,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Day,
            material_strength: 72,
            button_material_strength: 62,
            background_glow: true,
            show_kitten: true,
            reduce_motion: false,
            language: Lang::Zh,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    pub repeat: RepeatMode,
    pub shuffle: bool,
    /// Keep playing on launch if music was playing when the app closed (otherwise the last
    /// track is restored paused).
    pub resume_playing: bool,
    pub eq_enabled: bool,
    pub replay_gain: ReplayGainMode,
    /// Added to ReplayGain adjustments, in dB.
    pub replay_gain_preamp: i8,
    /// Start the next track without a gap when it has the same audio format.
    pub gapless: bool,
    /// Output device name; `None` follows the system default.
    pub output_device: Option<String>,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            repeat: RepeatMode::All,
            shuffle: false,
            resume_playing: false,
            eq_enabled: true,
            replay_gain: ReplayGainMode::Off,
            replay_gain_preamp: 0,
            gapless: true,
            output_device: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedWindowMode {
    #[default]
    Full,
    Mini,
    Island,
}

/// Where the window was: its top-left corner in logical pixels on the virtual desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub mode: SavedWindowMode,
    pub x: i32,
    pub y: i32,
    pub pinned: bool,
}

/// What was playing when the app last closed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Index into `Settings::playlist`.
    pub current: Option<usize>,
    pub position_ms: u64,
    pub was_playing: bool,
    pub window: Option<WindowPlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsSettings {
    /// Text size in percent of the default (see [`LYRICS_SCALE_RANGE`]).
    pub scale: u8,
    /// Per-track timing adjustments in milliseconds; positive shows lyrics earlier.
    pub offsets: BTreeMap<PathBuf, i32>,
    /// Lyrics files the user picked by hand for tracks without a matching `.lrc`.
    pub links: BTreeMap<PathBuf, PathBuf>,
}

impl Default for LyricsSettings {
    fn default() -> Self {
        Self { scale: 100, offsets: BTreeMap::new(), links: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    /// Folders scanned (recursively) for music.
    pub folders: Vec<PathBuf>,
    pub favorites: Vec<PathBuf>,
    /// Most recent first, at most [`RECENT_LIMIT`].
    pub recent: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The play queue.
    pub playlist: Vec<PathBuf>,
    /// Folder the open-file dialog starts in.
    pub last_dir: Option<PathBuf>,
    pub volume: u8,
    pub muted: bool,
    pub eq: EqLevels,
    pub ui: UiSettings,
    pub playback: PlaybackSettings,
    pub session: Session,
    pub lyrics: LyricsSettings,
    pub library: LibrarySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            playlist: Vec::new(),
            last_dir: None,
            volume: 80,
            muted: false,
            eq: EqLevels::default(),
            ui: UiSettings::default(),
            playback: PlaybackSettings::default(),
            session: Session::default(),
            lyrics: LyricsSettings::default(),
            library: LibrarySettings::default(),
        }
    }
}

impl Settings {
    /// Reads settings from `path`. A missing file yields the defaults.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                Ok(serde_json::from_slice::<Settings>(&bytes).map_err(io::Error::other)?.sanitized())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    /// Writes settings atomically (temp file + rename), creating the directory if needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }

    pub fn sanitized(mut self) -> Self {
        self.volume = self.volume.min(100);
        self.eq = self.eq.clamped();
        self.ui.material_strength = self.ui.material_strength.min(100);
        self.ui.button_material_strength = self.ui.button_material_strength.min(100);
        self.playback.replay_gain_preamp =
            self.playback.replay_gain_preamp.clamp(-MAX_PREAMP_DB, MAX_PREAMP_DB);
        self.lyrics.scale = self.lyrics.scale.clamp(LYRICS_SCALE_RANGE.0, LYRICS_SCALE_RANGE.1);
        self.lyrics.offsets.retain(|_, offset| *offset != 0);
        if self.session.current.is_some_and(|current| current >= self.playlist.len()) {
            self.session.current = None;
            self.session.position_ms = 0;
        }
        self.library.recent.truncate(RECENT_LIMIT);
        self
    }

    /// Moves `track` to the front of the recently played list.
    pub fn record_played(&mut self, track: &Path) {
        self.library.recent.retain(|recent| recent != track);
        self.library.recent.insert(0, track.to_path_buf());
        self.library.recent.truncate(RECENT_LIMIT);
    }

    pub fn is_favorite(&self, track: &Path) -> bool {
        self.library.favorites.iter().any(|favorite| favorite == track)
    }

    pub fn toggle_favorite(&mut self, track: &Path) {
        if self.is_favorite(track) {
            self.library.favorites.retain(|favorite| favorite != track);
        } else {
            self.library.favorites.push(track.to_path_buf());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_original_app() {
        let s = Settings::default();
        assert_eq!(s.volume, 80);
        assert_eq!(s.eq, EqLevels { bass: 50, mid: 50, treble: 50 });
        assert_eq!(s.ui.material_strength, 72);
        assert_eq!(s.ui.button_material_strength, 62);
        assert!(s.ui.background_glow);
        assert!(s.ui.show_kitten);
        assert!(!s.ui.reduce_motion);
        assert_eq!(s.playback.repeat, RepeatMode::All);
        assert!(s.playback.eq_enabled && s.playback.gapless && !s.playback.resume_playing);
        assert_eq!(s.lyrics.scale, 100);
        assert_eq!(s.ui.theme, ThemeMode::Day);
        assert_eq!(s.ui.language, Lang::Zh);
    }

    #[test]
    fn missing_and_unknown_fields_and_clamping() {
        let json = r#"{"volume":250,"ui":{"theme":"night","language":"en","future":1},"extra":true}"#;
        let s: Settings = serde_json::from_str::<Settings>(json).unwrap().sanitized();
        assert_eq!(s.volume, 100);
        assert_eq!(s.ui.theme, ThemeMode::Night);
        assert_eq!(s.ui.language, Lang::En);
        assert_eq!(s.ui.material_strength, 72);
        assert_eq!(s.eq, EqLevels::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("player-core-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        let mut s = Settings {
            playlist: vec![PathBuf::from("a.mp3"), PathBuf::from("目录/b.flac")],
            last_dir: Some(PathBuf::from("目录")),
            muted: true,
            ..Default::default()
        };
        s.eq.bass = 90;
        s.ui.background_glow = false;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);

        fs::write(&path, b"{not json").unwrap();
        assert!(Settings::load(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sanitizes_session_and_lyrics() {
        let mut s = Settings { playlist: vec![PathBuf::from("a.mp3")], ..Default::default() };
        s.session.current = Some(3);
        s.session.position_ms = 1000;
        s.lyrics.scale = 255;
        s.lyrics.offsets.insert(PathBuf::from("a.mp3"), 0);
        s.lyrics.offsets.insert(PathBuf::from("b.mp3"), -500);
        s.playback.replay_gain_preamp = 100;
        let s = s.sanitized();
        assert_eq!((s.session.current, s.session.position_ms), (None, 0));
        assert_eq!(s.lyrics.scale, LYRICS_SCALE_RANGE.1);
        assert_eq!(s.lyrics.offsets.len(), 1);
        assert_eq!(s.playback.replay_gain_preamp, MAX_PREAMP_DB);
    }

    #[test]
    fn recent_and_favorites() {
        let mut s = Settings::default();
        for i in 0..RECENT_LIMIT + 5 {
            s.record_played(Path::new(&format!("{i}.mp3")));
        }
        s.record_played(Path::new("3.mp3"));
        assert_eq!(s.library.recent.len(), RECENT_LIMIT);
        assert_eq!(s.library.recent[0], PathBuf::from("3.mp3"));
        assert_eq!(s.library.recent.iter().filter(|p| p.ends_with("3.mp3")).count(), 1);

        s.toggle_favorite(Path::new("a.mp3"));
        assert!(s.is_favorite(Path::new("a.mp3")));
        s.toggle_favorite(Path::new("a.mp3"));
        assert!(!s.is_favorite(Path::new("a.mp3")));
    }
}
