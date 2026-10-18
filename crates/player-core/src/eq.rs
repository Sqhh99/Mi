//! Three-band equalizer: user levels and the biquad filters that realise them.

use std::f32::consts::PI;

use serde::{Deserialize, Serialize};

/// Slider range is 0..=100; 50 is flat.
pub const LEVEL_FLAT: u8 = 50;
pub const LEVEL_MAX: u8 = 100;
/// Gain at either end of the slider range.
pub const MAX_GAIN_DB: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Band {
    Bass,
    Mid,
    Treble,
}

impl Band {
    pub const ALL: [Band; 3] = [Band::Bass, Band::Mid, Band::Treble];

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqLevels {
    pub bass: u8,
    pub mid: u8,
    pub treble: u8,
}

impl Default for EqLevels {
    fn default() -> Self {
        Self { bass: LEVEL_FLAT, mid: LEVEL_FLAT, treble: LEVEL_FLAT }
    }
}

impl EqLevels {
    pub fn get(&self, band: Band) -> u8 {
        match band {
            Band::Bass => self.bass,
            Band::Mid => self.mid,
            Band::Treble => self.treble,
        }
    }

    pub fn set(&mut self, band: Band, level: u8) {
        let level = level.min(LEVEL_MAX);
        match band {
            Band::Bass => self.bass = level,
            Band::Mid => self.mid = level,
            Band::Treble => self.treble = level,
        }
    }

    pub fn clamped(mut self) -> Self {
        for band in Band::ALL {
            self.set(band, self.get(band));
        }
        self
    }

    pub fn gains_db(&self) -> [f32; 3] {
        Band::ALL.map(|band| level_to_db(self.get(band)))
    }
}

/// Ready-made curves for the three bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EqPreset {
    Flat,
    BassBoost,
    Vocal,
    TrebleBoost,
    Rock,
    Pop,
    Soft,
    Electronic,
}

impl EqPreset {
    pub const ALL: [EqPreset; 8] = [
        EqPreset::Flat,
        EqPreset::BassBoost,
        EqPreset::Vocal,
        EqPreset::TrebleBoost,
        EqPreset::Rock,
        EqPreset::Pop,
        EqPreset::Soft,
        EqPreset::Electronic,
    ];

    pub fn levels(self) -> EqLevels {
        let (bass, mid, treble) = match self {
            EqPreset::Flat => (50, 50, 50),
            EqPreset::BassBoost => (75, 50, 50),
            EqPreset::Vocal => (42, 67, 54),
            EqPreset::TrebleBoost => (50, 50, 75),
            EqPreset::Rock => (67, 42, 65),
            EqPreset::Pop => (58, 62, 56),
            EqPreset::Soft => (54, 50, 38),
            EqPreset::Electronic => (70, 46, 62),
        };
        EqLevels { bass, mid, treble }
    }

    /// The preset these levels are exactly, if any.
    pub fn matching(levels: EqLevels) -> Option<EqPreset> {
        Self::ALL.into_iter().find(|preset| preset.levels() == levels)
    }
}

/// "+3.4 dB", "−6.0 dB" or "0 dB" for a slider level.
pub fn format_db(level: u8) -> String {
    let db = level_to_db(level);
    if db.abs() < 0.05 {
        "0 dB".to_string()
    } else if db > 0.0 {
        format!("+{db:.1} dB")
    } else {
        format!("\u{2212}{:.1} dB", -db)
    }
}

pub fn level_to_db(level: u8) -> f32 {
    (level.min(LEVEL_MAX) as f32 - LEVEL_FLAT as f32) / LEVEL_FLAT as f32 * MAX_GAIN_DB
}

/// Normalised biquad coefficients (a0 == 1), from the RBJ Audio EQ Cookbook.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Biquad {
    pub const IDENTITY: Biquad = Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// Low shelf with shelf slope S = 1.
    pub fn low_shelf(sample_rate: f32, freq: f32, gain_db: f32) -> Self {
        let (a, cos, alpha) = shelf_params(sample_rate, freq, gain_db);
        let sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) - (a - 1.0) * cos + sqrt_a_alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - sqrt_a_alpha),
            (a + 1.0) + (a - 1.0) * cos + sqrt_a_alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - sqrt_a_alpha,
        )
    }

    /// High shelf with shelf slope S = 1.
    pub fn high_shelf(sample_rate: f32, freq: f32, gain_db: f32) -> Self {
        let (a, cos, alpha) = shelf_params(sample_rate, freq, gain_db);
        let sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) + (a - 1.0) * cos + sqrt_a_alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - sqrt_a_alpha),
            (a + 1.0) - (a - 1.0) * cos + sqrt_a_alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - sqrt_a_alpha,
        )
    }

    pub fn peaking(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos = w0.cos();
        Self::normalized(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    fn normalized(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// Magnitude response at `freq`, for tests and diagnostics.
    pub fn magnitude_at(&self, sample_rate: f32, freq: f32) -> f32 {
        let w = 2.0 * PI * freq / sample_rate;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num =
            ((self.b0 + self.b1 * c1 + self.b2 * c2).powi(2) + (self.b1 * s1 + self.b2 * s2).powi(2)).sqrt();
        let den =
            ((1.0 + self.a1 * c1 + self.a2 * c2).powi(2) + (self.a1 * s1 + self.a2 * s2).powi(2)).sqrt();
        num / den
    }
}

fn shelf_params(sample_rate: f32, freq: f32, gain_db: f32) -> (f32, f32, f32) {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * freq / sample_rate;
    // alpha for S = 1: sin(w0)/2 * sqrt((A + 1/A)(1/S - 1) + 2) == sin(w0)/2 * sqrt(2)
    let alpha = w0.sin() / 2.0 * 2f32.sqrt();
    (a, w0.cos(), alpha)
}

/// Transposed direct form II state for one channel of one filter.
#[derive(Debug, Clone, Copy, Default)]
struct FilterState {
    z1: f32,
    z2: f32,
}

impl FilterState {
    #[inline]
    fn process(&mut self, f: &Biquad, x: f32) -> f32 {
        let y = f.b0 * x + self.z1;
        self.z1 = f.b1 * x - f.a1 * y + self.z2;
        self.z2 = f.b2 * x - f.a2 * y;
        y
    }
}

const BASS_HZ: f32 = 120.0;
const MID_HZ: f32 = 1000.0;
const MID_Q: f32 = 0.8;
const TREBLE_HZ: f32 = 8000.0;
/// Below this every band is treated as flat and samples pass through untouched.
const BYPASS_DB: f32 = 0.05;

/// Low-shelf + peaking + high-shelf filters for interleaved multi-channel audio.
#[derive(Debug, Clone)]
pub struct Equalizer {
    filters: [Biquad; 3],
    states: Vec<[FilterState; 3]>,
    preamp: f32,
    bypass: bool,
}

impl Equalizer {
    pub fn new(sample_rate: u32, channels: u16, gains_db: [f32; 3]) -> Self {
        let mut eq = Self {
            filters: [Biquad::IDENTITY; 3],
            states: vec![Default::default(); channels.max(1) as usize],
            preamp: 1.0,
            bypass: true,
        };
        eq.configure(sample_rate, gains_db);
        eq
    }

    /// Recomputes coefficients. Filter state is kept so live slider moves don't click.
    pub fn configure(&mut self, sample_rate: u32, gains_db: [f32; 3]) {
        let fs = sample_rate.max(1) as f32;
        let [bass, mid, treble] = gains_db;
        self.bypass = gains_db.iter().all(|g| g.abs() < BYPASS_DB);
        self.filters = [
            Biquad::low_shelf(fs, BASS_HZ, bass),
            Biquad::peaking(fs, MID_HZ.min(fs * 0.45), MID_Q, mid),
            Biquad::high_shelf(fs, TREBLE_HZ.min(fs * 0.45), treble),
        ];
        // Leave headroom for the largest boost so loud passages don't clip.
        let max_boost = gains_db.iter().copied().fold(0.0f32, f32::max);
        self.preamp = 10f32.powf(-max_boost / 20.0);
    }

    pub fn set_channels(&mut self, channels: u16) {
        self.states.resize(channels.max(1) as usize, Default::default());
    }

    /// Clears filter memory, e.g. after a seek.
    pub fn reset(&mut self) {
        self.states.iter_mut().for_each(|s| *s = Default::default());
    }

    #[inline]
    pub fn process(&mut self, channel: usize, sample: f32) -> f32 {
        if self.bypass {
            return sample;
        }
        let Some(states) = self.states.get_mut(channel) else {
            return sample;
        };
        let mut y = sample * self.preamp;
        for (state, filter) in states.iter_mut().zip(&self.filters) {
            y = state.process(filter, y);
        }
        y.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 44_100.0;

    fn db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    #[test]
    fn presets_and_labels() {
        assert_eq!(EqPreset::Flat.levels(), EqLevels::default());
        assert_eq!(EqPreset::matching(EqPreset::Rock.levels()), Some(EqPreset::Rock));
        assert_eq!(EqPreset::matching(EqLevels { bass: 1, mid: 2, treble: 3 }), None);
        assert!(EqPreset::ALL.iter().all(|p| p.levels().clamped() == p.levels()));
        assert_eq!(format_db(50), "0 dB");
        assert_eq!(format_db(75), "+6.0 dB");
        assert_eq!(format_db(0), "\u{2212}12.0 dB");
    }

    #[test]
    fn level_mapping() {
        assert_eq!(level_to_db(50), 0.0);
        assert_eq!(level_to_db(100), 12.0);
        assert_eq!(level_to_db(0), -12.0);
        assert_eq!(level_to_db(255), 12.0);
        let levels = EqLevels { bass: 200, mid: 0, treble: 75 }.clamped();
        assert_eq!(levels.bass, 100);
        assert_eq!(levels.gains_db(), [12.0, -12.0, 6.0]);
    }

    #[test]
    fn zero_gain_filters_are_flat() {
        for f in [
            Biquad::low_shelf(FS, 120.0, 0.0),
            Biquad::peaking(FS, 1000.0, 0.8, 0.0),
            Biquad::high_shelf(FS, 8000.0, 0.0),
        ] {
            for freq in [20.0, 120.0, 1000.0, 8000.0, 20000.0] {
                assert!((f.magnitude_at(FS, freq) - 1.0).abs() < 1e-3, "{f:?} at {freq}");
            }
        }
    }

    #[test]
    fn shelves_and_peak_hit_target_gain() {
        let low = Biquad::low_shelf(FS, 120.0, 6.0);
        assert!((db(low.magnitude_at(FS, 10.0)) - 6.0).abs() < 0.2);
        assert!(db(low.magnitude_at(FS, 15_000.0)).abs() < 0.2);

        let high = Biquad::high_shelf(FS, 8000.0, -6.0);
        assert!((db(high.magnitude_at(FS, 21_000.0)) + 6.0).abs() < 0.3);
        assert!(db(high.magnitude_at(FS, 50.0)).abs() < 0.2);

        let peak = Biquad::peaking(FS, 1000.0, 0.8, 9.0);
        assert!((db(peak.magnitude_at(FS, 1000.0)) - 9.0).abs() < 0.05);
    }

    #[test]
    fn filters_are_stable() {
        for gain in [-12.0, -6.0, 6.0, 12.0] {
            for f in [
                Biquad::low_shelf(FS, 120.0, gain),
                Biquad::peaking(FS, 1000.0, 0.8, gain),
                Biquad::high_shelf(FS, 8000.0, gain),
            ] {
                // Poles inside the unit circle: |a2| < 1 and |a1| < 1 + a2.
                assert!(f.a2.abs() < 1.0 && f.a1.abs() < 1.0 + f.a2, "{f:?}");
            }
        }
    }

    #[test]
    fn flat_equalizer_is_bit_exact() {
        let mut eq = Equalizer::new(44_100, 2, [0.0; 3]);
        for (i, x) in [0.5f32, -0.25, 0.125, 1.0].into_iter().enumerate() {
            assert_eq!(eq.process(i % 2, x), x);
        }
    }

    #[test]
    fn boosted_output_stays_in_range() {
        let mut eq = Equalizer::new(44_100, 1, [12.0, 12.0, 12.0]);
        for n in 0..10_000 {
            let x = (n as f32 * 0.05).sin();
            let y = eq.process(0, x);
            assert!((-1.0..=1.0).contains(&y));
        }
    }
}
