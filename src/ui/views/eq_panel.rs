//! The equalizer card shown over the full player: three bands with their gain in dB, an
//! on/off switch to compare with the untouched sound, presets and a reset.

use gpui::{
    App, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};
use player_core::eq::{Band, EqLevels, EqPreset, LEVEL_MAX, format_db};
use player_core::i18n::Key;

use crate::model::PlayerModel;
use crate::ui::icons::Icon;
use crate::ui::t;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Slider, Toggle};

#[derive(IntoElement)]
pub struct EqPanel {
    player: Entity<PlayerModel>,
}

impl EqPanel {
    pub fn new(player: Entity<PlayerModel>) -> Self {
        Self { player }
    }
}

fn preset_name(preset: EqPreset) -> Key {
    match preset {
        EqPreset::Flat => Key::PresetFlat,
        EqPreset::BassBoost => Key::PresetBassBoost,
        EqPreset::Vocal => Key::PresetVocal,
        EqPreset::TrebleBoost => Key::PresetTrebleBoost,
        EqPreset::Rock => Key::PresetRock,
        EqPreset::Pop => Key::PresetPop,
        EqPreset::Soft => Key::PresetSoft,
        EqPreset::Electronic => Key::PresetElectronic,
    }
}

impl RenderOnce for EqPanel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let player = self.player.read(cx);
        let (levels, enabled) = (player.eq(), player.eq_enabled());
        let active_preset = EqPreset::matching(levels);
        let band = |band: Band, label: Key| {
            let player = self.player.clone();
            let level = levels.get(band);
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(
                    div().h(px(110.0)).opacity(if enabled { 1.0 } else { 0.45 }).child(
                        Slider::new(("eq-band", band.index()), level as f32 / LEVEL_MAX as f32)
                            .vertical()
                            .handle_size(14.0)
                            .handle_color(theme.track_fill)
                            .always_show_handle()
                            .live()
                            .on_change(move |value, _, cx| {
                                let level = (value * LEVEL_MAX as f32).round() as u8;
                                player.update(cx, |player, cx| {
                                    player.set_eq_level(band, level, cx);
                                    player.set_eq_enabled(true, cx);
                                });
                            }),
                    ),
                )
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(10.5))
                        .text_color(if level == 50 { theme.text_muted } else { theme.text_secondary })
                        .child(format_db(level)),
                )
                .child(div().text_size(px(10.0)).text_color(theme.text_muted).child(t(cx, label)))
        };

        let toggle_player = self.player.clone();
        let reset_player = self.player.clone();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(t(cx, Key::Equalizer)),
            )
            .child(
                div()
                    .id("eq-reset")
                    .h(px(24.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.button_hover_bg))
                    .text_size(px(11.0))
                    .text_color(theme.text_subtle)
                    .child(svg().path(Icon::RotateCcw.path()).size(px(11.0)).text_color(theme.text_subtle))
                    .child(t(cx, Key::EqReset))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        reset_player.update(cx, |player, cx| player.set_eq_levels(EqLevels::default(), cx));
                    }),
            )
            .child(
                div()
                    .id("eq-enabled")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(theme.text_subtle)
                    .child(t(cx, Key::EqEnabled))
                    .child(div().child(Toggle::new(enabled)))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        toggle_player.update(cx, |player, cx| player.set_eq_enabled(!enabled, cx));
                    }),
            );

        let presets = div().flex().flex_wrap().gap(px(6.0)).children(EqPreset::ALL.map(|preset| {
            let selected = enabled && active_preset == Some(preset);
            let player = self.player.clone();
            div()
                .id(("eq-preset", preset as usize))
                .h(px(24.0))
                .px(px(9.0))
                .flex()
                .items_center()
                .rounded(px(12.0))
                .border_1()
                .border_color(if selected { theme.accent } else { theme.button_border })
                .bg(if selected { theme.button_active_bg } else { theme.button_bg })
                .cursor_pointer()
                .hover(|s| s.bg(theme.button_hover_bg))
                .text_size(px(11.0))
                .text_color(if selected { theme.text_primary } else { theme.text_secondary })
                .child(t(cx, preset_name(preset)))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    player.update(cx, |player, cx| {
                        player.set_eq_levels(preset.levels(), cx);
                        player.set_eq_enabled(true, cx);
                    });
                })
        }));

        div()
            .id("eq-panel")
            .w(px(330.0))
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .rounded(px(18.0))
            .bg(theme.surface_panel_bg)
            .border_1()
            .border_color(theme.surface_panel_border)
            .occlude()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap(px(44.0))
                    .child(band(Band::Bass, Key::Bass))
                    .child(band(Band::Mid, Key::Mid))
                    .child(band(Band::Treble, Key::Treble)),
            )
            .child(presets)
            .when(active_preset.is_none(), |el| {
                el.child(div().text_size(px(10.5)).text_color(theme.text_muted).child(t(cx, Key::EqCustom)))
            })
    }
}
