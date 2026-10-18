//! `MainView`: the window's root. Owns UI state (mode, overlays, pin) and composes the
//! full / mini / island layouts with the queue, library, settings and EQ overlays on top.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, WindowControlArea, div, linear_color_stop,
    linear_gradient, prelude::FluentBuilder, px, relative, size, svg,
};
use player_core::easing;
use player_core::geometry::Rect;
use player_core::i18n::Key;
use player_core::media_paths;
use player_core::settings::{SavedWindowMode, WindowPlacement};

use crate::actions::{
    CycleRepeat, KEY_CONTEXT, Next, Previous, SeekBackward, SeekForward, ToggleEq, ToggleFavorite,
    ToggleLibrary, ToggleLyrics, ToggleMute, TogglePlay, TogglePlaylist, ToggleShuffle, VolumeDown, VolumeUp,
};
use crate::model::{
    LibraryEvent, LibraryModel, PlayerEvent, PlayerModel, Problem, SettingsModel, Status, TrackIssue,
};
use crate::platform::NativeWindow;
use crate::ui::anim::Tween;
use crate::ui::icons::Icon;
use crate::ui::theme::{Theme, metrics};
use crate::ui::views::eq_bars::EqBars;
use crate::ui::views::eq_panel::EqPanel;
use crate::ui::views::island_wave::IslandWave;
use crate::ui::views::library::{LibraryView, LibraryViewEvent};
use crate::ui::views::lyrics::{LyricsEvent, LyricsView};
use crate::ui::views::queue::{ImportMode, QueueEvent, QueueView, import_paths};
use crate::ui::views::settings::SettingsPanel;
use crate::ui::widgets::IconButton;
use crate::ui::window_mode::{ModeTransition, TRANSITION_DURATION, WindowMode, layer_opacity};
use crate::ui::{t, tf};

const CONTROLS_HIDE_DELAY: Duration = Duration::from_secs(2);
const FRAME: Duration = Duration::from_millis(8);
const TOAST_DURATION: Duration = Duration::from_millis(3200);
const MAX_TOASTS: usize = 3;
const PLACEMENT_SAVE_DELAY: Duration = Duration::from_millis(600);
/// Seeking with the keyboard jumps this far.
const SEEK_STEP: f32 = 5.0;

/// Notifications for the app shell (tray labels follow window visibility).
pub enum MainViewEvent {
    VisibilityChanged,
}

/// Which full-window sheet slides in from the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sheet {
    Queue,
    Library,
}

struct Toast {
    id: u64,
    message: SharedString,
}

pub struct MainView {
    pub(super) player: Entity<PlayerModel>,
    pub(super) settings: Entity<SettingsModel>,
    focus: FocusHandle,
    native: Option<NativeWindow>,
    window_visible: bool,

    pub(super) mode: WindowMode,
    transition: Option<ModeTransition>,
    transition_task: Option<Task<()>>,
    pinned: bool,

    pub(super) show_lyrics: bool,
    saved_lyrics: bool,
    pending_settings: bool,
    pub(super) show_eq: bool,
    pub(super) sheet: Option<Sheet>,
    /// The sheet being shown or animated out (kept while `sheet_anim` runs down).
    sheet_shown: Option<Sheet>,
    settings_open: bool,

    sheet_anim: Tween,
    settings_anim: Tween,
    eq_anim: Tween,
    pub(super) lyrics_anim: Tween,
    controls_anim: Tween,
    hide_controls_task: Option<Task<()>>,

    pub(super) lyrics: Entity<LyricsView>,
    queue: Entity<QueueView>,
    library_view: Entity<LibraryView>,
    pub(super) eq_bars: Entity<EqBars>,
    pub(super) island_wave: Entity<IslandWave>,

    toasts: Vec<Toast>,
    next_toast: u64,
    toast_tasks: Vec<Task<()>>,
    import_task: Option<Task<()>>,
    placement_task: Option<Task<()>>,
}

impl EventEmitter<MainViewEvent> for MainView {}

impl Focusable for MainView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MainView {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        library: Entity<LibraryModel>,
        placement: Option<WindowPlacement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let native = NativeWindow::from_window(window);
        let mode = match placement.map(|placement| placement.mode) {
            Some(SavedWindowMode::Mini) => WindowMode::Mini,
            Some(SavedWindowMode::Island) => WindowMode::Island,
            _ => WindowMode::Full,
        };
        let pinned = mode == WindowMode::Island || placement.is_some_and(|placement| placement.pinned);
        let lyrics = cx.new(|cx| LyricsView::new(&player, settings.clone(), native, cx));
        let queue = cx.new(|cx| QueueView::new(player.clone(), settings.clone(), native, cx));
        let library_view =
            cx.new(|cx| LibraryView::new(library.clone(), player.clone(), settings.clone(), native, cx));
        let eq_bars = cx.new(|cx| EqBars::new(&player, &settings, cx));
        let island_wave = cx.new(|cx| IslandWave::new(&player, &settings, cx));

        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&lyrics, window, |this, _, event, window, cx| match event {
            LyricsEvent::Close => this.toggle_lyrics(window, cx),
            LyricsEvent::Toast(message) => this.toast(message.clone(), cx),
        })
        .detach();
        cx.subscribe_in(&queue, window, |this, _, event, window, cx| match event {
            QueueEvent::Close => this.set_sheet(None, window, cx),
            QueueEvent::Toast(message) => this.toast(message.clone(), cx),
        })
        .detach();
        cx.subscribe_in(&library_view, window, |this, _, event, window, cx| match event {
            LibraryViewEvent::Close => this.set_sheet(None, window, cx),
            LibraryViewEvent::Toast(message) => this.toast(message.clone(), cx),
        })
        .detach();
        cx.subscribe(&player, |this, _, event, cx| {
            if let PlayerEvent::Problem(problem) = event {
                let message = problem_message(problem, cx);
                this.toast(message, cx);
            }
        })
        .detach();
        cx.subscribe(&library, |this, _, event, cx| match event {
            LibraryEvent::ScanFinished { added, removed } if added + removed > 0 => {
                let message = tf(
                    cx,
                    Key::ToastScanDone,
                    &[("added", &added.to_string()), ("removed", &removed.to_string())],
                );
                this.toast(message.into(), cx);
            }
            LibraryEvent::ScanFinished { .. } => {}
        })
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| this.schedule_placement_save(window, cx))
            .detach();

        let focus = cx.focus_handle();
        window.focus(&focus);

        Self {
            player,
            settings,
            focus,
            native,
            window_visible: true,
            mode,
            transition: None,
            transition_task: None,
            pinned,
            show_lyrics: false,
            saved_lyrics: false,
            pending_settings: false,
            show_eq: false,
            sheet: None,
            sheet_shown: None,
            settings_open: false,
            sheet_anim: Tween::settled(0.0),
            settings_anim: Tween::settled(0.0),
            eq_anim: Tween::settled(0.0),
            lyrics_anim: Tween::settled(0.0),
            controls_anim: Tween::settled(1.0),
            hide_controls_task: None,
            lyrics,
            queue,
            library_view,
            eq_bars,
            island_wave,
            toasts: Vec::new(),
            next_toast: 0,
            toast_tasks: Vec::new(),
            import_task: None,
            placement_task: None,
        }
    }

    pub fn native(&self) -> Option<NativeWindow> {
        self.native
    }

    pub fn is_window_visible(&self) -> bool {
        self.window_visible
    }

    // ── Window visibility ───────────────────────────────────────────────────

    pub fn show_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(native) = self.native {
            let island =
                (self.mode == WindowMode::Island).then(|| self.island_rect(native, window)).flatten();
            native.defer(cx, move |native| {
                native.show();
                if let Some(rect) = island {
                    native.set_client_rect(rect);
                }
            });
        }
        window.activate_window();
        window.focus(&self.focus);
        self.set_window_visible(true, cx);
    }

    /// Hides to the tray. Without native support the window is minimized instead.
    pub fn hide_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.native {
            Some(native) => native.defer(cx, NativeWindow::hide),
            None => window.minimize_window(),
        }
        self.set_window_visible(false, cx);
    }

    pub fn toggle_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.native.map_or(self.window_visible, NativeWindow::is_visible);
        if visible {
            self.hide_window(window, cx);
        } else {
            self.show_window(window, cx);
        }
    }

    fn set_window_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.window_visible = visible;
        self.player.update(cx, |player, cx| player.set_quiet(!visible, cx));
        self.eq_bars.update(cx, |bars, cx| bars.set_window_visible(visible, cx));
        self.island_wave.update(cx, |wave, cx| wave.set_window_visible(visible, cx));
        cx.emit(MainViewEvent::VisibilityChanged);
    }

    fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        if let Some(native) = self.native {
            native.defer(cx, move |native| native.set_topmost(pinned));
        }
        self.save_placement_mode(cx);
        cx.notify();
    }

    /// Finishes restoring last time's mode once the window exists: pin state, the island's
    /// spot at the top of the screen, and which decorations animate.
    pub fn apply_initial_placement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.mode;
        self.island_wave.update(cx, |wave, cx| wave.set_shown(mode == WindowMode::Island, cx));
        self.eq_bars.update(cx, |bars, cx| bars.set_shown(mode == WindowMode::Full, cx));
        NativeWindow::set_edge_snapping(mode == WindowMode::Mini);
        if let Some(native) = self.native {
            let pinned = self.pinned;
            let island = (mode == WindowMode::Island).then(|| self.island_rect(native, window)).flatten();
            native.defer(cx, move |native| {
                native.set_topmost(pinned);
                if let Some(rect) = island {
                    native.set_client_rect(rect);
                }
            });
        }
    }

    fn schedule_placement_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transition.is_some() || self.mode == WindowMode::Island {
            return;
        }
        let origin = window.bounds().origin;
        let (x, y) = ((origin.x / px(1.0)).round() as i32, (origin.y / px(1.0)).round() as i32);
        self.placement_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PLACEMENT_SAVE_DELAY).await;
            this.update(cx, |this, cx| this.store_placement(Some((x, y)), cx)).ok();
        }));
    }

    fn save_placement_mode(&mut self, cx: &mut Context<Self>) {
        self.store_placement(None, cx);
    }

    /// Records the window mode, pin state and (when given) the window's position.
    fn store_placement(&mut self, origin: Option<(i32, i32)>, cx: &mut Context<Self>) {
        let mode = match self.mode {
            WindowMode::Full => SavedWindowMode::Full,
            WindowMode::Mini => SavedWindowMode::Mini,
            WindowMode::Island => SavedWindowMode::Island,
        };
        let pinned = self.pinned;
        self.settings.update(cx, |settings, cx| {
            settings.update(cx, |s| {
                let previous = s.session.window;
                let (x, y) = origin.or(previous.map(|p| (p.x, p.y))).unwrap_or((i32::MIN, i32::MIN));
                if x == i32::MIN {
                    return;
                }
                s.session.window = Some(WindowPlacement { mode, x, y, pinned });
            })
        });
    }

    // ── Toasts ──────────────────────────────────────────────────────────────

    pub fn toast(&mut self, message: SharedString, cx: &mut Context<Self>) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.retain(|toast| toast.message != message);
        self.toasts.push(Toast { id, message });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.toast_tasks.retain(|_| true);
        self.toast_tasks.push(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            this.update(cx, |this, cx| {
                this.toasts.retain(|toast| toast.id != id);
                cx.notify();
            })
            .ok();
        }));
        if self.toast_tasks.len() > 8 {
            self.toast_tasks.drain(..self.toast_tasks.len() - 8);
        }
        cx.notify();
    }

    // ── Panels ──────────────────────────────────────────────────────────────

    fn close_transient_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_sheet(None, window, cx);
        self.set_settings_open(false, window, cx);
        self.set_eq_open(false, cx);
    }

    pub(super) fn toggle_sheet(&mut self, sheet: Sheet, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.sheet == Some(sheet) { None } else { Some(sheet) };
        self.set_sheet(target, window, cx);
    }

    pub(super) fn set_sheet(&mut self, sheet: Option<Sheet>, window: &mut Window, cx: &mut Context<Self>) {
        if sheet.is_some() && self.mode != WindowMode::Full {
            return;
        }
        if sheet == self.sheet {
            return;
        }
        match self.sheet {
            Some(Sheet::Queue) => self.queue.update(cx, |queue, cx| queue.closed(cx)),
            Some(Sheet::Library) => self.library_view.update(cx, |library, cx| library.closed(cx)),
            None => {}
        }
        self.sheet = sheet;
        match sheet {
            Some(open) => {
                self.set_settings_open(false, window, cx);
                self.set_eq_open(false, cx);
                // Switching between sheets swaps the content without sliding.
                if self.sheet_shown.is_some() && self.sheet_anim.value() > 0.0 {
                    self.sheet_anim.jump_to(1.0);
                }
                self.sheet_shown = Some(open);
                match open {
                    Sheet::Queue => self.queue.update(cx, |queue, cx| queue.opened(window, cx)),
                    Sheet::Library => self.library_view.update(cx, |library, cx| library.opened(window, cx)),
                }
            }
            None => window.focus(&self.focus),
        }
        self.sheet_anim.animate_to(
            sheet.is_some() as u8 as f32,
            Duration::from_millis(320),
            easing::in_out_quad,
        );
        cx.notify();
    }

    fn set_settings_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = open;
        if open {
            self.set_sheet(None, window, cx);
            self.set_eq_open(false, cx);
        }
        self.settings_anim.animate_to(open as u8 as f32, Duration::from_millis(220), easing::in_out_quad);
        cx.notify();
    }

    pub(super) fn set_eq_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let open = open && self.mode == WindowMode::Full;
        self.show_eq = open;
        self.eq_anim.animate_to(open as u8 as f32, Duration::from_millis(200), easing::out_quad);
        cx.notify();
    }

    pub(super) fn toggle_eq(&mut self, cx: &mut Context<Self>) {
        if !self.show_eq && self.show_lyrics {
            self.set_lyrics(false, cx);
        }
        self.set_eq_open(!self.show_eq, cx);
    }

    fn set_lyrics(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_lyrics = show;
        if show {
            self.set_eq_open(false, cx);
        }
        self.lyrics.update(cx, |lyrics, cx| lyrics.set_visible(show, cx));
        self.lyrics_anim.animate_to(show as u8 as f32, Duration::from_millis(200), easing::out_quad);
        cx.notify();
    }

    pub(super) fn toggle_lyrics(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == WindowMode::Full {
            self.set_lyrics(!self.show_lyrics, cx);
        }
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != WindowMode::Full {
            self.pending_settings = true;
            self.switch_mode(WindowMode::Full, window, cx);
            return;
        }
        self.pending_settings = false;
        self.close_transient_panels(window, cx);
        self.set_settings_open(true, window, cx);
    }

    /// Adds files or folders dropped on the window. With nothing playing they start at once.
    fn on_files_dropped(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = paths.paths().to_vec();
        let player = self.player.read(cx);
        let mode = if player.tracks().is_empty() || player.status() == Status::Stopped {
            ImportMode::PlayNow
        } else {
            ImportMode::Append
        };
        let task = import_paths(self.player.clone(), paths, mode, cx);
        self.import_task = Some(cx.spawn(async move |this, cx| {
            if let Some(message) = task.await {
                this.update(cx, |this, cx| this.toast(message, cx)).ok();
            }
        }));
    }

    /// Plays files handed over from the file manager or a second launch. They join the end
    /// of the queue, so what was queued before isn't lost.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let task = import_paths(self.player.clone(), paths, ImportMode::AppendAndPlay, cx);
        self.import_task = Some(cx.spawn(async move |this, cx| {
            if let Some(message) = task.await {
                this.update(cx, |this, cx| this.toast(message, cx)).ok();
            }
        }));
    }

    pub(super) fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.player.read(cx).current_path().map(|path| path.to_path_buf()) else { return };
        self.settings.update(cx, |settings, cx| settings.update(cx, |s| s.toggle_favorite(&path)));
    }

    pub(super) fn is_current_favorite(&self, cx: &App) -> bool {
        self.player.read(cx).current_path().is_some_and(|path| self.settings.read(cx).get().is_favorite(path))
    }

    // ── Modes ───────────────────────────────────────────────────────────────

    fn toggle_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.mode == WindowMode::Mini { WindowMode::Full } else { WindowMode::Mini };
        self.switch_mode(target, window, cx);
    }

    fn island_rect(&self, native: NativeWindow, window: &Window) -> Option<Rect> {
        let monitor = native.monitor_rect()?;
        let scale = window.scale_factor();
        let (w, h) = WindowMode::Island.size();
        Some(monitor.top_centered(
            (w * scale).round() as i32,
            (h * scale).round() as i32,
            (metrics::ISLAND_TOP_MARGIN * scale).round() as i32,
        ))
    }

    /// Animates the window into another mode: bounds, corner radius and a content cross-fade.
    pub(super) fn switch_mode(&mut self, to: WindowMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == to || self.transition.is_some() {
            return;
        }
        let from = self.mode;
        if from == WindowMode::Full {
            self.saved_lyrics = self.show_lyrics;
        }
        if to != WindowMode::Full {
            self.pending_settings = false;
            self.close_transient_panels(window, cx);
            self.set_lyrics(false, cx);
            self.lyrics_anim.jump_to(0.0);
        }
        if to == WindowMode::Island {
            self.set_pinned(true, cx);
        }
        NativeWindow::set_edge_snapping(false);

        let scale = window.scale_factor();
        let (width, height) = to.size();
        let native = self.native;
        let placement = native.and_then(|native| {
            let current = native.client_rect()?;
            let monitor = native.monitor_rect()?;
            let (w, h) = ((width * scale).round() as i32, (height * scale).round() as i32);
            let target = match (from, to) {
                (_, WindowMode::Island) => self.island_rect(native, window)?,
                (WindowMode::Island, _) => monitor.centered(w, h),
                _ => Rect::new(current.x, current.y, w, h).clamped_within(&monitor),
            };
            Some((native, current, target))
        });
        let start_size = window.viewport_size();

        self.transition = Some(ModeTransition { from, to, progress: 0.0 });
        self.transition_task = Some(cx.spawn_in(window, async move |this, cx| {
            let start = Instant::now();
            loop {
                let t = (start.elapsed().as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).min(1.0);
                let eased = easing::in_out_cubic(t);
                // Outside of `update`: resizing re-enters the window procedure synchronously.
                if let Some((native, from_rect, to_rect)) = placement {
                    native.set_client_rect(from_rect.lerp(&to_rect, eased));
                }
                let updated = this.update_in(cx, |view, window, cx| {
                    if placement.is_none() {
                        let lerp = |a: gpui::Pixels, b: f32| px(easing::lerp(a / px(1.0), b, eased));
                        window.resize(size(lerp(start_size.width, width), lerp(start_size.height, height)));
                    }
                    if let Some(transition) = &mut view.transition {
                        transition.progress = eased;
                    }
                    if t >= 1.0 {
                        view.finish_transition(window, cx);
                    }
                    cx.notify();
                });
                if updated.is_err() || t >= 1.0 {
                    break;
                }
                cx.background_executor().timer(FRAME).await;
            }
        }));
        cx.notify();
    }

    fn finish_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(transition) = self.transition.take() else { return };
        self.mode = transition.to;
        NativeWindow::set_edge_snapping(self.mode == WindowMode::Mini);
        self.island_wave.update(cx, |wave, cx| wave.set_shown(self.mode == WindowMode::Island, cx));
        self.eq_bars.update(cx, |bars, cx| bars.set_shown(self.mode == WindowMode::Full, cx));
        if self.mode == WindowMode::Full {
            if self.pending_settings {
                self.pending_settings = false;
                self.close_transient_panels(window, cx);
                self.set_settings_open(true, window, cx);
            } else if self.saved_lyrics && !self.settings_open {
                self.set_lyrics(true, cx);
            }
        }
        if self.mode == WindowMode::Island {
            self.save_placement_mode(cx);
        } else {
            self.schedule_placement_save(window, cx);
        }
    }

    // ── Window controls ─────────────────────────────────────────────────────

    fn on_controls_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.hide_controls_task = None;
            self.controls_anim.animate_to(1.0, Duration::from_millis(300), easing::in_out_quad);
            cx.notify();
        } else {
            self.hide_controls_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(CONTROLS_HIDE_DELAY).await;
                this.update(cx, |view, cx| {
                    view.controls_anim.animate_to(0.0, Duration::from_millis(300), easing::in_out_quad);
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    fn render_window_controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let mini = self.mode == WindowMode::Mini;
        let button = |id: &'static str, icon: Icon| {
            IconButton::new(id, icon)
                .size(26.0, 12.0)
                .hover_bg(theme.hover_bg)
                .icon_opacity(theme.icon_soft_opacity)
        };
        div()
            .id("window-controls")
            .absolute()
            .top(px(4.0))
            .right(px(6.0))
            .p(px(10.0))
            .flex()
            .gap(px(6.0))
            .opacity(self.controls_anim.value())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.on_controls_hover(*hovered, cx)))
            .child(
                button("mode-mini", if mini { Icon::PictureInPicture2 } else { Icon::PictureInPicture })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mini(window, cx))),
            )
            .child(button("mode-island", Icon::Pill).on_click(
                cx.listener(|this, _, window, cx| this.switch_mode(WindowMode::Island, window, cx)),
            ))
            .when(NativeWindow::SUPPORTS_PLACEMENT, |el| {
                el.child(
                    button("pin", Icon::Pin)
                        .active(self.pinned)
                        .icon_opacity(theme.icon_muted_opacity)
                        .on_click(cx.listener(|this, _, _, cx| this.set_pinned(!this.pinned, cx))),
                )
            })
            .child(
                button("settings", Icon::Settings)
                    .active(self.settings_open)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .child(button("minimize", Icon::Minus).on_click(|_, window, _| window.minimize_window()))
            .child(
                button("close", Icon::X)
                    .hover_bg(theme.close_hover_bg)
                    .on_click(cx.listener(|this, _, window, cx| this.hide_window(window, cx))),
            )
    }

    fn render_drag_strip(&self) -> impl IntoElement {
        div()
            .id("drag-strip")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(metrics::DRAG_HEIGHT))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                if !NativeWindow::SUPPORTS_PLACEMENT {
                    window.start_window_move();
                }
            })
    }

    fn render_surface(&self, island: bool, radius: f32, theme: &Theme) -> impl IntoElement {
        let (top, bottom) = if island {
            (theme.island_highlight, theme.island_bottom_tint)
        } else {
            (theme.surface_highlight, theme.surface_bottom_tint)
        };
        let mini = self.mode == WindowMode::Mini;
        // Soft glows: a small blob blurred by its own shadow, kept clear of the corners so
        // the transparent window corners are never tinted.
        let glow = |left: f32, top: f32, width: f32, height: f32, color: gpui::Hsla| {
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(height))
                .rounded_full()
                .bg(color)
                .shadow(vec![gpui::BoxShadow {
                    color,
                    offset: gpui::point(px(0.0), px(0.0)),
                    blur_radius: px(width * 0.6),
                    spread_radius: px(width * 0.15),
                }])
        };

        // One full-size layer: GPUI clamps corner radii to half the element's height, so
        // splitting the gradient into short bands would square off the island's ends.
        div()
            .absolute()
            .size_full()
            .rounded(px(radius))
            .bg(linear_gradient(180.0, linear_color_stop(top, 0.0), linear_color_stop(bottom, 1.0)))
            .when(!island && mini, |el| {
                el.child(glow(30.0, 20.0, 90.0, 50.0, theme.surface_glow_blue)).child(glow(
                    240.0,
                    70.0,
                    70.0,
                    70.0,
                    theme.surface_glow_rose,
                ))
            })
            .when(!island && !mini, |el| {
                el.child(glow(70.0, 50.0, 160.0, 110.0, theme.surface_glow_blue)).child(glow(
                    360.0,
                    140.0,
                    130.0,
                    130.0,
                    theme.surface_glow_rose,
                ))
            })
    }

    fn render_mode_layer(
        &mut self,
        layer: WindowMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut opacity = layer_opacity(layer, self.mode, self.transition.as_ref());
        // Hide content fully covered by an opaque overlay.
        if self.settings_anim.value() >= 1.0 || (layer == WindowMode::Full && self.sheet_anim.value() >= 1.0)
        {
            opacity = 0.0;
        }
        if opacity <= 0.0 {
            return None;
        }
        let (width, height) = layer.size();
        let kitten = self.settings.read(cx).get().ui.show_kitten.then(|| render_kitten(layer, cx));
        let content = match layer {
            WindowMode::Full => self.render_full(window, cx).into_any_element(),
            WindowMode::Mini => self.render_mini(cx).into_any_element(),
            WindowMode::Island => self.render_island(cx).into_any_element(),
        };
        // The kitten goes first so it stays behind the controls.
        let positioned =
            div().absolute().w(px(width)).h(px(height)).opacity(opacity).children(kitten).child(content);
        // The island is centered in whatever the window currently is; the others hang from
        // the top-left so they are revealed, not squeezed, while the window grows.
        Some(if layer == WindowMode::Island {
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(positioned.relative())
                .into_any_element()
        } else {
            positioned.top_0().left_0().into_any_element()
        })
    }

    /// Transient messages, bottom center.
    fn render_toasts(&self, theme: &Theme) -> Option<impl IntoElement + use<>> {
        if self.toasts.is_empty() || self.mode == WindowMode::Island {
            return None;
        }
        let bottom = if self.mode == WindowMode::Mini { 12.0 } else { 86.0 };
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(bottom))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .children(self.toasts.iter().map(|toast| {
                    div()
                        .max_w(px(520.0))
                        .px(px(16.0))
                        .py(px(9.0))
                        .rounded(px(12.0))
                        .bg(theme.surface_panel_bg.opacity(0.97))
                        .border_1()
                        .border_color(theme.surface_panel_border)
                        .shadow(vec![gpui::BoxShadow {
                            color: gpui::hsla(0.0, 0.0, 0.0, if theme.dark { 0.4 } else { 0.12 }),
                            offset: gpui::point(px(0.0), px(4.0)),
                            blur_radius: px(14.0),
                            spread_radius: px(0.0),
                        }])
                        .text_size(px(12.5))
                        .text_color(theme.text_secondary)
                        .child(toast.message.clone())
                })),
        )
    }

    /// A banner with a reconnect button while the audio device is unavailable.
    fn render_device_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.mode == WindowMode::Island {
            return None;
        }
        let problem = self.player.read(cx).device_problem()?;
        let theme = cx.global::<Theme>().clone();
        let message = t(
            cx,
            match problem {
                Problem::DeviceLost => Key::DeviceBannerLost,
                _ => Key::DeviceBannerUnavailable,
            },
        );
        let warning = gpui::hsla(0.08, 0.85, if theme.dark { 0.62 } else { 0.45 }, 1.0);
        Some(
            div().absolute().top(px(14.0)).left_0().right_0().flex().justify_center().child(
                div()
                    .h(px(36.0))
                    .pl(px(14.0))
                    .pr(px(4.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(12.0))
                    .bg(theme.surface_panel_bg)
                    .border_1()
                    .border_color(warning.opacity(0.5))
                    .text_size(px(12.5))
                    .text_color(theme.text_secondary)
                    .child(svg().path(Icon::Speaker.path()).size(px(14.0)).text_color(warning))
                    .child(message)
                    .child(
                        div()
                            .id("device-retry")
                            .h(px(28.0))
                            .px(px(12.0))
                            .flex()
                            .items_center()
                            .rounded(px(9.0))
                            .cursor_pointer()
                            .bg(theme.button_active_bg)
                            .hover(|s| s.bg(theme.button_hover_bg))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(t(cx, Key::Reconnect))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.player.update(cx, |player, cx| player.retry_device(cx))
                            })),
                    ),
            ),
        )
    }
}

/// The text shown for a playback problem.
fn problem_message(problem: &Problem, cx: &App) -> SharedString {
    match problem {
        Problem::Skipped { path, issue } => {
            let reason = t(
                cx,
                match issue {
                    TrackIssue::Missing => Key::IssueMissing,
                    TrackIssue::Unreadable(_) => Key::IssueUnreadable,
                },
            );
            let name = media_paths::display_stem(path);
            tf(cx, Key::ToastSkipped, &[("name", &name), ("reason", reason)]).into()
        }
        Problem::NothingPlayable => t(cx, Key::ToastNothingPlayable).into(),
        Problem::DeviceUnavailable(_) => t(cx, Key::ToastDeviceUnavailable).into(),
        Problem::DeviceLost => t(cx, Key::ToastDeviceLost).into(),
    }
}

/// The app icon's kitten, faintly peeking in from the layer's bottom-right corner.
fn render_kitten(layer: WindowMode, cx: &App) -> impl IntoElement + use<> {
    let (size, right) = match layer {
        WindowMode::Full => (metrics::KITTEN_SIZE, 0.0),
        WindowMode::Mini => (metrics::KITTEN_SIZE_MINI, 0.0),
        WindowMode::Island => (metrics::KITTEN_SIZE_ISLAND, metrics::KITTEN_ISLAND_RIGHT),
    };
    svg()
        .absolute()
        .bottom_0()
        .right(px(right))
        .size(px(size))
        .path(Icon::Kitten.path())
        .text_color(cx.global::<Theme>().kitten)
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let animating =
            [&self.sheet_anim, &self.settings_anim, &self.eq_anim, &self.lyrics_anim, &self.controls_anim]
                .iter()
                .any(|tween| tween.is_running());
        if animating {
            window.request_animation_frame();
        }
        if !self.sheet_anim.is_running() && self.sheet.is_none() {
            self.sheet_shown = None;
        }

        let island_shell =
            self.transition.as_ref().map_or(self.mode == WindowMode::Island, ModeTransition::island_shell);
        let radius =
            self.transition.as_ref().map_or(self.mode.corner_radius(), ModeTransition::corner_radius);

        let layers: Vec<AnyElement> = [WindowMode::Full, WindowMode::Mini, WindowMode::Island]
            .into_iter()
            .filter_map(|layer| self.render_mode_layer(layer, window, cx))
            .collect();

        let full = self.mode == WindowMode::Full && self.transition.is_none();
        let eq = self.eq_anim.value();
        let sheet = self.sheet_anim.value();
        let settings = self.settings_anim.value();
        let player = self.player.clone();

        let window_width = window.viewport_size().width;
        // Overlays own their (rounded) background; GPUI clips children only to rectangles,
        // so a square background inside would poke out of the window's rounded corners.
        let overlay = |fraction: f32| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .rounded(px(radius))
                .overflow_hidden()
                .bg(theme.surface_overlay_bg)
                .opacity(fraction)
        };
        let sheet_content: Option<AnyElement> = match self.sheet_shown {
            Some(Sheet::Queue) => Some(self.queue.clone().into_any_element()),
            Some(Sheet::Library) => Some(self.library_view.clone().into_any_element()),
            None => None,
        };

        let show_chrome = self.transition.is_none()
            && self.mode != WindowMode::Island
            && self.sheet.is_none()
            && !self.settings_open
            && !self.show_eq;
        let drop_accent = theme.accent;

        div()
            .id("main")
            .key_context(KEY_CONTEXT)
            .on_action(
                cx.listener(|this, _: &TogglePlay, _, cx| this.player.update(cx, |p, cx| p.toggle_play(cx))),
            )
            .on_action(
                cx.listener(|this, _: &Previous, _, cx| this.player.update(cx, |p, cx| p.previous(cx))),
            )
            .on_action(cx.listener(|this, _: &Next, _, cx| this.player.update(cx, |p, cx| p.next(cx))))
            .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                this.player.update(cx, |p, cx| p.seek_by(-SEEK_STEP, cx))
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.player.update(cx, |p, cx| p.seek_by(SEEK_STEP, cx))
            }))
            .on_action(
                cx.listener(|this, _: &VolumeUp, _, cx| {
                    this.player.update(cx, |p, cx| p.adjust_volume(5, cx))
                }),
            )
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| {
                this.player.update(cx, |p, cx| p.adjust_volume(-5, cx))
            }))
            .on_action(
                cx.listener(|this, _: &ToggleMute, _, cx| this.player.update(cx, |p, cx| p.toggle_mute(cx))),
            )
            .on_action(
                cx.listener(|this, _: &CycleRepeat, _, cx| {
                    this.player.update(cx, |p, cx| p.cycle_repeat(cx))
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleShuffle, _, cx| {
                this.player.update(cx, |p, cx| p.toggle_shuffle(cx))
            }))
            .on_action(
                cx.listener(|this, _: &TogglePlaylist, window, cx| {
                    this.toggle_sheet(Sheet::Queue, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleLibrary, window, cx| {
                this.toggle_sheet(Sheet::Library, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLyrics, window, cx| this.toggle_lyrics(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleEq, _, cx| this.toggle_eq(cx)))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| this.toggle_favorite(cx)))
            .size_full()
            .relative()
            .overflow_hidden()
            .rounded(px(radius))
            .bg(if island_shell { theme.island_bg } else { theme.surface_bg })
            .font_family(theme.font_family.clone())
            .text_color(theme.text_primary)
            // Keyboard focus lives on an invisible child: a focusable element under the cursor
            // claims mouse-downs, which on Windows would also cancel native window dragging.
            .child(div().absolute().size_0().track_focus(&self.focus))
            .child(self.render_surface(island_shell, radius, &theme))
            .children(layers)
            // Files dropped anywhere on the player (sheets handle their own drops).
            .child(
                div()
                    .id("file-drop")
                    .absolute()
                    .size_full()
                    .rounded(px(radius))
                    .drag_over::<ExternalPaths>(move |style, _, _, _| {
                        style.bg(drop_accent.opacity(0.06)).border_2().border_color(drop_accent.opacity(0.7))
                    })
                    .on_drop(
                        cx.listener(|this, paths: &ExternalPaths, _, cx| this.on_files_dropped(paths, cx)),
                    ),
            )
            .when(full && eq > 0.0, |el| {
                el.child(
                    div()
                        .id("eq-overlay")
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(eq)
                        .on_click(cx.listener(|this, _, _, cx| this.set_eq_open(false, cx)))
                        .child(EqPanel::new(player)),
                )
            })
            .when_some(sheet_content.filter(|_| sheet > 0.0), |el, content| {
                // Slides in from the right: the sheet grows from the right edge and the
                // full-width content rides on its left edge.
                el.child(
                    overlay(sheet)
                        .w(relative(sheet))
                        .child(div().absolute().top_0().bottom_0().left_0().w(window_width).child(content)),
                )
            })
            .when(settings > 0.0, |el| {
                let view = cx.entity().downgrade();
                el.child(overlay(settings).left_0().child(SettingsPanel::new(
                    self.settings.clone(),
                    radius,
                    move |window, cx| {
                        view.update(cx, |this, cx| this.set_settings_open(false, window, cx)).ok();
                    },
                )))
            })
            .when(show_chrome, |el| el.child(self.render_drag_strip()).child(self.render_window_controls(cx)))
            .children(self.render_device_banner(cx))
            .children(self.render_toasts(&theme))
            // The window outline is drawn last so overlays never cover it.
            .when(!island_shell, |el| {
                el.child(
                    div()
                        .absolute()
                        .size_full()
                        .rounded(px(radius))
                        .border_1()
                        .border_color(theme.surface_border),
                )
            })
    }
}
