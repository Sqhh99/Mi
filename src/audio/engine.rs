//! The audio thread: owns the output device and the per-track rodio player.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError};
use player_core::track::ReplayGainMode;
use rodio::cpal::traits::HostTrait;
use rodio::cpal::{self, StreamError};
use rodio::{Decoder, DeviceSinkBuilder, DeviceTrait, MixerDeviceSink, Player, Source};

use super::eq_source::{EqParams, EqSource};
use super::gapless::{BoxedSource, Feed, GaplessSource};
use super::{AudioCommand, AudioEvent, LoadError, LoadId};
use crate::media;

const TICK: Duration = Duration::from_millis(50);
/// Position updates are sent every N ticks while playing.
const POSITION_EVERY_TICKS: u32 = 3;

pub(super) fn spawn(
    commands: Receiver<AudioCommand>,
    events: async_channel::Sender<AudioEvent>,
    eq: Arc<EqParams>,
) {
    std::thread::Builder::new()
        .name("audio-engine".into())
        .spawn(move || Engine::new(events, eq).run(commands))
        .expect("failed to spawn audio thread");
}

pub(super) fn output_device_names() -> Vec<String> {
    match cpal::default_host().output_devices() {
        Ok(devices) => devices.filter_map(|device| device_name(&device)).collect(),
        Err(err) => {
            log::warn!("cannot list output devices: {err}");
            Vec::new()
        }
    }
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|description| description.name().to_string())
}

struct Track {
    id: LoadId,
    player: Player,
    feed: Arc<Feed>,
    ended: bool,
    /// The preloaded follower and its duration, until it takes over.
    preload: Option<(LoadId, Option<Duration>)>,
}

struct Engine {
    events: async_channel::Sender<AudioEvent>,
    eq: Arc<EqParams>,
    sink: Option<MixerDeviceSink>,
    /// Set from the device's error callback when the stream died.
    sink_lost: Arc<AtomicBool>,
    device: Option<String>,
    track: Option<Track>,
    volume: f32,
    replay_gain: (ReplayGainMode, f32),
}

impl Engine {
    fn new(events: async_channel::Sender<AudioEvent>, eq: Arc<EqParams>) -> Self {
        Self {
            events,
            eq,
            sink: None,
            sink_lost: Arc::new(AtomicBool::new(false)),
            device: None,
            track: None,
            volume: 0.8,
            replay_gain: (ReplayGainMode::Off, 0.0),
        }
    }

    fn run(mut self, commands: Receiver<AudioCommand>) {
        let mut ticks = 0u32;
        loop {
            match commands.recv_timeout(TICK) {
                Ok(AudioCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            ticks = ticks.wrapping_add(1);
            self.poll(ticks.is_multiple_of(POSITION_EVERY_TICKS));
        }
        self.track = None;
    }

    fn emit(&self, event: AudioEvent) {
        let _ = self.events.send_blocking(event);
    }

    /// Opens the output device on first use so startup never blocks on audio drivers.
    fn sink(&mut self) -> Result<&MixerDeviceSink, LoadError> {
        if self.sink.is_none() {
            let mut sink = self.open_sink().map_err(|err| {
                log::error!("cannot open audio output: {err}");
                LoadError::Device(err)
            })?;
            sink.log_on_drop(false);
            self.sink_lost.store(false, Ordering::Release);
            self.sink = Some(sink);
        }
        Ok(self.sink.as_ref().expect("sink was just opened"))
    }

    fn open_sink(&self) -> Result<MixerDeviceSink, String> {
        let lost = self.sink_lost.clone();
        let on_error = move |err: StreamError| match err {
            StreamError::DeviceNotAvailable | StreamError::StreamInvalidated => {
                log::warn!("audio output lost: {err}");
                lost.store(true, Ordering::Release);
            }
            other => log::debug!("audio stream: {other}"),
        };
        let chosen = self.device.as_deref().and_then(|name| {
            let found = cpal::default_host()
                .output_devices()
                .ok()?
                .find(|device| device_name(device).as_deref() == Some(name));
            if found.is_none() {
                log::warn!("output device {name:?} not found; using the default");
            }
            found
        });
        let builder = match chosen {
            Some(device) => DeviceSinkBuilder::from_device(device),
            None => DeviceSinkBuilder::from_default_device(),
        }
        .map_err(|err| err.to_string())?;
        builder.with_error_callback(on_error).open_sink_or_fallback().map_err(|err| err.to_string())
    }

    fn handle(&mut self, command: AudioCommand) {
        match command {
            AudioCommand::Load { path, id, autoplay, start } => {
                // Drop the previous player first so its device stream stops immediately.
                self.track = None;
                match self.load(&path, id, autoplay, start) {
                    Ok((track, duration)) => {
                        self.track = Some(track);
                        self.emit(AudioEvent::Loaded { id, duration });
                        self.emit(AudioEvent::Playing { id, playing: autoplay });
                    }
                    Err(error) => {
                        log::warn!("cannot play {}: {error:?}", path.display());
                        self.emit(AudioEvent::LoadFailed { id, error });
                    }
                }
            }
            AudioCommand::Preload { path, id } => {
                let Some(track) = &self.track else { return };
                match self.decode(&path) {
                    Ok((source, duration)) => {
                        track.feed.set_next(Some((id, source)));
                        if let Some(track) = &mut self.track {
                            track.preload = Some((id, duration));
                        }
                    }
                    Err(error) => {
                        log::warn!("cannot preload {}: {error:?}", path.display());
                        self.emit(AudioEvent::PreloadFailed { id, error });
                    }
                }
            }
            AudioCommand::CancelPreload => {
                if let Some(track) = &mut self.track {
                    track.feed.set_next(None);
                    track.preload = None;
                }
            }
            AudioCommand::Play => self.set_playing(true),
            AudioCommand::Pause => self.set_playing(false),
            AudioCommand::Stop => {
                if let Some(track) = self.track.take() {
                    self.emit(AudioEvent::Playing { id: track.id, playing: false });
                }
            }
            AudioCommand::Seek(position) => {
                if let Some(track) = &mut self.track {
                    if let Err(err) = track.player.try_seek(position) {
                        log::warn!("seek failed: {err}");
                    }
                    track.ended = false;
                    let event = AudioEvent::Position { id: track.id, position: track.feed.position() };
                    self.emit(event);
                }
            }
            AudioCommand::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                if let Some(track) = &self.track {
                    track.player.set_volume(self.volume);
                }
            }
            AudioCommand::SetReplayGain { mode, preamp_db } => self.replay_gain = (mode, preamp_db),
            AudioCommand::SetDevice(device) => {
                if device != self.device {
                    self.device = device;
                    self.track = None;
                    self.sink = None;
                }
            }
            AudioCommand::Shutdown => {}
        }
    }

    /// Opens and decodes a track, with its ReplayGain adjustment applied.
    fn decode(&self, path: &Path) -> Result<(BoxedSource, Option<Duration>), LoadError> {
        let file = File::open(path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => LoadError::Missing,
            _ => LoadError::Unreadable(err.to_string()),
        })?;
        let decoder = Decoder::try_from(file).map_err(|err| LoadError::Unreadable(err.to_string()))?;
        let duration = decoder.total_duration();
        let (mode, preamp_db) = self.replay_gain;
        let gain = match mode {
            ReplayGainMode::Off => 1.0,
            _ => media::read_tags(path).map_or(1.0, |tags| tags.replay_gain.factor(mode, preamp_db)),
        };
        let source: BoxedSource =
            if gain == 1.0 { Box::new(decoder) } else { Box::new(decoder.amplify(gain)) };
        Ok((source, duration))
    }

    fn load(
        &mut self,
        path: &Path,
        id: LoadId,
        autoplay: bool,
        start: Duration,
    ) -> Result<(Track, Option<Duration>), LoadError> {
        let (source, duration) = self.decode(path)?;
        let (source, feed) = GaplessSource::new(id, source);
        let source = EqSource::new(source, self.eq.clone());
        let volume = self.volume;
        let sink = self.sink()?;
        let player = Player::connect_new(sink.mixer());
        player.set_volume(volume);
        if !autoplay {
            player.pause();
        }
        player.append(source);
        if !start.is_zero()
            && let Err(err) = player.try_seek(start)
        {
            log::warn!("cannot resume {} at {start:?}: {err}", path.display());
        }
        Ok((Track { id, player, feed, ended: false, preload: None }, duration))
    }

    fn set_playing(&mut self, playing: bool) {
        // A finished player has no source left; the model reloads the track instead.
        let Some(track) = self.track.as_ref().filter(|track| !track.ended) else { return };
        if playing {
            track.player.play();
        } else {
            track.player.pause();
        }
        let id = track.id;
        self.emit(AudioEvent::Playing { id, playing });
    }

    fn poll(&mut self, report_position: bool) {
        if self.sink.is_some() && self.sink_lost.load(Ordering::Acquire) {
            self.track = None;
            self.sink = None;
            self.emit(AudioEvent::DeviceLost("output device disconnected".into()));
            return;
        }
        let Some(track) = &mut self.track else { return };
        if track.ended || track.player.is_paused() {
            return;
        }
        let playing_id = track.feed.current_id();
        if playing_id != track.id {
            let duration = track.preload.take().filter(|(id, _)| *id == playing_id).and_then(|(_, d)| d);
            let from = std::mem::replace(&mut track.id, playing_id);
            self.emit(AudioEvent::Advanced { from, id: playing_id, duration });
            return;
        }
        if track.player.empty() {
            track.ended = true;
            track.preload = None;
            let id = track.id;
            self.emit(AudioEvent::TrackEnded { id });
        } else if report_position {
            let event = AudioEvent::Position { id: track.id, position: track.feed.position() };
            self.emit(event);
        }
    }
}
