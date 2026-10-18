//! Views composing the main window.

mod album_art;
mod controls;
mod eq_bars;
mod eq_panel;
mod full;
mod island;
mod island_wave;
mod library;
mod lyrics;
mod mini;
mod queue;
mod root;
mod settings;

pub use root::{MainView, MainViewEvent};

/// Shortcuts that only apply inside particular views.
pub fn bind_keys(cx: &mut gpui::App) {
    queue::bind_keys(cx);
    library::bind_keys(cx);
}
