//! Application wiring: models, the main window, tray, system media controls,
//! single-instance activation and settings persistence.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, Application, Bounds, Entity, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions, point, px, size,
};
use player_core::media_paths;
use player_core::settings::{SavedWindowMode, WindowPlacement};

use crate::actions;
use crate::assets::Assets;
use crate::model::{LibraryModel, PlayerEvent, PlayerModel, SettingsModel, Status};
use crate::platform::media_controls::{MediaCommand, NowPlaying, PlaybackState, SystemMedia};
use crate::platform::single_instance::{self, Instance, Request};
use crate::platform::tray::{Tray, TrayAction, TrayState};
use crate::ui::Locale;
use crate::ui::theme::{Theme, metrics};
use crate::ui::views::{MainView, MainViewEvent};

const SESSION_SAVE_INTERVAL: Duration = Duration::from_secs(10);
/// How often the system media controls get a fresh position while playing.
const MEDIA_POSITION_INTERVAL: Duration = Duration::from_secs(5);

/// Requests that arrive from outside the window (tray menu, media keys, a second launch).
#[derive(Debug, Clone)]
enum AppCommand {
    Tray(TrayAction),
    Media(MediaCommand),
    Activate,
    Open(Vec<PathBuf>),
}

/// Audio files named on the command line ("Open with Mi" passes them this way).
fn files_from_args() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.is_dir() || media_paths::is_audio_file(path))
        .map(|path| std::fs::canonicalize(&path).map(strip_verbatim).unwrap_or(path))
        .collect()
}

/// `canonicalize` on Windows returns `\\?\C:\...`; keep the familiar form.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(plain) if !plain.starts_with("UNC") => PathBuf::from(plain),
        _ => path,
    }
}

pub fn run() {
    let startup_files = files_from_args();
    let request =
        if startup_files.is_empty() { Request::Activate } else { Request::Open(startup_files.clone()) };
    let listener = match single_instance::acquire(&request) {
        Instance::Primary(listener) => listener,
        Instance::Secondary => {
            log::info!("already running; handed the request to the existing window");
            return;
        }
    };

    let (commands, command_rx) = async_channel::unbounded();
    let app = Application::new().with_assets(Assets);
    // macOS delivers "Open with" files as URLs instead of arguments.
    app.on_open_urls({
        let commands = commands.clone();
        move |urls| {
            let paths: Vec<PathBuf> = urls
                .iter()
                .filter_map(|url| url.strip_prefix("file://"))
                .map(|path| PathBuf::from(percent_decode(path)))
                .collect();
            if !paths.is_empty() {
                let _ = commands.send_blocking(AppCommand::Open(paths));
            }
        }
    });
    app.run(move |cx: &mut App| {
        if let Err(err) = init(listener, startup_files, commands, command_rx, cx) {
            log::error!("failed to start: {err:#}");
            cx.quit();
        }
    });
}

/// Decodes `%20`-style escapes in a file URL path.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn init(
    listener: Option<interprocess::local_socket::Listener>,
    startup_files: Vec<PathBuf>,
    commands: async_channel::Sender<AppCommand>,
    command_rx: async_channel::Receiver<AppCommand>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let settings = cx.new(|_| SettingsModel::load());
    let initial = settings.read(cx).get().clone();
    apply_ui_settings(&settings, cx);
    // Most settings writes are bookkeeping (the session, recently played); only rebuild the
    // theme when the interface settings themselves change.
    let mut applied_ui = initial.ui.clone();
    cx.observe(&settings, move |settings, cx| {
        let ui = &settings.read(cx).get().ui;
        if *ui != applied_ui {
            applied_ui = ui.clone();
            apply_ui_settings(&settings, cx);
            cx.refresh_windows();
        }
    })
    .detach();

    let player = cx.new(|cx| PlayerModel::new(&initial, cx));
    player.update(cx, |player, cx| {
        let session = &initial.session;
        let autoplay = initial.playback.resume_playing && session.was_playing && startup_files.is_empty();
        let position = Duration::from_millis(session.position_ms);
        player.restore(initial.playlist.clone(), session.current, position, autoplay, cx)
    });
    persist_player_state(&player, &settings, cx);
    follow_output_settings(&player, &settings, cx);
    let library = cx.new(|cx| LibraryModel::new(settings.clone(), cx));

    actions::bind_keys(cx);
    let window = open_main_window(player.clone(), settings.clone(), library, initial.session.window, cx)?;
    if !startup_files.is_empty() {
        window.update(cx, |view, _, cx| view.open_paths(startup_files, cx))?;
    }

    let tray = match Tray::new({
        let commands = commands.clone();
        move |action| {
            let _ = commands.send_blocking(AppCommand::Tray(action));
        }
    }) {
        Ok(tray) => Some(Rc::new(RefCell::new(tray))),
        Err(err) => {
            log::warn!("system tray unavailable: {err:#}");
            None
        }
    };
    if let Some(listener) = listener {
        let commands = commands.clone();
        single_instance::listen(listener, move |request| {
            let command = match request {
                Request::Activate => AppCommand::Activate,
                Request::Open(paths) => AppCommand::Open(paths),
            };
            let _ = commands.send_blocking(command);
        });
    }
    let native = window.read(cx).ok().and_then(MainView::native);
    match SystemMedia::new(native.and_then(|native| native.raw_handle()), {
        let commands = commands.clone();
        move |command| {
            let _ = commands.send_blocking(AppCommand::Media(command));
        }
    }) {
        Ok(media) => sync_media_controls(media, &player, cx),
        Err(err) => log::warn!("system media controls unavailable: {err:#}"),
    }

    cx.spawn({
        let player = player.clone();
        async move |cx| {
            while let Ok(command) = command_rx.recv().await {
                if cx.update(|cx| handle_command(command, window, &player, cx)).is_err() {
                    break;
                }
            }
        }
    })
    .detach();

    if let Some(tray) = tray {
        sync_tray(tray, window, &player, &settings, cx)?;
    }

    cx.on_app_quit({
        let player = player.clone();
        let settings = settings.clone();
        move |cx| {
            save_session(&player, &settings, cx);
            settings.update(cx, |settings, _| settings.save_now());
            player.read(cx).shutdown();
            async {}
        }
    })
    .detach();

    cx.activate(true);
    Ok(())
}

fn apply_ui_settings(settings: &Entity<SettingsModel>, cx: &mut App) {
    let ui = &settings.read(cx).get().ui;
    let (theme, lang) = (Theme::new(ui), ui.language);
    cx.set_global(theme);
    cx.set_global(Locale(lang));
}

/// Mirrors the queue, playback preferences, lyrics adjustments and the listening session
/// into the settings file.
fn persist_player_state(player: &Entity<PlayerModel>, settings: &Entity<SettingsModel>, cx: &mut App) {
    cx.subscribe(player, {
        let settings = settings.clone();
        move |player, event, cx| match event {
            PlayerEvent::Playlist => {
                let playlist = player.read(cx).tracks().to_vec();
                settings.update(cx, |settings, cx| settings.update(cx, |s| s.playlist = playlist));
                save_session(&player, &settings, cx);
            }
            PlayerEvent::Preferences => {
                let player = player.read(cx);
                let (volume, muted, eq, eq_enabled) =
                    (player.volume(), player.muted(), player.eq(), player.eq_enabled());
                let (repeat, shuffle) = (player.repeat(), player.shuffle());
                settings.update(cx, |settings, cx| {
                    settings.update(cx, |s| {
                        s.volume = volume;
                        s.muted = muted;
                        s.eq = eq;
                        s.playback.eq_enabled = eq_enabled;
                        s.playback.repeat = repeat;
                        s.playback.shuffle = shuffle;
                    })
                });
            }
            PlayerEvent::Lyrics => {
                let player = player.read(cx);
                let (links, offsets) = (player.lyrics_links().clone(), player.lyrics_offsets().clone());
                settings.update(cx, |settings, cx| {
                    settings.update(cx, |s| {
                        s.lyrics.links = links;
                        s.lyrics.offsets = offsets;
                    })
                });
            }
            PlayerEvent::TrackStarted(path) => {
                let path = path.clone();
                settings.update(cx, |settings, cx| settings.update(cx, |s| s.record_played(&path)));
            }
            PlayerEvent::State => save_session(&player, &settings, cx),
            PlayerEvent::Problem(_) => {}
        }
    })
    .detach();

    // While playing, the position only changes in the player; save it now and then so a
    // crash or power loss doesn't lose more than a few seconds.
    cx.spawn({
        let (player, settings) = (player.downgrade(), settings.downgrade());
        async move |cx| {
            loop {
                cx.background_executor().timer(SESSION_SAVE_INTERVAL).await;
                let saved = cx.update(|cx| {
                    let (Some(player), Some(settings)) = (player.upgrade(), settings.upgrade()) else {
                        return false;
                    };
                    if player.read(cx).is_playing() {
                        save_session(&player, &settings, cx);
                    }
                    true
                });
                if !matches!(saved, Ok(true)) {
                    break;
                }
            }
        }
    })
    .detach();
}

/// Records the current track, position and play state for the next launch.
fn save_session(player: &Entity<PlayerModel>, settings: &Entity<SettingsModel>, cx: &mut App) {
    let player = player.read(cx);
    let current = player.current_index();
    let position_ms = if current.is_some() { player.position().as_millis() as u64 } else { 0 };
    let was_playing = player.is_playing();
    settings.update(cx, |settings, cx| {
        settings.update(cx, |s| {
            s.session.current = current;
            s.session.position_ms = position_ms;
            s.session.was_playing = was_playing;
        })
    });
}

/// Pushes ReplayGain, gapless and output-device changes to the player.
fn follow_output_settings(player: &Entity<PlayerModel>, settings: &Entity<SettingsModel>, cx: &mut App) {
    let player = player.clone();
    let mut last = settings.read(cx).get().clone();
    cx.observe(settings, move |settings, cx| {
        let current = settings.read(cx).get().clone();
        if current.playback == last.playback {
            last = current;
            return;
        }
        let reload = PlayerModel::replay_gain_changed(&last, &current)
            || last.playback.output_device != current.playback.output_device;
        player.update(cx, |player, cx| {
            player.apply_output_settings(&current);
            if reload {
                player.reload_output(cx);
            }
        });
        last = current;
    })
    .detach();
}

/// Where the window opens: last time's position if it is still on a screen, else centered.
fn initial_bounds(placement: Option<WindowPlacement>, cx: &App) -> Bounds<gpui::Pixels> {
    let mode = placement.map_or(SavedWindowMode::Full, |placement| placement.mode);
    let (width, height) = match mode {
        SavedWindowMode::Full => metrics::FULL_SIZE,
        SavedWindowMode::Mini => metrics::MINI_SIZE,
        SavedWindowMode::Island => metrics::ISLAND_SIZE,
    };
    let window_size = size(px(width), px(height));
    if let Some(placement) = placement.filter(|placement| placement.mode != SavedWindowMode::Island) {
        let origin = point(px(placement.x as f32), px(placement.y as f32));
        // Keep at least the top-left corner (where the title area is) reachable.
        let grip = Bounds::new(origin, size(px(80.0), px(40.0)));
        if cx.displays().iter().any(|display| display.bounds().intersects(&grip)) {
            return Bounds::new(origin, window_size);
        }
    }
    Bounds::centered(None, window_size, cx)
}

fn open_main_window(
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    library: Entity<LibraryModel>,
    placement: Option<WindowPlacement>,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<MainView>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(initial_bounds(placement, cx))),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: false,
        is_minimizable: true,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some("com.sqhh99.Mi".into()),
        ..Default::default()
    };
    let dark = cx.global::<Theme>().dark;
    let window = cx.open_window(options, |window, cx| {
        window.set_window_title("Mi");
        cx.new(|cx| MainView::new(player, settings.clone(), library, placement, window, cx))
    })?;

    window.update(cx, |view, window, cx| {
        // Closing the last window would quit the app; hide to the tray instead.
        let view_handle = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            view_handle.update(cx, |view, cx| view.hide_window(window, cx)).ok();
            false
        });
        if let Some(native) = view.native() {
            native.defer(cx, move |native| {
                native.apply_chrome(dark);
                native.install_hooks();
            });
            // Keep the DWM dark-mode hint in sync with the theme.
            cx.observe_global::<Theme>(move |_, cx| {
                let dark = cx.global::<Theme>().dark;
                native.defer(cx, move |native| native.apply_chrome(dark));
            })
            .detach();
        }
        view.apply_initial_placement(window, cx);
    })?;
    Ok(window)
}

fn handle_command(
    command: AppCommand,
    window: WindowHandle<MainView>,
    player: &Entity<PlayerModel>,
    cx: &mut App,
) {
    let player_command = |cx: &mut App, f: fn(&mut PlayerModel, &mut gpui::Context<PlayerModel>)| {
        player.update(cx, f);
    };
    match command {
        AppCommand::Activate
        | AppCommand::Tray(TrayAction::ShowWindow)
        | AppCommand::Media(MediaCommand::Raise) => {
            window.update(cx, |view, window, cx| view.show_window(window, cx)).ok();
        }
        AppCommand::Open(paths) => {
            window
                .update(cx, |view, window, cx| {
                    view.show_window(window, cx);
                    view.open_paths(paths, cx);
                })
                .ok();
        }
        AppCommand::Media(command) => player.update(cx, |player, cx| match command {
            MediaCommand::Play => player.play(cx),
            MediaCommand::Pause | MediaCommand::Stop => player.pause(),
            MediaCommand::Toggle => player.toggle_play(cx),
            MediaCommand::Next => player.next(cx),
            MediaCommand::Previous => player.previous(cx),
            MediaCommand::SeekBy(seconds) => player.seek_by(seconds, cx),
            MediaCommand::SetPosition(position) => player.seek(position, cx),
            MediaCommand::Raise => {}
        }),
        AppCommand::Tray(TrayAction::ToggleWindow) => {
            window.update(cx, |view, window, cx| view.toggle_window(window, cx)).ok();
        }
        AppCommand::Tray(TrayAction::TogglePlay) => player_command(cx, PlayerModel::toggle_play),
        AppCommand::Tray(TrayAction::Previous) => player_command(cx, PlayerModel::previous),
        AppCommand::Tray(TrayAction::Next) => player_command(cx, PlayerModel::next),
        AppCommand::Tray(TrayAction::VolumeUp) => player.update(cx, |p, cx| p.adjust_volume(10, cx)),
        AppCommand::Tray(TrayAction::VolumeDown) => player.update(cx, |p, cx| p.adjust_volume(-10, cx)),
        AppCommand::Tray(TrayAction::ToggleMute) => player_command(cx, PlayerModel::toggle_mute),
        AppCommand::Tray(TrayAction::CycleRepeat) => player_command(cx, PlayerModel::cycle_repeat),
        AppCommand::Tray(TrayAction::ToggleShuffle) => player_command(cx, PlayerModel::toggle_shuffle),
        AppCommand::Tray(TrayAction::Quit) => cx.quit(),
    }
}

type PlayerRefresh = dyn Fn(&Entity<PlayerModel>, &mut App);

/// Keeps the system media controls (media keys, flyout, Now Playing) in step with the player.
fn sync_media_controls(media: SystemMedia, player: &Entity<PlayerModel>, cx: &mut App) {
    // Owned by the refresh closure, which the subscriptions below keep alive with the app.
    let media = RefCell::new(media);
    let refresh: Rc<PlayerRefresh> = Rc::new({
        move |player: &Entity<PlayerModel>, cx: &mut App| {
            let player = player.read(cx);
            let track = player.track();
            let state = match player.status() {
                Status::Playing => PlaybackState::Playing,
                Status::Paused => PlaybackState::Paused,
                Status::Stopped if track.is_some() => PlaybackState::Paused,
                Status::Stopped => PlaybackState::Stopped,
            };
            media.borrow_mut().update(NowPlaying {
                title: track.map(|track| track.title.to_string()),
                artist: track.and_then(|track| track.artist.as_ref().map(ToString::to_string)),
                album: track.and_then(|track| track.album.as_ref().map(ToString::to_string)),
                duration: player.duration(),
                cover: track.and_then(|track| track.cover.clone()),
                state,
                // Whole seconds: the system interpolates while playing.
                position: Duration::from_secs(player.position().as_secs()),
            });
        }
    });
    refresh(player, cx);
    cx.subscribe(player, {
        let refresh = refresh.clone();
        move |player, event, cx| {
            if matches!(event, PlayerEvent::State | PlayerEvent::TrackStarted(_)) {
                refresh(&player, cx);
            }
        }
    })
    .detach();
    cx.spawn({
        let player = player.downgrade();
        async move |cx| {
            loop {
                cx.background_executor().timer(MEDIA_POSITION_INTERVAL).await;
                let alive = cx.update(|cx| {
                    let Some(player) = player.upgrade() else { return false };
                    if player.read(cx).is_playing() {
                        refresh(&player, cx);
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }
    })
    .detach();
}

/// Keeps tray labels, check marks and tooltip in step with the app.
fn sync_tray(
    tray: Rc<RefCell<Tray>>,
    window: WindowHandle<MainView>,
    player: &Entity<PlayerModel>,
    settings: &Entity<SettingsModel>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let refresh: Rc<dyn Fn(&mut App)> = Rc::new({
        let player = player.clone();
        let settings = settings.clone();
        move |cx: &mut App| {
            let window_visible = window.read(cx).map(MainView::is_window_visible).unwrap_or(true);
            let lang = settings.read(cx).get().ui.language;
            let player = player.read(cx);
            tray.borrow_mut().update(TrayState {
                lang,
                window_visible,
                title: player.track().map(|track| track.title.to_string()),
                playing: player.status() == Status::Playing,
                paused: player.status() == Status::Paused,
                muted: player.muted(),
                repeat: player.repeat(),
                shuffle: player.shuffle(),
            });
        }
    });

    refresh(cx);
    cx.subscribe(player, {
        let refresh = refresh.clone();
        move |_, event, cx| {
            if matches!(event, PlayerEvent::State | PlayerEvent::Preferences) {
                refresh(cx);
            }
        }
    })
    .detach();
    cx.observe(settings, {
        let refresh = refresh.clone();
        move |_, cx| refresh(cx)
    })
    .detach();
    let view = window.entity(cx)?;
    cx.subscribe(&view, move |_, event: &MainViewEvent, cx| match event {
        MainViewEvent::VisibilityChanged => refresh(cx),
    })
    .detach();
    Ok(())
}
