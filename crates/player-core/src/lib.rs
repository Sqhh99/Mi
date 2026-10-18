//! Platform-independent core of Mi: queue logic, lyrics, EQ math, settings and
//! strings. Nothing here depends on the UI toolkit or the audio backend.

pub mod easing;
pub mod eq;
pub mod geometry;
pub mod i18n;
pub mod library;
pub mod lyrics;
pub mod media_paths;
pub mod queue;
pub mod settings;
pub mod time;
pub mod track;
