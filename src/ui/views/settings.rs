//! Settings overlay: interface (theme, language, materials, decorations), playback
//! (resume, gapless, ReplayGain, output device) and a list of keyboard shortcuts.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowControlArea, div, prelude::FluentBuilder,
    px, svg,
};
use player_core::i18n::{Key, Lang};
use player_core::settings::{MAX_PREAMP_DB, Settings, ThemeMode};
use player_core::track::ReplayGainMode;

use crate::audio;
use crate::model::SettingsModel;
use crate::ui::icons::Icon;
use crate::ui::t;
use crate::ui::theme::Theme;
use crate::ui::widgets::{IconButton, Slider, Toggle};

const SIDEBAR_WIDTH: f32 = 190.0;
const HEADER_HEIGHT: f32 = 56.0;
const SECTION_INSET: f32 = 24.0;

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Section {
    #[default]
    Interface,
    Playback,
    Shortcuts,
}

/// Output devices, listed in the background the first time the playback page opens.
#[derive(Default)]
struct DeviceList {
    devices: Option<Vec<String>>,
    loading: bool,
}

#[derive(IntoElement)]
pub struct SettingsPanel {
    settings: Entity<SettingsModel>,
    /// The window's corner radius, so the sidebar background follows its left corners.
    radius: f32,
    on_close: CloseHandler,
}

impl SettingsPanel {
    pub fn new(
        settings: Entity<SettingsModel>,
        radius: f32,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self { settings, radius, on_close: Rc::new(on_close) }
    }
}

fn update(settings: &Entity<SettingsModel>, cx: &mut App, change: impl FnOnce(&mut Settings)) {
    settings.update(cx, |model, cx| model.update(cx, change));
}

fn section_title(text: &'static str, theme: &Theme) -> impl IntoElement {
    div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text_primary).child(text)
}

fn hint(text: &'static str, theme: &Theme) -> impl IntoElement {
    div().text_size(px(12.0)).text_color(theme.text_muted).child(text)
}

/// A selectable card with a title and a hint line.
fn choice_card(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    hint: Option<&'static str>,
    selected: bool,
    theme: &Theme,
    on_select: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(190.0))
        .h(px(if hint.is_some() { 104.0 } else { 56.0 }))
        .px(px(16.0))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(18.0))
        .border_1()
        .border_color(if selected { theme.accent } else { theme.surface_panel_border })
        .bg(if selected { theme.button_active_bg } else { theme.surface_panel_bg })
        .cursor_pointer()
        .on_click(move |_, _, cx| on_select(cx))
        .child(
            div()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(title.into()),
        )
        .children(hint.map(|hint| div().text_size(px(12.0)).text_color(theme.text_subtle).child(hint)))
}

fn strength_slider(
    id: &'static str,
    label: &'static str,
    value: u8,
    theme: &Theme,
    on_change: impl Fn(u8, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().text_size(px(14.0)).text_color(theme.text_secondary).child(label))
        .child(
            Slider::new(id, value as f32 / 100.0)
                .fill(theme.accent)
                .handle_size(18.0)
                .handle_border()
                .always_show_handle()
                .live()
                .on_change(move |fraction, _, cx| on_change((fraction * 100.0).round() as u8, cx)),
        )
        .child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_size(px(12.0))
                .text_color(theme.text_muted)
                .child(format!("{value}%")),
        )
}

/// A clickable row with a title, a hint and an on/off switch.
fn toggle_row(
    id: &'static str,
    title: &'static str,
    hint: &'static str,
    on: bool,
    theme: &Theme,
    on_toggle: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .min_h(px(62.0))
        .py(px(10.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .rounded(px(16.0))
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .hover(|s| s.bg(theme.button_hover_bg).border_color(theme.button_border))
        .active(|s| s.bg(theme.button_pressed_bg))
        .on_click(move |_, _, cx| on_toggle(cx))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(div().text_size(px(14.0)).text_color(theme.text_secondary).child(title))
                .child(div().text_size(px(12.0)).text_color(theme.text_muted).child(hint)),
        )
        .child(Toggle::new(on))
}

fn interface_section(settings: &Entity<SettingsModel>, theme: &Theme, cx: &App) -> Vec<AnyElement> {
    let ui = settings.read(cx).get().ui.clone();
    let themes = [
        ("theme-day", ThemeMode::Day, Key::ThemeDay, Key::ThemeDayHint),
        ("theme-night", ThemeMode::Night, Key::ThemeNight, Key::ThemeNightHint),
    ];
    let theme_section =
        div().flex().flex_col().gap(px(14.0)).child(section_title(t(cx, Key::Theme), theme)).child(
            div().flex().gap(px(14.0)).children(themes.map(|(id, mode, title, hint)| {
                let settings = settings.clone();
                choice_card(id, t(cx, title), Some(t(cx, hint)), ui.theme == mode, theme, move |cx| {
                    update(&settings, cx, |s| s.ui.theme = mode)
                })
            })),
        );

    let language_section = div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(section_title(t(cx, Key::Language), theme))
        .child(hint(t(cx, Key::LanguageHint), theme))
        .child(div().flex().gap(px(14.0)).children(Lang::ALL.map(|lang| {
            let settings = settings.clone();
            let id = match lang {
                Lang::Zh => "lang-zh",
                Lang::En => "lang-en",
            };
            choice_card(id, lang.native_name(), None, ui.language == lang, theme, move |cx| {
                update(&settings, cx, |s| s.ui.language = lang)
            })
        })));

    let toggle = |id, title, hint_key, on: bool, change: fn(&mut Settings)| {
        let settings = settings.clone();
        toggle_row(id, t(cx, title), t(cx, hint_key), on, theme, move |cx| update(&settings, cx, change))
    };
    let material_section = div()
        .flex()
        .flex_col()
        .gap(px(18.0))
        .child(section_title(t(cx, Key::Material), theme))
        .child(strength_slider("material", t(cx, Key::SurfaceMaterial), ui.material_strength, theme, {
            let settings = settings.clone();
            move |value, cx| update(&settings, cx, |s| s.ui.material_strength = value)
        }))
        .child(strength_slider(
            "button-material",
            t(cx, Key::ButtonMaterial),
            ui.button_material_strength,
            theme,
            {
                let settings = settings.clone();
                move |value, cx| update(&settings, cx, |s| s.ui.button_material_strength = value)
            },
        ))
        .child(toggle(
            "background-glow",
            Key::BackgroundGlow,
            Key::BackgroundGlowHint,
            ui.background_glow,
            |s| s.ui.background_glow = !s.ui.background_glow,
        ))
        .child(toggle("show-kitten", Key::ShowKitten, Key::ShowKittenHint, ui.show_kitten, |s| {
            s.ui.show_kitten = !s.ui.show_kitten
        }))
        .child(toggle("reduce-motion", Key::ReduceMotion, Key::ReduceMotionHint, ui.reduce_motion, |s| {
            s.ui.reduce_motion = !s.ui.reduce_motion
        }));

    vec![
        theme_section.into_any_element(),
        language_section.into_any_element(),
        material_section.into_any_element(),
    ]
}

fn playback_section(
    settings: &Entity<SettingsModel>,
    devices: &Entity<DeviceList>,
    theme: &Theme,
    cx: &mut App,
) -> Vec<AnyElement> {
    let playback = settings.read(cx).get().playback.clone();
    let toggle = |id, title, hint_key, on: bool, change: fn(&mut Settings)| {
        let settings = settings.clone();
        toggle_row(id, t(cx, title), t(cx, hint_key), on, theme, move |cx| update(&settings, cx, change))
    };
    let general = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(toggle(
            "resume-playing",
            Key::ResumePlaying,
            Key::ResumePlayingHint,
            playback.resume_playing,
            |s| s.playback.resume_playing = !s.playback.resume_playing,
        ))
        .child(toggle("gapless", Key::Gapless, Key::GaplessHint, playback.gapless, |s| {
            s.playback.gapless = !s.playback.gapless
        }));

    let modes = [
        ("rg-off", ReplayGainMode::Off, Key::ReplayGainOff),
        ("rg-track", ReplayGainMode::Track, Key::ReplayGainTrack),
        ("rg-album", ReplayGainMode::Album, Key::ReplayGainAlbum),
    ];
    let preamp = playback.replay_gain_preamp;
    let preamp_settings = settings.clone();
    let replay_gain = div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(section_title(t(cx, Key::ReplayGain), theme))
        .child(hint(t(cx, Key::ReplayGainHint), theme))
        .child(div().flex().gap(px(14.0)).children(modes.map(|(id, mode, title)| {
            let settings = settings.clone();
            choice_card(id, t(cx, title), None, playback.replay_gain == mode, theme, move |cx| {
                update(&settings, cx, |s| s.playback.replay_gain = mode)
            })
        })))
        .when(playback.replay_gain != ReplayGainMode::Off, |el| {
            let range = MAX_PREAMP_DB as f32 * 2.0;
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(theme.text_secondary)
                            .child(t(cx, Key::ReplayGainPreamp)),
                    )
                    .child(
                        Slider::new("rg-preamp", (preamp as f32 + MAX_PREAMP_DB as f32) / range)
                            .fill(theme.accent)
                            .handle_size(18.0)
                            .handle_border()
                            .always_show_handle()
                            .on_change(move |fraction, _, cx| {
                                let db = (fraction * range - MAX_PREAMP_DB as f32).round() as i8;
                                update(&preamp_settings, cx, |s| s.playback.replay_gain_preamp = db)
                            }),
                    )
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(if preamp > 0 { format!("+{preamp} dB") } else { format!("{preamp} dB") }),
                    ),
            )
        });

    // Device names come from a background query the first time they are needed.
    let list = devices.read(cx);
    if list.devices.is_none() && !list.loading {
        refresh_devices(devices, cx);
    }
    let names = devices.read(cx).devices.clone().unwrap_or_default();
    let chosen = playback.output_device.clone();
    let mut rows: Vec<(Option<String>, SharedString)> = vec![(None, t(cx, Key::SystemDefault).into())];
    rows.extend(names.into_iter().map(|name| (Some(name.clone()), name.into())));
    if let Some(missing) =
        chosen.clone().filter(|name| !rows.iter().any(|(row, _)| row.as_ref() == Some(name)))
    {
        rows.push((Some(missing.clone()), missing.into()));
    }
    let refresh = devices.clone();
    let output = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(section_title(t(cx, Key::OutputDevice), theme))
                .child(
                    IconButton::new("devices-refresh", Icon::RefreshCw)
                        .size(26.0, 13.0)
                        .hover_bg(theme.button_hover_bg)
                        .on_click(move |_, _, cx| refresh_devices(&refresh, cx)),
                ),
        )
        .child(hint(t(cx, Key::OutputDeviceHint), theme))
        .children(rows.into_iter().enumerate().map(|(index, (device, label))| {
            let selected = device == chosen;
            let settings = settings.clone();
            div()
                .id(("output-device", index))
                .h(px(42.0))
                .px(px(14.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(if selected { theme.accent } else { theme.surface_panel_border })
                .bg(if selected { theme.button_active_bg } else { theme.surface_panel_bg })
                .cursor_pointer()
                .hover(|s| s.bg(theme.button_hover_bg))
                .on_click(move |_, _, cx| {
                    let device = device.clone();
                    update(&settings, cx, |s| s.playback.output_device = device)
                })
                .child(svg().path(Icon::Speaker.path()).size(px(15.0)).text_color(theme.icon.opacity(0.7)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.0))
                        .text_color(theme.text_secondary)
                        .child(label),
                )
                .when(selected, |el| {
                    el.child(svg().path(Icon::Check.path()).size(px(14.0)).text_color(theme.accent))
                })
        }));

    vec![general.into_any_element(), replay_gain.into_any_element(), output.into_any_element()]
}

fn refresh_devices(devices: &Entity<DeviceList>, cx: &mut App) {
    devices.update(cx, |list, _| list.loading = true);
    let devices = devices.downgrade();
    cx.spawn(async move |cx| {
        let names = cx.background_executor().spawn(async { audio::output_devices() }).await;
        cx.update(|cx| {
            devices
                .update(cx, |list, cx| {
                    list.devices = Some(names);
                    list.loading = false;
                    cx.notify();
                })
                .ok();
            cx.refresh_windows();
        })
        .ok();
    })
    .detach();
}

fn shortcuts_section(theme: &Theme, cx: &App) -> Vec<AnyElement> {
    let secondary = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };
    let rows: Vec<(String, &'static str)> = vec![
        ("Space".into(), t(cx, Key::ShortcutPlayPause)),
        ("← / →".into(), t(cx, Key::ShortcutPrevNext)),
        ("Shift + ← / →".into(), t(cx, Key::ShortcutSeek)),
        ("↑ / ↓".into(), t(cx, Key::ShortcutVolume)),
        ("M".into(), t(cx, Key::Mute)),
        ("L".into(), t(cx, Key::ShortcutQueue)),
        ("B".into(), t(cx, Key::Library)),
        ("Y".into(), t(cx, Key::Lyrics)),
        ("E".into(), t(cx, Key::Equalizer)),
        ("R".into(), t(cx, Key::ShortcutCycleRepeat)),
        ("S".into(), t(cx, Key::Shuffle)),
        ("F".into(), t(cx, Key::Favorite)),
        (format!("{secondary} + F"), t(cx, Key::ShortcutSearch)),
        ("Delete".into(), t(cx, Key::ShortcutDelete)),
    ];
    let table = div().flex().flex_col().gap(px(4.0)).children(rows.into_iter().map(|(keys, action)| {
        div()
            .h(px(36.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .rounded(px(10.0))
            .hover(|s| s.bg(theme.button_hover_bg))
            .child(div().flex_1().text_size(px(13.0)).text_color(theme.text_secondary).child(action))
            .child(
                div()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(theme.button_bg)
                    .border_1()
                    .border_color(theme.button_border)
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(11.5))
                    .text_color(theme.text_primary)
                    .child(keys),
            )
    }));
    vec![
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(section_title(t(cx, Key::ShortcutsSection), theme))
            .child(table)
            .child(hint(t(cx, Key::ShortcutMediaKeys), theme))
            .into_any_element(),
    ]
}

impl RenderOnce for SettingsPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let section_state = window.use_keyed_state("settings-section", cx, |_, _| Section::default());
        let devices = window.use_keyed_state("settings-devices", cx, |_, _| DeviceList::default());
        let section = *section_state.read(cx);
        let settings = self.settings;

        let nav = |id: &'static str, target: Section, icon: Icon, label: &'static str| {
            let active = section == target;
            let state = section_state.clone();
            div()
                .id(id)
                .h(px(46.0))
                .px(px(14.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(px(14.0))
                .cursor_pointer()
                .map(|el| {
                    if active {
                        el.bg(theme.button_active_bg).border_1().border_color(theme.button_border)
                    } else {
                        el.hover(|s| s.bg(theme.button_hover_bg))
                    }
                })
                .on_click(move |_, _, cx| {
                    state.update(cx, |section, cx| {
                        *section = target;
                        cx.notify();
                    })
                })
                .child(svg().path(icon.path()).size(px(16.0)).text_color(theme.icon))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(theme.text_primary)
                        .child(label),
                )
        };

        let sidebar = div()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .rounded_l(px(self.radius))
            .bg(theme.surface_panel_bg)
            .child(
                div()
                    .mb(px(10.0))
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(t(cx, Key::Settings)),
            )
            .child(nav("section-interface", Section::Interface, Icon::Monitor, t(cx, Key::Interface)))
            .child(nav("section-playback", Section::Playback, Icon::Speaker, t(cx, Key::PlaybackSection)))
            .child(nav(
                "section-shortcuts",
                Section::Shortcuts,
                Icon::Keyboard,
                t(cx, Key::ShortcutsSection),
            ));

        let on_close = self.on_close.clone();
        let header = div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .px(px(18.0))
            .flex()
            .items_center()
            .justify_end()
            .window_control_area(WindowControlArea::Drag)
            .child(
                IconButton::new("settings-close", Icon::X)
                    .size(30.0, 12.0)
                    .hover_bg(theme.close_hover_bg)
                    .icon_opacity(0.6)
                    .on_click(move |_, window, cx| on_close(window, cx)),
            );

        let sections = match section {
            Section::Interface => interface_section(&settings, &theme, cx),
            Section::Playback => playback_section(&settings, &devices, &theme, cx),
            Section::Shortcuts => shortcuts_section(&theme, cx),
        };
        let content = div().id(("settings-content", section as usize)).flex_1().overflow_y_scroll().child(
            div()
                .p(px(SECTION_INSET))
                .pt(px(4.0))
                .flex()
                .flex_col()
                .gap(px(SECTION_INSET * 2.0))
                .children(sections),
        );

        div()
            .size_full()
            .flex()
            .occlude()
            .child(sidebar)
            .child(div().flex_1().h_full().flex().flex_col().child(header).child(content))
    }
}
