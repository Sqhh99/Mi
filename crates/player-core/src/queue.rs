//! Playback queue: track order, repeat modes, shuffle with back/forward history, and the
//! edits the queue view makes (append, play next, remove, reorder).

use std::ops::Range;
use std::path::{Path, PathBuf};

use fastrand::Rng;
use serde::{Deserialize, Serialize};

/// What happens when playback reaches the end of a track on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    /// Stop after the last track (in shuffle: after every track has played once).
    Off,
    /// Start over from the beginning (in shuffle: start a new round).
    #[default]
    All,
    /// Replay the current track.
    One,
}

impl RepeatMode {
    /// The next mode for a repeat button that cycles off → all → one.
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

/// What the player should do after the user presses "previous".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Previous {
    /// Load and play the track at this index.
    Play(usize),
    /// Seek the current track back to the start (shuffle mode with no history left).
    Restart,
}

/// What the player should do when the current track finishes on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackEnd {
    /// Replay the current track (repeat-one is on).
    Repeat,
    /// Load and play the track at this index.
    Play(usize),
}

/// Result of removing tracks from the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    /// The current track was kept; nothing needs to change in the player.
    Other,
    /// The current track was removed. Play the given index, or stop if the queue is now empty.
    Current(Option<usize>),
}

#[derive(Debug, Default, Clone)]
pub struct PlayQueue {
    tracks: Vec<PathBuf>,
    current: usize,
    repeat: RepeatMode,
    shuffle: bool,
    /// Shuffle only: tracks visited this round, in order. Walking back and forth moves
    /// `history_pos`; new random picks come from tracks not in here yet.
    history: Vec<usize>,
    history_pos: Option<usize>,
}

impl PlayQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracks(&self) -> &[PathBuf] {
        &self.tracks
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Index of the current track, or `None` when the queue is empty.
    pub fn current(&self) -> Option<usize> {
        (!self.tracks.is_empty()).then_some(self.current)
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.tracks.get(self.current).map(PathBuf::as_path)
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    pub fn set_shuffle(&mut self, enabled: bool) {
        self.shuffle = enabled;
        self.reset_history();
    }

    /// Replaces all tracks and selects the one at `start` (or the first).
    pub fn set_tracks(&mut self, tracks: Vec<PathBuf>, start: usize) {
        self.current = if start < tracks.len() { start } else { 0 };
        self.tracks = tracks;
        self.reset_history();
    }

    pub fn clear(&mut self) {
        self.set_tracks(Vec::new(), 0);
    }

    /// Selects a track explicitly (e.g. clicked in the queue). Returns false if out of range.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.tracks.len() {
            return false;
        }
        self.current = index;
        self.record(index);
        true
    }

    /// Adds tracks at the end and returns their indices. An empty queue selects the first.
    pub fn append(&mut self, paths: Vec<PathBuf>) -> Range<usize> {
        let at = self.tracks.len();
        self.insert(at, paths)
    }

    /// Inserts tracks right after the current one ("play next") and returns their indices.
    pub fn insert_next(&mut self, paths: Vec<PathBuf>) -> Range<usize> {
        let at = self.current().map_or(0, |current| current + 1);
        self.insert(at, paths)
    }

    /// Inserts tracks before index `at` (clamped to the end) and returns their indices.
    pub fn insert(&mut self, at: usize, paths: Vec<PathBuf>) -> Range<usize> {
        let at = at.min(self.tracks.len());
        let count = paths.len();
        let was_empty = self.tracks.is_empty();
        self.tracks.splice(at..at, paths);
        if was_empty {
            self.current = 0;
            self.reset_history();
        } else {
            let shift = |index: usize| if index >= at { index + count } else { index };
            self.current = shift(self.current);
            self.history.iter_mut().for_each(|index| *index = shift(*index));
        }
        at..at + count
    }

    /// Removes the tracks at `indices` (any order, duplicates and out-of-range ignored).
    ///
    /// When the current track goes, the track that followed it becomes current (wrapping to
    /// the first). Returns `None` if nothing was removed.
    pub fn remove(&mut self, indices: &[usize]) -> Option<Removed> {
        let len = self.tracks.len();
        let mut remove = vec![false; len];
        for &index in indices.iter().filter(|&&index| index < len) {
            remove[index] = true;
        }
        if !remove.contains(&true) {
            return None;
        }

        // Old index → new index for every kept track.
        let mut mapping = vec![None; len];
        let mut kept = Vec::with_capacity(len);
        for (index, track) in std::mem::take(&mut self.tracks).into_iter().enumerate() {
            if !remove[index] {
                mapping[index] = Some(kept.len());
                kept.push(track);
            }
        }
        self.tracks = kept;

        let removed_current = remove[self.current];
        if self.tracks.is_empty() {
            self.current = 0;
            self.reset_history();
            return Some(Removed::Current(None));
        }
        if removed_current {
            self.current = mapping[self.current..].iter().flatten().next().copied().unwrap_or(0);
            self.reset_history();
            return Some(Removed::Current(Some(self.current)));
        }
        self.current = mapping[self.current].unwrap_or(0);
        self.remap_history(|index| mapping.get(index).copied().flatten());
        Some(Removed::Other)
    }

    /// Moves the tracks at `indices` so they sit, in their current order, where the track at
    /// `before` is now (`before == len` moves them to the end). Returns their new indices.
    pub fn move_tracks(&mut self, indices: &[usize], before: usize) -> Range<usize> {
        let len = self.tracks.len();
        let mut moving = vec![false; len];
        for &index in indices.iter().filter(|&&index| index < len) {
            moving[index] = true;
        }
        let moved: Vec<usize> = (0..len).filter(|&index| moving[index]).collect();
        let rest: Vec<usize> = (0..len).filter(|&index| !moving[index]).collect();
        let insert_at = rest.iter().take_while(|&&index| index < before).count();

        // new position → old index
        let order: Vec<usize> =
            rest[..insert_at].iter().chain(&moved).chain(&rest[insert_at..]).copied().collect();
        let mut mapping = vec![0; len];
        for (new, &old) in order.iter().enumerate() {
            mapping[old] = new;
        }

        let mut old_tracks: Vec<Option<PathBuf>> =
            std::mem::take(&mut self.tracks).into_iter().map(Some).collect();
        self.tracks = order.iter().filter_map(|&old| old_tracks[old].take()).collect();
        if len > 0 {
            self.current = mapping[self.current];
        }
        self.remap_history(|index| mapping.get(index).copied());
        insert_at..insert_at + moved.len()
    }

    /// Points the track at `index` to another file (e.g. one the user located after a move).
    pub fn replace(&mut self, index: usize, path: PathBuf) -> bool {
        match self.tracks.get_mut(index) {
            Some(track) => {
                *track = path;
                true
            }
            None => false,
        }
    }

    /// Advances to the next track because the user asked to, and returns its index.
    ///
    /// Always moves somewhere: the end of the queue wraps around regardless of the repeat
    /// mode. In shuffle mode this first walks forward through history (after "previous" was
    /// used), then picks a random track that hasn't been played this round.
    pub fn next(&mut self, rng: &mut Rng) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if self.shuffle {
            if !self.step_forward_in_history() && !self.pick_unvisited(rng) {
                self.start_new_round(rng);
            }
        } else {
            self.current = (self.current + 1) % self.tracks.len();
        }
        Some(self.current)
    }

    pub fn previous(&mut self) -> Option<Previous> {
        if self.tracks.is_empty() {
            return None;
        }
        if self.shuffle {
            return Some(match self.history_pos {
                Some(pos) if pos > 0 => {
                    self.history_pos = Some(pos - 1);
                    self.current = self.history[pos - 1];
                    Previous::Play(self.current)
                }
                _ => Previous::Restart,
            });
        }
        let len = self.tracks.len();
        self.current = (self.current + len - 1) % len;
        Some(Previous::Play(self.current))
    }

    /// Decides what follows a track that finished on its own; `None` means stop.
    pub fn on_track_end(&mut self, rng: &mut Rng) -> Option<TrackEnd> {
        if self.tracks.is_empty() {
            return None;
        }
        if self.repeat == RepeatMode::One {
            return Some(TrackEnd::Repeat);
        }
        if self.shuffle {
            if self.step_forward_in_history() || self.pick_unvisited(rng) {
                return Some(TrackEnd::Play(self.current));
            }
            if self.repeat == RepeatMode::Off {
                return None;
            }
            self.start_new_round(rng);
            return Some(TrackEnd::Play(self.current));
        }
        if self.current + 1 < self.tracks.len() {
            self.current += 1;
        } else if self.repeat == RepeatMode::Off {
            return None;
        } else {
            self.current = 0;
        }
        Some(TrackEnd::Play(self.current))
    }

    /// The track `on_track_end` would play next, without changing anything. Shuffle picks are
    /// random, so a peek in shuffle mode only sees tracks already in the history.
    pub fn peek_track_end(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if self.repeat == RepeatMode::One {
            return Some(self.current);
        }
        if self.shuffle {
            let pos = self.history_pos?;
            return self.history.get(pos + 1).copied();
        }
        if self.current + 1 < self.tracks.len() {
            Some(self.current + 1)
        } else if self.repeat == RepeatMode::Off {
            None
        } else {
            Some(0)
        }
    }

    /// Like [`peek_track_end`](Self::peek_track_end), but in shuffle mode it settles the
    /// random pick now, as a "forward" history entry that `on_track_end` will then follow, so
    /// the track can be preloaded. Returns `None` where the choice can't be made ahead of
    /// time (stopping, or the start of a new shuffle round).
    pub fn plan_track_end(&mut self, rng: &mut Rng) -> Option<usize> {
        if let Some(next) = self.peek_track_end() {
            return Some(next);
        }
        if !self.shuffle || self.repeat == RepeatMode::One {
            return None;
        }
        let pos = self.history_pos?;
        let candidates = self.unvisited();
        if candidates.is_empty() {
            return None;
        }
        let pick = candidates[rng.usize(0..candidates.len())];
        self.history.truncate(pos + 1);
        self.history.push(pick);
        Some(pick)
    }

    fn step_forward_in_history(&mut self) -> bool {
        match self.history_pos {
            Some(pos) if pos + 1 < self.history.len() => {
                self.history_pos = Some(pos + 1);
                self.current = self.history[pos + 1];
                true
            }
            _ => false,
        }
    }

    /// Moves to a random track not yet played this round. False if every track has played.
    fn pick_unvisited(&mut self, rng: &mut Rng) -> bool {
        let candidates = self.unvisited();
        if candidates.is_empty() {
            return false;
        }
        self.current = candidates[rng.usize(0..candidates.len())];
        self.record(self.current);
        true
    }

    fn unvisited(&self) -> Vec<usize> {
        let mut visited = vec![false; self.tracks.len()];
        for &index in &self.history {
            if let Some(slot) = visited.get_mut(index) {
                *slot = true;
            }
        }
        (0..self.tracks.len()).filter(|&index| !visited[index]).collect()
    }

    /// Every track has played: forget the round and pick a track other than the current one.
    fn start_new_round(&mut self, rng: &mut Rng) {
        self.reset_history();
        let len = self.tracks.len();
        if len > 1 {
            // Pick from the other len-1 tracks without retrying.
            let pick = rng.usize(0..len - 1);
            self.current = if pick >= self.current { pick + 1 } else { pick };
        }
        self.record(self.current);
    }

    fn reset_history(&mut self) {
        self.history.clear();
        self.history_pos = None;
        if !self.tracks.is_empty() {
            self.record(self.current);
        }
    }

    /// Rewrites history indices after an edit, dropping tracks that no longer exist.
    fn remap_history(&mut self, map: impl Fn(usize) -> Option<usize>) {
        if !self.shuffle {
            return;
        }
        let pos = self.history_pos.unwrap_or(0);
        let mut history = Vec::with_capacity(self.history.len());
        let mut new_pos = None;
        for (i, &index) in self.history.iter().enumerate() {
            if let Some(mapped) = map(index) {
                history.push(mapped);
            }
            if i == pos {
                new_pos = history.len().checked_sub(1);
            }
        }
        self.history = history;
        self.history_pos = new_pos;
        if self.history_pos.and_then(|pos| self.history.get(pos)) != Some(&self.current) {
            self.reset_history();
        }
    }

    /// Records a visit in shuffle history, discarding any "forward" entries.
    fn record(&mut self, index: usize) {
        if !self.shuffle || index >= self.tracks.len() {
            return;
        }
        if let Some(pos) = self.history_pos
            && self.history.get(pos) == Some(&index)
        {
            return;
        }
        let keep = self.history_pos.map_or(0, |pos| pos + 1);
        self.history.truncate(keep);
        self.history.push(index);
        self.history_pos = Some(self.history.len() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|name| PathBuf::from(format!("{name}.mp3"))).collect()
    }

    fn queue(n: usize) -> PlayQueue {
        let mut q = PlayQueue::new();
        q.set_tracks((0..n).map(|i| PathBuf::from(format!("{i}.mp3"))).collect(), 0);
        q
    }

    fn names(q: &PlayQueue) -> Vec<String> {
        q.tracks().iter().map(|p| p.file_stem().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn sequential_wraps_both_ways() {
        let mut rng = Rng::with_seed(1);
        let mut q = queue(3);
        assert_eq!(q.current(), Some(0));
        assert_eq!(q.next(&mut rng), Some(1));
        assert_eq!(q.next(&mut rng), Some(2));
        assert_eq!(q.next(&mut rng), Some(0));
        assert_eq!(q.previous(), Some(Previous::Play(2)));
    }

    #[test]
    fn empty_queue_is_inert() {
        let mut rng = Rng::with_seed(1);
        let mut q = PlayQueue::new();
        assert_eq!(q.current(), None);
        assert_eq!(q.next(&mut rng), None);
        assert_eq!(q.previous(), None);
        assert_eq!(q.on_track_end(&mut rng), None);
        assert_eq!(q.peek_track_end(), None);
        assert_eq!(q.remove(&[0]), None);
        assert!(!q.select(0));
        assert_eq!(q.move_tracks(&[0], 0), 0..0);
    }

    #[test]
    fn set_tracks_starts_at_index() {
        let mut q = PlayQueue::new();
        q.set_tracks(paths(&["a", "b", "c"]), 2);
        assert_eq!(q.current(), Some(2));
        q.set_tracks(paths(&["a"]), 5);
        assert_eq!(q.current(), Some(0));
    }

    #[test]
    fn repeat_one_replays() {
        let mut rng = Rng::with_seed(1);
        let mut q = queue(3);
        q.set_repeat(RepeatMode::One);
        assert_eq!(q.on_track_end(&mut rng), Some(TrackEnd::Repeat));
        // Manual skipping still moves on.
        assert_eq!(q.next(&mut rng), Some(1));
        q.set_repeat(RepeatMode::All);
        assert_eq!(q.on_track_end(&mut rng), Some(TrackEnd::Play(2)));
    }

    #[test]
    fn repeat_all_wraps_and_off_stops_at_end() {
        let mut rng = Rng::with_seed(1);
        let mut q = queue(2);
        q.select(1);
        assert_eq!(q.peek_track_end(), Some(0));
        assert_eq!(q.on_track_end(&mut rng), Some(TrackEnd::Play(0)));

        q.select(1);
        q.set_repeat(RepeatMode::Off);
        assert_eq!(q.peek_track_end(), None);
        assert_eq!(q.on_track_end(&mut rng), None);
        assert_eq!(q.current(), Some(1));
        // The next button still wraps.
        assert_eq!(q.next(&mut rng), Some(0));
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(RepeatMode::Off.cycle(), RepeatMode::All);
        assert_eq!(RepeatMode::All.cycle(), RepeatMode::One);
        assert_eq!(RepeatMode::One.cycle(), RepeatMode::Off);
    }

    #[test]
    fn shuffle_plays_every_track_once_per_round() {
        let mut rng = Rng::with_seed(42);
        let mut q = queue(6);
        q.set_shuffle(true);
        let mut seen = vec![q.current().unwrap()];
        for _ in 0..5 {
            match q.on_track_end(&mut rng) {
                Some(TrackEnd::Play(index)) => seen.push(index),
                other => panic!("unexpected {other:?}"),
            }
        }
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5]);

        // Repeat all: a new round starts, never with the track that just played.
        let last = q.current().unwrap();
        assert!(matches!(q.on_track_end(&mut rng), Some(TrackEnd::Play(i)) if i != last));
    }

    #[test]
    fn shuffle_with_repeat_off_stops_after_round() {
        let mut rng = Rng::with_seed(5);
        let mut q = queue(3);
        q.set_shuffle(true);
        q.set_repeat(RepeatMode::Off);
        assert!(q.on_track_end(&mut rng).is_some());
        assert!(q.on_track_end(&mut rng).is_some());
        assert_eq!(q.on_track_end(&mut rng), None);
    }

    #[test]
    fn planned_shuffle_pick_is_played_next() {
        let mut rng = Rng::with_seed(21);
        let mut q = queue(8);
        q.set_shuffle(true);
        let planned = q.plan_track_end(&mut rng).unwrap();
        assert_ne!(Some(planned), q.current());
        // Planning again is stable, and peeking sees the plan.
        assert_eq!(q.plan_track_end(&mut rng), Some(planned));
        assert_eq!(q.peek_track_end(), Some(planned));
        assert_eq!(q.on_track_end(&mut rng), Some(TrackEnd::Play(planned)));

        // Sequential planning is just a peek; the last track with repeat off has no plan.
        let mut q = queue(2);
        q.select(1);
        q.set_repeat(RepeatMode::Off);
        assert_eq!(q.plan_track_end(&mut rng), None);
    }

    #[test]
    fn shuffle_never_repeats_current() {
        let mut rng = Rng::with_seed(42);
        let mut q = queue(5);
        q.set_shuffle(true);
        for _ in 0..200 {
            let before = q.current().unwrap();
            let after = q.next(&mut rng).unwrap();
            assert_ne!(before, after);
        }
    }

    #[test]
    fn shuffle_single_track_stays() {
        let mut rng = Rng::with_seed(42);
        let mut q = queue(1);
        q.set_shuffle(true);
        assert_eq!(q.next(&mut rng), Some(0));
    }

    #[test]
    fn shuffle_history_back_and_forward() {
        let mut rng = Rng::with_seed(7);
        let mut q = queue(10);
        q.set_shuffle(true);
        let a = q.current().unwrap();
        let b = q.next(&mut rng).unwrap();
        let c = q.next(&mut rng).unwrap();

        assert_eq!(q.previous(), Some(Previous::Play(b)));
        assert_eq!(q.peek_track_end(), Some(c));
        assert_eq!(q.previous(), Some(Previous::Play(a)));
        assert_eq!(q.previous(), Some(Previous::Restart));
        assert_eq!(q.current(), Some(a));

        // Forward replays history before picking new random tracks.
        assert_eq!(q.next(&mut rng), Some(b));
        assert_eq!(q.next(&mut rng), Some(c));
    }

    #[test]
    fn shuffle_select_truncates_forward_history() {
        let mut rng = Rng::with_seed(3);
        let mut q = queue(10);
        q.set_shuffle(true);
        let a = q.current().unwrap();
        let b = q.next(&mut rng).unwrap();
        q.next(&mut rng);
        assert_eq!(q.previous(), Some(Previous::Play(b)));

        let picked = (0..10).find(|i| ![a, b].contains(i)).unwrap();
        assert!(q.select(picked));
        assert_eq!(q.previous(), Some(Previous::Play(b)));
        assert_eq!(q.next(&mut rng), Some(picked));
    }

    #[test]
    fn append_and_insert_next() {
        let mut q = PlayQueue::new();
        assert_eq!(q.append(paths(&["a", "b"])), 0..2);
        assert_eq!(q.current(), Some(0));
        q.select(1);
        assert_eq!(q.insert_next(paths(&["x", "y"])), 2..4);
        assert_eq!(names(&q), ["a", "b", "x", "y"]);
        assert_eq!(q.insert(0, paths(&["z"])), 0..1);
        assert_eq!(names(&q), ["z", "a", "b", "x", "y"]);
        assert_eq!(q.current_path(), Some(Path::new("b.mp3")));

        let mut empty = PlayQueue::new();
        assert_eq!(empty.insert_next(paths(&["n"])), 0..1);
        assert_eq!(empty.current(), Some(0));
    }

    #[test]
    fn insert_keeps_shuffle_history() {
        let mut rng = Rng::with_seed(11);
        let mut q = queue(5);
        q.set_shuffle(true);
        let a = q.current_path().unwrap().to_path_buf();
        q.next(&mut rng);
        q.insert(0, paths(&["new"]));
        assert_eq!(q.previous().map(|_| q.current_path().unwrap().to_path_buf()), Some(a));
    }

    #[test]
    fn remove_before_current_shifts_index() {
        let mut q = queue(4);
        q.select(2);
        assert_eq!(q.remove(&[0]), Some(Removed::Other));
        assert_eq!(q.current(), Some(1));
        assert_eq!(q.current_path(), Some(Path::new("2.mp3")));
    }

    #[test]
    fn remove_after_current_keeps_index() {
        let mut q = queue(4);
        q.select(1);
        assert_eq!(q.remove(&[3]), Some(Removed::Other));
        assert_eq!(q.current(), Some(1));
    }

    #[test]
    fn remove_current_moves_to_following_track() {
        let mut q = queue(3);
        q.select(1);
        assert_eq!(q.remove(&[1]), Some(Removed::Current(Some(1))));
        assert_eq!(q.current_path(), Some(Path::new("2.mp3")));

        // Removing the last track wraps to the first.
        q.select(1);
        assert_eq!(q.remove(&[1]), Some(Removed::Current(Some(0))));
    }

    #[test]
    fn remove_many_ignores_duplicates_and_out_of_range() {
        let mut q = queue(6);
        q.select(3);
        assert_eq!(q.remove(&[5, 0, 0, 3, 99, 4]), Some(Removed::Current(Some(0))));
        assert_eq!(names(&q), ["1", "2"]);
        assert_eq!(q.remove(&[42]), None);
    }

    #[test]
    fn remove_only_track_empties_queue() {
        let mut q = queue(1);
        assert_eq!(q.remove(&[0]), Some(Removed::Current(None)));
        assert!(q.is_empty());
        assert_eq!(q.current(), None);
    }

    #[test]
    fn remove_other_keeps_shuffle_history() {
        let mut rng = Rng::with_seed(9);
        let mut q = queue(6);
        q.set_shuffle(true);
        let first = q.current_path().unwrap().to_path_buf();
        q.next(&mut rng);
        let other = (0..6).find(|&i| q.tracks()[i] != first && Some(i) != q.current()).unwrap();
        q.remove(&[other]);
        assert!(matches!(q.previous(), Some(Previous::Play(_))));
        assert_eq!(q.current_path(), Some(first.as_path()));
    }

    #[test]
    fn remove_current_resets_shuffle_history() {
        let mut rng = Rng::with_seed(9);
        let mut q = queue(6);
        q.set_shuffle(true);
        q.next(&mut rng);
        q.next(&mut rng);
        q.remove(&[q.current().unwrap()]);
        assert_eq!(q.previous(), Some(Previous::Restart));
    }

    #[test]
    fn move_tracks_reorders_and_follows_current() {
        let mut q = PlayQueue::new();
        q.set_tracks(paths(&["a", "b", "c", "d", "e"]), 1);

        // Drag "d" before "b".
        assert_eq!(q.move_tracks(&[3], 1), 1..2);
        assert_eq!(names(&q), ["a", "d", "b", "c", "e"]);
        assert_eq!(q.current_path(), Some(Path::new("b.mp3")));

        // Move "a" and "c" to the end, keeping their order.
        assert_eq!(q.move_tracks(&[3, 0], 5), 3..5);
        assert_eq!(names(&q), ["d", "b", "e", "a", "c"]);
        assert_eq!(q.current(), Some(1));

        // Dropping a block onto one of its own rows leaves the order alone.
        assert_eq!(q.move_tracks(&[3, 4], 4), 3..5);
        assert_eq!(names(&q), ["d", "b", "e", "a", "c"]);
    }

    #[test]
    fn replace_and_clear() {
        let mut q = queue(2);
        assert!(q.replace(1, PathBuf::from("moved.mp3")));
        assert!(!q.replace(5, PathBuf::from("x.mp3")));
        assert_eq!(q.tracks()[1], PathBuf::from("moved.mp3"));
        q.clear();
        assert!(q.is_empty());
    }
}
