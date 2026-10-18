//! Decorative spectrum bars under the album art: random heights while playing, a
//! shimmering line while paused.
//!
//! The bars are decoration, not a real spectrum, so they only animate while they can be
//! seen (full mode, window shown) and "reduce motion" is off; otherwise they rest.

use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Task, Window, div, linear_color_stop,
    linear_gradient, px, relative,
};
use player_core::easing;

use crate::model::{PlayerModel, SettingsModel};
use crate::ui::anim::Tween;
use crate::ui::theme::Theme;

const BAR_COUNT: usize = 25;
const HEIGHT: f32 = 26.0;
const SHIMMER_PERIOD: Duration = Duration::from_millis(2600);

pub struct EqBars {
    playing: bool,
    /// The full layout (which holds the bars) is the current mode.
    shown: bool,
    window_visible: bool,
    reduce_motion: bool,
    bars: Vec<(Tween, f32)>,
    shimmer_start: Instant,
    rng: fastrand::Rng,
    _pulse: Option<Task<()>>,
}

impl EqBars {
    pub fn new(
        player: &Entity<PlayerModel>,
        settings: &Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut rng = fastrand::Rng::new();
        let bars =
            (0..BAR_COUNT).map(|_| (Tween::settled(8.0 + rng.f32() * 8.0), 0.7 + rng.f32() * 0.3)).collect();
        cx.observe(player, |this, player, cx| {
            let playing = player.read(cx).is_playing();
            if playing != this.playing {
                this.playing = playing;
                this.shimmer_start = Instant::now();
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
            shown: true,
            window_visible: true,
            reduce_motion: settings.read(cx).get().ui.reduce_motion,
            bars,
            shimmer_start: Instant::now(),
            rng,
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

    fn animating(&self) -> bool {
        self.shown && self.window_visible && !self.reduce_motion
    }

    fn update_pulse(&mut self, cx: &mut Context<Self>) {
        let pulse = self.playing && self.animating();
        if pulse == self._pulse.is_some() {
            cx.notify();
            return;
        }
        self._pulse = pulse.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    let Ok(delay) = this.update(cx, |bars, cx| {
                        bars.pulse();
                        cx.notify();
                        Duration::from_millis(80 + bars.rng.u64(0..70))
                    }) else {
                        break;
                    };
                    cx.background_executor().timer(delay).await;
                }
            })
        });
        if !pulse {
            // Settle on a calm, even shape instead of freezing mid-bounce.
            for (i, (tween, _)) in self.bars.iter_mut().enumerate() {
                let center = 1.0 - (i as f32 - BAR_COUNT as f32 / 2.0).abs() / BAR_COUNT as f32;
                tween.animate_to(6.0 + 6.0 * center, Duration::from_millis(240), easing::out_quad);
            }
        }
        cx.notify();
    }

    fn pulse(&mut self) {
        let half = BAR_COUNT as f32 / 2.0;
        for (i, (tween, _)) in self.bars.iter_mut().enumerate() {
            let base = 4.0 + self.rng.f32() * (HEIGHT - 4.0);
            // Bars near the middle tend to be taller.
            let center = 1.0 - (i as f32 - half).abs() / half * 0.3;
            let duration = Duration::from_millis(120 + self.rng.u64(0..60));
            tween.animate_to((base * center + 2.0).min(HEIGHT), duration, easing::out_quad);
        }
    }
}

impl Render for EqBars {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        if !self.playing {
            let shimmer = self.animating();
            if shimmer {
                window.request_animation_frame();
            }
            let t = (self.shimmer_start.elapsed().as_secs_f32() / SHIMMER_PERIOD.as_secs_f32()).fract();
            let highlight = theme.accent.opacity(0.35);
            // Slides a 40%-wide highlight from just off the left edge to the right edge.
            let left = -0.4 + 1.4 * easing::in_out_quad(t);
            let track =
                div().relative().w_full().h(px(3.0)).rounded_full().overflow_hidden().bg(theme.track_bg);
            let track = if shimmer {
                track.child(
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .w(relative(0.4))
                        .left(relative(left))
                        .flex()
                        .child(div().h_full().flex_1().bg(linear_gradient(
                            90.0,
                            linear_color_stop(highlight.opacity(0.0), 0.0),
                            linear_color_stop(highlight, 1.0),
                        )))
                        .child(div().h_full().flex_1().bg(linear_gradient(
                            90.0,
                            linear_color_stop(highlight, 0.0),
                            linear_color_stop(highlight.opacity(0.0), 1.0),
                        ))),
                )
            } else {
                track
            };
            return div().h(px(HEIGHT)).w_full().flex().items_center().child(track).into_any_element();
        }

        if self.bars.iter().any(|(tween, _)| tween.is_running()) {
            window.request_animation_frame();
        }
        let color = theme.text_primary;
        div()
            .h(px(HEIGHT))
            .w_full()
            .flex()
            .items_end()
            .gap(px(3.0))
            .children(self.bars.iter().map(|(tween, opacity)| {
                div().flex_1().h(px(tween.value())).rounded(px(1.5)).bg(color.opacity(*opacity))
            }))
            .into_any_element()
    }
}
