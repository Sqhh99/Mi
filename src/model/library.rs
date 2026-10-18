//! `LibraryModel`: the indexed music collection under the user's library folders.
//!
//! The index is stored next to the settings and refreshed incrementally: on startup, when
//! folders change and on request, only new or modified files have their tags read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Context, Entity, EventEmitter, Task};
use player_core::library::{self, Album, Artist, Folder, LibraryIndex, LibraryTrack};

use super::settings::{SettingsModel, config_dir};
use crate::media;

const READ_BATCH: usize = 48;

/// Album, artist and folder groupings, rebuilt when the index changes.
pub struct Groupings {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub folders: Vec<Folder>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanProgress {
    /// Audio files found under the library folders.
    pub found: usize,
    /// Tags read so far, out of `to_read`.
    pub read: usize,
    pub to_read: usize,
}

pub enum LibraryEvent {
    ScanFinished { added: usize, removed: usize },
}

pub struct LibraryModel {
    settings: Entity<SettingsModel>,
    index: Arc<LibraryIndex>,
    by_path: HashMap<PathBuf, usize>,
    groupings: Option<Arc<Groupings>>,
    scan: Option<ScanProgress>,
    /// A scan was requested while one was running.
    rescan_pending: bool,
    folders: Vec<PathBuf>,
    store: Option<PathBuf>,
    loaded: bool,
    revision: u64,
    task: Option<Task<()>>,
    save_task: Option<Task<()>>,
}

impl EventEmitter<LibraryEvent> for LibraryModel {}

impl LibraryModel {
    pub fn new(settings: Entity<SettingsModel>, cx: &mut Context<Self>) -> Self {
        let store = config_dir().map(|dir| dir.join("library.json"));
        let folders = settings.read(cx).get().library.folders.clone();
        cx.observe(&settings, |this, settings, cx| {
            let folders = settings.read(cx).get().library.folders.clone();
            if folders != this.folders {
                this.folders = folders;
                this.rescan(cx);
            }
        })
        .detach();

        let mut model = Self {
            settings,
            index: Arc::default(),
            by_path: HashMap::new(),
            groupings: None,
            scan: None,
            rescan_pending: false,
            folders,
            store: store.clone(),
            loaded: false,
            revision: 0,
            task: None,
            save_task: None,
        };
        // Show the stored index right away, then bring it up to date.
        model.task = Some(cx.spawn(async move |this, cx| {
            let index = match store {
                Some(store) => {
                    cx.background_executor().spawn(async move { LibraryIndex::load(&store) }).await
                }
                None => LibraryIndex::default(),
            };
            this.update(cx, |this, cx| {
                this.loaded = true;
                this.set_index(index, cx);
                this.task = None;
                this.rescan(cx);
            })
            .ok();
        }));
        model
    }

    pub fn folders(&self) -> &[PathBuf] {
        &self.folders
    }

    pub fn tracks(&self) -> &[LibraryTrack] {
        &self.index.tracks
    }

    pub fn track(&self, path: &Path) -> Option<&LibraryTrack> {
        self.by_path.get(path).map(|&index| &self.index.tracks[index])
    }

    pub fn track_at(&self, index: usize) -> Option<&LibraryTrack> {
        self.index.tracks.get(index)
    }

    /// Changes whenever the index does, so views can drop cached results.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn scan_progress(&self) -> Option<ScanProgress> {
        self.scan
    }

    pub fn groupings(&mut self) -> Arc<Groupings> {
        self.groupings
            .get_or_insert_with(|| {
                let tracks = &self.index.tracks;
                Arc::new(Groupings {
                    albums: library::albums(tracks),
                    artists: library::artists(tracks),
                    folders: library::folders(tracks),
                })
            })
            .clone()
    }

    pub fn add_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(cx, |s| {
                for folder in folders {
                    // A folder inside one already in the library adds nothing.
                    let covered = s.library.folders.iter().any(|known| folder.starts_with(known));
                    if !covered {
                        s.library.folders.retain(|known| !known.starts_with(&folder));
                        s.library.folders.push(folder);
                    }
                }
            })
        });
    }

    pub fn remove_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        let folder = folder.to_path_buf();
        self.settings
            .update(cx, |settings, cx| settings.update(cx, |s| s.library.folders.retain(|f| *f != folder)));
    }

    /// Brings the index up to date with the library folders.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if !self.loaded {
            return;
        }
        if self.scan.is_some() {
            self.rescan_pending = true;
            return;
        }
        let folders = self.folders.clone();
        let existing = self.index.clone();
        self.scan = Some(ScanProgress::default());
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let plan = cx
                .background_executor()
                .spawn(async move {
                    let found = library::scan_folders(&folders);
                    let count = found.len();
                    (library::plan_rescan(&existing.tracks, found), count)
                })
                .await;
            let (plan, found) = plan;
            let to_read = plan.read.len();
            let removed = plan.removed;
            let mut tracks = plan.keep;
            if this
                .update(cx, |this, cx| {
                    this.scan = Some(ScanProgress { found, read: 0, to_read });
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            for batch in plan.read.chunks(READ_BATCH) {
                let batch = batch.to_vec();
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        batch
                            .into_iter()
                            .map(|file| LibraryTrack {
                                tags: media::read_tags(&file.path).unwrap_or_default(),
                                path: file.path,
                                modified: file.modified,
                                size: file.size,
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                tracks.extend(read);
                let alive = this.update(cx, |this, cx| {
                    if let Some(scan) = &mut this.scan {
                        scan.read = to_read.min(scan.read + READ_BATCH);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let index = LibraryIndex::from_tracks(tracks);
            this.update(cx, |this, cx| {
                this.scan = None;
                this.task = None;
                let changed = to_read > 0 || removed > 0;
                if changed || this.index.tracks.len() != index.tracks.len() {
                    this.set_index(index, cx);
                    this.save(cx);
                }
                cx.emit(LibraryEvent::ScanFinished { added: to_read, removed });
                if std::mem::take(&mut this.rescan_pending) {
                    this.rescan(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_index(&mut self, index: LibraryIndex, cx: &mut Context<Self>) {
        self.by_path =
            index.tracks.iter().enumerate().map(|(position, track)| (track.path.clone(), position)).collect();
        self.index = Arc::new(index);
        self.groupings = None;
        self.revision += 1;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let index = self.index.clone();
        self.save_task = Some(cx.background_executor().spawn(async move {
            if let Err(err) = index.save(&store) {
                log::error!("cannot save the library index to {}: {err}", store.display());
            }
        }));
    }
}
