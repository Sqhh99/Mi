//! LRC lyrics: encoding detection and parsing.

use std::sync::LazyLock;

use encoding_rs::{Encoding, GB18030, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE};
use regex::Regex;

static TIME_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d+):(\d+)(?:\.(\d+))?\]").expect("valid regex"));
static META_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(ti|ar|al|id|offset):([^\]]*)\]").expect("valid regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
    /// Further lines with the same timestamp, usually a translation or romanization.
    pub translation: Option<String>,
}

impl LyricLine {
    pub fn new(time_ms: u64, text: impl Into<String>) -> Self {
        Self { time_ms, text: text.into(), translation: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lyrics {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// The file's `[offset:]` in milliseconds (already applied to the line times).
    pub offset_ms: i64,
    pub lines: Vec<LyricLine>,
}

impl Lyrics {
    /// Decodes raw `.lrc` bytes (guessing the text encoding) and parses them.
    pub fn from_bytes(data: &[u8]) -> Self {
        Self::parse(&decode(data))
    }

    pub fn parse(content: &str) -> Self {
        let mut lyrics = Lyrics::default();
        for line in content.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty()) {
            lyrics.parse_line(line);
        }
        // A positive offset makes the lyrics appear sooner.
        if lyrics.offset_ms != 0 {
            for line in &mut lyrics.lines {
                line.time_ms = (line.time_ms as i64 - lyrics.offset_ms).max(0) as u64;
            }
        }
        // Stable sort keeps file order for lines sharing a timestamp.
        lyrics.lines.sort_by_key(|line| line.time_ms);
        lyrics.group_simultaneous_lines();
        lyrics
    }

    /// Folds lines that share a timestamp into the first one, as its translation.
    fn group_simultaneous_lines(&mut self) {
        let mut grouped: Vec<LyricLine> = Vec::with_capacity(self.lines.len());
        for line in std::mem::take(&mut self.lines) {
            match grouped.last_mut() {
                Some(previous) if previous.time_ms == line.time_ms => {
                    if previous.text == line.text || previous.translation.as_deref() == Some(&line.text) {
                        continue;
                    }
                    match &mut previous.translation {
                        Some(translation) => {
                            translation.push('\n');
                            translation.push_str(&line.text);
                        }
                        None => previous.translation = Some(line.text),
                    }
                }
                _ => grouped.push(line),
            }
        }
        self.lines = grouped;
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Index of the last line whose timestamp is at or before `position_ms`.
    pub fn index_at(&self, position_ms: u64) -> Option<usize> {
        self.lines.partition_point(|line| line.time_ms <= position_ms).checked_sub(1)
    }

    fn parse_line(&mut self, line: &str) {
        if let Some(meta) = META_TAG.captures(line) {
            let value = meta[2].trim().to_string();
            match &meta[1] {
                "ti" => self.title = Some(value),
                "ar" => self.artist = Some(value),
                "al" => self.album = Some(value),
                "offset" => self.offset_ms = value.trim_start_matches('+').parse().unwrap_or(0),
                _ => {}
            }
            return;
        }

        let mut timestamps = Vec::new();
        let mut text_start = 0;
        for caps in TIME_TAG.captures_iter(line) {
            if let Some(ms) = timestamp_ms(&caps) {
                timestamps.push(ms);
                text_start = caps.get(0).map_or(text_start, |m| m.end());
            }
        }

        let text = line[text_start..].trim();
        if timestamps.is_empty() || text.is_empty() {
            return;
        }
        // A line may carry several timestamps ("[00:10][01:20]chorus").
        self.lines.extend(timestamps.into_iter().map(|time_ms| LyricLine::new(time_ms, text)));
    }
}

fn timestamp_ms(caps: &regex::Captures<'_>) -> Option<u64> {
    let minutes: u64 = caps[1].parse().ok()?;
    let seconds: u64 = caps[2].parse().ok()?;
    let fraction = caps.get(3).map_or(0, |m| {
        let value: u64 = m.as_str().parse().unwrap_or(0);
        match m.as_str().len() {
            1 => value * 100,
            2 => value * 10,
            3 => value,
            _ => 0,
        }
    });
    Some((minutes * 60 + seconds) * 1000 + fraction)
}

/// Decodes lyrics bytes, picking the encoding that yields the most plausible text.
///
/// A byte-order mark wins outright. Otherwise each candidate is scored by how many LRC
/// timestamps it produces, minus penalties for replacement characters, NULs and mojibake.
pub fn decode(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    if let Some((encoding, bom_len)) = Encoding::for_bom(data) {
        let (text, _) = encoding.decode_without_bom_handling(&data[bom_len..]);
        return text.into_owned();
    }

    const CANDIDATES: [&Encoding; 5] = [UTF_8, UTF_16LE, UTF_16BE, SHIFT_JIS, GB18030];
    let mut best: Option<(i64, String)> = None;
    for encoding in CANDIDATES {
        let (text, _) = encoding.decode_without_bom_handling(data);
        let score = score(&text);
        // Strict comparison: earlier candidates win ties (UTF-8 first).
        if best.as_ref().is_none_or(|(best_score, _)| score > *best_score) {
            best = Some((score, text.into_owned()));
        }
    }
    best.map(|(_, text)| text).unwrap_or_default()
}

fn score(text: &str) -> i64 {
    let timestamps = TIME_TAG.find_iter(text).count() as i64;
    let mut replacements = 0i64;
    let mut nulls = 0i64;
    let mut mojibake = 0i64;
    for ch in text.chars() {
        match ch as u32 {
            0xFFFD => replacements += 1,
            0 => nulls += 1,
            0x80..=0x9F | 0xE000..=0xF8FF => mojibake += 1,
            _ => {}
        }
    }
    timestamps * 100 - replacements * 60 - nulls * 40 - mojibake * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_and_lines() {
        let lyrics =
            Lyrics::parse("[ti:Song]\n[ar: Artist ]\n[al:Album]\n[id:abc]\n[00:01.50]first\n[00:03]second\n");
        assert_eq!(lyrics.title.as_deref(), Some("Song"));
        assert_eq!(lyrics.artist.as_deref(), Some("Artist"));
        assert_eq!(lyrics.album.as_deref(), Some("Album"));
        assert_eq!(lyrics.lines, vec![LyricLine::new(1500, "first"), LyricLine::new(3000, "second"),]);
    }

    #[test]
    fn fraction_widths() {
        let lyrics = Lyrics::parse("[00:00.5]a\n[00:01.25]b\n[00:02.125]c\n[01:00.0000]d");
        let times: Vec<_> = lyrics.lines.iter().map(|l| l.time_ms).collect();
        assert_eq!(times, vec![500, 1250, 2125, 60000]);
    }

    #[test]
    fn multiple_timestamps_and_sorting() {
        let lyrics = Lyrics::parse("[00:30.00][00:10.00]chorus\n[00:20.00]verse\r\n");
        let lines: Vec<_> = lyrics.lines.iter().map(|l| (l.time_ms, l.text.as_str())).collect();
        assert_eq!(lines, vec![(10000, "chorus"), (20000, "verse"), (30000, "chorus")]);
    }

    #[test]
    fn skips_empty_and_untimed_lines() {
        let lyrics = Lyrics::parse("[00:01.00]\nplain text\n[00:02.00]  kept  ");
        assert_eq!(lyrics.lines, vec![LyricLine::new(2000, "kept")]);
    }

    #[test]
    fn applies_offset_tag() {
        let lyrics = Lyrics::parse("[offset:+500]\n[00:00.20]a\n[00:02.00]b");
        assert_eq!(lyrics.offset_ms, 500);
        let times: Vec<_> = lyrics.lines.iter().map(|l| l.time_ms).collect();
        assert_eq!(times, vec![0, 1500]);
        let later = Lyrics::parse("[offset:-250]\n[00:01.00]a");
        assert_eq!(later.lines[0].time_ms, 1250);
    }

    #[test]
    fn groups_translations_with_the_same_timestamp() {
        let lyrics = Lyrics::parse(
            "[00:01.00]I love you\n[00:01.00]我爱你\n[00:01.00]我爱你\n[00:03.00]solo\n[00:01.00]wo ai ni",
        );
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "I love you");
        assert_eq!(lyrics.lines[0].translation.as_deref(), Some("我爱你\nwo ai ni"));
        assert_eq!(lyrics.lines[1].translation, None);
    }

    #[test]
    fn index_at_boundaries() {
        let lyrics = Lyrics::parse("[00:01.00]a\n[00:02.00]b\n[00:03.00]c");
        assert_eq!(lyrics.index_at(0), None);
        assert_eq!(lyrics.index_at(999), None);
        assert_eq!(lyrics.index_at(1000), Some(0));
        assert_eq!(lyrics.index_at(2500), Some(1));
        assert_eq!(lyrics.index_at(99_000), Some(2));
        assert_eq!(Lyrics::default().index_at(1000), None);
    }

    const SAMPLE: &str = "[00:01.00]稻香\n[00:02.00]对这个世界如果你有太多的抱怨\n";

    #[test]
    fn decodes_utf8() {
        assert_eq!(decode(SAMPLE.as_bytes()), SAMPLE);
    }

    #[test]
    fn decodes_utf16_with_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(SAMPLE.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode(&bytes), SAMPLE);
    }

    #[test]
    fn decodes_utf16_without_bom() {
        let le: Vec<u8> = SAMPLE.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = SAMPLE.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(decode(&le), SAMPLE);
        assert_eq!(decode(&be), SAMPLE);
    }

    #[test]
    fn decodes_gb18030() {
        let (bytes, _, _) = GB18030.encode(SAMPLE);
        assert_eq!(decode(&bytes), SAMPLE);
    }

    #[test]
    fn decodes_shift_jis() {
        let text = "[00:01.00]こんにちは世界\n[00:02.00]ありがとう\n";
        let (bytes, _, _) = SHIFT_JIS.encode(text);
        assert_eq!(decode(&bytes), text);
    }

    #[test]
    fn parses_bundled_examples() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut parsed = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "lrc") {
                let lyrics = Lyrics::from_bytes(&std::fs::read(&path).unwrap());
                assert!(!lyrics.is_empty(), "{} produced no lines", path.display());
                assert!(lyrics.lines.iter().all(|l| !l.text.contains('\u{FFFD}')));
                parsed += 1;
            }
        }
        assert!(parsed > 0);
    }
}
