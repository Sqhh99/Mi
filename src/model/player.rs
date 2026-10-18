//! `PlayerModel`: the single source of truth for playback state, shared by every view.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, EventEmitter, Image, SharedString, Task};
use player_core::eq::{Band, EqLevels};
use player_core::lyrics::Lyrics;
use player_core::media_paths;
use player_core::queue::{PlayQueue, Previous, Removed, RepeatMode, TrackEnd};
use player_core::settings::Settings;
use player_core::track::{ReplayGainMode, TrackTags};

use crate::audio::{AudioCommand, AudioEvent, AudioHandle, LoadError, LoadId};
use crate::media::{self, Cover, LyricsSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Playing,
    Paused,
}

/// Why a track in the queue can't be played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackIssue {
    Missing,
    Unreadable(String),
}

/// Something worth telling the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A track was skipped because it couldn't be played.
    Skipped { path: PathBuf, issue: TrackIssue },
    /// Every track in the queue failed; playback stopped.
    NothingPlayable,
    /// No audio output could be opened.
    DeviceUnavailable(String),
    /// The output device disappeared; playback is paused until it is reconnected.
    DeviceLost,
}

/// Emitted for changes other parts of the app persist or mirror (settings, tray).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    /// The queue's contents or order changed.
    Playlist,
    /// Volume, mute, EQ, repeat or shuffle changed.
    Preferences,
    /// Playback status or the current track changed.
    State,
    /// A track started playing (for "recently played").
    TrackStarted(PathBuf),
    /// Lyrics links or timing offsets changed.
    Lyrics,
    Problem(Problem),
}

/// Display information for the current track.
#[derive(Clone)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub title: SharedString,
    pub artist: Option<SharedString>,
    pub album: Option<SharedString>,
    pub cover: Option<Arc<Image>>,
    pub lyrics_source: Option<LyricsSource>,
}

/// Restarting is nicer than jumping back when this far into a track.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
/// The next track is decoded this long before the current one ends.
const PRELOAD_LEAD: Duration = Duration::from_secs(12);
const TAG_PROBE_BATCH: usize = 16;

/// A track handed to the engine to follow the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Preload {
    id: LoadId,
    index: usize,
    path: PathBuf,
}

pub struct PlayerModel {
    audio: AudioHandle,
    queue: PlayQueue,
    rng: fastrand::Rng,
    /// Last id handed to the engine (loads and preloads draw from one counter).
    last_id: LoadId,
    /// Id of the track the engine is playing.
    load_id: LoadId,
    /// Whether the engine currently holds the queue's current track.
    loaded: bool,
    status: Status,
    position: Duration,
    duration: Duration,
    volume: u8,
    muted: bool,
    eq: EqLevels,
    eq_enabled: bool,
    gapless: bool,
    preload: Option<Preload>,
    track: Option<TrackInfo>,
    lyrics: Arc<Lyrics>,
    lyric_index: Option<usize>,
    lyrics_links: BTreeMap<PathBuf, PathBuf>,
    lyrics_offsets: BTreeMap<PathBuf, i32>,
    tags: HashMap<PathBuf, Arc<TrackTags>>,
    /// Bumped whenever tags arrive, so views can refresh search results.
    tags_version: u64,
    issues: HashMap<PathBuf, TrackIssue>,
    device_problem: Option<Problem>,
    /// Where to resume once the output device is back.
    resume_after_device_loss: Option<(Duration, bool)>,
    consecutive_failures: usize,
    /// While the window is hidden, position ticks update state without redrawing anything.
    quiet: bool,
    /// Tracks whose tags are being read.
    probing: HashSet<PathBuf>,
    probe_requests: async_channel::Sender<Vec<PathBuf>>,
    _audio_events: Task<()>,
    _tag_prober: Task<()>,
    details_task: Option<Task<()>>,
}

impl EventEmitter<PlayerEvent> for PlayerModel {}

impl PlayerModel {
    pub fn new(settings: &Settings, cx: &mut Context<Self>) -> Self {
        let eq_enabled = settings.playback.eq_enabled;
        let gains = if eq_enabled { settings.eq.gains_db() } else { [0.0; 3] };
        let (audio, events) = AudioHandle::spawn(gains);
        let audio_events = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |model, cx| model.on_audio_event(event, cx)).is_err() {
                    break;
                }
            }
        });
        let (probe_requests, probe_rx) = async_channel::unbounded::<Vec<PathBuf>>();
        let tag_prober = cx.spawn(async move |this, cx| {
            while let Ok(paths) = probe_rx.recv().await {
                for batch in paths.chunks(TAG_PROBE_BATCH) {
                    let batch = batch.to_vec();
                    let tags = cx
                        .background_executor()
                        .spawn(async move {
                            batch.into_iter().map(|path| (media::read_tags(&path), path)).collect::<Vec<_>>()
                        })
                        .await;
                    if this.update(cx, |model, cx| model.on_tags_read(tags, cx)).is_err() {
                        return;
                    }
                }
            }
        });
        let mut queue = PlayQueue::new();
        queue.set_repeat(settings.playback.repeat);
        queue.set_shuffle(settings.playback.shuffle);
        let mut model = Self {
            audio,
            queue,
            rng: fastrand::Rng::new(),
            last_id: 0,
            load_id: 0,
            loaded: false,
            status: Status::Stopped,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            volume: settings.volume.min(100),
            muted: settings.muted,
            eq: settings.eq,
            eq_enabled,
            gapless: settings.playback.gapless,
            preload: None,
            track: None,
            lyrics: Arc::default(),
            lyric_index: None,
            lyrics_links: settings.lyrics.links.clone(),
            lyrics_offsets: settings.lyrics.offsets.clone(),
            tags: HashMap::new(),
            tags_version: 0,
            issues: HashMap::new(),
            device_problem: None,
            resume_after_device_loss: None,
            consecutive_failures: 0,
            quiet: false,
            probing: HashSet::new(),
            probe_requests,
            _audio_events: audio_events,
            _tag_prober: tag_prober,
            details_task: None,
        };
        model.apply_volume();
        model.apply_output_settings(settings);
        model
    }

    // ── Read access ─────────────────────────────────────────────────────────

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn is_playing(&self) -> bool {
        self.status == Status::Playing
    }

    pub fn position(&self) -> Duration {
        self.position
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn eq(&self) -> EqLevels {
        self.eq
    }

    pub fn eq_enabled(&self) -> bool {
        self.eq_enabled
    }

    pub fn repeat(&self) -> RepeatMode {
        self.queue.repeat()
    }

    pub fn shuffle(&self) -> bool {
        self.queue.shuffle()
    }

    pub fn track(&self) -> Option<&TrackInfo> {
        self.track.as_ref()
    }

    pub fn tracks(&self) -> &[PathBuf] {
        self.queue.tracks()
    }

    pub fn current_index(&self) -> Option<usize> {
        self.queue.current()
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.queue.current_path()
    }

    /// Tags of a queued track, once they have been read in the background.
    pub fn tags(&self, path: &Path) -> Option<&TrackTags> {
        self.tags.get(path).map(Arc::as_ref)
    }

    pub fn tags_version(&self) -> u64 {
        self.tags_version
    }

    pub fn track_duration(&self, index: usize) -> Option<Duration> {
        let path = self.queue.tracks().get(index)?;
        self.tags(path)?.duration_ms.map(Duration::from_millis)
    }

    pub fn issue(&self, path: &Path) -> Option<&TrackIssue> {
        self.issues.get(path)
    }

    /// The current audio-device problem, if playback is blocked by one.
    pub fn device_problem(&self) -> Option<&Problem> {
        self.device_problem.as_ref()
    }

    pub fn lyrics(&self) -> &Arc<Lyrics> {
        &self.lyrics
    }

    pub fn lyric_index(&self) -> Option<usize> {
        self.lyric_index
    }

    pub fn lyrics_links(&self) -> &BTreeMap<PathBuf, PathBuf> {
        &self.lyrics_links
    }

    pub fn lyrics_offsets(&self) -> &BTreeMap<PathBuf, i32> {
        &self.lyrics_offsets
    }

    /// The user's timing adjustment for the current track's lyrics, in milliseconds.
    pub fn lyrics_offset(&self) -> i32 {
        self.current_path().and_then(|path| self.lyrics_offsets.get(path)).copied().unwrap_or(0)
    }

    /// Playback position at which lyric line `index` is shown.
    pub fn lyric_time(&self, index: usize) -> Option<Duration> {
        let line = self.lyrics.lines.get(index)?;
        Some(Duration::from_millis((line.time_ms as i64 - self.lyrics_offset() as i64).max(0) as u64))
    }

    // ── Queue commands ──────────────────────────────────────────────────────

    /// Restores the queue at startup, optionally loading the current track at `position`.
    pub fn restore(
        &mut self,
        tracks: Vec<PathBuf>,
        current: Option<usize>,
        position: Duration,
        autoplay: bool,
        cx: &mut Context<Self>,
    ) {
        self.queue.set_tracks(tracks, current.unwrap_or(0));
        self.start_tag_probe(self.queue.tracks().to_vec(), cx);
        if current.is_some() && (autoplay || !position.is_zero()) {
            self.load_current_at(autoplay, position, cx);
        } else {
            self.refresh_track(cx);
        }
        self.changed(cx);
    }

    /// Replaces the queue with `tracks` and plays the one at `start`.
    pub fn play_tracks(&mut self, tracks: Vec<PathBuf>, start: usize, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        self.queue.set_tracks(tracks, start);
        self.consecutive_failures = 0;
        self.start_tag_probe(self.queue.tracks().to_vec(), cx);
        cx.emit(PlayerEvent::Playlist);
        self.load_current(true, cx);
    }

    /// Adds tracks to the end of the queue. Returns their indices.
    pub fn enqueue(&mut self, tracks: Vec<PathBuf>, cx: &mut Context<Self>) -> Range<usize> {
        let was_empty = self.queue.is_empty();
        let range = self.queue.append(tracks);
        self.after_insert(range.clone(), was_empty, cx);
        range
    }

    /// Inserts tracks right after the current one. Returns their indices.
    pub fn play_next(&mut self, tracks: Vec<PathBuf>, cx: &mut Context<Self>) -> Range<usize> {
        let was_empty = self.queue.is_empty();
        let range = self.queue.insert_next(tracks);
        self.after_insert(range.clone(), was_empty, cx);
        range
    }

    /// Inserts tracks before queue index `at`. Returns their indices.
    pub fn insert_tracks(&mut self, at: usize, tracks: Vec<PathBuf>, cx: &mut Context<Self>) -> Range<usize> {
        let was_empty = self.queue.is_empty();
        let range = self.queue.insert(at, tracks);
        self.after_insert(range.clone(), was_empty, cx);
        range
    }

    fn after_insert(&mut self, range: Range<usize>, was_empty: bool, cx: &mut Context<Self>) {
        if range.is_empty() {
            return;
        }
        self.start_tag_probe(self.queue.tracks()[range].to_vec(), cx);
        if was_empty {
            self.refresh_track(cx);
        }
        cx.emit(PlayerEvent::Playlist);
        self.sync_preload();
        self.changed(cx);
    }

    pub fn remove(&mut self, indices: &[usize], cx: &mut Context<Self>) {
        let Some(removed) = self.queue.remove(indices) else { return };
        if let Removed::Current(next) = removed {
            let resume = self.status == Status::Playing;
            self.stop_engine();
            if next.is_some() && resume {
                self.load_current(true, cx);
            } else {
                self.refresh_track(cx);
            }
        }
        cx.emit(PlayerEvent::Playlist);
        self.sync_preload();
        self.changed(cx);
    }

    /// Moves tracks so they sit before the track now at `before`. Returns their new indices.
    pub fn move_tracks(&mut self, indices: &[usize], before: usize, cx: &mut Context<Self>) -> Range<usize> {
        let range = self.queue.move_tracks(indices, before);
        cx.emit(PlayerEvent::Playlist);
        self.sync_preload();
        self.changed(cx);
        range
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.stop_engine();
        self.queue.clear();
        self.refresh_track(cx);
        cx.emit(PlayerEvent::Playlist);
        self.changed(cx);
    }

    /// Points a queue entry at the file the user located after it went missing.
    pub fn relocate(&mut self, index: usize, path: PathBuf, cx: &mut Context<Self>) {
        let Some(old) = self.queue.tracks().get(index).cloned() else { return };
        if !self.queue.replace(index, path.clone()) {
            return;
        }
        self.issues.remove(&old);
        self.issues.remove(&path);
        if let Some(link) = self.lyrics_links.remove(&old) {
            self.lyrics_links.insert(path.clone(), link);
            cx.emit(PlayerEvent::Lyrics);
        }
        self.start_tag_probe(vec![path], cx);
        if self.queue.current() == Some(index) {
            self.stop_engine();
            self.refresh_track(cx);
        }
        cx.emit(PlayerEvent::Playlist);
        self.sync_preload();
        self.changed(cx);
    }

    // ── Transport ───────────────────────────────────────────────────────────

    pub fn play_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.queue.select(index) {
            self.consecutive_failures = 0;
            self.load_current(true, cx);
        }
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        match self.status {
            Status::Playing => self.audio.send(AudioCommand::Pause),
            Status::Paused if self.loaded => self.audio.send(AudioCommand::Play),
            _ if self.queue.current().is_some() => {
                let start = if self.position < self.duration { self.position } else { Duration::ZERO };
                self.load_current_at(true, start, cx)
            }
            _ => {}
        }
    }

    pub fn pause(&mut self) {
        if self.status == Status::Playing {
            self.audio.send(AudioCommand::Pause);
        }
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
        if self.status != Status::Playing {
            self.toggle_play(cx);
        }
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        if self.queue.next(&mut self.rng).is_some() {
            self.consecutive_failures = 0;
            self.load_current(true, cx);
        }
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) {
        if self.loaded && self.position > RESTART_THRESHOLD {
            self.restart(cx);
            return;
        }
        match self.queue.previous() {
            Some(Previous::Play(_)) => {
                self.consecutive_failures = 0;
                self.load_current(true, cx);
            }
            Some(Previous::Restart) if self.loaded => self.restart(cx),
            Some(Previous::Restart) => self.load_current(true, cx),
            None => {}
        }
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.seek(Duration::ZERO, cx);
        self.audio.send(AudioCommand::Play);
    }

    pub fn seek(&mut self, position: Duration, cx: &mut Context<Self>) {
        let position = if self.duration.is_zero() { position } else { position.min(self.duration) };
        if !self.loaded {
            // Seeking a stopped track loads it paused at that spot.
            if self.queue.current().is_some() {
                self.load_current_at(false, position, cx);
            }
            return;
        }
        self.audio.send(AudioCommand::Seek(position));
        self.set_position(position, cx);
    }

    /// Jumps by `delta` seconds (negative = back).
    pub fn seek_by(&mut self, delta: f32, cx: &mut Context<Self>) {
        let target = (self.position.as_secs_f32() + delta).max(0.0);
        self.seek(Duration::from_secs_f32(target), cx);
    }

    pub fn set_volume(&mut self, volume: u8, cx: &mut Context<Self>) {
        let volume = volume.min(100);
        if volume == self.volume {
            return;
        }
        self.volume = volume;
        self.apply_volume();
        cx.emit(PlayerEvent::Preferences);
        self.changed(cx);
    }

    pub fn adjust_volume(&mut self, delta: i16, cx: &mut Context<Self>) {
        let volume = (self.volume as i16 + delta).clamp(0, 100) as u8;
        self.set_volume(volume, cx);
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.apply_volume();
        cx.emit(PlayerEvent::Preferences);
        self.changed(cx);
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.set_repeat(self.queue.repeat().cycle(), cx);
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode, cx: &mut Context<Self>) {
        self.queue.set_repeat(repeat);
        cx.emit(PlayerEvent::Preferences);
        self.sync_preload();
        self.changed(cx);
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.queue.set_shuffle(!self.queue.shuffle());
        cx.emit(PlayerEvent::Preferences);
        self.sync_preload();
        self.changed(cx);
    }

    // ── Equalizer ───────────────────────────────────────────────────────────

    pub fn set_eq_level(&mut self, band: Band, level: u8, cx: &mut Context<Self>) {
        if self.eq.get(band) == level {
            return;
        }
        self.eq.set(band, level);
        self.apply_eq(cx);
    }

    pub fn set_eq_levels(&mut self, levels: EqLevels, cx: &mut Context<Self>) {
        if self.eq == levels {
            return;
        }
        self.eq = levels.clamped();
        self.apply_eq(cx);
    }

    pub fn set_eq_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.eq_enabled == enabled {
            return;
        }
        self.eq_enabled = enabled;
        self.apply_eq(cx);
    }

    fn apply_eq(&mut self, cx: &mut Context<Self>) {
        self.audio.set_eq(if self.eq_enabled { self.eq.gains_db() } else { [0.0; 3] });
        cx.emit(PlayerEvent::Preferences);
        cx.notify();
    }

    // ── Lyrics ──────────────────────────────────────────────────────────────

    /// Shifts the current track's lyrics by `delta_ms` (positive = earlier).
    pub fn adjust_lyrics_offset(&mut self, delta_ms: i32, cx: &mut Context<Self>) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else { return };
        let offset = self.lyrics_offset().saturating_add(delta_ms);
        self.set_lyrics_offset(path, offset, cx);
    }

    pub fn reset_lyrics_offset(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.current_path().map(Path::to_path_buf) {
            self.set_lyrics_offset(path, 0, cx);
        }
    }

    fn set_lyrics_offset(&mut self, path: PathBuf, offset: i32, cx: &mut Context<Self>) {
        if offset == 0 {
            self.lyrics_offsets.remove(&path);
        } else {
            self.lyrics_offsets.insert(path, offset);
        }
        self.set_position(self.position, cx);
        cx.emit(PlayerEvent::Lyrics);
    }

    /// Uses `lrc` as the lyrics of the current track (or forgets the link with `None`).
    pub fn link_lyrics(&mut self, lrc: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else { return };
        match lrc {
            Some(lrc) => self.lyrics_links.insert(path, lrc),
            None => self.lyrics_links.remove(&path),
        };
        cx.emit(PlayerEvent::Lyrics);
        // Reload details so the new lyrics show up.
        self.track = None;
        self.refresh_track(cx);
    }

    // ── Settings pushed from outside ────────────────────────────────────────

    /// Applies output-related settings (ReplayGain, gapless, device).
    pub fn apply_output_settings(&mut self, settings: &Settings) {
        let playback = &settings.playback;
        let preamp_db = playback.replay_gain_preamp as f32;
        self.audio.send(AudioCommand::SetReplayGain { mode: playback.replay_gain, preamp_db });
        self.audio.send(AudioCommand::SetDevice(playback.output_device.clone()));
        if self.gapless != playback.gapless {
            self.gapless = playback.gapless;
            self.sync_preload();
        }
    }

    /// Reloads the current track after the output device or ReplayGain changed.
    pub fn reload_output(&mut self, cx: &mut Context<Self>) {
        if self.loaded {
            let (position, playing) = (self.position, self.status == Status::Playing);
            self.load_current_at(playing, position, cx);
        }
    }

    /// Tries the output device again after it failed.
    pub fn retry_device(&mut self, cx: &mut Context<Self>) {
        let (position, playing) = self.resume_after_device_loss.take().unwrap_or((self.position, true));
        self.device_problem = None;
        if self.queue.current().is_some() {
            self.load_current_at(playing, position, cx);
        }
        self.changed(cx);
    }

    /// Called when the window is hidden or shown again.
    pub fn set_quiet(&mut self, quiet: bool, cx: &mut Context<Self>) {
        if self.quiet != quiet {
            self.quiet = quiet;
            if !quiet {
                cx.notify();
            }
        }
    }

    /// Stops the audio thread; called when the app quits.
    pub fn shutdown(&self) {
        self.audio.send(AudioCommand::Shutdown);
    }

    /// Whether `replay_gain` would make a reload worthwhile.
    pub fn replay_gain_changed(old: &Settings, new: &Settings) -> bool {
        old.playback.replay_gain != new.playback.replay_gain
            || (new.playback.replay_gain != ReplayGainMode::Off
                && old.playback.replay_gain_preamp != new.playback.replay_gain_preamp)
    }

    // ── Internals ───────────────────────────────────────────────────────────

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(PlayerEvent::State);
        cx.notify();
    }

    fn apply_volume(&self) {
        // Squared so the slider feels even: loudness is roughly logarithmic in amplitude.
        let level = self.volume as f32 / 100.0;
        let volume = if self.muted { 0.0 } else { level * level };
        self.audio.send(AudioCommand::SetVolume(volume));
    }

    fn next_id(&mut self) -> LoadId {
        self.last_id += 1;
        self.last_id
    }

    fn stop_engine(&mut self) {
        self.audio.send(AudioCommand::Stop);
        self.loaded = false;
        self.preload = None;
        self.status = Status::Stopped;
        self.position = Duration::ZERO;
    }

    fn load_current(&mut self, autoplay: bool, cx: &mut Context<Self>) {
        self.load_current_at(autoplay, Duration::ZERO, cx);
    }

    fn load_current_at(&mut self, autoplay: bool, start: Duration, cx: &mut Context<Self>) {
        let Some(path) = self.queue.current_path().map(Path::to_path_buf) else {
            return;
        };
        self.load_id = self.next_id();
        self.preload = None;
        self.device_problem = None;
        self.audio.send(AudioCommand::Load { path, id: self.load_id, autoplay, start });
        self.loaded = true;
        self.status = if autoplay { Status::Playing } else { Status::Paused };
        self.duration = self.queue.current().and_then(|i| self.track_duration(i)).unwrap_or_default();
        self.set_position(start, cx);
        self.refresh_track(cx);
        self.changed(cx);
    }

    fn set_position(&mut self, position: Duration, cx: &mut Context<Self>) {
        self.position = position;
        let ms = position.as_millis() as i64 + self.lyrics_offset() as i64;
        self.lyric_index = self.lyrics.index_at(ms.max(0) as u64);
        if !self.quiet {
            cx.notify();
        }
    }

    /// Hands the engine the track that will follow, or takes it back if the plan changed.
    fn sync_preload(&mut self) {
        let remaining = self.duration.saturating_sub(self.position);
        let wanted = (self.gapless
            && self.loaded
            && self.status == Status::Playing
            && !self.duration.is_zero()
            && remaining <= PRELOAD_LEAD)
            .then(|| self.queue.plan_track_end(&mut self.rng))
            .flatten()
            .and_then(|index| Some((index, self.queue.tracks().get(index)?.clone())))
            .filter(|(_, path)| !self.issues.contains_key(path));

        match (wanted, &self.preload) {
            (Some((index, path)), Some(current)) if current.index == index && current.path == path => {}
            (Some((index, path)), _) => {
                let id = self.next_id();
                self.audio.send(AudioCommand::Preload { path: path.clone(), id });
                self.preload = Some(Preload { id, index, path });
            }
            (None, Some(_)) => {
                self.audio.send(AudioCommand::CancelPreload);
                self.preload = None;
            }
            (None, None) => {}
        }
    }

    /// Updates track info for the queue's current track, loading tags in the background.
    fn refresh_track(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.queue.current_path().map(Path::to_path_buf) else {
            self.track = None;
            self.lyrics = Arc::default();
            self.lyric_index = None;
            self.details_task = None;
            return;
        };
        if self.track.as_ref().is_some_and(|track| track.path == path) {
            return;
        }

        let known = self.tags.get(&path).cloned();
        self.track = Some(TrackInfo {
            title: known
                .as_ref()
                .map_or_else(|| media_paths::display_stem(&path), |tags| tags.display_title(&path))
                .into(),
            artist: known.as_ref().and_then(|tags| tags.artist.clone()).map(Into::into),
            album: known.as_ref().and_then(|tags| tags.album.clone()).map(Into::into),
            cover: None,
            lyrics_source: None,
            path: path.clone(),
        });
        self.lyrics = Arc::default();
        self.lyric_index = None;

        let link = self.lyrics_links.get(&path).cloned();
        self.details_task = Some(cx.spawn(async move |this, cx| {
            let lookup = path.clone();
            let details = cx
                .background_executor()
                .spawn(async move { media::read_details(&lookup, link.as_deref()) })
                .await;
            this.update(cx, |model, cx| {
                let Some(track) = model.track.as_mut().filter(|track| track.path == path) else {
                    return;
                };
                track.title = details.tags.display_title(&path).into();
                track.artist = details.tags.artist.clone().map(Into::into);
                track.album = details.tags.album.clone().map(Into::into);
                track.cover = details.cover.map(Cover::into_image);
                track.lyrics_source = details.lyrics_source;
                model.lyrics = Arc::new(details.lyrics);
                if model.duration.is_zero()
                    && let Some(ms) = details.tags.duration_ms
                {
                    model.duration = Duration::from_millis(ms);
                }
                model.tags.insert(path, Arc::new(details.tags));
                model.tags_version += 1;
                model.set_position(model.position, cx);
                model.changed(cx);
            })
            .ok();
        }));
    }

    /// Reads tags of `paths` not seen yet, in small background batches.
    fn start_tag_probe(&mut self, paths: Vec<PathBuf>, _cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| !self.tags.contains_key(path) && self.probing.insert(path.clone()))
            .collect();
        if !paths.is_empty() {
            let _ = self.probe_requests.try_send(paths);
        }
    }

    fn on_tags_read(&mut self, tags: Vec<(Option<TrackTags>, PathBuf)>, cx: &mut Context<Self>) {
        self.tags_version += 1;
        for (tags, path) in tags {
            self.probing.remove(&path);
            if !path.exists() {
                self.issues.entry(path.clone()).or_insert(TrackIssue::Missing);
            }
            self.tags.insert(path, Arc::new(tags.unwrap_or_default()));
        }
        if self.duration.is_zero()
            && let Some(duration) = self.current_index().and_then(|i| self.track_duration(i))
        {
            self.duration = duration;
        }
        cx.notify();
    }

    fn on_audio_event(&mut self, event: AudioEvent, cx: &mut Context<Self>) {
        match event {
            AudioEvent::Loaded { id, duration } if id == self.load_id => {
                self.consecutive_failures = 0;
                if let Some(path) = self.current_path().map(Path::to_path_buf) {
                    self.issues.remove(&path);
                    if self.status == Status::Playing {
                        cx.emit(PlayerEvent::TrackStarted(path));
                    }
                }
                if let Some(duration) = duration.filter(|d| !d.is_zero()) {
                    self.duration = duration;
                }
                cx.notify();
            }
            AudioEvent::LoadFailed { id, error } if id == self.load_id => {
                self.loaded = false;
                self.on_load_failed(error, cx);
            }
            AudioEvent::Advanced { from, id, duration } if from == self.load_id => {
                let Some(preload) = self.preload.take().filter(|preload| preload.id == id) else {
                    // The engine moved on to a track that was withdrawn meanwhile: the current
                    // track has ended, so continue the way the queue says.
                    self.on_track_finished(cx);
                    return;
                };
                let landed = self.queue.on_track_end(&mut self.rng);
                if landed != Some(TrackEnd::Play(preload.index)) || self.current_path() != Some(&preload.path)
                {
                    self.queue.select(preload.index);
                }
                self.load_id = id;
                self.consecutive_failures = 0;
                self.duration = duration
                    .filter(|d| !d.is_zero())
                    .or_else(|| self.track_duration(preload.index))
                    .unwrap_or_default();
                self.set_position(Duration::ZERO, cx);
                self.refresh_track(cx);
                cx.emit(PlayerEvent::TrackStarted(preload.path));
                self.changed(cx);
            }
            AudioEvent::PreloadFailed { id, error } => {
                if let Some(preload) = self.preload.as_ref().filter(|preload| preload.id == id) {
                    if let Some(issue) = issue_for(&error) {
                        self.issues.insert(preload.path.clone(), issue);
                    }
                    // Keep the entry so it isn't retried; the normal path takes over at the end.
                    cx.notify();
                }
            }
            AudioEvent::Position { id, position } if id == self.load_id => {
                self.set_position(position, cx);
                self.sync_preload();
            }
            AudioEvent::Playing { id, playing } if id == self.load_id => {
                let status = if playing { Status::Playing } else { Status::Paused };
                if status != self.status {
                    self.status = status;
                    self.sync_preload();
                    self.changed(cx);
                }
            }
            AudioEvent::TrackEnded { id } if id == self.load_id => self.on_track_finished(cx),
            AudioEvent::DeviceLost(error) => {
                log::error!("audio device lost: {error}");
                if self.loaded {
                    self.resume_after_device_loss = Some((self.position, self.status == Status::Playing));
                }
                self.loaded = false;
                self.preload = None;
                self.status = Status::Paused;
                self.device_problem = Some(Problem::DeviceLost);
                cx.emit(PlayerEvent::Problem(Problem::DeviceLost));
                self.changed(cx);
            }
            // Events from a superseded load.
            _ => {}
        }
    }

    /// The current track played to its end (without a preloaded follower taking over).
    fn on_track_finished(&mut self, cx: &mut Context<Self>) {
        self.preload = None;
        match self.queue.on_track_end(&mut self.rng) {
            Some(TrackEnd::Repeat | TrackEnd::Play(_)) => self.load_current(true, cx),
            None => {
                self.loaded = false;
                self.status = Status::Stopped;
                self.set_position(Duration::ZERO, cx);
                self.changed(cx);
            }
        }
    }

    fn on_load_failed(&mut self, error: LoadError, cx: &mut Context<Self>) {
        let Some(issue) = issue_for(&error) else {
            // The device failed, not the track: stop and let the user retry.
            let LoadError::Device(message) = error else { return };
            if self.resume_after_device_loss.is_none() {
                self.resume_after_device_loss = Some((self.position, self.status == Status::Playing));
            }
            self.status = Status::Stopped;
            let problem = Problem::DeviceUnavailable(message);
            self.device_problem = Some(problem.clone());
            cx.emit(PlayerEvent::Problem(problem));
            self.changed(cx);
            return;
        };
        if let Some(path) = self.current_path().map(Path::to_path_buf) {
            self.issues.insert(path.clone(), issue.clone());
            cx.emit(PlayerEvent::Problem(Problem::Skipped { path, issue }));
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures < self.queue.len() && self.queue.next(&mut self.rng).is_some() {
            self.load_current(true, cx);
        } else {
            self.consecutive_failures = 0;
            self.status = Status::Stopped;
            if self.queue.len() > 1 {
                cx.emit(PlayerEvent::Problem(Problem::NothingPlayable));
            }
            self.changed(cx);
        }
    }
}

fn issue_for(error: &LoadError) -> Option<TrackIssue> {
    match error {
        LoadError::Missing => Some(TrackIssue::Missing),
        LoadError::Unreadable(message) => Some(TrackIssue::Unreadable(message.clone())),
        LoadError::Device(_) => None,
    }
}
