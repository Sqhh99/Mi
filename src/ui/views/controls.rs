//! Playback controls shared by the full and mini layouts.

use gpui::{App, ClickEvent, Entity, IntoElement, ParentElement, Styled, Window, div, px};
use player_core::queue::RepeatMode;
use player_core::time::fmt_clock;

use crate::model::PlayerModel;
use crate::ui::icons::Icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::{IconButton, Slider};

/// Click handler that runs a `PlayerModel` command.
pub fn player_action(
    player: &Entity<PlayerModel>,
    action: fn(&mut PlayerModel, &mut gpui::Context<PlayerModel>),
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let player = player.clone();
    move |_, _, cx| player.update(cx, action)
}

/// Previous / play-pause / next.
pub fn transport(
    id: &'static str,
    player: &Entity<PlayerModel>,
    button: f32,
    icon: f32,
    gap: f32,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.global::<Theme>();
    let playing = player.read(cx).is_playing();
    div()
        .flex()
        .items_center()
        .gap(px(gap))
        .child(
            IconButton::new((id, 0usize), Icon::SkipBack)
                .size(button, icon)
                .hover_bg(theme.hover_bg)
                .icon_opacity(theme.icon_soft_opacity)
                .on_click(player_action(player, PlayerModel::previous)),
        )
        .child(
            IconButton::new((id, 1usize), if playing { Icon::Pause } else { Icon::Play })
                .size(button, icon)
                .hover_bg(theme.hover_bg)
                .icon_opacity(theme.icon_strong_opacity)
                .on_click(player_action(player, PlayerModel::toggle_play)),
        )
        .child(
            IconButton::new((id, 2usize), Icon::SkipForward)
                .size(button, icon)
                .hover_bg(theme.hover_bg)
                .icon_opacity(theme.icon_soft_opacity)
                .on_click(player_action(player, PlayerModel::next)),
        )
}

/// The repeat button's icon for each mode (the button is highlighted unless repeat is off).
pub fn repeat_icon(repeat: RepeatMode) -> Icon {
    match repeat {
        RepeatMode::One => Icon::Repeat1,
        RepeatMode::Off | RepeatMode::All => Icon::Repeat,
    }
}

/// Shuffle or repeat toggle.
pub fn mode_toggle(
    id: &'static str,
    icon: Icon,
    active: bool,
    size: f32,
    icon_size: f32,
    cx: &App,
) -> IconButton {
    let theme = cx.global::<Theme>();
    IconButton::new(id, icon)
        .size(size, icon_size)
        .active(active)
        .hover_bg(theme.hover_bg)
        .icon_opacity(theme.icon_muted_opacity)
}

/// Seek bar with elapsed / total time underneath.
pub fn progress(
    id: &'static str,
    player: &Entity<PlayerModel>,
    thickness: f32,
    label_size: f32,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.global::<Theme>();
    let model = player.read(cx);
    let (position, duration) = (model.position(), model.duration());
    let fraction = if duration.is_zero() { 0.0 } else { position.as_secs_f32() / duration.as_secs_f32() };
    let seek_player = player.clone();
    let label = |text: String| {
        div()
            .font_family(theme.mono_font_family.clone())
            .text_size(px(label_size))
            .text_color(theme.text_muted)
            .child(text)
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(Slider::new(id, fraction).thickness(thickness).on_change(move |value, _, cx| {
            seek_player.update(cx, |player, cx| {
                let target = player.duration().mul_f32(value);
                player.seek(target, cx);
            });
        }))
        .child(
            div()
                .flex()
                .justify_between()
                .child(label(fmt_clock(position.as_millis() as u64)))
                .child(label(fmt_clock(duration.as_millis() as u64))),
        )
}

/// Mute button plus a live volume slider.
pub fn volume(player: &Entity<PlayerModel>, cx: &App) -> impl IntoElement {
    let theme = cx.global::<Theme>();
    let model = player.read(cx);
    let silent = model.muted() || model.volume() == 0;
    let icon = if silent {
        Icon::VolumeX
    } else if model.volume() < 50 {
        Icon::Volume1
    } else {
        Icon::Volume2
    };
    let volume_player = player.clone();
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            IconButton::new("mute", icon)
                .size(26.0, 14.0)
                .hover_bg(theme.hover_bg)
                .icon_opacity(theme.icon_soft_opacity)
                .on_click(player_action(player, PlayerModel::toggle_mute)),
        )
        .child(
            div().w(px(110.0)).child(
                Slider::new("volume", model.volume() as f32 / 100.0)
                    .thickness(5.0)
                    .always_show_handle()
                    .live()
                    .on_change(move |value, _, cx| {
                        volume_player
                            .update(cx, |player, cx| player.set_volume((value * 100.0).round() as u8, cx))
                    }),
            ),
        )
}
