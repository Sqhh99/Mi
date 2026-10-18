//! No-op window helpers for platforms without native support.

use gpui::Window;
use player_core::geometry::Rect;

#[derive(Debug, Clone, Copy)]
pub struct NativeWindow;

impl NativeWindow {
    pub fn from_window(_window: &Window) -> Option<Self> {
        None
    }

    pub const SUPPORTS_PLACEMENT: bool = false;

    pub fn raw_handle(self) -> Option<*mut std::ffi::c_void> {
        None
    }

    pub fn client_rect(self) -> Option<Rect> {
        None
    }

    pub fn monitor_rect(self) -> Option<Rect> {
        None
    }

    pub fn set_client_rect(self, _rect: Rect) {}

    pub fn set_topmost(self, _topmost: bool) {}

    pub fn show(self) {}

    pub fn hide(self) {}

    pub fn is_visible(self) -> bool {
        true
    }

    pub fn apply_chrome(self, _dark: bool) {}

    pub fn install_hooks(self) {}

    pub fn set_edge_snapping(_enabled: bool) {}

    /// Native dialogs are shown without an owner window on these platforms.
    pub fn dialog_parent(self) -> DialogParent {
        DialogParent
    }
}

pub struct DialogParent;

impl raw_window_handle::HasWindowHandle for DialogParent {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl raw_window_handle::HasDisplayHandle for DialogParent {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}
