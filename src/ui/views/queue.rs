//! The play queue overlay: search, multi-selection, drag-to-reorder, and per-track actions
//! (play, play next, remove, locate a moved file, retry).

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, ClickEvent, Context, Div, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Task,
    UniformListScrollHandle, Window, WindowControlArea, actions, div, prelude::FluentBuilder, px, svg,
    uniform_list,
};
use player_core::i18n::Key;
use player_core::media_paths;
use player_core::time::{UNKNOWN_DURATION, fmt_duration, fmt_long_duration};
use player_core::track::search_terms;

use crate::model::{PlayerEvent, PlayerModel, SettingsModel, Status, TrackIssue};
use crate::platform::NativeWindow;
use crate::ui::dialogs::{self, Pick};
use crate::ui::icons::Icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::{IconButton, Menu, TextInput, TextInputEvent};
use crate::ui::{t, tf};

const HEADER_HEIGHT: f32 = 64.0;
const ROW_HEIGHT: f32 = 54.0;
const ROW_GAP: f32 = 6.0;
const KEY_CONTEXT: &str = "Queue";

actions!(queue, [RemoveSelected, SelectAllRows, PlaySelected, Dismiss, FocusSearch]);

pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("delete", RemoveSelected, context),
        KeyBinding::new("backspace", RemoveSelected, context),
        KeyBinding::new("secondary-a", SelectAllRows, context),
        KeyBinding::new("enter", PlaySelected, context),
        KeyBinding::new("escape", Dismiss, context),
        KeyBinding::new("secondary-f", FocusSearch, context),
    ]);
}

pub enum QueueEvent {
    Close,
    Toast(SharedString),
}

/// How imported files join the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Replace the queue and start playing.
    PlayNow,
    /// Insert after the current track.
    PlayNext,
    /// Add at the end.
    Append,
    /// Add at the end and play the first added track.
    AppendAndPlay,
    /// Insert before this queue index.
    InsertAt(usize),
}

/// Expands `paths` (files and folders) in the background and adds the audio files found.
/// Resolves to a message for the user.
pub fn import_paths(
    player: Entity<PlayerModel>,
    paths: Vec<PathBuf>,
    mode: ImportMode,
    cx: &mut App,
) -> Task<Option<SharedString>> {
    cx.spawn(async move |cx| {
        let tracks =
            cx.background_executor().spawn(async move { media_paths::collect_audio_files(&paths) }).await;
        cx.update(|cx| {
            if tracks.is_empty() {
                return Some(t(cx, Key::ToastNoAudio).into());
            }
            let count = tracks.len().to_string();
            player.update(cx, |player, cx| match mode {
                ImportMode::PlayNow => {
                    player.play_tracks(tracks, 0, cx);
                    None
                }
                ImportMode::PlayNext => {
                    player.play_next(tracks, cx);
                    Some(tf(cx, Key::ToastPlayNext, &[("n", &count)]).into())
                }
                ImportMode::Append => {
                    player.enqueue(tracks, cx);
                    Some(tf(cx, Key::ToastAdded, &[("n", &count)]).into())
                }
                ImportMode::AppendAndPlay => {
                    let range = player.enqueue(tracks, cx);
                    player.play_index(range.start, cx);
                    None
                }
                ImportMode::InsertAt(at) => {
                    player.insert_tracks(at, tracks, cx);
                    Some(tf(cx, Key::ToastAdded, &[("n", &count)]).into())
                }
            })
        })
        .ok()
        .flatten()
    })
}

/// Rows being dragged to a new place in the queue.
#[derive(Clone)]
struct DraggedTracks {
    indices: Vec<usize>,
    label: SharedString,
}

impl DraggedTracks {
    /// Whether dropping on `target` moves the rows down (they then land after it).
    fn moves_down_to(&self, target: usize) -> bool {
        self.indices.iter().all(|&index| index < target)
    }
}

struct DragPreview {
    label: SharedString,
    position: Point<Pixels>,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div().pl(self.position.x - px(24.0)).pt(self.position.y - px(16.0)).child(
            div()
                .max_w(px(280.0))
                .h(px(32.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(10.0))
                .bg(theme.surface_panel_bg)
                .border_1()
                .border_color(theme.accent)
                .text_size(px(12.0))
                .text_color(theme.text_primary)
                .font_family(theme.font_family.clone())
                .child(svg().path(Icon::Music2.path()).size(px(14.0)).text_color(theme.accent))
                .child(div().truncate().child(self.label.clone())),
        )
    }
}

enum QueueMenu {
    Add(Point<Pixels>),
    More(Point<Pixels>),
    Row(Point<Pixels>),
}

pub struct QueueView {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    native: Option<NativeWindow>,
    focus: FocusHandle,
    search: Entity<TextInput>,
    terms: Vec<String>,
    /// Queue indices matching the search, in queue order; `None` when not searching.
    filtered: Option<Vec<usize>>,
    /// Tag reads seen when `filtered` was computed (results change as tags arrive).
    filtered_tags: u64,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    /// Set while this view edits the queue so it can keep its own selection.
    editing: bool,
    menu: Option<QueueMenu>,
    scroll: UniformListScrollHandle,
    task: Option<Task<()>>,
}

impl EventEmitter<QueueEvent> for QueueView {}

impl Focusable for QueueView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueueView {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        native: Option<NativeWindow>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| TextInput::new(t(cx, Key::SearchQueue), cx));
        cx.subscribe(&search, |this, search, event, cx| match event {
            TextInputEvent::Changed => {
                this.terms = search_terms(&search.read(cx).committed_text());
                this.refilter(cx);
            }
            TextInputEvent::Submit => {
                if let Some(index) = this.row_index(0, cx) {
                    this.player.update(cx, |player, cx| player.play_index(index, cx));
                }
            }
            TextInputEvent::Cancel => cx.emit(QueueEvent::Close),
        })
        .detach();
        cx.observe(&player, |this, player, cx| {
            if this.filtered.is_some() && player.read(cx).tags_version() != this.filtered_tags {
                this.refilter(cx);
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&player, |this, _, event, cx| {
            if *event == PlayerEvent::Playlist {
                if !this.editing {
                    this.selected.clear();
                    this.anchor = None;
                }
                this.refilter(cx);
            }
        })
        .detach();
        Self {
            player,
            settings,
            native,
            focus: cx.focus_handle(),
            search,
            terms: Vec::new(),
            filtered: None,
            filtered_tags: 0,
            selected: BTreeSet::new(),
            anchor: None,
            editing: false,
            menu: None,
            scroll: UniformListScrollHandle::new(),
            task: None,
        }
    }

    /// Called when the overlay opens: shows the current track and takes keyboard focus.
    pub fn opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_current(cx);
        window.focus(&self.focus);
        let placeholder = t(cx, Key::SearchQueue);
        self.search.update(cx, |search, _| search.set_placeholder(placeholder));
    }

    pub fn closed(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    fn reveal_current(&self, cx: &App) {
        let Some(current) = self.player.read(cx).current_index() else { return };
        let row = match &self.filtered {
            Some(rows) => rows.iter().position(|&index| index == current),
            None => Some(current),
        };
        if let Some(row) = row {
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        self.filtered_tags = player.tags_version();
        self.filtered = (!self.terms.is_empty()).then(|| {
            player
                .tracks()
                .iter()
                .enumerate()
                .filter(|(_, path)| player.tags(path).cloned().unwrap_or_default().matches(path, &self.terms))
                .map(|(index, _)| index)
                .collect()
        });
        let len = player.tracks().len();
        self.selected.retain(|&index| index < len);
        cx.notify();
    }

    fn row_count(&self, cx: &App) -> usize {
        self.filtered.as_ref().map_or_else(|| self.player.read(cx).tracks().len(), Vec::len)
    }

    fn row_index(&self, row: usize, cx: &App) -> Option<usize> {
        match &self.filtered {
            Some(rows) => rows.get(row).copied(),
            None => (row < self.player.read(cx).tracks().len()).then_some(row),
        }
    }

    fn visible_indices(&self, cx: &App) -> Vec<usize> {
        match &self.filtered {
            Some(rows) => rows.clone(),
            None => (0..self.player.read(cx).tracks().len()).collect(),
        }
    }

    // ── Selection ───────────────────────────────────────────────────────────

    fn on_row_click(
        &mut self,
        index: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus);
        let modifiers = event.modifiers();
        if event.click_count() >= 2 && !modifiers.modified() {
            self.player.update(cx, |player, cx| player.play_index(index, cx));
            return;
        }
        if modifiers.shift
            && let Some(anchor) = self.anchor
        {
            let visible = self.visible_indices(cx);
            let (Some(a), Some(b)) =
                (visible.iter().position(|&i| i == anchor), visible.iter().position(|&i| i == index))
            else {
                return;
            };
            let range = a.min(b)..=a.max(b);
            if !modifiers.secondary() {
                self.selected.clear();
            }
            self.selected.extend(visible[range].iter().copied());
        } else if modifiers.secondary() {
            if !self.selected.remove(&index) {
                self.selected.insert(index);
            }
            self.anchor = Some(index);
        } else {
            self.selected = BTreeSet::from([index]);
            self.anchor = Some(index);
        }
        cx.notify();
    }

    fn open_row_menu(
        &mut self,
        index: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus);
        if !self.selected.contains(&index) {
            self.selected = BTreeSet::from([index]);
            self.anchor = Some(index);
        }
        self.menu = Some(QueueMenu::Row(event.position));
        cx.notify();
    }

    fn selection(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }

    /// Runs a queue edit and then selects the rows it reports.
    fn edit(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut PlayerModel, &mut Context<PlayerModel>) -> Option<Range<usize>>,
    ) {
        self.editing = true;
        let range = self.player.update(cx, f);
        self.editing = false;
        self.selected = range.clone().map(BTreeSet::from_iter).unwrap_or_default();
        self.anchor = range.map(|range| range.start);
        cx.notify();
    }

    // ── Actions ─────────────────────────────────────────────────────────────

    fn remove_selected(&mut self, _: &RemoveSelected, _: &mut Window, cx: &mut Context<Self>) {
        let indices = self.selection();
        if indices.is_empty() {
            return;
        }
        let count = indices.len().to_string();
        self.edit(cx, |player, cx| {
            player.remove(&indices, cx);
            None
        });
        cx.emit(QueueEvent::Toast(tf(cx, Key::ToastRemoved, &[("n", &count)]).into()));
    }

    fn select_all(&mut self, _: &SelectAllRows, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.visible_indices(cx).into_iter().collect();
        cx.notify();
    }

    fn play_selected(&mut self, _: &PlaySelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(&index) = self.selected.first() {
            self.player.update(cx, |player, cx| player.play_index(index, cx));
        }
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        } else if !self.selected.is_empty() {
            self.selected.clear();
            cx.notify();
        } else {
            cx.emit(QueueEvent::Close);
        }
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus_handle(cx));
    }

    fn play_selected_next(&mut self, cx: &mut Context<Self>) {
        let indices = self.selection();
        let Some(current) = self.player.read(cx).current_index() else { return };
        let indices: Vec<usize> = indices.into_iter().filter(|&index| index != current).collect();
        if indices.is_empty() {
            return;
        }
        let count = indices.len().to_string();
        self.edit(cx, |player, cx| Some(player.move_tracks(&indices, current + 1, cx)));
        cx.emit(QueueEvent::Toast(tf(cx, Key::ToastPlayNext, &[("n", &count)]).into()));
    }

    fn drop_tracks(&mut self, dragged: &DraggedTracks, target: usize, cx: &mut Context<Self>) {
        let before = if dragged.moves_down_to(target) { target + 1 } else { target };
        let indices = dragged.indices.clone();
        self.edit(cx, |player, cx| Some(player.move_tracks(&indices, before, cx)));
    }

    fn drop_files(&mut self, paths: &ExternalPaths, target: usize, cx: &mut Context<Self>) {
        self.import(paths.paths().to_vec(), ImportMode::InsertAt(target), cx);
    }

    fn import(&mut self, paths: Vec<PathBuf>, mode: ImportMode, cx: &mut Context<Self>) {
        let task = import_paths(self.player.clone(), paths, mode, cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            if let Some(message) = task.await {
                this.update(cx, |_, cx| cx.emit(QueueEvent::Toast(message))).ok();
            }
        }));
    }

    fn pick_and_import(&mut self, kind: Pick, mode: ImportMode, cx: &mut Context<Self>) {
        let last_dir = self.settings.read(cx).get().last_dir.clone();
        let pick = dialogs::pick(kind, last_dir.as_deref(), self.native, cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            let paths = pick.await;
            let Some(first) = paths.first() else { return };
            let folder =
                if first.is_dir() { Some(first.clone()) } else { first.parent().map(Path::to_path_buf) };
            this.update(cx, |this, cx| {
                this.settings.update(cx, |settings, cx| settings.update(cx, |s| s.last_dir = folder));
                this.import(paths, mode, cx);
            })
            .ok();
        }));
    }

    /// Asks for the new location of a missing track. Other missing tracks from the same
    /// folder are fixed too when a file of the same name sits in the new folder.
    fn locate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(old) = self.player.read(cx).tracks().get(index).cloned() else { return };
        let start = old
            .parent()
            .filter(|dir| dir.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| self.settings.read(cx).get().last_dir.clone());
        let pick = dialogs::pick(Pick::AudioFile, start.as_deref(), self.native, cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            let Some(new) = pick.await.into_iter().next() else { return };
            this.update(cx, |this, cx| this.relocate(old, new, cx)).ok();
        }));
    }

    fn relocate(&mut self, old: PathBuf, new: PathBuf, cx: &mut Context<Self>) {
        let (old_dir, new_dir) = (old.parent().map(Path::to_path_buf), new.parent().map(Path::to_path_buf));
        let player = self.player.read(cx);
        let mut moves: Vec<(usize, PathBuf)> = Vec::new();
        for (index, path) in player.tracks().iter().enumerate() {
            if *path == old {
                moves.push((index, new.clone()));
            } else if matches!(player.issue(path), Some(TrackIssue::Missing))
                && path.parent().map(Path::to_path_buf) == old_dir
                && let (Some(dir), Some(name)) = (&new_dir, path.file_name())
                && dir.join(name).is_file()
            {
                moves.push((index, dir.join(name)));
            }
        }
        let count = moves.len().to_string();
        self.edit(cx, |player, cx| {
            for (index, path) in moves {
                player.relocate(index, path, cx);
            }
            None
        });
        cx.emit(QueueEvent::Toast(tf(cx, Key::ToastRelocated, &[("n", &count)]).into()));
    }

    fn remove_unplayable(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        let indices: Vec<usize> = player
            .tracks()
            .iter()
            .enumerate()
            .filter(|(_, path)| player.issue(path).is_some())
            .map(|(index, _)| index)
            .collect();
        if indices.is_empty() {
            return;
        }
        let count = indices.len().to_string();
        self.edit(cx, |player, cx| {
            player.remove(&indices, cx);
            None
        });
        cx.emit(QueueEvent::Toast(tf(cx, Key::ToastRemoved, &[("n", &count)]).into()));
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let theme = cx.global::<Theme>().clone();
        let entity = cx.entity().downgrade();
        let rows: Vec<usize> = range.filter_map(|row| self.row_index(row, cx)).collect();
        let player = self.player.read(cx);
        let current = player.current_index();
        let status = player.status();
        let selection = self.selection();
        rows.into_iter()
            .filter_map(|index| {
                let path = player.tracks().get(index)?;
                let tags = player.tags(path);
                let issue = player.issue(path).cloned();
                let is_current = current == Some(index);
                let is_selected = self.selected.contains(&index);
                let title =
                    tags.map_or_else(|| media_paths::display_stem(path), |tags| tags.display_title(path));
                let subtitle = tags
                    .and_then(|tags| match (&tags.artist, &tags.album) {
                        (Some(artist), Some(album)) => Some(format!("{artist} · {album}")),
                        (Some(artist), None) => Some(artist.clone()),
                        (None, Some(album)) => Some(album.clone()),
                        (None, None) => None,
                    })
                    .or_else(|| media_paths::folder_name(path))
                    .unwrap_or_default();
                let duration = tags
                    .and_then(|tags| tags.duration_ms)
                    .map(fmt_duration)
                    .unwrap_or_else(|| UNKNOWN_DURATION.to_string());
                let dragged = DraggedTracks {
                    indices: if is_selected { selection.clone() } else { vec![index] },
                    label: if is_selected && selection.len() > 1 {
                        tf(cx, Key::SelectedCount, &[("n", &selection.len().to_string())]).into()
                    } else {
                        title.clone().into()
                    },
                };
                let group: SharedString = format!("queue-row-{index}").into();

                let leading = div()
                    .w(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .map(|el| {
                        if is_current {
                            let icon = if status == Status::Playing { Icon::AudioLines } else { Icon::Pause };
                            el.child(svg().path(icon.path()).size(px(15.0)).text_color(theme.accent))
                        } else {
                            el.child(
                                div()
                                    .group_hover(group.clone(), |s| s.invisible())
                                    .child((index + 1).to_string()),
                            )
                        }
                    });

                let play_entity = entity.clone();
                let remove_entity = entity.clone();
                let click_entity = entity.clone();
                let menu_entity = entity.clone();
                let drop_entity = entity.clone();
                let files_entity = entity.clone();
                Some(
                    div().h(px(ROW_HEIGHT + ROW_GAP)).pb(px(ROW_GAP)).child(
                        div()
                            .id(("queue-row", index))
                            .group(group.clone())
                            .relative()
                            .h(px(ROW_HEIGHT))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .rounded(px(14.0))
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .cursor_pointer()
                            .map(|row| {
                                if is_selected {
                                    row.bg(theme.accent.opacity(if theme.dark { 0.22 } else { 0.14 }))
                                } else if is_current {
                                    row.bg(theme.playlist_row_active_bg)
                                } else {
                                    row.hover(|s| s.bg(theme.playlist_row_hover_bg))
                                }
                            })
                            .on_click(move |event, window, cx| {
                                click_entity
                                    .update(cx, |this, cx| this.on_row_click(index, event, window, cx))
                                    .ok();
                            })
                            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                                menu_entity
                                    .update(cx, |this, cx| this.open_row_menu(index, event, window, cx))
                                    .ok();
                            })
                            .on_drag(dragged, |dragged: &DraggedTracks, position, _, cx| {
                                cx.new(|_| DragPreview { label: dragged.label.clone(), position })
                            })
                            .drag_over::<DraggedTracks>({
                                let accent = theme.accent;
                                move |style, _, _, _| style.border_color(accent).bg(accent.opacity(0.08))
                            })
                            .drag_over::<ExternalPaths>({
                                let accent = theme.accent;
                                move |style, _, _, _| style.border_color(accent)
                            })
                            .on_drop(move |dragged: &DraggedTracks, _, cx| {
                                drop_entity.update(cx, |this, cx| this.drop_tracks(dragged, index, cx)).ok();
                            })
                            .on_drop(move |paths: &ExternalPaths, _, cx| {
                                files_entity.update(cx, |this, cx| this.drop_files(paths, index, cx)).ok();
                            })
                            .child(leading)
                            .when(!is_current, |row| {
                                // A play button takes the place of the number on hover.
                                row.child(
                                    div()
                                        .absolute()
                                        .left(px(8.0))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(28.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .invisible()
                                        .group_hover(group.clone(), |s| s.visible())
                                        .child(
                                            IconButton::new(("queue-play", index), Icon::Play)
                                                .size(26.0, 13.0)
                                                .hover_bg(theme.button_hover_bg)
                                                .on_click(move |_, _, cx| {
                                                    play_entity
                                                        .update(cx, |this, cx| {
                                                            this.player
                                                                .update(cx, |p, cx| p.play_index(index, cx))
                                                        })
                                                        .ok();
                                                }),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if issue.is_some() {
                                                theme.text_muted
                                            } else if is_current {
                                                theme.accent_dark
                                            } else {
                                                theme.text_secondary
                                            })
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.0))
                                            .text_color(theme.text_subtle)
                                            .child(subtitle),
                                    ),
                            )
                            .children(issue.map(|issue| issue_badge(&issue, &theme, cx)))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(px(11.0))
                                    .text_color(theme.text_muted)
                                    .group_hover(group.clone(), |s| s.invisible())
                                    .child(duration),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(6.0))
                                    .top_0()
                                    .bottom_0()
                                    .flex()
                                    .items_center()
                                    .invisible()
                                    .group_hover(group, |s| s.visible())
                                    .child(
                                        IconButton::new(("queue-remove", index), Icon::X)
                                            .size(26.0, 12.0)
                                            .hover_bg(theme.close_hover_bg)
                                            .on_click(move |_, _, cx| {
                                                remove_entity
                                                    .update(cx, |this, cx| {
                                                        this.edit(cx, |player, cx| {
                                                            player.remove(&[index], cx);
                                                            None
                                                        })
                                                    })
                                                    .ok();
                                            }),
                                    ),
                            ),
                    ),
                )
            })
            .collect()
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let player = self.player.read(cx);
        let count = player.tracks().len();
        let total: u64 = player
            .tracks()
            .iter()
            .filter_map(|path| player.tags(path).and_then(|tags| tags.duration_ms))
            .sum();
        let summary = if count == 0 {
            String::new()
        } else {
            let songs = tf(cx, Key::SongCount, &[("n", &count.to_string())]);
            format!("{songs} · {}", fmt_long_duration(total))
        };

        div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .px(px(18.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(t(cx, Key::Playlist)),
                    )
                    .child(div().text_size(px(12.0)).text_color(theme.text_muted).child(summary)),
            )
            .child(div().flex_1())
            .child(self.render_search(cx))
            .child(
                IconButton::new("queue-add", Icon::Plus)
                    .size(30.0, 15.0)
                    .hover_bg(theme.button_hover_bg)
                    .icon_opacity(0.7)
                    .active(matches!(self.menu, Some(QueueMenu::Add(_))))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        this.menu = Some(QueueMenu::Add(event.position()));
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("queue-more", Icon::Ellipsis)
                    .size(30.0, 15.0)
                    .hover_bg(theme.button_hover_bg)
                    .icon_opacity(0.7)
                    .active(matches!(self.menu, Some(QueueMenu::More(_))))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        this.menu = Some(QueueMenu::More(event.position()));
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("queue-close", Icon::X)
                    .size(30.0, 12.0)
                    .hover_bg(theme.close_hover_bg)
                    .icon_opacity(0.6)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(QueueEvent::Close))),
            )
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div()
            .id("queue-search")
            .w(px(220.0))
            .h(px(32.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(10.0))
            .bg(theme.button_bg)
            .border_1()
            .border_color(theme.button_border)
            .text_size(px(12.5))
            .text_color(theme.text_primary)
            .line_height(px(18.0))
            .child(svg().path(Icon::Search.path()).size(px(14.0)).text_color(theme.text_muted))
            .child(self.search.clone())
            .when(!self.terms.is_empty(), |el| {
                el.child(
                    IconButton::new("queue-search-clear", Icon::X)
                        .size(20.0, 10.0)
                        .hover_bg(theme.button_hover_bg)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.search.update(cx, |search, cx| search.clear(cx));
                        })),
                )
            })
    }

    fn render_selection_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.selected.len() < 2 {
            return None;
        }
        let theme = cx.global::<Theme>().clone();
        let count = self.selected.len().to_string();
        let action = |id: &'static str, icon: Icon, label: &'static str| {
            div()
                .id(id)
                .h(px(28.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(theme.text_secondary)
                .hover(|s| s.bg(theme.button_hover_bg))
                .child(svg().path(icon.path()).size(px(13.0)).text_color(theme.icon.opacity(0.75)))
                .child(label)
        };
        Some(
            div()
                .flex_none()
                .mx(px(16.0))
                .mt(px(4.0))
                .h(px(40.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .rounded(px(12.0))
                .bg(theme.accent.opacity(if theme.dark { 0.16 } else { 0.10 }))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(tf(cx, Key::SelectedCount, &[("n", &count)])),
                )
                .child(
                    action("sel-next", Icon::ListStart, t(cx, Key::PlayNext))
                        .on_click(cx.listener(|this, _, _, cx| this.play_selected_next(cx))),
                )
                .child(action("sel-remove", Icon::Trash2, t(cx, Key::RemoveFromQueue)).on_click(
                    cx.listener(|this, _, window, cx| this.remove_selected(&RemoveSelected, window, cx)),
                ))
                .child(action("sel-clear", Icon::X, t(cx, Key::ClearSelection)).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.selected.clear();
                        cx.notify();
                    },
                ))),
        )
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<Menu> {
        let menu = self.menu.as_ref()?;
        let entity = cx.entity().downgrade();
        let dismiss = {
            let entity = entity.clone();
            move |_: &mut Window, cx: &mut App| {
                entity.update(cx, |this, cx| this.close_menu(cx)).ok();
            }
        };
        let run = |f: fn(&mut QueueView, &mut Context<QueueView>)| {
            let entity = entity.clone();
            move |_: &mut Window, cx: &mut App| {
                entity.update(cx, f).ok();
            }
        };
        Some(match menu {
            QueueMenu::Add(position) => Menu::new("queue-add-menu", *position, dismiss)
                .width(210.0)
                .item(
                    t(cx, Key::AddFilesToQueue),
                    Some(Icon::Plus),
                    run(|this, cx| this.pick_and_import(Pick::AudioFiles, ImportMode::Append, cx)),
                )
                .item(
                    t(cx, Key::AddFolderToQueue),
                    Some(Icon::FolderPlus),
                    run(|this, cx| this.pick_and_import(Pick::Folders, ImportMode::Append, cx)),
                )
                .separator()
                .item(
                    t(cx, Key::PlayFilesNext),
                    Some(Icon::ListStart),
                    run(|this, cx| this.pick_and_import(Pick::AudioFiles, ImportMode::PlayNext, cx)),
                )
                .item(
                    t(cx, Key::PlayFilesNow),
                    Some(Icon::Play),
                    run(|this, cx| this.pick_and_import(Pick::AudioFiles, ImportMode::PlayNow, cx)),
                ),
            QueueMenu::More(position) => {
                let empty = self.player.read(cx).tracks().is_empty();
                let menu = Menu::new("queue-more-menu", *position, dismiss).width(210.0);
                if empty {
                    menu.disabled_item(t(cx, Key::ClearQueue), Some(Icon::ListX))
                } else {
                    menu.item(
                        t(cx, Key::RevealCurrent),
                        Some(Icon::LocateFixed),
                        run(|this, cx| this.reveal_current(cx)),
                    )
                    .item(
                        t(cx, Key::RemoveUnavailable),
                        Some(Icon::CircleAlert),
                        run(Self::remove_unplayable),
                    )
                    .separator()
                    .danger_item(
                        t(cx, Key::ClearQueue),
                        Some(Icon::ListX),
                        run(|this, cx| {
                            this.edit(cx, |player, cx| {
                                player.clear(cx);
                                None
                            })
                        }),
                    )
                }
            }
            QueueMenu::Row(position) => {
                let selection = self.selection();
                let player = self.player.read(cx);
                let single = (selection.len() == 1).then(|| selection[0]);
                let single_path = single.and_then(|index| player.tracks().get(index).cloned());
                let issue = single_path.as_ref().and_then(|path| player.issue(path).cloned());
                let current = player.current_index();
                let mut menu = Menu::new("queue-row-menu", *position, dismiss).width(200.0);
                if let Some(index) = single {
                    menu = menu.item(t(cx, Key::PlayNow), Some(Icon::Play), {
                        let entity = entity.clone();
                        move |_, cx| {
                            entity
                                .update(cx, |this, cx| {
                                    this.player.update(cx, |p, cx| p.play_index(index, cx))
                                })
                                .ok();
                        }
                    });
                }
                if selection.iter().any(|&index| Some(index) != current) && current.is_some() {
                    menu =
                        menu.item(t(cx, Key::PlayNext), Some(Icon::ListStart), run(Self::play_selected_next));
                }
                if let Some(index) = single {
                    if matches!(issue, Some(TrackIssue::Missing)) {
                        menu = menu.item(t(cx, Key::Locate), Some(Icon::FileSearch), {
                            let entity = entity.clone();
                            move |_, cx| {
                                entity.update(cx, |this, cx| this.locate(index, cx)).ok();
                            }
                        });
                    }
                    if issue.is_some() {
                        menu = menu.item(t(cx, Key::Retry), Some(Icon::RotateCw), {
                            let entity = entity.clone();
                            move |_, cx| {
                                entity
                                    .update(cx, |this, cx| {
                                        this.player.update(cx, |p, cx| p.play_index(index, cx))
                                    })
                                    .ok();
                            }
                        });
                    }
                }
                if let Some(path) = single_path.filter(|path| path.exists()) {
                    menu = menu.item(t(cx, Key::ShowInFolder), Some(Icon::Folder), move |_, cx| {
                        cx.reveal_path(&path);
                    });
                }
                menu.separator().danger_item(
                    t(cx, Key::RemoveFromQueue),
                    Some(Icon::Trash2),
                    move |window, cx| {
                        entity.update(cx, |this, cx| this.remove_selected(&RemoveSelected, window, cx)).ok();
                    },
                )
            }
        })
    }
}

fn issue_badge(issue: &TrackIssue, theme: &Theme, cx: &App) -> impl IntoElement + use<> {
    let (label, color) = match issue {
        TrackIssue::Missing => {
            (t(cx, Key::IssueMissing), gpui::hsla(0.08, 0.85, if theme.dark { 0.62 } else { 0.45 }, 1.0))
        }
        TrackIssue::Unreadable(_) => {
            (t(cx, Key::IssueUnreadable), gpui::hsla(0.0, 0.7, if theme.dark { 0.66 } else { 0.5 }, 1.0))
        }
    };
    div()
        .flex_none()
        .h(px(20.0))
        .px(px(7.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded(px(6.0))
        .bg(color.opacity(0.12))
        .text_size(px(10.5))
        .text_color(color)
        .child(svg().path(Icon::CircleAlert.path()).size(px(11.0)).text_color(color))
        .child(label)
}

impl Render for QueueView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let count = self.row_count(cx);
        let queue_empty = self.player.read(cx).tracks().is_empty();

        let body = if queue_empty {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .child(svg().path(Icon::ListMusic.path()).size(px(32.0)).text_color(theme.text_muted))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .child(t(cx, Key::PlaylistEmpty)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child(t(cx, Key::PlaylistEmptyHint)),
                )
                .child(
                    div()
                        .pt(px(6.0))
                        .flex()
                        .gap(px(10.0))
                        .child(
                            pill_button("queue-empty-files", Icon::Plus, t(cx, Key::AddFilesToQueue), &theme)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pick_and_import(Pick::AudioFiles, ImportMode::Append, cx)
                                })),
                        )
                        .child(
                            pill_button(
                                "queue-empty-folder",
                                Icon::FolderPlus,
                                t(cx, Key::AddFolderToQueue),
                                &theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_and_import(Pick::Folders, ImportMode::Append, cx)
                            })),
                        ),
                )
                .into_any_element()
        } else if count == 0 {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .text_color(theme.text_muted)
                .child(t(cx, Key::NoMatches))
                .into_any_element()
        } else {
            uniform_list("queue", count, cx.processor(|this, range, _window, cx| this.render_rows(range, cx)))
                .track_scroll(self.scroll.clone())
                .flex_1()
                .px(px(16.0))
                .pt(px(12.0))
                .pb(px(10.0))
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .occlude()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::remove_selected))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::play_selected))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::focus_search))
            .child(self.render_header(cx))
            .children(self.render_selection_bar(cx))
            .child(body)
            .children(self.render_menu(cx))
    }
}

/// A rounded text button with an icon, for empty states.
pub fn pill_button(id: &'static str, icon: Icon, label: &'static str, theme: &Theme) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(34.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.button_border)
        .bg(theme.button_bg)
        .cursor_pointer()
        .hover(|s| s.bg(theme.button_hover_bg))
        .active(|s| s.bg(theme.button_pressed_bg))
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_secondary)
        .child(svg().path(icon.path()).size(px(15.0)).text_color(theme.icon.opacity(0.78)))
        .child(label)
}
