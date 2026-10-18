//! Track metadata: tags and embedded art via lofty, with sidecar-file fallbacks.
//!
//! Everything here is blocking file I/O and is meant to run on a background executor.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::ImageFormat;
use lofty::config::ParseOptions;
use lofty::file::TaggedFile;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use player_core::lyrics::Lyrics;
use player_core::media_paths;
use player_core::track::{ReplayGain, TrackTags};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Webp,
    Gif,
    Bmp,
}

impl ImageKind {
    /// Identifies an image by its magic bytes; tag MIME types are often wrong.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [0x89, b'P', b'N', b'G', ..] => Some(Self::Png),
            [0xFF, 0xD8, 0xFF, ..] => Some(Self::Jpeg),
            [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(Self::Webp),
            [b'G', b'I', b'F', b'8', ..] => Some(Self::Gif),
            [b'B', b'M', ..] => Some(Self::Bmp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cover {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,
}

impl Cover {
    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        ImageKind::sniff(&bytes).map(|kind| Self { kind, bytes })
    }

    pub fn into_image(self) -> Arc<gpui::Image> {
        let format = match self.kind {
            ImageKind::Png => ImageFormat::Png,
            ImageKind::Jpeg => ImageFormat::Jpeg,
            ImageKind::Webp => ImageFormat::Webp,
            ImageKind::Gif => ImageFormat::Gif,
            ImageKind::Bmp => ImageFormat::Bmp,
        };
        Arc::new(gpui::Image::from_bytes(format, self.bytes))
    }
}

/// Where a track's lyrics came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsSource {
    /// An `.lrc` file found next to the track or picked by the user.
    File(PathBuf),
    /// Synced lyrics stored in the track's tags.
    Embedded,
}

#[derive(Debug, Clone, Default)]
pub struct TrackDetails {
    pub tags: TrackTags,
    pub cover: Option<Cover>,
    pub lyrics: Lyrics,
    pub lyrics_source: Option<LyricsSource>,
}

/// Reads tags, cover art and lyrics for the track at `path`.
///
/// Cover art prefers the embedded front cover, then any embedded picture, then a sidecar
/// image. Lyrics come from `lyrics_link` (a file the user picked), a sidecar `.lrc` file,
/// or synced lyrics embedded in the tags, in that order.
pub fn read_details(path: &Path, lyrics_link: Option<&Path>) -> TrackDetails {
    let mut details = TrackDetails::default();
    let mut embedded_lyrics = None;

    match Probe::open(path).and_then(|probe| probe.read()) {
        Ok(file) => {
            details.tags = tags_of(&file);
            if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
                details.cover = best_picture(tag.pictures())
                    .and_then(|picture| Cover::from_bytes(picture.data().to_vec()));
                embedded_lyrics = tag
                    .get_string(ItemKey::Lyrics)
                    .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
                    .map(Lyrics::parse)
                    .filter(|lyrics| !lyrics.is_empty());
            }
        }
        Err(err) => log::debug!("no tags for {}: {err}", path.display()),
    }

    if details.cover.is_none() {
        details.cover = media_paths::sidecar_cover(path)
            .and_then(|cover| std::fs::read(cover).ok())
            .and_then(Cover::from_bytes);
    }

    let lrc = lyrics_link.filter(|link| link.is_file()).map(Path::to_path_buf);
    if let Some(lrc) = lrc.or_else(|| media_paths::sidecar_lyrics(path)) {
        match std::fs::read(&lrc) {
            Ok(bytes) => {
                details.lyrics = Lyrics::from_bytes(&bytes);
                details.lyrics_source = Some(LyricsSource::File(lrc));
            }
            Err(err) => log::warn!("cannot read {}: {err}", lrc.display()),
        }
    }
    if details.lyrics_source.is_none()
        && let Some(lyrics) = embedded_lyrics
    {
        details.lyrics = lyrics;
        details.lyrics_source = Some(LyricsSource::Embedded);
    }

    details
}

/// A small square JPEG of the track's cover (embedded, or the folder's image), for grids.
pub fn read_thumbnail(path: &Path, size: u32) -> Option<Cover> {
    let embedded = Probe::open(path).ok().and_then(|probe| probe.read().ok()).and_then(|file| {
        let tag = file.primary_tag().or_else(|| file.first_tag())?;
        best_picture(tag.pictures()).map(|picture| picture.data().to_vec())
    });
    let bytes = embedded.or_else(|| std::fs::read(media_paths::sidecar_cover(path)?).ok())?;
    let image = image::load_from_memory(&bytes).ok()?;
    let thumbnail = image.resize_to_fill(size, size, image::imageops::FilterType::Triangle).into_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85).encode_image(&thumbnail).ok()?;
    Some(Cover { kind: ImageKind::Jpeg, bytes: jpeg })
}

/// Reads tags and audio properties without decoding cover art.
pub fn read_tags(path: &Path) -> Option<TrackTags> {
    let options = ParseOptions::new().read_cover_art(false);
    let file = Probe::open(path).ok()?.options(options).read().ok()?;
    Some(tags_of(&file))
}

fn tags_of(file: &TaggedFile) -> TrackTags {
    let duration = file.properties().duration();
    let mut tags = TrackTags {
        duration_ms: (!duration.is_zero()).then_some(duration.as_millis() as u64),
        ..Default::default()
    };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return tags;
    };
    tags.title = non_empty(tag.title().as_deref());
    tags.artist = non_empty(tag.artist().as_deref());
    tags.album = non_empty(tag.album().as_deref());
    tags.album_artist = non_empty(tag.get_string(ItemKey::AlbumArtist));
    tags.track_number = tag.track();
    tags.disc_number = tag.disk();
    tags.year = tag.date().map(|date| date.year as u32);
    let gain = |key| tag.get_string(key).and_then(ReplayGain::parse_value);
    tags.replay_gain = ReplayGain {
        track_gain: gain(ItemKey::ReplayGainTrackGain),
        track_peak: gain(ItemKey::ReplayGainTrackPeak),
        album_gain: gain(ItemKey::ReplayGainAlbumGain),
        album_peak: gain(ItemKey::ReplayGainAlbumPeak),
    };
    tags
}

fn best_picture(pictures: &[Picture]) -> Option<&Picture> {
    pictures.iter().find(|picture| picture.pic_type() == PictureType::CoverFront).or_else(|| pictures.first())
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_image_formats() {
        assert_eq!(ImageKind::sniff(&[0x89, b'P', b'N', b'G', 0x0D]), Some(ImageKind::Png));
        assert_eq!(ImageKind::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageKind::Jpeg));
        assert_eq!(ImageKind::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageKind::Webp));
        assert_eq!(ImageKind::sniff(b"not an image"), None);
    }

    #[test]
    fn reads_sidecars_and_duration() {
        let dir = std::env::temp_dir().join(format!("mi-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("song.wav");
        // One second of 8 kHz mono silence.
        let mut wav = Vec::new();
        let data_len = 8000u32 * 2;
        wav.extend(b"RIFF");
        wav.extend((36 + data_len).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(8000u32.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(data_len.to_le_bytes());
        wav.resize(wav.len() + data_len as usize, 0);
        std::fs::write(&audio, wav).unwrap();
        std::fs::write(dir.join("song.lrc"), "[00:00.50]hello").unwrap();
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        std::fs::copy(examples.join("Bittersweet Chocolate.jpg"), dir.join("song.jpg")).unwrap();

        let duration = read_tags(&audio).unwrap().duration_ms.unwrap();
        assert!((duration as i64 - 1000).abs() <= 10, "{duration:?}");

        let details = read_details(&audio, None);
        assert_eq!(details.tags.title, None);
        assert_eq!(details.lyrics.lines.len(), 1);
        assert_eq!(details.lyrics_source, Some(LyricsSource::File(dir.join("song.lrc"))));
        assert_eq!(details.cover.map(|cover| cover.kind), Some(ImageKind::Jpeg));
        let thumbnail = read_thumbnail(&audio, 64).unwrap();
        assert_eq!(image::load_from_memory(&thumbnail.bytes).unwrap().width(), 64);

        // A linked lyrics file wins over the sidecar.
        std::fs::write(dir.join("other.lrc"), "[00:00.50]a\n[00:01.00]b").unwrap();
        let details = read_details(&audio, Some(&dir.join("other.lrc")));
        assert_eq!(details.lyrics.lines.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
