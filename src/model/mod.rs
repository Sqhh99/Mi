//! Application state shared between views, tray and persistence.

pub mod library;
pub mod player;
pub mod settings;

pub use library::{LibraryEvent, LibraryModel};
pub use player::{PlayerEvent, PlayerModel, Problem, Status, TrackIssue};
pub use settings::SettingsModel;
