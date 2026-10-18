//! The full-size layout: album art on the left, title, controls and lyrics on the right.

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px, svg,
};
use player_core::i18n::Key;
use player_core::queue::RepeatMode;

use super::album_art::AlbumArt;
use super::controls::{mode_toggle, player_action, progress, repeat_icon, transport, volume};
use super::root::{MainView, Sheet};
use crate::model::PlayerModel;
use crate::ui::icons::Icon;
use crate::ui::t;
use crate::ui::theme::{Theme, metrics};
use crate::ui::widgets::IconButton;

impl MainView {
    pub(super) fn render_full(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let player = self.player.read(cx);
        let playing = player.is_playing();
        let (repeat, shuffle) = (player.repeat(), player.shuffle());
        let track = player.track().cloned();
        let lyrics = self.lyrics_anim.value();

        let title =
            track.as_ref().map_or_else(|| t(cx, Key::NoSongSelected).into(), |track| track.title.clone());
        let artist = track.as_ref().map_or_else(
            || t(cx, Key::LocalMusic).to_string(),
            |track| {
                let parts: Vec<&str> =
                    [&track.artist, &track.album].into_iter().flatten().map(|part| part.as_ref()).collect();
                if parts.is_empty() { t(cx, Key::LocalMusic).to_string() } else { parts.join(" · ") }
            },
        );
        let favorite = self.is_current_favorite(cx);

        let album_column = div()
            .flex_none()
            .w(px(metrics::ALBUM_COLUMN_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(22.0))
            .child(AlbumArt::new(
                "full-art",
                track.as_ref().and_then(|track| track.cover.clone()),
                metrics::ALBUM_SIZE,
                metrics::ALBUM_RADIUS,
                playing,
            ))
            .child(div().w(px(metrics::ALBUM_SIZE)).child(self.eq_bars.clone()));

        let header = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pt(px(58.0))
            .pr(px(metrics::HEADER_INSET))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(metrics::TITLE_SIZE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .when(track.is_some(), |el| {
                        el.child(
                            IconButton::new(
                                "favorite",
                                if favorite { Icon::HeartFilled } else { Icon::Heart },
                            )
                            .size(32.0, 17.0)
                            .active(favorite)
                            .hover_bg(theme.hover_bg)
                            .icon_opacity(theme.icon_muted_opacity)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(metrics::ARTIST_SIZE))
                    .text_color(theme.text_subtle)
                    .child(artist),
            );

        let panel_toggles = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .gap(px(10.0))
                    .child(
                        mode_toggle("lyrics-toggle", Icon::MicVocal, self.show_lyrics, 34.0, 16.0, cx)
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_lyrics(window, cx))),
                    )
                    .child(
                        mode_toggle("eq-toggle", Icon::SlidersVertical, self.show_eq, 34.0, 16.0, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_eq(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.0))
                    .child(
                        mode_toggle("shuffle", Icon::Shuffle, shuffle, 34.0, 16.0, cx)
                            .on_click(player_action(&self.player, PlayerModel::toggle_shuffle)),
                    )
                    .child(
                        mode_toggle("repeat", repeat_icon(repeat), repeat != RepeatMode::Off, 34.0, 16.0, cx)
                            .on_click(player_action(&self.player, PlayerModel::cycle_repeat)),
                    ),
            );

        let controls = div()
            .absolute()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(10.0))
            .opacity(1.0 - lyrics)
            .child(panel_toggles)
            .child(progress("full-progress", &self.player, metrics::PROGRESS_HEIGHT, 11.0, cx))
            .child(div().flex().justify_center().pt(px(8.0)).child(transport(
                "full-transport",
                &self.player,
                metrics::CONTROL_SIZE_MD,
                24.0,
                36.0,
                cx,
            )));

        let center = div()
            .relative()
            .flex_1()
            .min_h(px(200.0))
            .when(lyrics < 1.0, |el| el.child(controls))
            .when(lyrics > 0.0, |el| {
                el.child(div().absolute().size_full().opacity(lyrics).child(self.lyrics.clone()))
            });

        let sheet_button = |id: &'static str, sheet: Sheet, icon: Icon, label: &'static str| {
            let open = self.sheet == Some(sheet);
            div()
                .id(id)
                .h(px(36.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.button_border)
                .cursor_pointer()
                .map(|el| {
                    if open {
                        el.bg(theme.button_active_bg)
                    } else {
                        el.bg(theme.button_bg)
                            .hover(|s| s.bg(theme.button_hover_bg))
                            .active(|s| s.bg(theme.button_pressed_bg))
                    }
                })
                .on_click(cx.listener(move |this, _, window, cx| this.toggle_sheet(sheet, window, cx)))
                .child(svg().path(icon.path()).size(px(16.0)).text_color(theme.icon.opacity(0.78)))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .child(label),
                )
        };
        let sheet_buttons = div()
            .flex()
            .gap(px(10.0))
            .child(sheet_button("playlist-button", Sheet::Queue, Icon::ListMusic, t(cx, Key::Playlist)))
            .child(sheet_button("library-button", Sheet::Library, Icon::Library, t(cx, Key::Library)));

        let bottom_bar = div()
            .flex_none()
            .h(px(metrics::BOTTOM_BAR_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .child(sheet_buttons)
            .child(volume(&self.player, cx));

        let details_column =
            div().flex_1().min_w_0().h_full().flex().flex_col().child(header).child(center).child(bottom_bar);

        div()
            .size_full()
            .p(px(metrics::OUTER_PADDING))
            .flex()
            .gap(px(metrics::COLUMN_SPACING))
            .child(album_column)
            .child(details_column)
    }
}
