//! Tag information about one audio file, shared by the queue and the music library.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::media_paths;

/// ReplayGain values from a track's tags. Gains are in dB, peaks are linear (1.0 = full scale).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainMode {
    #[default]
    Off,
    Track,
    Album,
}

impl ReplayGain {
    /// Parses tag values such as "-6.54 dB" or "0.988".
    pub fn parse_value(text: &str) -> Option<f32> {
        let number = text.trim().trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace());
        number.trim().parse::<f32>().ok().filter(|value| value.is_finite())
    }

    /// Linear gain to apply for `mode`, with `preamp_db` added and the result limited so the
    /// track's peak doesn't clip. Album mode falls back to track values and vice versa.
    pub fn factor(&self, mode: ReplayGainMode, preamp_db: f32) -> f32 {
        let (gain, peak) = match mode {
            ReplayGainMode::Off => return 1.0,
            ReplayGainMode::Track => {
                (self.track_gain.or(self.album_gain), self.track_peak.or(self.album_peak))
            }
            ReplayGainMode::Album => {
                (self.album_gain.or(self.track_gain), self.album_peak.or(self.track_peak))
            }
        };
        let Some(gain) = gain else { return 1.0 };
        let factor = 10f32.powf((gain + preamp_db) / 20.0);
        match peak.filter(|peak| *peak > 0.0) {
            Some(peak) => factor.min(1.0 / peak),
            None => factor,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: Option<u64>,
    pub replay_gain: ReplayGain,
}

impl TrackTags {
    /// The title tag, or the file name without its extension.
    pub fn display_title(&self, path: &Path) -> String {
        self.title.clone().unwrap_or_else(|| media_paths::display_stem(path))
    }

    /// The artist used to group albums: album artist first, then track artist.
    pub fn grouping_artist(&self) -> Option<&str> {
        self.album_artist.as_deref().or(self.artist.as_deref())
    }

    /// Whether every word of `query` (already lowercased) appears in the title, artist,
    /// album or file name.
    pub fn matches(&self, path: &Path, query: &[String]) -> bool {
        if query.is_empty() {
            return true;
        }
        let haystack = [
            self.title.as_deref(),
            self.artist.as_deref(),
            self.album.as_deref(),
            self.album_artist.as_deref(),
            path.file_name().and_then(|name| name.to_str()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
        query.iter().all(|word| haystack.contains(word.as_str()))
    }
}

/// Splits a search box's text into lowercase words.
pub fn search_terms(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replay_gain_values() {
        assert_eq!(ReplayGain::parse_value("-6.54 dB"), Some(-6.54));
        assert_eq!(ReplayGain::parse_value(" +2.1dB "), Some(2.1));
        assert_eq!(ReplayGain::parse_value("0.988"), Some(0.988));
        assert_eq!(ReplayGain::parse_value("loud"), None);
    }

    #[test]
    fn replay_gain_factor_and_peak_limit() {
        let gain = ReplayGain {
            track_gain: Some(-6.0),
            track_peak: Some(0.5),
            album_gain: Some(6.0),
            album_peak: Some(0.9),
        };
        assert_eq!(gain.factor(ReplayGainMode::Off, 0.0), 1.0);
        assert!((gain.factor(ReplayGainMode::Track, 0.0) - 0.501).abs() < 0.01);
        // +6 dB would be ~2.0, but the 0.9 peak limits it to 1/0.9.
        assert!((gain.factor(ReplayGainMode::Album, 0.0) - 1.0 / 0.9).abs() < 0.001);
        // Album mode falls back to the track gain.
        let track_only = ReplayGain { track_gain: Some(-6.0), ..Default::default() };
        assert!((track_only.factor(ReplayGainMode::Album, 0.0) - 0.501).abs() < 0.01);
        assert_eq!(ReplayGain::default().factor(ReplayGainMode::Track, 3.0), 1.0);
    }

    #[test]
    fn search_matches_all_words_anywhere() {
        let tags = TrackTags {
            title: Some("稻香".into()),
            artist: Some("Jay Chou".into()),
            album: Some("魔杰座".into()),
            ..Default::default()
        };
        let path = Path::new("music/daoxiang.flac");
        assert!(tags.matches(path, &search_terms("jay 稻香")));
        assert!(tags.matches(path, &search_terms("DAOXIANG")));
        assert!(tags.matches(path, &[]));
        assert!(!tags.matches(path, &search_terms("jay blue")));
        assert_eq!(tags.display_title(path), "稻香");
        assert_eq!(TrackTags::default().display_title(path), "daoxiang");
    }
}
