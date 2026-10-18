//! Colors and metrics, ported from the original `Theme.qml`.
//!
//! Most surface colors depend on the day/night mode and on the two "material strength"
//! settings, so the theme is rebuilt whenever those settings change.

use gpui::{Global, Hsla, Rgba, SharedString};
use player_core::settings::{ThemeMode, UiSettings};

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Hsla {
    Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a.clamp(0.0, 1.0) }.into()
}

fn hex(rgb: u32) -> Hsla {
    rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 1.0)
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,

    pub font_family: SharedString,
    pub mono_font_family: SharedString,
    pub lyrics_font_family: SharedString,

    // Window surface
    pub surface_bg: Hsla,
    pub surface_border: Hsla,
    pub surface_overlay_bg: Hsla,
    pub surface_panel_bg: Hsla,
    pub surface_panel_border: Hsla,
    pub surface_highlight: Hsla,
    pub surface_bottom_tint: Hsla,
    pub surface_glow_blue: Hsla,
    pub surface_glow_rose: Hsla,
    /// The corner kitten: white at night, black by day, kept faint.
    pub kitten: Hsla,

    // Dynamic island
    pub island_bg: Hsla,
    pub island_highlight: Hsla,
    pub island_bottom_tint: Hsla,
    pub island_wave: Hsla,
    pub island_wave_glow: Hsla,

    // Buttons
    pub button_bg: Hsla,
    pub button_hover_bg: Hsla,
    pub button_pressed_bg: Hsla,
    pub button_border: Hsla,
    pub button_active_bg: Hsla,
    pub playlist_row_hover_bg: Hsla,
    pub playlist_row_active_bg: Hsla,
    pub close_hover_bg: Hsla,
    pub hover_bg: Hsla,

    // Album art placeholder
    pub album_placeholder_border: Hsla,
    pub album_placeholder_start: Hsla,
    pub album_placeholder_end: Hsla,

    // Text and icons
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_subtle: Hsla,
    pub text_muted: Hsla,
    pub icon: Hsla,
    pub icon_active: Hsla,
    pub icon_opacity: f32,
    pub icon_muted_opacity: f32,
    pub icon_soft_opacity: f32,
    pub icon_strong_opacity: f32,

    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_dark: Hsla,

    // Sliders
    pub track_bg: Hsla,
    pub track_fill: Hsla,
    pub handle: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn new(ui: &UiSettings) -> Self {
        let dark = ui.theme == ThemeMode::Night;
        let m = ui.material_strength.min(100) as f32 / 100.0;
        let b = ui.button_material_strength.min(100) as f32 / 100.0;
        let glow = ui.background_glow;
        let pick = |night: Hsla, day: Hsla| if dark { night } else { day };
        let transparent = rgba(0, 0, 0, 0.0);

        Self {
            dark,
            font_family: if cfg!(windows) { "Microsoft YaHei UI" } else { ".SystemUIFont" }.into(),
            mono_font_family: if cfg!(windows) { "Consolas" } else { "Menlo" }.into(),
            lyrics_font_family: if cfg!(windows) { "Microsoft YaHei UI" } else { ".SystemUIFont" }.into(),

            surface_bg: pick(rgba(12, 12, 14, 0.78 + 0.14 * m), rgba(248, 248, 249, 0.84 + 0.10 * m)),
            surface_border: pick(rgba(255, 255, 255, 0.10 + 0.10 * m), rgba(226, 228, 233, 0.50 + 0.24 * m)),
            surface_overlay_bg: pick(rgba(16, 16, 18, 0.88 + 0.08 * m), rgba(252, 252, 253, 0.90 + 0.06 * m)),
            surface_panel_bg: pick(rgba(24, 24, 27, 0.84 + 0.08 * m), rgba(253, 253, 253, 0.82 + 0.10 * m)),
            surface_panel_border: pick(
                rgba(255, 255, 255, 0.10 + 0.08 * m),
                rgba(229, 231, 235, 0.62 + 0.12 * m),
            ),
            surface_highlight: pick(
                rgba(255, 255, 255, 0.04 + 0.04 * m),
                rgba(255, 255, 255, 0.38 + 0.14 * m),
            ),
            surface_bottom_tint: pick(
                rgba(255, 255, 255, 0.02 + 0.02 * m),
                rgba(203, 213, 225, 0.04 + 0.06 * m),
            ),
            surface_glow_blue: if glow {
                pick(rgba(59, 130, 246, 0.08), rgba(191, 219, 254, 0.15))
            } else {
                transparent
            },
            surface_glow_rose: if glow {
                pick(rgba(255, 255, 255, 0.03), rgba(233, 213, 255, 0.10))
            } else {
                transparent
            },

            kitten: pick(rgba(255, 255, 255, 0.11), rgba(0, 0, 0, 0.075)),

            island_bg: pick(rgba(12, 12, 14, 0.80 + 0.12 * m), rgba(248, 248, 249, 0.88 + 0.08 * m)),
            island_highlight: pick(
                rgba(255, 255, 255, 0.025 + 0.02 * m),
                rgba(255, 255, 255, 0.18 + 0.08 * m),
            ),
            island_bottom_tint: pick(
                rgba(255, 255, 255, 0.01 + 0.01 * m),
                rgba(203, 213, 225, 0.03 + 0.03 * m),
            ),
            island_wave: hex(0x2ef2c5),
            island_wave_glow: pick(rgba(46, 242, 197, 0.42), rgba(46, 242, 197, 0.28)),

            button_bg: pick(rgba(255, 255, 255, 0.10 + 0.08 * b), rgba(255, 255, 255, 0.22 + 0.20 * b)),
            button_hover_bg: pick(rgba(255, 255, 255, 0.14 + 0.10 * b), rgba(255, 255, 255, 0.34 + 0.22 * b)),
            button_pressed_bg: pick(
                rgba(255, 255, 255, 0.18 + 0.10 * b),
                rgba(226, 232, 240, 0.46 + 0.22 * b),
            ),
            button_border: pick(rgba(255, 255, 255, 0.12 + 0.10 * b), rgba(226, 228, 233, 0.28 + 0.18 * b)),
            button_active_bg: pick(rgba(90, 150, 255, 0.20 + 0.12 * b), rgba(239, 246, 255, 0.58 + 0.18 * b)),
            playlist_row_hover_bg: pick(
                rgba(255, 255, 255, 0.06 + 0.04 * m),
                rgba(15, 23, 42, 0.03 + 0.02 * m),
            ),
            playlist_row_active_bg: pick(
                rgba(90, 150, 255, 0.16 + 0.08 * m),
                rgba(219, 234, 254, 0.48 + 0.10 * m),
            ),
            close_hover_bg: pick(rgba(239, 68, 68, 0.24), hex(0xffe4e6)),
            hover_bg: pick(rgba(255, 255, 255, 0.08), rgba(0, 0, 0, 0.04)),

            album_placeholder_border: pick(rgba(255, 255, 255, 0.10), hex(0xe5e7eb)),
            album_placeholder_start: pick(rgba(97, 97, 105, 0.62), hex(0xd1d5db)),
            album_placeholder_end: pick(rgba(32, 32, 36, 0.96), hex(0xf3f4f6)),

            text_primary: pick(hex(0xf5f5f5), hex(0x111827)),
            text_secondary: pick(hex(0xe5e7eb), hex(0x1f2937)),
            text_subtle: pick(hex(0xc9cdd3), hex(0x6b7280)),
            text_muted: pick(hex(0xaeb4be), hex(0x9ca3af)),
            icon: pick(hex(0xf3f4f6), hex(0x1f2937)),
            icon_active: pick(hex(0xffffff), hex(0x1d4ed8)),
            icon_opacity: if dark { 0.90 } else { 0.88 },
            icon_muted_opacity: if dark { 0.82 } else { 0.74 },
            icon_soft_opacity: if dark { 0.88 } else { 0.82 },
            icon_strong_opacity: if dark { 1.00 } else { 0.96 },

            accent: pick(hex(0x7ab2ff), hex(0x3b82f6)),
            accent_soft: pick(rgba(122, 178, 255, 0.16), hex(0xeff6ff)),
            accent_dark: pick(hex(0xdbeafe), hex(0x1d4ed8)),

            track_bg: pick(rgba(255, 255, 255, 0.12), rgba(15, 23, 42, 0.10)),
            track_fill: pick(hex(0xf3f4f6), hex(0x1f2937)),
            handle: pick(hex(0xffffff), hex(0x1f2937)),
        }
    }
}

/// Fixed sizes from the original layout, in logical pixels.
pub mod metrics {
    pub const FULL_SIZE: (f32, f32) = (980.0, 712.0);
    pub const MINI_SIZE: (f32, f32) = (558.0, 180.0);
    pub const ISLAND_SIZE: (f32, f32) = (160.0, 46.0);
    pub const ISLAND_TOP_MARGIN: f32 = 8.0;

    pub const RADIUS_FULL: f32 = 8.0;
    pub const RADIUS_MINI: f32 = 8.0;
    pub const RADIUS_ISLAND: f32 = ISLAND_SIZE.1 / 2.0;

    pub const OUTER_PADDING: f32 = 40.0;
    pub const OUTER_PADDING_MINI: f32 = 14.0;
    pub const COLUMN_SPACING: f32 = 32.0;
    pub const ALBUM_COLUMN_WIDTH: f32 = 396.0;
    pub const ALBUM_SIZE: f32 = 300.0;
    pub const ALBUM_SIZE_MINI: f32 = 128.0;
    pub const ALBUM_RADIUS: f32 = 28.0;
    pub const ALBUM_RADIUS_MINI: f32 = 18.0;
    pub const ISLAND_COVER_SIZE: f32 = 26.0;

    pub const TITLE_SIZE: f32 = 34.0;
    pub const ARTIST_SIZE: f32 = 17.0;
    pub const TITLE_SIZE_MINI: f32 = 18.0;
    pub const ARTIST_SIZE_MINI: f32 = 12.0;
    /// Space kept free on the right of titles for the window controls.
    pub const HEADER_INSET: f32 = 64.0;

    pub const CONTROL_SIZE_MD: f32 = 48.0;
    pub const PROGRESS_HEIGHT: f32 = 6.0;
    pub const PROGRESS_HEIGHT_MINI: f32 = 5.0;
    pub const BOTTOM_BAR_HEIGHT: f32 = 36.0;
    pub const DRAG_HEIGHT: f32 = 48.0;

    pub const KITTEN_SIZE: f32 = 60.0;
    pub const KITTEN_SIZE_MINI: f32 = 36.0;
    pub const KITTEN_SIZE_ISLAND: f32 = 16.0;
    /// The island's kitten sits on the straight part of the pill's bottom edge.
    pub const KITTEN_ISLAND_RIGHT: f32 = 22.0;
}
