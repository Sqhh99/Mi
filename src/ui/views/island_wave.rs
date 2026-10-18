//! The five-bar "bouncing peak" indicator shown in the dynamic island.
//!
//! It only bounces while the island is on screen and "reduce motion" is off.

use std::time::Duration;

use gpui::{Context, Entity, IntoElement, ParentElement, Render, Styled, Task, Window, div, px};
use player_core::easing;

use crate::model::{PlayerModel, SettingsModel};
use crate::ui::anim::Tween;
use crate::ui::theme::Theme;

const BAR_COUNT: usize = 5;
const BAR_WIDTH: f32 = 4.0;
const GAP: f32 = 2.0;
const IDLE: f32 = 4.0;
const MID: f32 = 8.0;
const PEAK: f32 = 13.0;
const STEP: Duration = Duration::from_millis(160);
const MORPH: Duration = Duration::from_millis(180);
pub const BUBBLE_SIZE: (f32, f32) = (44.0, 24.0);

pub struct IslandWave {
    playing: bool,
    /// The island is the current mode.
    shown: bool,
    window_visible: bool,
    reduce_motion: bool,
    active: usize,
    forward: bool,
    heights: [Tween; BAR_COUNT],
    _pulse: Option<Task<()>>,
}

impl IslandWave {
    pub fn new(
        player: &Entity<PlayerModel>,
        settings: &Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(player, |this, player, cx| {
            let playing = player.read(cx).is_playing();
            if playing != this.playing {
                this.playing = playing;
                this.update_pulse(cx);
            }
        })
        .detach();
        cx.observe(settings, |this, settings, cx| {
            let reduce_motion = settings.read(cx).get().ui.reduce_motion;
            if reduce_motion != this.reduce_motion {
                this.reduce_motion = reduce_motion;
                this.update_pulse(cx);
            }
        })
        .detach();
        Self {
            playing: false,
            shown: false,
            window_visible: true,
            reduce_motion: settings.read(cx).get().ui.reduce_motion,
            active: 0,
            forward: true,
            heights: [Tween::settled(IDLE); BAR_COUNT],
            _pulse: None,
        }
    }

    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if shown != self.shown {
            self.shown = shown;
            self.update_pulse(cx);
        }
    }

    pub fn set_window_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible != self.window_visible {
            self.window_visible = visible;
            self.update_pulse(cx);
        }
    }

    fn bouncing(&self) -> bool {
        self.playing && self.shown && self.window_visible && !self.reduce_motion
    }

    fn update_pulse(&mut self, cx: &mut Context<Self>) {
        let bouncing = self.bouncing();
        self.active = 0;
        self.forward = true;
        self.retarget();
        self._pulse = bouncing.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(STEP).await;
                    if this
                        .update(cx, |wave, cx| {
                            wave.advance();
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }

    fn advance(&mut self) {
        if self.active == BAR_COUNT - 1 {
            self.forward = false;
        } else if self.active == 0 {
            self.forward = true;
        }
        self.active = if self.forward { self.active + 1 } else { self.active - 1 };
        self.retarget();
    }

    fn retarget(&mut self) {
        let bouncing = self.bouncing();
        // Without motion a playing island shows a still "peak" so its state stays readable.
        let still = self.playing && !bouncing;
        for (i, tween) in self.heights.iter_mut().enumerate() {
            let height =
                match (bouncing || still, i.abs_diff(if still { BAR_COUNT / 2 } else { self.active })) {
                    (false, _) => IDLE,
                    (true, 0) => PEAK,
                    (true, 1) => MID,
                    _ => IDLE,
                };
            tween.animate_to(height, MORPH, easing::in_out_quad);
        }
    }
}

impl Render for IslandWave {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        if self.heights.iter().any(Tween::is_running) {
            window.request_animation_frame();
        }
        let glow_opacity = if self.playing { 0.9 } else { 0.35 };
        div()
            .w(px(BUBBLE_SIZE.0))
            .h(px(BUBBLE_SIZE.1))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(GAP))
            .children(self.heights.iter().map(|tween| {
                let height = tween.value();
                div()
                    .relative()
                    .w(px(BAR_WIDTH))
                    .h(px(BUBBLE_SIZE.1))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        // Soft glow behind the bar, 1.45× its size.
                        div()
                            .absolute()
                            .w(px(BAR_WIDTH * 1.45))
                            .h(px(height * 1.45))
                            .rounded_full()
                            .bg(theme.island_wave_glow.opacity(glow_opacity)),
                    )
                    .child(div().w(px(BAR_WIDTH)).h(px(height)).rounded_full().bg(theme.island_wave))
            }))
    }
}
