//! Scrolling, synchronized lyrics. The current line is highlighted and smoothly kept in
//! the vertical center; clicking a line seeks to it. Scrolling by hand pauses the
//! following for a while, and a toolbar adjusts size, timing and the lyrics file.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Entity, EventEmitter, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Task, Window, div, point,
    prelude::FluentBuilder, px, svg,
};
use player_core::easing;
use player_core::i18n::Key;
use player_core::lyrics::Lyrics;
use player_core::media_paths;
use player_core::settings::LYRICS_SCALE_RANGE;

use crate::media::LyricsSource;
use crate::model::{PlayerModel, SettingsModel};
use crate::platform::NativeWindow;
use crate::ui::anim::Tween;
use crate::ui::dialogs::{self, Pick};
use crate::ui::icons::Icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::IconButton;
use crate::ui::{t, tf};

const SCROLL_DURATION: Duration = Duration::from_millis(300);
const LINE_GAP: f32 = 16.0;
const TOOLBAR_SPACE: f32 = 60.0;
/// After scrolling by hand, following resumes after this long without scrolling.
const FOLLOW_RESUME_DELAY: Duration = Duration::from_secs(5);
const OFFSET_STEP_MS: i32 = 500;
const SCALE_STEP: u8 = 10;
const BASE_SIZE: f32 = 13.0;
const CURRENT_SIZE: f32 = 16.0;

pub enum LyricsEvent {
    Close,
    Toast(SharedString),
}

pub struct LyricsView {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    native: Option<NativeWindow>,
    lyrics: Arc<Lyrics>,
    current: Option<usize>,
    scroll: ScrollHandle,
    /// Animated scroll distance from the top (positive = scrolled down).
    scroll_y: Option<Tween>,
    /// Set when the highlight moved; centering happens after the next layout.
    pending_center: bool,
    visible: bool,
    /// False while the user is scrolling through the lyrics themselves.
    following: bool,
    resume_task: Option<Task<()>>,
    pick_task: Option<Task<()>>,
}

impl EventEmitter<LyricsEvent> for LyricsView {}

impl LyricsView {
    pub fn new(
        player: &Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        native: Option<NativeWindow>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(player, |this, player, cx| {
            let (lyrics, index) = {
                let player = player.read(cx);
                (player.lyrics().clone(), player.lyric_index())
            };
            if !Arc::ptr_eq(&this.lyrics, &lyrics) {
                this.lyrics = lyrics;
                this.current = None;
                this.scroll_y = None;
                this.following = true;
                this.resume_task = None;
                this.scroll.set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }
            if index != this.current {
                this.current = index;
                this.pending_center = this.following;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        let model = player.read(cx);
        Self {
            player: player.clone(),
            settings,
            native,
            lyrics: model.lyrics().clone(),
            current: model.lyric_index(),
            scroll: ScrollHandle::new(),
            scroll_y: None,
            pending_center: true,
            visible: false,
            following: true,
            resume_task: None,
            pick_task: None,
        }
    }

    /// Called by the parent when the panel is shown or hidden; recenters on show.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible != self.visible {
            self.visible = visible;
            self.pending_center = visible;
            self.following = true;
            self.scroll_y = None;
            cx.notify();
        }
    }

    /// Starts scrolling so the current line sits in the middle of the viewport.
    fn center_current_line(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.current else { return };
        let Some(line) = self.scroll.bounds_for_item(index) else { return };
        let viewport = self.scroll.bounds();
        let offset = self.scroll.offset();
        let max_scroll = self.scroll.max_offset().height / px(1.0);

        let line_center = (line.center().y - viewport.origin.y - offset.y) / px(1.0);
        let target = (line_center - viewport.size.height / px(1.0) / 2.0).clamp(0.0, max_scroll.max(0.0));
        let current = -offset.y / px(1.0);

        let reduce_motion = self.settings.read(cx).get().ui.reduce_motion;
        let mut tween = Tween::settled(current);
        if reduce_motion {
            tween.jump_to(target);
        } else {
            tween.animate_to(target, SCROLL_DURATION, easing::out_cubic);
        }
        self.scroll_y = Some(tween);
        cx.notify();
    }

    /// The user scrolled: stop following until they leave the lyrics alone for a while.
    fn on_manual_scroll(&mut self, cx: &mut Context<Self>) {
        self.following = false;
        self.scroll_y = None;
        self.resume_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOLLOW_RESUME_DELAY).await;
            this.update(cx, |this, cx| this.resume_following(cx)).ok();
        }));
        cx.notify();
    }

    fn resume_following(&mut self, cx: &mut Context<Self>) {
        self.following = true;
        self.resume_task = None;
        self.pending_center = true;
        cx.notify();
    }

    fn seek_to_line(&mut self, index: usize, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| {
            if let Some(time) = player.lyric_time(index) {
                player.seek(time, cx);
                player.play(cx);
            }
        });
        self.resume_following(cx);
    }

    fn change_scale(&mut self, larger: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(cx, |s| {
                let scale = s.lyrics.scale;
                s.lyrics.scale =
                    if larger { scale.saturating_add(SCALE_STEP) } else { scale.saturating_sub(SCALE_STEP) }
                        .clamp(LYRICS_SCALE_RANGE.0, LYRICS_SCALE_RANGE.1);
            })
        });
        self.pending_center = self.following;
    }

    fn shift_offset(&mut self, delta_ms: Option<i32>, cx: &mut Context<Self>) {
        let offset = self.player.update(cx, |player, cx| {
            match delta_ms {
                Some(delta) => player.adjust_lyrics_offset(delta, cx),
                None => player.reset_lyrics_offset(cx),
            }
            player.lyrics_offset()
        });
        let message = tf(cx, Key::ToastLyricsOffset, &[("offset", &format_offset(offset))]);
        cx.emit(LyricsEvent::Toast(message.into()));
    }

    fn choose_lyrics_file(&mut self, cx: &mut Context<Self>) {
        let start =
            self.player.read(cx).current_path().and_then(|path| path.parent()).map(|dir| dir.to_path_buf());
        let pick = dialogs::pick(Pick::Lyrics, start.as_deref(), self.native, cx);
        self.pick_task = Some(cx.spawn(async move |this, cx| {
            let Some(file) = pick.await.into_iter().next() else { return };
            this.update(cx, |this, cx| {
                let name = media_paths::display_name(&file);
                this.player.update(cx, |player, cx| player.link_lyrics(Some(file), cx));
                cx.emit(LyricsEvent::Toast(tf(cx, Key::ToastLyricsLinked, &[("name", &name)]).into()));
            })
            .ok();
        }));
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let player = self.player.read(cx);
        let has_track = player.current_path().is_some();
        let offset = player.lyrics_offset();
        let linked = player.current_path().is_some_and(|path| player.lyrics_links().contains_key(path));
        let source_label = match player.track().and_then(|track| track.lyrics_source.as_ref()) {
            Some(LyricsSource::File(path)) => media_paths::display_name(path),
            Some(LyricsSource::Embedded) => t(cx, Key::EmbeddedLyrics).to_string(),
            None => String::new(),
        };
        let has_lyrics = !self.lyrics.is_empty();
        let small = |id: &'static str, icon: Icon| {
            IconButton::new(id, icon)
                .size(28.0, 14.0)
                .hover_bg(theme.hover_bg)
                .icon_opacity(theme.icon_muted_opacity)
        };

        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(10.0))
            .h(px(40.0))
            .px(px(4.0))
            .flex()
            .items_center()
            .justify_between()
            .child(div().w(px(150.0)).flex().items_center().gap(px(2.0)).when(has_lyrics, |el| {
                el.child(
                    small("lyrics-smaller", Icon::MinusCircle)
                        .on_click(cx.listener(|this, _, _, cx| this.change_scale(false, cx))),
                )
                .child(svg().path(Icon::ALargeSmall.path()).size(px(15.0)).text_color(theme.text_muted))
                .child(
                    small("lyrics-larger", Icon::PlusCircle)
                        .on_click(cx.listener(|this, _, _, cx| this.change_scale(true, cx))),
                )
            }))
            .child(
                IconButton::new("lyrics-back", Icon::ChevronDown)
                    .bordered()
                    .hover_bg(theme.hover_bg)
                    .icon_opacity(theme.icon_soft_opacity)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(LyricsEvent::Close))),
            )
            .child(
                div()
                    .w(px(150.0))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(2.0))
                    .when(has_lyrics, |el| {
                        el.child(small("lyrics-earlier", Icon::ChevronLeft).on_click(
                            cx.listener(|this, _, _, cx| this.shift_offset(Some(OFFSET_STEP_MS), cx)),
                        ))
                        .child(
                            div()
                                .id("lyrics-offset")
                                .min_w(px(42.0))
                                .h(px(24.0))
                                .px(px(4.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.hover_bg))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(px(11.0))
                                .text_color(if offset == 0 { theme.text_muted } else { theme.accent })
                                .child(format_offset(offset))
                                .on_click(cx.listener(|this, _, _, cx| this.shift_offset(None, cx))),
                        )
                        .child(small("lyrics-later", Icon::ChevronRight).on_click(
                            cx.listener(|this, _, _, cx| this.shift_offset(Some(-OFFSET_STEP_MS), cx)),
                        ))
                    })
                    .when(has_track, |el| {
                        el.child(
                            small("lyrics-link", if linked { Icon::Unlink } else { Icon::Link })
                                .active(linked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if linked {
                                        this.player.update(cx, |player, cx| player.link_lyrics(None, cx));
                                    } else {
                                        this.choose_lyrics_file(cx);
                                    }
                                })),
                        )
                    }),
            )
            .when(!source_label.is_empty(), |el| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(-12.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .text_size(px(9.5))
                        .text_color(theme.text_muted.opacity(0.7))
                        .child(source_label),
                )
            })
    }
}

/// "+0.5s", "-1.0s" or "0.0s".
fn format_offset(offset_ms: i32) -> String {
    let seconds = offset_ms as f32 / 1000.0;
    if offset_ms > 0 { format!("+{seconds:.1}s") } else { format!("{seconds:.1}s") }
}

impl Render for LyricsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let scale = self.settings.read(cx).get().lyrics.scale as f32 / 100.0;

        if self.pending_center && self.visible && self.following {
            self.pending_center = false;
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                this.update(cx, |view, cx| view.center_current_line(cx)).ok();
            });
        }
        if let Some(tween) = &self.scroll_y {
            self.scroll.set_offset(point(px(0.0), px(-tween.value())));
            if tween.is_running() {
                window.request_animation_frame();
            } else {
                self.scroll_y = None;
            }
        }

        let body = if self.lyrics.is_empty() {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .child(div().text_size(px(13.0)).text_color(theme.text_muted).child(t(cx, Key::NoLyrics)))
                .when(self.player.read(cx).current_path().is_some(), |el| {
                    el.child(
                        div()
                            .id("lyrics-choose")
                            .h(px(30.0))
                            .px(px(12.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(theme.button_border)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.button_hover_bg))
                            .text_size(px(12.0))
                            .text_color(theme.text_secondary)
                            .child(
                                svg()
                                    .path(Icon::Link.path())
                                    .size(px(13.0))
                                    .text_color(theme.icon.opacity(0.7)),
                            )
                            .child(t(cx, Key::LinkLyrics))
                            .on_click(cx.listener(|this, _, _, cx| this.choose_lyrics_file(cx))),
                    )
                })
                .into_any_element()
        } else {
            div()
                .id("lyrics-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .on_scroll_wheel(cx.listener(|this, _, _, cx| this.on_manual_scroll(cx)))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP * scale))
                .px(px(16.0))
                .py(px(20.0))
                .font_family(theme.lyrics_font_family.clone())
                .children(self.lyrics.lines.iter().enumerate().map(|(i, line)| {
                    let current = Some(i) == self.current;
                    let (size, color) = if current {
                        (CURRENT_SIZE * scale, theme.text_primary)
                    } else {
                        (BASE_SIZE * scale, theme.text_muted.opacity(0.7))
                    };
                    div()
                        .id(("lyric-line", i))
                        .flex_none()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(3.0 * scale))
                        .py(px(2.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover_bg))
                        .on_click(cx.listener(move |this, _, _, cx| this.seek_to_line(i, cx)))
                        .child(
                            div()
                                .w_full()
                                .text_center()
                                .text_size(px(size))
                                .text_color(color)
                                .when(current, |el| el.font_weight(FontWeight::SEMIBOLD))
                                .child(line.text.clone()),
                        )
                        .children(line.translation.clone().map(|translation| {
                            div()
                                .w_full()
                                .text_center()
                                .text_size(px(size * 0.82))
                                .text_color(if current {
                                    theme.text_subtle
                                } else {
                                    theme.text_muted.opacity(0.55)
                                })
                                .child(translation)
                        }))
                }))
                .into_any_element()
        };

        let resume = (!self.following && self.current.is_some()).then(|| {
            div().absolute().top(px(8.0)).left_0().right_0().flex().justify_center().child(
                div()
                    .id("lyrics-resume")
                    .h(px(28.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(14.0))
                    .bg(theme.surface_panel_bg)
                    .border_1()
                    .border_color(theme.surface_panel_border)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.button_hover_bg))
                    .text_size(px(11.5))
                    .text_color(theme.text_secondary)
                    .child(svg().path(Icon::Crosshair.path()).size(px(12.0)).text_color(theme.accent))
                    .child(t(cx, Key::BackToCurrentLine))
                    .on_click(cx.listener(|this, _, _, cx| this.resume_following(cx))),
            )
        });

        div()
            .size_full()
            .relative()
            .child(div().absolute().top_0().left_0().right_0().bottom(px(TOOLBAR_SPACE)).child(body))
            .children(resume)
            .child(self.render_toolbar(cx))
    }
}
