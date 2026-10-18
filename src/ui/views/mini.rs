//! The compact horizontal layout.

use gpui::{Context, FontWeight, IntoElement, ParentElement, Styled, div, px};
use player_core::i18n::Key;
use player_core::queue::RepeatMode;

use super::album_art::AlbumArt;
use super::controls::{mode_toggle, player_action, progress, repeat_icon, transport};
use super::root::MainView;
use crate::model::PlayerModel;
use crate::ui::icons::Icon;
use crate::ui::t;
use crate::ui::theme::{Theme, metrics};

impl MainView {
    pub(super) fn render_mini(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let player = self.player.read(cx);
        let playing = player.is_playing();
        let (repeat, shuffle) = (player.repeat(), player.shuffle());
        let track = player.track().cloned();
        let title =
            track.as_ref().map_or_else(|| t(cx, Key::NoSongSelected).into(), |track| track.title.clone());
        let artist = track
            .as_ref()
            .and_then(|track| track.artist.clone())
            .unwrap_or_else(|| t(cx, Key::LocalMusic).into());

        let info = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pt(px(15.0))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .truncate()
                            .pr(px(metrics::HEADER_INSET))
                            .text_size(px(metrics::TITLE_SIZE_MINI))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(metrics::ARTIST_SIZE_MINI))
                            .text_color(theme.text_subtle)
                            .child(artist),
                    ),
            )
            .child(div().pt(px(6.0)).child(progress(
                "mini-progress",
                &self.player,
                metrics::PROGRESS_HEIGHT_MINI,
                10.0,
                cx,
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        mode_toggle("mini-shuffle", Icon::Shuffle, shuffle, 26.0, 14.0, cx)
                            .on_click(player_action(&self.player, PlayerModel::toggle_shuffle)),
                    )
                    .child(transport("mini-transport", &self.player, 34.0, 16.0, 10.0, cx))
                    .child(
                        mode_toggle(
                            "mini-repeat",
                            repeat_icon(repeat),
                            repeat != RepeatMode::Off,
                            26.0,
                            14.0,
                            cx,
                        )
                        .on_click(player_action(&self.player, PlayerModel::cycle_repeat)),
                    ),
            );

        div()
            .size_full()
            .pl(px(22.0))
            .pr(px(metrics::OUTER_PADDING_MINI))
            .py(px(metrics::OUTER_PADDING_MINI))
            .flex()
            .items_center()
            .gap(px(18.0))
            .child(AlbumArt::new(
                "mini-art",
                track.and_then(|track| track.cover),
                metrics::ALBUM_SIZE_MINI,
                metrics::ALBUM_RADIUS_MINI,
                playing,
            ))
            .child(info)
    }
}
