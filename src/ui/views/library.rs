//! The music library overlay: songs, albums, artists, folders, favorites and recently
//! played, plus management of the library folders.
//!
//! Playing from here replaces the queue with the list being viewed; "play next" and
//! "add to queue" leave the queue alone.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Image, InteractiveElement, IntoElement, KeyBinding, MouseButton, ObjectFit, ParentElement,
    Pixels, Point, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    Task, UniformListScrollHandle, Window, WindowControlArea, actions, div, img, prelude::FluentBuilder, px,
    svg, uniform_list,
};
use player_core::i18n::Key;
use player_core::library::{self, AlbumKey, SongOrder};
use player_core::media_paths;
use player_core::time::{UNKNOWN_DURATION, fmt_duration, fmt_long_duration};
use player_core::track::{TrackTags, search_terms};

use super::queue::pill_button;
use crate::media;
use crate::model::{LibraryModel, PlayerModel, SettingsModel};
use crate::platform::NativeWindow;
use crate::ui::dialogs::{self, Pick};
use crate::ui::icons::Icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::{IconButton, Menu, TextInput, TextInputEvent};
use crate::ui::{t, tf};

const SIDEBAR_WIDTH: f32 = 196.0;
const HEADER_HEIGHT: f32 = 64.0;
const ROW_HEIGHT: f32 = 52.0;
const ROW_GAP: f32 = 4.0;
const CARD_SIZE: f32 = 150.0;
const CARD_GAP: f32 = 22.0;
const CARDS_PER_ROW: usize = 4;
const CARD_ROW_HEIGHT: f32 = CARD_SIZE + 62.0;
const THUMBNAIL_PX: u32 = 300;
/// Covers kept in memory before the cache starts over.
const COVER_CACHE_LIMIT: usize = 400;
const KEY_CONTEXT: &str = "Library";

actions!(library, [GoBack, FocusLibrarySearch]);

pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", GoBack, context),
        KeyBinding::new("secondary-f", FocusLibrarySearch, context),
    ]);
}

pub enum LibraryViewEvent {
    Close,
    Toast(SharedString),
}

#[derive(Debug, Clone, PartialEq)]
enum Page {
    Songs,
    Albums,
    Album(AlbumKey),
    Artists,
    Artist(String),
    Folders,
    Folder(PathBuf),
    Favorites,
    Recent,
    ManageFolders,
}

impl Page {
    fn is_detail(&self) -> bool {
        matches!(self, Page::Album(_) | Page::Artist(_) | Page::Folder(_))
    }

    /// The sidebar entry highlighted for this page.
    fn section(&self) -> Page {
        match self {
            Page::Album(_) => Page::Albums,
            Page::Artist(_) => Page::Artists,
            Page::Folder(_) => Page::Folders,
            other => other.clone(),
        }
    }
}

/// What the main area lists.
enum Listing {
    Tracks(Arc<Vec<PathBuf>>),
    Albums(Vec<usize>),
    Artists(Vec<usize>),
    Folders(Vec<usize>),
}

enum LibraryMenu {
    Track { position: Point<Pixels>, path: PathBuf },
    Sort(Point<Pixels>),
}

pub struct LibraryView {
    library: Entity<LibraryModel>,
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    native: Option<NativeWindow>,
    focus: FocusHandle,
    page: Page,
    back: Vec<Page>,
    search: Entity<TextInput>,
    terms: Vec<String>,
    order: SongOrder,
    /// The track list of the current page, kept until the page, search, order or index change.
    cached: Option<(CacheKey, Arc<Vec<PathBuf>>)>,
    scroll: UniformListScrollHandle,
    covers: HashMap<PathBuf, Option<Arc<Image>>>,
    cover_requests: async_channel::Sender<PathBuf>,
    covers_pending: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    menu: Option<LibraryMenu>,
    task: Option<Task<()>>,
    _cover_loader: Task<()>,
}

#[derive(Clone, PartialEq)]
struct CacheKey {
    page: Page,
    terms: Vec<String>,
    order: SongOrder,
    revision: u64,
}

impl EventEmitter<LibraryViewEvent> for LibraryView {}

impl Focusable for LibraryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LibraryView {
    pub fn new(
        library: Entity<LibraryModel>,
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        native: Option<NativeWindow>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| TextInput::new(t(cx, Key::SearchLibrary), cx));
        cx.subscribe(&search, |this, search, event, cx| match event {
            TextInputEvent::Changed => {
                this.terms = search_terms(&search.read(cx).committed_text());
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            TextInputEvent::Submit => {}
            TextInputEvent::Cancel => cx.emit(LibraryViewEvent::Close),
        })
        .detach();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();

        let (cover_requests, requests) = async_channel::unbounded::<PathBuf>();
        let cover_loader = cx.spawn(async move |this, cx| {
            while let Ok(path) = requests.recv().await {
                let lookup = path.clone();
                let cover = cx
                    .background_executor()
                    .spawn(async move { media::read_thumbnail(&lookup, THUMBNAIL_PX) })
                    .await
                    .map(media::Cover::into_image);
                let stored = this.update(cx, |this, cx| {
                    if this.covers.len() >= COVER_CACHE_LIMIT {
                        this.covers.clear();
                    }
                    this.covers_pending.remove(&path);
                    this.covers.insert(path, cover);
                    cx.notify();
                });
                if stored.is_err() {
                    break;
                }
            }
        });

        Self {
            library,
            player,
            settings,
            native,
            focus: cx.focus_handle(),
            page: Page::Albums,
            back: Vec::new(),
            search,
            terms: Vec::new(),
            order: SongOrder::default(),
            cached: None,
            scroll: UniformListScrollHandle::new(),
            covers: HashMap::new(),
            cover_requests,
            covers_pending: HashSet::new(),
            selected: None,
            menu: None,
            task: None,
            _cover_loader: cover_loader,
        }
    }

    pub fn opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if self.library.read(cx).folders().is_empty() && !self.page.is_detail() {
            self.page = Page::ManageFolders;
        }
        let placeholder = t(cx, Key::SearchLibrary);
        self.search.update(cx, |search, _| search.set_placeholder(placeholder));
    }

    pub fn closed(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == self.page {
            return;
        }
        if page.is_detail() {
            self.back.push(self.page.clone());
        } else {
            self.back.clear();
        }
        self.page = page;
        self.selected = None;
        self.menu = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.search.update(cx, |search, cx| search.clear(cx));
        cx.notify();
    }

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        } else if let Some(page) = self.back.pop() {
            self.page = page;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
        } else {
            cx.emit(LibraryViewEvent::Close);
        }
    }

    fn focus_search(&mut self, _: &FocusLibrarySearch, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus_handle(cx));
    }

    // ── Data ────────────────────────────────────────────────────────────────

    /// Tags for a track: from the library index, else whatever the player has read.
    fn tags_of(&self, path: &Path, cx: &App) -> Option<TrackTags> {
        self.library
            .read(cx)
            .track(path)
            .map(|track| track.tags.clone())
            .or_else(|| self.player.read(cx).tags(path).cloned())
    }

    fn listing(&mut self, cx: &mut Context<Self>) -> Listing {
        let groupings = self.library.update(cx, |library, _| library.groupings());
        let library = self.library.read(cx);
        let tracks = library.tracks();
        let terms = self.terms.clone();
        let track_matches = |index: usize| tracks[index].tags.matches(&tracks[index].path, &terms);
        let text_matches = |text: &str| {
            let text = text.to_lowercase();
            terms.iter().all(|term| text.contains(term.as_str()))
        };
        match &self.page {
            Page::Albums => Listing::Albums(
                groupings
                    .albums
                    .iter()
                    .enumerate()
                    .filter(|(_, album)| {
                        terms.is_empty()
                            || text_matches(&format!(
                                "{}\n{}",
                                album.title,
                                album.artist.as_deref().unwrap_or("")
                            ))
                    })
                    .map(|(index, _)| index)
                    .collect(),
            ),
            Page::Artists => Listing::Artists(
                groupings
                    .artists
                    .iter()
                    .enumerate()
                    .filter(|(_, artist)| terms.is_empty() || text_matches(&artist.name))
                    .map(|(index, _)| index)
                    .collect(),
            ),
            Page::Folders => {
                let roots = library.folders().to_vec();
                Listing::Folders(
                    groupings
                        .folders
                        .iter()
                        .enumerate()
                        .filter(|(_, folder)| {
                            terms.is_empty() || text_matches(&library::relative_folder(&folder.path, &roots))
                        })
                        .map(|(index, _)| index)
                        .collect(),
                )
            }
            Page::ManageFolders => Listing::Tracks(Arc::default()),
            page => {
                let key = CacheKey {
                    page: page.clone(),
                    terms: terms.clone(),
                    order: self.order,
                    revision: library.revision(),
                };
                let settings = self.settings.read(cx).get();
                let dynamic = matches!(page, Page::Favorites | Page::Recent);
                if !dynamic
                    && let Some((cached_key, list)) = &self.cached
                    && *cached_key == key
                {
                    return Listing::Tracks(list.clone());
                }
                let indices = |list: &[usize]| -> Vec<PathBuf> {
                    list.iter().filter(|&&i| track_matches(i)).map(|&i| tracks[i].path.clone()).collect()
                };
                let paths: Vec<PathBuf> = match page {
                    Page::Songs => indices(&library::sorted_songs(tracks, self.order)),
                    Page::Album(key) => groupings
                        .albums
                        .iter()
                        .find(|album| album.key == *key)
                        .map_or_else(Vec::new, |a| indices(&a.tracks)),
                    Page::Artist(name) => groupings
                        .artists
                        .iter()
                        .find(|artist| artist.name == *name)
                        .map_or_else(Vec::new, |a| indices(&a.tracks)),
                    Page::Folder(path) => groupings
                        .folders
                        .iter()
                        .find(|folder| folder.path == *path)
                        .map_or_else(Vec::new, |f| indices(&f.tracks)),
                    Page::Favorites | Page::Recent => {
                        let list = if *page == Page::Favorites {
                            &settings.library.favorites
                        } else {
                            &settings.library.recent
                        };
                        let player = self.player.read(cx);
                        list.iter()
                            .filter(|path| {
                                terms.is_empty()
                                    || library
                                        .track(path)
                                        .map(|track| track.tags.clone())
                                        .or_else(|| player.tags(path).cloned())
                                        .unwrap_or_default()
                                        .matches(path, &terms)
                            })
                            .cloned()
                            .collect()
                    }
                    _ => Vec::new(),
                };
                let list = Arc::new(paths);
                if !dynamic {
                    self.cached = Some((key, list.clone()));
                }
                Listing::Tracks(list)
            }
        }
    }

    fn cover(&mut self, path: &Path) -> Option<Arc<Image>> {
        if let Some(cover) = self.covers.get(path) {
            return cover.clone();
        }
        if self.covers_pending.insert(path.to_path_buf()) {
            let _ = self.cover_requests.try_send(path.to_path_buf());
        }
        None
    }

    // ── Actions ─────────────────────────────────────────────────────────────

    fn play_list(&mut self, list: Arc<Vec<PathBuf>>, start: usize, shuffle: bool, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let start = if shuffle { fastrand::usize(0..list.len()) } else { start };
        self.player.update(cx, |player, cx| {
            if shuffle && !player.shuffle() {
                player.toggle_shuffle(cx);
            }
            player.play_tracks(list.to_vec(), start, cx);
        });
    }

    fn add_to_queue(&mut self, tracks: Vec<PathBuf>, next: bool, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        let count = tracks.len().to_string();
        let message = self.player.update(cx, |player, cx| {
            if next {
                player.play_next(tracks, cx);
                tf(cx, Key::ToastPlayNext, &[("n", &count)])
            } else {
                player.enqueue(tracks, cx);
                tf(cx, Key::ToastAdded, &[("n", &count)])
            }
        });
        cx.emit(LibraryViewEvent::Toast(message.into()));
    }

    fn toggle_favorite(&mut self, path: &Path, cx: &mut Context<Self>) {
        let path = path.to_path_buf();
        self.settings.update(cx, |settings, cx| settings.update(cx, |s| s.toggle_favorite(&path)));
    }

    fn add_folders(&mut self, cx: &mut Context<Self>) {
        let start = self.library.read(cx).folders().last().cloned();
        let pick = dialogs::pick(Pick::Folders, start.as_deref(), self.native, cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            let folders = pick.await;
            if folders.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                this.library.update(cx, |library, cx| library.add_folders(folders, cx));
            })
            .ok();
        }));
    }

    fn album_key_of(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<AlbumKey> {
        let groupings = self.library.update(cx, |library, _| library.groupings());
        let library = self.library.read(cx);
        let index = library.tracks().iter().position(|track| track.path == path)?;
        groupings.albums.iter().find(|album| album.tracks.contains(&index)).map(|album| album.key.clone())
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let section = self.page.section();
        let library = self.library.read(cx);
        let (songs, scan) = (library.tracks().len(), library.scan_progress());
        let settings = self.settings.read(cx).get();
        let (favorites, recent) = (settings.library.favorites.len(), settings.library.recent.len());
        let entry = |id: &'static str,
                     page: Page,
                     icon: Icon,
                     label: &'static str,
                     count: Option<usize>,
                     cx: &mut Context<Self>| {
            let active = section == page;
            div()
                .id(id)
                .h(px(38.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(px(12.0))
                .cursor_pointer()
                .map(|el| {
                    if active {
                        el.bg(theme.button_active_bg)
                    } else {
                        el.hover(|s| s.bg(theme.button_hover_bg))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.navigate(page.clone(), cx)))
                .child(svg().path(icon.path()).size(px(16.0)).text_color(if active {
                    theme.icon_active
                } else {
                    theme.icon.opacity(0.75)
                }))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.5))
                        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(if active { theme.text_primary } else { theme.text_secondary })
                        .child(label),
                )
                .children(count.filter(|count| *count > 0).map(|count| {
                    div().text_size(px(11.0)).text_color(theme.text_muted).child(count.to_string())
                }))
        };
        let status = match scan {
            Some(scan) if scan.to_read > 0 => tf(
                cx,
                Key::ScanningProgress,
                &[("read", &scan.read.to_string()), ("total", &scan.to_read.to_string())],
            ),
            Some(_) => t(cx, Key::Scanning).to_string(),
            None => String::new(),
        };

        div()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .bg(theme.surface_panel_bg)
            .child(
                div()
                    .h(px(36.0))
                    .px(px(6.0))
                    .mb(px(10.0))
                    .flex()
                    .items_center()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .window_control_area(WindowControlArea::Drag)
                    .child(t(cx, Key::Library)),
            )
            .child(entry("lib-songs", Page::Songs, Icon::Music2, t(cx, Key::Songs), Some(songs), cx))
            .child(entry("lib-albums", Page::Albums, Icon::Disc3, t(cx, Key::Albums), None, cx))
            .child(entry("lib-artists", Page::Artists, Icon::UserRound, t(cx, Key::Artists), None, cx))
            .child(entry("lib-folders", Page::Folders, Icon::Folder, t(cx, Key::Folders), None, cx))
            .child(div().h(px(1.0)).mx(px(8.0)).my(px(8.0)).bg(theme.surface_panel_border))
            .child(entry(
                "lib-favorites",
                Page::Favorites,
                Icon::Heart,
                t(cx, Key::Favorites),
                Some(favorites),
                cx,
            ))
            .child(entry("lib-recent", Page::Recent, Icon::History, t(cx, Key::Recent), Some(recent), cx))
            .child(div().flex_1())
            .when(!status.is_empty(), |el| {
                el.child(
                    div()
                        .px(px(10.0))
                        .pb(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(svg().path(Icon::RefreshCw.path()).size(px(12.0)).text_color(theme.accent))
                        .child(status),
                )
            })
            .child(entry(
                "lib-manage",
                Page::ManageFolders,
                Icon::FolderPlus,
                t(cx, Key::ManageFolders),
                None,
                cx,
            ))
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let title: SharedString = match &self.page {
            Page::Songs => t(cx, Key::Songs).into(),
            Page::Albums => t(cx, Key::Albums).into(),
            Page::Artists => t(cx, Key::Artists).into(),
            Page::Folders => t(cx, Key::Folders).into(),
            Page::Favorites => t(cx, Key::Favorites).into(),
            Page::Recent => t(cx, Key::Recent).into(),
            Page::ManageFolders => t(cx, Key::ManageFolders).into(),
            Page::Album(_) | Page::Artist(_) | Page::Folder(_) => SharedString::default(),
        };
        let searchable = self.page != Page::ManageFolders;
        div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .px(px(18.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .window_control_area(WindowControlArea::Drag)
            .when(!self.back.is_empty(), |el| {
                el.child(
                    IconButton::new("lib-back", Icon::ChevronLeft)
                        .size(30.0, 16.0)
                        .hover_bg(theme.button_hover_bg)
                        .on_click(cx.listener(|this, _, window, cx| this.go_back(&GoBack, window, cx))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(title),
            )
            .when(self.page == Page::Songs, |el| {
                el.child(
                    IconButton::new("lib-sort", Icon::SlidersVertical)
                        .size(30.0, 14.0)
                        .hover_bg(theme.button_hover_bg)
                        .icon_opacity(0.7)
                        .active(matches!(self.menu, Some(LibraryMenu::Sort(_))))
                        .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                            this.menu = Some(LibraryMenu::Sort(event.position()));
                            cx.notify();
                        })),
                )
            })
            .when(searchable, |el| {
                el.child(
                    div()
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
                        .child(self.search.clone()),
                )
            })
            .child(
                IconButton::new("lib-close", Icon::X)
                    .size(30.0, 12.0)
                    .hover_bg(theme.close_hover_bg)
                    .icon_opacity(0.6)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(LibraryViewEvent::Close))),
            )
    }

    /// Cover, title and actions above an album / artist / folder's track list.
    fn render_detail_header(
        &mut self,
        list: &Arc<Vec<PathBuf>>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.global::<Theme>().clone();
        let groupings = self.library.update(cx, |library, _| library.groupings());
        let total: u64 = list.iter().filter_map(|path| self.tags_of(path, cx)?.duration_ms).sum();
        let songs = tf(cx, Key::SongCount, &[("n", &list.len().to_string())]);
        let (title, subtitle, cover_path, icon) = match &self.page {
            Page::Album(key) => {
                let album = groupings.albums.iter().find(|album| album.key == *key)?;
                let mut parts =
                    vec![album.artist.clone().unwrap_or_else(|| t(cx, Key::UnknownArtist).into())];
                parts.extend(album.year.map(|year| year.to_string()));
                (album.title.clone(), parts.join(" · "), list.first().cloned(), Icon::Disc3)
            }
            Page::Artist(name) => {
                let artist = groupings.artists.iter().find(|artist| artist.name == *name)?;
                (
                    artist.name.clone(),
                    tf(cx, Key::AlbumCount, &[("n", &artist.albums.to_string())]),
                    None,
                    Icon::UserRound,
                )
            }
            Page::Folder(path) => {
                let roots = self.library.read(cx).folders().to_vec();
                (
                    media_paths::folder_name(&path.join("x")).unwrap_or_default(),
                    library::relative_folder(path, &roots),
                    list.first().cloned(),
                    Icon::Folder,
                )
            }
            Page::Songs | Page::Favorites | Page::Recent => {
                if list.is_empty() {
                    return None;
                }
                (String::new(), String::new(), None, Icon::Music2)
            }
            _ => return None,
        };
        let cover = cover_path.and_then(|path| self.cover(&path));
        let entity = cx.entity().downgrade();
        let actions =
            {
                let (play, shuffle, add) = (list.clone(), list.clone(), list.clone());
                let (e1, e2, e3) = (entity.clone(), entity.clone(), entity.clone());
                div()
                    .flex()
                    .gap(px(10.0))
                    .child(pill_button("lib-play-all", Icon::Play, t(cx, Key::PlayAll), &theme).on_click(
                        move |_, _, cx| {
                            e1.update(cx, |this, cx| this.play_list(play.clone(), 0, false, cx)).ok();
                        },
                    ))
                    .child(
                        pill_button("lib-shuffle-all", Icon::Shuffle, t(cx, Key::ShuffleAll), &theme)
                            .on_click(move |_, _, cx| {
                                e2.update(cx, |this, cx| this.play_list(shuffle.clone(), 0, true, cx)).ok();
                            }),
                    )
                    .child(
                        pill_button("lib-add-all", Icon::ListPlus, t(cx, Key::AddAllToQueue), &theme)
                            .on_click(move |_, _, cx| {
                                e3.update(cx, |this, cx| this.add_to_queue(add.to_vec(), false, cx)).ok();
                            }),
                    )
            };
        if title.is_empty() {
            return Some(div().flex_none().px(px(20.0)).pb(px(10.0)).child(actions).into_any_element());
        }
        Some(
            div()
                .flex_none()
                .px(px(20.0))
                .pb(px(14.0))
                .flex()
                .gap(px(20.0))
                .child(cover_tile(cover, icon, 128.0, 18.0, &theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_end()
                        .gap(px(6.0))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(24.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.0))
                                .text_color(theme.text_subtle)
                                .child(subtitle),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child(format!("{songs} · {}", fmt_long_duration(total))),
                        )
                        .child(div().pt(px(6.0)).child(actions)),
                )
                .into_any_element(),
        )
    }

    fn render_track_rows(
        &mut self,
        list: Arc<Vec<PathBuf>>,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let theme = cx.global::<Theme>().clone();
        let entity = cx.entity().downgrade();
        let current = self.player.read(cx).current_path().map(Path::to_path_buf);
        let favorites: HashSet<PathBuf> =
            self.settings.read(cx).get().library.favorites.iter().cloned().collect();
        range
            .filter_map(|row| {
                let path = list.get(row)?.clone();
                let tags = self.tags_of(&path, cx).unwrap_or_default();
                let title = tags.display_title(&path);
                let subtitle = [tags.artist.clone(), tags.album.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let subtitle = if subtitle.is_empty() {
                    media_paths::folder_name(&path).unwrap_or_default()
                } else {
                    subtitle
                };
                let duration =
                    tags.duration_ms.map(fmt_duration).unwrap_or_else(|| UNKNOWN_DURATION.to_string());
                let is_current = current.as_deref() == Some(path.as_path());
                let is_selected = self.selected.as_deref() == Some(path.as_path());
                let favorite = favorites.contains(&path);
                let missing = !path.exists();
                let group: SharedString = format!("lib-row-{row}").into();
                let (click, menu, play, heart) =
                    (entity.clone(), entity.clone(), entity.clone(), entity.clone());
                let (click_list, play_list) = (list.clone(), list.clone());
                let (menu_path, heart_path, select_path) = (path.clone(), path.clone(), path.clone());
                Some(
                    div().h(px(ROW_HEIGHT + ROW_GAP)).pb(px(ROW_GAP)).child(
                        div()
                            .id(("lib-row", row))
                            .group(group.clone())
                            .relative()
                            .h(px(ROW_HEIGHT))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .map(|el| {
                                if is_selected {
                                    el.bg(theme.accent.opacity(if theme.dark { 0.20 } else { 0.12 }))
                                } else if is_current {
                                    el.bg(theme.playlist_row_active_bg)
                                } else {
                                    el.hover(|s| s.bg(theme.playlist_row_hover_bg))
                                }
                            })
                            .on_click(move |event, _, cx| {
                                click
                                    .update(cx, |this, cx| {
                                        if event.click_count() >= 2 {
                                            this.play_list(click_list.clone(), row, false, cx);
                                        } else {
                                            this.selected = Some(select_path.clone());
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            })
                            .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                                menu.update(cx, |this, cx| {
                                    this.selected = Some(menu_path.clone());
                                    this.menu = Some(LibraryMenu::Track {
                                        position: event.position,
                                        path: menu_path.clone(),
                                    });
                                    cx.notify();
                                })
                                .ok();
                            })
                            .child(
                                div()
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
                                            el.child(
                                                svg()
                                                    .path(Icon::AudioLines.path())
                                                    .size(px(15.0))
                                                    .text_color(theme.accent),
                                            )
                                        } else {
                                            el.child(
                                                div()
                                                    .group_hover(group.clone(), |s| s.invisible())
                                                    .child((row + 1).to_string()),
                                            )
                                        }
                                    }),
                            )
                            .child(
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
                                        IconButton::new(("lib-play", row), Icon::Play)
                                            .size(26.0, 13.0)
                                            .hover_bg(theme.button_hover_bg)
                                            .on_click(move |_, _, cx| {
                                                play.update(cx, |this, cx| {
                                                    this.play_list(play_list.clone(), row, false, cx)
                                                })
                                                .ok();
                                            }),
                                    ),
                            )
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
                                            .text_color(if missing {
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
                            .child(
                                div()
                                    .flex_none()
                                    .when(!favorite, |el| {
                                        el.invisible().group_hover(group.clone(), |s| s.visible())
                                    })
                                    .child(
                                        IconButton::new(
                                            ("lib-heart", row),
                                            if favorite { Icon::HeartFilled } else { Icon::Heart },
                                        )
                                        .size(26.0, 13.0)
                                        .hover_bg(theme.button_hover_bg)
                                        .active(favorite)
                                        .on_click(move |_, _, cx| {
                                            heart
                                                .update(cx, |this, cx| this.toggle_favorite(&heart_path, cx))
                                                .ok();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(40.0))
                                    .text_right()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(px(11.0))
                                    .text_color(theme.text_muted)
                                    .child(duration),
                            ),
                    ),
                )
            })
            .collect()
    }

    fn render_album_rows(
        &mut self,
        albums: Arc<Vec<usize>>,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let theme = cx.global::<Theme>().clone();
        let groupings = self.library.update(cx, |library, _| library.groupings());
        let unknown = t(cx, Key::UnknownArtist);
        range
            .map(|row| {
                let start = row * CARDS_PER_ROW;
                let cards: Vec<AnyElement> =
                    albums[start.min(albums.len())..(start + CARDS_PER_ROW).min(albums.len())]
                        .iter()
                        .map(|&index| {
                            let album = &groupings.albums[index];
                            let first = album
                                .tracks
                                .first()
                                .and_then(|&i| self.library.read(cx).track_at(i))
                                .map(|t| t.path.clone());
                            let cover = first.and_then(|path| self.cover(&path));
                            let key = album.key.clone();
                            div()
                                .id(("album-card", index))
                                .w(px(CARD_SIZE))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.navigate(Page::Album(key.clone()), cx)
                                }))
                                .child(div().rounded(px(16.0)).hover(|s| s.opacity(0.85)).child(cover_tile(
                                    cover,
                                    Icon::Disc3,
                                    CARD_SIZE,
                                    16.0,
                                    &theme,
                                )))
                                .child(
                                    div()
                                        .pt(px(4.0))
                                        .truncate()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_secondary)
                                        .child(album.title.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.0))
                                        .text_color(theme.text_subtle)
                                        .child(album.artist.clone().unwrap_or_else(|| unknown.to_string())),
                                )
                                .into_any_element()
                        })
                        .collect();
                div().h(px(CARD_ROW_HEIGHT)).flex().gap(px(CARD_GAP)).children(cards)
            })
            .collect()
    }

    /// Artist and folder lists: an icon, a name and a count.
    fn render_group_rows(
        &mut self,
        items: Arc<Vec<usize>>,
        artists: bool,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let theme = cx.global::<Theme>().clone();
        let groupings = self.library.update(cx, |library, _| library.groupings());
        let roots = self.library.read(cx).folders().to_vec();
        range
            .filter_map(|row| {
                let index = *items.get(row)?;
                let (name, detail, page, icon) = if artists {
                    let artist = &groupings.artists[index];
                    let songs = tf(cx, Key::SongCount, &[("n", &artist.tracks.len().to_string())]);
                    let albums = tf(cx, Key::AlbumCount, &[("n", &artist.albums.to_string())]);
                    (
                        artist.name.clone(),
                        format!("{albums} · {songs}"),
                        Page::Artist(artist.name.clone()),
                        Icon::UserRound,
                    )
                } else {
                    let folder = &groupings.folders[index];
                    (
                        library::relative_folder(&folder.path, &roots),
                        tf(cx, Key::SongCount, &[("n", &folder.tracks.len().to_string())]),
                        Page::Folder(folder.path.clone()),
                        Icon::Folder,
                    )
                };
                Some(
                    div().h(px(ROW_HEIGHT + ROW_GAP)).pb(px(ROW_GAP)).child(
                        div()
                            .id(("lib-group", row))
                            .h(px(ROW_HEIGHT))
                            .px(px(12.0))
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.playlist_row_hover_bg))
                            .on_click(cx.listener(move |this, _, _, cx| this.navigate(page.clone(), cx)))
                            .child(
                                div()
                                    .size(px(34.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(theme.button_bg)
                                    .child(
                                        svg()
                                            .path(icon.path())
                                            .size(px(16.0))
                                            .text_color(theme.icon.opacity(0.7)),
                                    ),
                            )
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
                                            .text_color(theme.text_secondary)
                                            .child(name),
                                    )
                                    .child(
                                        div().text_size(px(11.0)).text_color(theme.text_subtle).child(detail),
                                    ),
                            )
                            .child(
                                svg()
                                    .path(Icon::ChevronRight.path())
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
                )
            })
            .collect()
    }

    fn render_manage(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let library = self.library.read(cx);
        let folders = library.folders().to_vec();
        let scanning = library.scan_progress().is_some();
        let tracks = library.tracks().len();
        let groupings = self.library.update(cx, |library, _| library.groupings());
        let stats = tf(
            cx,
            Key::LibraryStats,
            &[
                ("songs", &tracks.to_string()),
                ("albums", &groupings.albums.len().to_string()),
                ("artists", &groupings.artists.len().to_string()),
            ],
        );
        let rows: Vec<AnyElement> = folders
            .iter()
            .enumerate()
            .map(|(index, folder)| {
                let remove = folder.clone();
                div()
                    .h(px(48.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(px(12.0))
                    .bg(theme.button_bg)
                    .border_1()
                    .border_color(theme.button_border)
                    .child(svg().path(Icon::Folder.path()).size(px(16.0)).text_color(theme.icon.opacity(0.7)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.0))
                            .text_color(theme.text_secondary)
                            .child(folder.display().to_string()),
                    )
                    .child(
                        IconButton::new(("lib-remove-folder", index), Icon::Trash2)
                            .size(28.0, 13.0)
                            .hover_bg(theme.close_hover_bg)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let folder = remove.clone();
                                this.library.update(cx, |library, cx| library.remove_folder(&folder, cx));
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        div()
            .id("lib-manage")
            .flex_1()
            .overflow_y_scroll()
            .px(px(24.0))
            .pb(px(24.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .when(folders.is_empty(), |el| {
                el.child(
                    div()
                        .pt(px(30.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(10.0))
                        .child(svg().path(Icon::Library.path()).size(px(36.0)).text_color(theme.text_muted))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_secondary)
                                .child(t(cx, Key::LibraryEmpty)),
                        )
                        .child(
                            div()
                                .max_w(px(420.0))
                                .text_center()
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child(t(cx, Key::LibraryEmptyHint)),
                        ),
                )
            })
            .when(!folders.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child(t(cx, Key::LibraryFoldersHint)),
                )
                .child(div().text_size(px(12.0)).text_color(theme.text_subtle).child(stats))
            })
            .children(rows)
            .child(
                div()
                    .flex()
                    .gap(px(10.0))
                    .when(folders.is_empty(), |el| el.justify_center())
                    .child(
                        pill_button("lib-add-folder", Icon::FolderPlus, t(cx, Key::AddMusicFolder), &theme)
                            .on_click(cx.listener(|this, _, _, cx| this.add_folders(cx))),
                    )
                    .when(!folders.is_empty(), |el| {
                        el.child(
                            pill_button(
                                "lib-rescan",
                                Icon::RefreshCw,
                                t(cx, if scanning { Key::Scanning } else { Key::Rescan }),
                                &theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.library.update(cx, |library, cx| library.rescan(cx))
                            })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let no_folders = self.library.read(cx).folders().is_empty();
        let (icon, text) = match &self.page {
            Page::Favorites if self.terms.is_empty() => (Icon::Heart, t(cx, Key::FavoritesEmpty)),
            Page::Recent if self.terms.is_empty() => (Icon::History, t(cx, Key::RecentEmpty)),
            _ if !self.terms.is_empty() => (Icon::Search, t(cx, Key::NoMatches)),
            _ if no_folders => (Icon::Library, t(cx, Key::LibraryEmptyHint)),
            _ => (Icon::RefreshCw, t(cx, Key::Scanning)),
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .child(svg().path(icon.path()).size(px(30.0)).text_color(theme.text_muted))
            .child(
                div()
                    .max_w(px(420.0))
                    .text_center()
                    .text_size(px(13.0))
                    .text_color(theme.text_muted)
                    .child(text),
            )
            .when(
                no_folders && self.terms.is_empty() && !matches!(self.page, Page::Favorites | Page::Recent),
                |el| {
                    el.child(
                        pill_button("lib-empty-add", Icon::FolderPlus, t(cx, Key::AddMusicFolder), &theme)
                            .on_click(cx.listener(|this, _, _, cx| this.add_folders(cx))),
                    )
                },
            )
            .into_any_element()
    }

    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<Menu> {
        let entity = cx.entity().downgrade();
        let dismiss = {
            let entity = entity.clone();
            move |_: &mut Window, cx: &mut App| {
                entity
                    .update(cx, |this, cx| {
                        this.menu = None;
                        cx.notify();
                    })
                    .ok();
            }
        };
        match self.menu.as_ref()? {
            LibraryMenu::Sort(position) => {
                let current = self.order;
                let mut menu =
                    Menu::new("lib-sort-menu", *position, dismiss).width(170.0).header(t(cx, Key::SortBy));
                for (order, key) in [
                    (SongOrder::Title, Key::SortTitle),
                    (SongOrder::Artist, Key::SortArtist),
                    (SongOrder::Album, Key::SortAlbum),
                    (SongOrder::Added, Key::SortAdded),
                ] {
                    let entity = entity.clone();
                    menu = menu.item(t(cx, key), (order == current).then_some(Icon::Check), move |_, cx| {
                        entity
                            .update(cx, |this, cx| {
                                this.order = order;
                                cx.notify();
                            })
                            .ok();
                    });
                }
                Some(menu)
            }
            LibraryMenu::Track { position, path } => {
                let (position, path) = (*position, path.clone());
                let favorite = self.settings.read(cx).get().is_favorite(&path);
                let tags = self.tags_of(&path, cx).unwrap_or_default();
                let album = self.album_key_of(&path, cx);
                let artist = tags.artist.clone().filter(|_| self.library.read(cx).track(&path).is_some());
                let list = match self.listing(cx) {
                    Listing::Tracks(list) => list,
                    _ => Arc::default(),
                };
                let start = list.iter().position(|p| *p == path).unwrap_or(0);
                let on = |f: Box<ViewAction>| {
                    let entity = entity.clone();
                    move |_: &mut Window, cx: &mut App| {
                        entity.update(cx, |this, cx| f(this, cx)).ok();
                    }
                };
                let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
                let mut menu = Menu::new("lib-track-menu", position, dismiss)
                    .width(200.0)
                    .item(
                        t(cx, Key::PlayNow),
                        Some(Icon::Play),
                        on(Box::new(move |this, cx| this.play_list(list.clone(), start, false, cx))),
                    )
                    .item(
                        t(cx, Key::PlayNext),
                        Some(Icon::ListStart),
                        on(Box::new(move |this, cx| this.add_to_queue(vec![p1.clone()], true, cx))),
                    )
                    .item(
                        t(cx, Key::AddToQueue),
                        Some(Icon::ListPlus),
                        on(Box::new(move |this, cx| this.add_to_queue(vec![p2.clone()], false, cx))),
                    )
                    .separator()
                    .item(
                        t(cx, if favorite { Key::Unfavorite } else { Key::Favorite }),
                        Some(if favorite { Icon::HeartFilled } else { Icon::Heart }),
                        on(Box::new(move |this, cx| this.toggle_favorite(&p3, cx))),
                    );
                if let Some(album) = album.filter(|key| self.page != Page::Album(key.clone())) {
                    menu = menu.item(
                        t(cx, Key::GoToAlbum),
                        Some(Icon::Disc3),
                        on(Box::new(move |this, cx| this.navigate(Page::Album(album.clone()), cx))),
                    );
                }
                if let Some(artist) = artist.filter(|name| self.page != Page::Artist(name.clone())) {
                    menu = menu.item(
                        t(cx, Key::GoToArtist),
                        Some(Icon::UserRound),
                        on(Box::new(move |this, cx| this.navigate(Page::Artist(artist.clone()), cx))),
                    );
                }
                if p4.exists() {
                    menu = menu
                        .item(t(cx, Key::ShowInFolder), Some(Icon::Folder), move |_, cx| cx.reveal_path(&p4));
                }
                Some(menu)
            }
        }
    }
}

type ViewAction = dyn Fn(&mut LibraryView, &mut Context<LibraryView>);

/// A square cover image, or a placeholder with an icon.
fn cover_tile(cover: Option<Arc<Image>>, icon: Icon, size: f32, radius: f32, theme: &Theme) -> AnyElement {
    match cover {
        Some(cover) => {
            img(cover).size(px(size)).rounded(px(radius)).object_fit(ObjectFit::Cover).into_any_element()
        }
        None => div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            .border_1()
            .border_color(theme.album_placeholder_border)
            .bg(gpui::linear_gradient(
                180.0,
                gpui::linear_color_stop(theme.album_placeholder_start, 0.0),
                gpui::linear_color_stop(theme.album_placeholder_end, 1.0),
            ))
            .child(svg().path(icon.path()).size(px(size * 0.28)).text_color(theme.text_muted.opacity(0.6)))
            .into_any_element(),
    }
}

impl Render for LibraryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.page == Page::ManageFolders {
            self.render_manage(cx)
        } else {
            match self.listing(cx) {
                Listing::Tracks(list) => {
                    let header = self.render_detail_header(&list, cx);
                    let count = list.len();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .children(header)
                        .child(if count == 0 {
                            self.render_empty(cx)
                        } else {
                            uniform_list(
                                "lib-tracks",
                                count,
                                cx.processor(move |this, range, _, cx| {
                                    this.render_track_rows(list.clone(), range, cx)
                                }),
                            )
                            .track_scroll(self.scroll.clone())
                            .flex_1()
                            .px(px(14.0))
                            .pb(px(10.0))
                            .into_any_element()
                        })
                        .into_any_element()
                }
                Listing::Albums(albums) if albums.is_empty() => self.render_empty(cx),
                Listing::Albums(albums) => {
                    let rows = albums.len().div_ceil(CARDS_PER_ROW);
                    let albums = Arc::new(albums);
                    uniform_list(
                        "lib-albums",
                        rows,
                        cx.processor(move |this, range, _, cx| {
                            this.render_album_rows(albums.clone(), range, cx)
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .px(px(24.0))
                    .pt(px(4.0))
                    .into_any_element()
                }
                Listing::Artists(items) | Listing::Folders(items) if items.is_empty() => {
                    self.render_empty(cx)
                }
                Listing::Artists(items) => {
                    let items = Arc::new(items);
                    uniform_list(
                        "lib-artists",
                        items.len(),
                        cx.processor(move |this, range, _, cx| {
                            this.render_group_rows(items.clone(), true, range, cx)
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .px(px(14.0))
                    .into_any_element()
                }
                Listing::Folders(items) => {
                    let items = Arc::new(items);
                    uniform_list(
                        "lib-folders",
                        items.len(),
                        cx.processor(move |this, range, _, cx| {
                            this.render_group_rows(items.clone(), false, range, cx)
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .px(px(14.0))
                    .into_any_element()
                }
            }
        };

        div()
            .size_full()
            .flex()
            .occlude()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::focus_search))
            .child(self.render_sidebar(cx))
            .child(
                div().flex_1().min_w_0().h_full().flex().flex_col().child(self.render_header(cx)).child(body),
            )
            .children(self.render_menu(cx))
    }
}
