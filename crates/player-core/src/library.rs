//! The music library: an index of the audio files under the user's music folders, kept up
//! to date incrementally, plus the album / artist / folder groupings the library view shows.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::media_paths;
use crate::track::TrackTags;

/// Bumped when the stored format changes; older files are rebuilt from scratch.
const INDEX_VERSION: u32 = 1;
/// Deep enough for any real music collection, and stops symlink loops.
const MAX_SCAN_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryTrack {
    pub path: PathBuf,
    /// Modification time (seconds since the epoch) and size when the tags were read.
    pub modified: u64,
    pub size: u64,
    pub tags: TrackTags,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryIndex {
    pub version: u32,
    /// Sorted by path.
    pub tracks: Vec<LibraryTrack>,
}

impl LibraryIndex {
    /// Reads the index; a missing, unreadable or outdated file gives an empty index.
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else { return Self::default() };
        match serde_json::from_slice::<LibraryIndex>(&bytes) {
            Ok(index) if index.version == INDEX_VERSION => index,
            _ => Self::default(),
        }
    }

    /// Writes the index atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut copy = self.clone();
        copy.version = INDEX_VERSION;
        let json = serde_json::to_vec(&copy).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }

    pub fn from_tracks(mut tracks: Vec<LibraryTrack>) -> Self {
        tracks.sort_by(|a, b| a.path.cmp(&b.path));
        tracks.dedup_by(|a, b| a.path == b.path);
        Self { version: INDEX_VERSION, tracks }
    }
}

/// An audio file seen on disk during a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFile {
    pub path: PathBuf,
    pub modified: u64,
    pub size: u64,
}

/// Lists the audio files under `folders` (recursively) with their size and mtime.
pub fn scan_folders(folders: &[PathBuf]) -> Vec<FoundFile> {
    let mut found = Vec::new();
    for folder in folders {
        scan_dir(folder, &mut found, 0);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

fn scan_dir(dir: &Path, found: &mut Vec<FoundFile>, depth: usize) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else { continue };
        if metadata.is_dir() {
            // Skip hidden folders such as `.git` or macOS `.Trashes`.
            if !path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with('.')) {
                scan_dir(&path, found, depth + 1);
            }
        } else if media_paths::is_audio_file(&path) {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            found.push(FoundFile { path, modified, size: metadata.len() });
        }
    }
}

/// What a rescan has to do, given the previous index and the files now on disk.
#[derive(Debug, Default)]
pub struct ScanPlan {
    /// Unchanged tracks, reused as they are.
    pub keep: Vec<LibraryTrack>,
    /// New or changed files whose tags must be read.
    pub read: Vec<FoundFile>,
    /// How many indexed tracks are gone.
    pub removed: usize,
}

pub fn plan_rescan(existing: &[LibraryTrack], found: Vec<FoundFile>) -> ScanPlan {
    let previous: HashMap<&Path, &LibraryTrack> =
        existing.iter().map(|track| (track.path.as_path(), track)).collect();
    let mut plan = ScanPlan::default();
    let mut seen = 0;
    for file in found {
        match previous.get(file.path.as_path()) {
            Some(track) if track.modified == file.modified && track.size == file.size => {
                seen += 1;
                plan.keep.push((*track).clone());
            }
            Some(_) => {
                seen += 1;
                plan.read.push(file);
            }
            None => plan.read.push(file),
        }
    }
    plan.removed = existing.len() - seen;
    plan
}

/// Identifies an album: its title plus the artist it's filed under (both lowercased), or the
/// folder for tracks without an album tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AlbumKey {
    Tagged { album: String, artist: String },
    Folder(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Album {
    pub key: AlbumKey,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u32>,
    /// Indices into the library's tracks, in disc / track order.
    pub tracks: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Artist {
    pub name: String,
    pub tracks: Vec<usize>,
    pub albums: usize,
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub path: PathBuf,
    pub tracks: Vec<usize>,
}

/// Sort orders for the song list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SongOrder {
    #[default]
    Title,
    Artist,
    Album,
    /// Newest files first.
    Added,
}

/// Case-insensitive key for sorting display text.
fn sort_key(text: &str) -> String {
    text.trim().to_lowercase()
}

fn track_order(a: &LibraryTrack, b: &LibraryTrack) -> std::cmp::Ordering {
    let number = |track: &LibraryTrack| {
        (track.tags.disc_number.unwrap_or(1), track.tags.track_number.unwrap_or(u32::MAX))
    };
    number(a)
        .cmp(&number(b))
        .then_with(|| sort_key(&a.tags.display_title(&a.path)).cmp(&sort_key(&b.tags.display_title(&b.path))))
}

pub fn albums(tracks: &[LibraryTrack]) -> Vec<Album> {
    let mut groups: BTreeMap<AlbumKey, Album> = BTreeMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let tags = &track.tags;
        let key = match &tags.album {
            Some(album) => AlbumKey::Tagged {
                album: sort_key(album),
                artist: tags.grouping_artist().map(sort_key).unwrap_or_default(),
            },
            None => AlbumKey::Folder(track.path.parent().map(Path::to_path_buf).unwrap_or_default()),
        };
        let album = groups.entry(key.clone()).or_insert_with(|| Album {
            title: tags
                .album
                .clone()
                .unwrap_or_else(|| media_paths::folder_name(&track.path).unwrap_or_default()),
            artist: tags.grouping_artist().map(str::to_string),
            year: tags.year,
            key,
            tracks: Vec::new(),
        });
        album.year = album.year.or(tags.year);
        album.tracks.push(index);
    }
    let mut albums: Vec<Album> = groups.into_values().collect();
    for album in &mut albums {
        album.tracks.sort_by(|&a, &b| track_order(&tracks[a], &tracks[b]));
    }
    albums.sort_by(|a, b| {
        sort_key(a.artist.as_deref().unwrap_or("\u{10FFFF}"))
            .cmp(&sort_key(b.artist.as_deref().unwrap_or("\u{10FFFF}")))
            .then_with(|| a.year.cmp(&b.year))
            .then_with(|| sort_key(&a.title).cmp(&sort_key(&b.title)))
    });
    albums
}

pub fn artists(tracks: &[LibraryTrack]) -> Vec<Artist> {
    let mut groups: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let Some(name) = track.tags.artist.as_deref().or(track.tags.album_artist.as_deref()) else {
            continue;
        };
        groups.entry(sort_key(name)).or_insert_with(|| (name.to_string(), Vec::new())).1.push(index);
    }
    groups
        .into_values()
        .map(|(name, mut indices)| {
            indices.sort_by(|&a, &b| {
                let album = |i: usize| tracks[i].tags.album.as_deref().map(sort_key).unwrap_or_default();
                album(a).cmp(&album(b)).then_with(|| track_order(&tracks[a], &tracks[b]))
            });
            let mut albums: Vec<String> =
                indices.iter().filter_map(|&i| tracks[i].tags.album.as_deref().map(sort_key)).collect();
            albums.sort();
            albums.dedup();
            Artist { name, albums: albums.len(), tracks: indices }
        })
        .collect()
}

pub fn folders(tracks: &[LibraryTrack]) -> Vec<Folder> {
    let mut groups: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let dir = track.path.parent().map(Path::to_path_buf).unwrap_or_default();
        groups.entry(dir).or_default().push(index);
    }
    groups
        .into_iter()
        .map(|(path, mut indices)| {
            indices.sort_by(|&a, &b| tracks[a].path.cmp(&tracks[b].path));
            Folder { path, tracks: indices }
        })
        .collect()
}

/// Indices of all tracks in `order`.
pub fn sorted_songs(tracks: &[LibraryTrack], order: SongOrder) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..tracks.len()).collect();
    let title = |i: usize| sort_key(&tracks[i].tags.display_title(&tracks[i].path));
    let field = |i: usize, f: fn(&TrackTags) -> Option<&str>| f(&tracks[i].tags).map(sort_key);
    match order {
        SongOrder::Title => indices.sort_by_cached_key(|&i| title(i)),
        SongOrder::Artist => indices.sort_by_cached_key(|&i| {
            (field(i, |t| t.artist.as_deref()).is_none(), field(i, |t| t.artist.as_deref()), title(i))
        }),
        SongOrder::Album => {
            indices.sort_by(|&a, &b| {
                let album =
                    |i| (field(i, |t| t.album.as_deref()).is_none(), field(i, |t| t.album.as_deref()));
                album(a).cmp(&album(b)).then_with(|| track_order(&tracks[a], &tracks[b]))
            });
        }
        SongOrder::Added => indices.sort_by(|&a, &b| {
            tracks[b].modified.cmp(&tracks[a].modified).then_with(|| title(a).cmp(&title(b)))
        }),
    }
    indices
}

/// Folder path shown relative to the library root that contains it.
pub fn relative_folder(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter_map(|root| {
            let relative = path.strip_prefix(root).ok()?;
            let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            Some(if relative.as_os_str().is_empty() {
                name
            } else {
                Path::new(&name).join(relative).display().to_string()
            })
        })
        .min_by_key(String::len)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(
        path: &str,
        title: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        number: Option<u32>,
    ) -> LibraryTrack {
        LibraryTrack {
            path: PathBuf::from(path),
            modified: 0,
            size: 0,
            tags: TrackTags {
                title: title.map(Into::into),
                artist: artist.map(Into::into),
                album: album.map(Into::into),
                track_number: number,
                ..Default::default()
            },
        }
    }

    fn sample() -> Vec<LibraryTrack> {
        LibraryIndex::from_tracks(vec![
            track("m/jay/b.mp3", Some("稻香"), Some("周杰伦"), Some("魔杰座"), Some(2)),
            track("m/jay/a.mp3", Some("给我一首歌的时间"), Some("周杰伦"), Some("魔杰座"), Some(1)),
            track("m/misc/x.flac", Some("Bittersweet"), Some("Ado"), None, None),
            track("m/misc/y.flac", None, None, None, None),
            track("m/jay2/c.mp3", Some("晴天"), Some("周杰伦"), Some("叶惠美"), Some(3)),
        ])
        .tracks
    }

    #[test]
    fn groups_albums_by_tag_or_folder() {
        let tracks = sample();
        let albums = albums(&tracks);
        let titles: Vec<&str> = albums.iter().map(|a| a.title.as_str()).collect();
        // Albums sort by artist ("Ado" before "周杰伦"), then year, then title. The untagged
        // folder takes its artist from its first track.
        assert_eq!(titles, ["misc", "叶惠美", "魔杰座"]);
        let mojiezuo = albums.iter().find(|a| a.title == "魔杰座").unwrap();
        let ordered: Vec<&str> =
            mojiezuo.tracks.iter().map(|&i| tracks[i].tags.title.as_deref().unwrap()).collect();
        assert_eq!(ordered, ["给我一首歌的时间", "稻香"]);
        assert_eq!(mojiezuo.artist.as_deref(), Some("周杰伦"));
        let folder_album = albums.iter().find(|a| matches!(a.key, AlbumKey::Folder(_))).unwrap();
        assert_eq!(folder_album.title, "misc");
        assert_eq!(folder_album.tracks.len(), 2);
    }

    #[test]
    fn groups_artists_and_folders() {
        let tracks = sample();
        let artists = artists(&tracks);
        let jay = artists.iter().find(|a| a.name == "周杰伦").unwrap();
        assert_eq!((jay.tracks.len(), jay.albums), (3, 2));
        assert_eq!(artists.len(), 2);

        let folders = folders(&tracks);
        let paths: Vec<_> = folders.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("m/jay"), PathBuf::from("m/jay2"), PathBuf::from("m/misc")]);
    }

    #[test]
    fn sorts_songs() {
        let mut tracks = sample();
        tracks[0].modified = 50;
        let titles = |order| {
            sorted_songs(&tracks, order)
                .into_iter()
                .map(|i| tracks[i].tags.display_title(&tracks[i].path))
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(SongOrder::Title)[0], "Bittersweet");
        assert_eq!(titles(SongOrder::Artist).last().unwrap(), "y");
        assert_eq!(titles(SongOrder::Added)[0], tracks[0].tags.display_title(&tracks[0].path));
        let by_album = titles(SongOrder::Album);
        assert_eq!(&by_album[..1], ["晴天"]);
    }

    #[test]
    fn plans_incremental_rescans() {
        let existing = vec![
            LibraryTrack { path: "a.mp3".into(), modified: 1, size: 10, tags: TrackTags::default() },
            LibraryTrack { path: "b.mp3".into(), modified: 1, size: 10, tags: TrackTags::default() },
            LibraryTrack { path: "gone.mp3".into(), modified: 1, size: 10, tags: TrackTags::default() },
        ];
        let found = vec![
            FoundFile { path: "a.mp3".into(), modified: 1, size: 10 },
            FoundFile { path: "b.mp3".into(), modified: 2, size: 10 },
            FoundFile { path: "new.mp3".into(), modified: 1, size: 5 },
        ];
        let plan = plan_rescan(&existing, found);
        assert_eq!(plan.keep.len(), 1);
        let read: Vec<_> = plan.read.iter().map(|f| f.path.clone()).collect();
        assert_eq!(read, [PathBuf::from("b.mp3"), PathBuf::from("new.mp3")]);
        assert_eq!(plan.removed, 1);
    }

    #[test]
    fn scans_folders_and_round_trips_index() {
        let dir = std::env::temp_dir().join(format!("mi-library-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        for file in ["one.mp3", "sub/two.FLAC", ".hidden/three.mp3", "cover.jpg"] {
            fs::write(dir.join(file), b"abc").unwrap();
        }
        let found = scan_folders(std::slice::from_ref(&dir));
        let names: Vec<_> =
            found.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["one.mp3", "two.FLAC"]);
        assert!(found.iter().all(|f| f.size == 3));

        let index = LibraryIndex::from_tracks(
            found
                .into_iter()
                .map(|f| LibraryTrack {
                    path: f.path,
                    modified: f.modified,
                    size: f.size,
                    tags: TrackTags::default(),
                })
                .collect(),
        );
        let file = dir.join("library.json");
        index.save(&file).unwrap();
        assert_eq!(LibraryIndex::load(&file), index);
        fs::write(&file, b"{\"version\":0,\"tracks\":[]}").unwrap();
        assert!(LibraryIndex::load(&file).tracks.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn relative_folder_names() {
        let roots = [PathBuf::from("/music"), PathBuf::from("/music/jay")];
        assert_eq!(
            relative_folder(Path::new("/music/jay/2008"), &roots),
            Path::new("jay").join("2008").display().to_string()
        );
        assert_eq!(relative_folder(Path::new("/music"), &roots), "music");
        assert_eq!(relative_folder(Path::new("/other"), &roots), Path::new("/other").display().to_string());
    }
}
