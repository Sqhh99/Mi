//! OS integration behind a small, platform-neutral facade.
//!
//! GPUI has no API for window position, always-on-top or hide/show, so those go through
//! [`NativeWindow`]. On Windows it talks to Win32 directly; elsewhere it is a no-op and
//! callers fall back to what GPUI offers.
//!
//! Win32 calls that resize or show a window send messages synchronously to GPUI's window
//! procedure, which then needs the app state. They must therefore never run while the app
//! is borrowed (inside an `update` or event handler); use [`NativeWindow::defer`].

pub mod media_controls;
pub mod single_instance;
pub mod tray;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::NativeWindow;

#[cfg(not(windows))]
mod fallback;
#[cfg(not(windows))]
pub use fallback::NativeWindow;

impl NativeWindow {
    /// Runs `f` on the main thread after the current update has finished.
    pub fn defer(self, cx: &gpui::App, f: impl FnOnce(NativeWindow) + 'static) {
        cx.spawn(async move |_| f(self)).detach();
    }
}
