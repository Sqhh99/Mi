//! A [`Source`] that plays one track and, if the next one was handed over in time and has
//! the same format, carries straight on into it without a gap.
//!
//! It also keeps the position within the current track, which rodio's own position can't
//! provide once several tracks have played through one source.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::LoadId;

pub type BoxedSource = Box<dyn Source + Send>;

/// How many samples pass between updates of the shared position.
const PUBLISH_EVERY: u64 = 1024;

/// State shared between the audio thread's engine and the source inside the mixer.
pub struct Feed {
    /// The track to continue with when the current one ends.
    next: Mutex<Option<(LoadId, BoxedSource)>>,
    /// Id of the track the source is playing now.
    current_id: AtomicU64,
    /// Samples (not frames) played in the current track.
    samples: AtomicU64,
    /// Channels and sample rate of the current track, packed as `channels << 32 | rate`.
    format: AtomicU64,
}

impl Feed {
    fn new(id: LoadId, channels: ChannelCount, rate: SampleRate) -> Self {
        Self {
            next: Mutex::new(None),
            current_id: AtomicU64::new(id),
            samples: AtomicU64::new(0),
            format: AtomicU64::new(pack(channels, rate)),
        }
    }

    pub fn current_id(&self) -> LoadId {
        self.current_id.load(Ordering::Acquire)
    }

    /// Position within the current track.
    pub fn position(&self) -> Duration {
        let (channels, rate) = unpack(self.format.load(Ordering::Relaxed));
        let samples = self.samples.load(Ordering::Relaxed);
        let frames = samples / channels.max(1) as u64;
        Duration::from_secs_f64(frames as f64 / rate.max(1) as f64)
    }

    pub fn set_next(&self, next: Option<(LoadId, BoxedSource)>) {
        *self.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    #[cfg(test)]
    fn has_next(&self) -> bool {
        self.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some()
    }
}

fn pack(channels: ChannelCount, rate: SampleRate) -> u64 {
    ((channels.get() as u64) << 32) | rate.get() as u64
}

fn unpack(format: u64) -> (u16, u32) {
    ((format >> 32) as u16, format as u32)
}

pub struct GaplessSource {
    current: BoxedSource,
    feed: Arc<Feed>,
    channels: ChannelCount,
    rate: SampleRate,
    samples: u64,
}

impl GaplessSource {
    pub fn new(id: LoadId, first: BoxedSource) -> (Self, Arc<Feed>) {
        let (channels, rate) = (first.channels(), first.sample_rate());
        let feed = Arc::new(Feed::new(id, channels, rate));
        (Self { current: first, feed: feed.clone(), channels, rate, samples: 0 }, feed)
    }

    fn publish(&self) {
        self.feed.samples.store(self.samples, Ordering::Relaxed);
    }

    /// Switches to the queued track if it can continue seamlessly. A track in another format
    /// is dropped; the engine then reports the end and the player loads it the normal way.
    fn advance(&mut self) -> bool {
        let Some((id, next)) = self.feed.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
        else {
            return false;
        };
        if next.channels() != self.channels || next.sample_rate() != self.rate {
            return false;
        }
        self.current = next;
        self.samples = 0;
        self.publish();
        self.feed.current_id.store(id, Ordering::Release);
        true
    }
}

impl Iterator for GaplessSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        loop {
            if let Some(sample) = self.current.next() {
                self.samples += 1;
                if self.samples.is_multiple_of(PUBLISH_EVERY) {
                    self.publish();
                }
                return Some(sample);
            }
            self.publish();
            if !self.advance() {
                return None;
            }
        }
    }
}

impl Source for GaplessSource {
    fn current_span_len(&self) -> Option<usize> {
        self.current.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.current.try_seek(pos)?;
        let frames = (pos.as_secs_f64() * self.rate.get() as f64) as u64;
        self.samples = frames * self.channels.get() as u64;
        self.publish();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rodio::buffer::SamplesBuffer;

    use super::*;

    fn buffer(channels: u16, rate: u32, samples: usize, value: f32) -> BoxedSource {
        let channels = ChannelCount::new(channels).unwrap();
        let rate = SampleRate::new(rate).unwrap();
        Box::new(SamplesBuffer::new(channels, rate, vec![value; samples]))
    }

    #[test]
    fn continues_into_matching_track() {
        let (mut source, feed) = GaplessSource::new(1, buffer(2, 1000, 4, 0.1));
        feed.set_next(Some((2, buffer(2, 1000, 6, 0.2))));
        let samples: Vec<f32> = source.by_ref().collect();
        assert_eq!(samples.len(), 10);
        assert_eq!(samples[4], 0.2);
        assert_eq!(feed.current_id(), 2);
        assert!(!feed.has_next());
        // 6 samples of stereo at 1 kHz = 3 ms into the second track.
        assert_eq!(feed.position(), Duration::from_millis(3));
    }

    #[test]
    fn stops_before_track_in_another_format() {
        let (source, feed) = GaplessSource::new(1, buffer(2, 1000, 4, 0.1));
        feed.set_next(Some((2, buffer(2, 2000, 6, 0.2))));
        assert_eq!(source.count(), 4);
        assert_eq!(feed.current_id(), 1);
    }

    #[test]
    fn ends_without_next() {
        let (source, feed) = GaplessSource::new(7, buffer(1, 1000, 3, 0.5));
        assert_eq!(source.count(), 3);
        assert_eq!(feed.position(), Duration::from_millis(3));
    }
}
