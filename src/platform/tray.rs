//! System tray icon and menu.

use player_core::i18n::{Key, Lang, tr};
use player_core::queue::RepeatMode;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const ICON_PNG: &[u8] = include_bytes!("../../resources/appIcon.png");
const ICON_SIZE: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    ToggleWindow,
    TogglePlay,
    Previous,
    Next,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    CycleRepeat,
    ToggleShuffle,
    Quit,
}

/// Everything the tray displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayState {
    pub lang: Lang,
    pub window_visible: bool,
    pub title: Option<String>,
    pub playing: bool,
    pub paused: bool,
    pub muted: bool,
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

pub struct Tray {
    icon: TrayIcon,
    window: MenuItem,
    play: MenuItem,
    previous: MenuItem,
    next: MenuItem,
    volume_up: MenuItem,
    volume_down: MenuItem,
    mute: MenuItem,
    repeat: MenuItem,
    shuffle: CheckMenuItem,
    quit: MenuItem,
    state: Option<TrayState>,
}

impl Tray {
    /// Creates the tray icon. `on_action` may be called from any thread.
    pub fn new(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> anyhow::Result<Self> {
        let item = || MenuItem::new("", true, None);
        let check = || CheckMenuItem::new("", true, false, None);
        let (window, play, previous, next) = (item(), item(), item(), item());
        let (volume_up, volume_down, mute, quit) = (item(), item(), item(), item());
        let (repeat, shuffle) = (item(), check());

        let menu = Menu::new();
        menu.append_items(&[
            &window,
            &PredefinedMenuItem::separator(),
            &play,
            &previous,
            &next,
            &PredefinedMenuItem::separator(),
            &volume_up,
            &volume_down,
            &mute,
            &PredefinedMenuItem::separator(),
            &repeat,
            &shuffle,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        let actions = [
            (window.id().clone(), TrayAction::ToggleWindow),
            (play.id().clone(), TrayAction::TogglePlay),
            (previous.id().clone(), TrayAction::Previous),
            (next.id().clone(), TrayAction::Next),
            (volume_up.id().clone(), TrayAction::VolumeUp),
            (volume_down.id().clone(), TrayAction::VolumeDown),
            (mute.id().clone(), TrayAction::ToggleMute),
            (repeat.id().clone(), TrayAction::CycleRepeat),
            (shuffle.id().clone(), TrayAction::ToggleShuffle),
            (quit.id().clone(), TrayAction::Quit),
        ];
        let on_action = std::sync::Arc::new(on_action);
        let on_menu = on_action.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, action)) = actions.iter().find(|(id, _)| *id == event.id) {
                on_menu(*action);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left, button_state: MouseButtonState::Up, ..
            }
            | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => {
                on_action(TrayAction::ShowWindow)
            }
            TrayIconEvent::Click {
                button: MouseButton::Middle, button_state: MouseButtonState::Up, ..
            } => on_action(TrayAction::TogglePlay),
            _ => {}
        }));

        let icon = TrayIconBuilder::new()
            .with_icon(load_icon()?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        Ok(Self {
            icon,
            window,
            play,
            previous,
            next,
            volume_up,
            volume_down,
            mute,
            repeat,
            shuffle,
            quit,
            state: None,
        })
    }

    pub fn update(&mut self, state: TrayState) {
        if self.state.as_ref() == Some(&state) {
            return;
        }
        let t = |key| tr(state.lang, key);
        self.window.set_text(t(if state.window_visible { Key::HideWindow } else { Key::ShowWindow }));
        self.play.set_text(t(if state.playing { Key::Pause } else { Key::Play }));
        self.previous.set_text(t(Key::Previous));
        self.next.set_text(t(Key::Next));
        self.volume_up.set_text(t(Key::VolumeUp));
        self.volume_down.set_text(t(Key::VolumeDown));
        self.mute.set_text(t(if state.muted { Key::Unmute } else { Key::Mute }));
        self.repeat.set_text(t(match state.repeat {
            RepeatMode::Off => Key::RepeatOff,
            RepeatMode::All => Key::RepeatAll,
            RepeatMode::One => Key::RepeatOne,
        }));
        self.shuffle.set_text(t(Key::Shuffle));
        self.shuffle.set_checked(state.shuffle);
        self.quit.set_text(t(Key::Quit));

        let tooltip = match &state.title {
            Some(title) if state.playing => format!("{title}{}", t(Key::StatusPlaying)),
            Some(title) if state.paused => format!("{title}{}", t(Key::StatusPaused)),
            Some(title) => title.clone(),
            None => t(Key::AppName).to_string(),
        };
        if let Err(err) = self.icon.set_tooltip(Some(tooltip)) {
            log::warn!("cannot update tray tooltip: {err}");
        }
        self.state = Some(state);
    }
}

fn load_icon() -> anyhow::Result<tray_icon::Icon> {
    let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)?
        .resize(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(tray_icon::Icon::from_rgba(image.into_raw(), width, height)?)
}
