//! The operating system's media controls: media keys, the Windows media flyout (SMTC),
//! macOS Now Playing and Linux MPRIS, through `souvlaki`.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Image, ImageFormat};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

/// Seek step for "seek forward/backward" buttons that don't say how far.
const SEEK_STEP: f32 = 10.0;

/// What the system asked the player to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaCommand {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// Relative seek in seconds.
    SeekBy(f32),
    SetPosition(Duration),
    Raise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

/// Everything the system controls display.
#[derive(Clone, PartialEq)]
pub struct NowPlaying {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Duration,
    pub cover: Option<Arc<Image>>,
    pub state: PlaybackState,
    pub position: Duration,
}

pub struct SystemMedia {
    controls: MediaControls,
    shown: Option<NowPlaying>,
    cover_file: Option<PathBuf>,
}

impl SystemMedia {
    /// `window` is the native window handle (required on Windows). `on_command` may be
    /// called from any thread.
    pub fn new(
        window: Option<*mut c_void>,
        on_command: impl Fn(MediaCommand) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let config = PlatformConfig { display_name: "Mi", dbus_name: "com.sqhh99.mi", hwnd: window };
        let mut controls = MediaControls::new(config).map_err(|err| anyhow::anyhow!("{err:?}"))?;
        controls
            .attach(move |event| {
                let command = match event {
                    MediaControlEvent::Play => MediaCommand::Play,
                    MediaControlEvent::Pause => MediaCommand::Pause,
                    MediaControlEvent::Toggle => MediaCommand::Toggle,
                    MediaControlEvent::Next => MediaCommand::Next,
                    MediaControlEvent::Previous => MediaCommand::Previous,
                    MediaControlEvent::Stop => MediaCommand::Stop,
                    MediaControlEvent::Seek(direction) => MediaCommand::SeekBy(signed(direction, SEEK_STEP)),
                    MediaControlEvent::SeekBy(direction, by) => {
                        MediaCommand::SeekBy(signed(direction, by.as_secs_f32()))
                    }
                    MediaControlEvent::SetPosition(MediaPosition(position)) => {
                        MediaCommand::SetPosition(position)
                    }
                    MediaControlEvent::Raise => MediaCommand::Raise,
                    _ => return,
                };
                on_command(command);
            })
            .map_err(|err| anyhow::anyhow!("{err:?}"))?;
        Ok(Self { controls, shown: None, cover_file: None })
    }

    /// Pushes what changed since the last update to the system.
    pub fn update(&mut self, now: NowPlaying) {
        let previous = self.shown.take();
        let metadata_changed = previous.as_ref().is_none_or(|shown| {
            shown.title != now.title
                || shown.artist != now.artist
                || shown.album != now.album
                || shown.duration != now.duration
                || shown.cover.as_ref().map(|c| c.id()) != now.cover.as_ref().map(|c| c.id())
        });
        if metadata_changed {
            let cover_url = now.cover.as_ref().and_then(|cover| self.write_cover(cover));
            let metadata = MediaMetadata {
                title: Some(now.title.as_deref().unwrap_or("Mi")),
                artist: Some(now.artist.as_deref().unwrap_or_default()),
                album: Some(now.album.as_deref().unwrap_or_default()),
                cover_url: cover_url.as_deref(),
                duration: (!now.duration.is_zero()).then_some(now.duration),
            };
            if let Err(err) = self.controls.set_metadata(metadata) {
                log::debug!("media controls: cannot set metadata: {err:?}");
            }
        }
        let playback_changed = metadata_changed
            || previous
                .as_ref()
                .is_none_or(|shown| shown.state != now.state || shown.position != now.position);
        if playback_changed {
            let progress = Some(MediaPosition(now.position));
            let playback = match now.state {
                PlaybackState::Stopped => MediaPlayback::Stopped,
                PlaybackState::Playing => MediaPlayback::Playing { progress },
                PlaybackState::Paused => MediaPlayback::Paused { progress },
            };
            if let Err(err) = self.controls.set_playback(playback) {
                log::debug!("media controls: cannot set playback: {err:?}");
            }
        }
        self.shown = Some(now);
    }

    /// The system loads artwork from a file, so the current cover is written to a temp file.
    fn write_cover(&mut self, cover: &Image) -> Option<String> {
        let extension = match cover.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Bmp => "bmp",
            _ => return None,
        };
        let dir = std::env::temp_dir().join("Mi");
        std::fs::create_dir_all(&dir).ok()?;
        // A new name per cover: some systems cache artwork by path.
        let path = dir.join(format!("now-playing-{}.{extension}", cover.id()));
        if let Some(old) = self.cover_file.take().filter(|old| *old != path) {
            let _ = std::fs::remove_file(old);
        }
        if !path.exists() {
            std::fs::write(&path, &cover.bytes).ok()?;
        }
        self.cover_file = Some(path.clone());
        Some(file_url(&path))
    }
}

impl Drop for SystemMedia {
    fn drop(&mut self) {
        if let Some(file) = self.cover_file.take() {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn signed(direction: SeekDirection, seconds: f32) -> f32 {
    match direction {
        SeekDirection::Forward => seconds,
        SeekDirection::Backward => -seconds,
    }
}

/// souvlaki strips a literal `file://` on Windows and passes the rest to the file API;
/// elsewhere a proper file URL (`file:///…`) is expected.
fn file_url(path: &std::path::Path) -> String {
    if cfg!(windows) {
        format!("file://{}", path.display())
    } else {
        let encoded: String = path
            .to_string_lossy()
            .chars()
            .map(|ch| match ch {
                ' ' => "%20".to_string(),
                '#' => "%23".to_string(),
                '%' => "%25".to_string(),
                '?' => "%3F".to_string(),
                ch => ch.to_string(),
            })
            .collect();
        format!("file://{encoded}")
    }
}
