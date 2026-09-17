//! Every colour, size, and type choice in one place.
//!
//! The palette is GitHub Primer's light scale, so the app sits next to
//! github.com without clashing. Dark mode is a later diff: add a second set
//! here and select with floem's `dark_mode` style selector.

use floem::peniko::Color;

/// Surfaces.
pub const CANVAS: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub const CANVAS_SUBTLE: Color = Color::from_rgb8(0xf6, 0xf8, 0xfa);
pub const CANVAS_SIDEBAR: Color = Color::from_rgb8(0xfa, 0xfb, 0xfc);
pub const CANVAS_HOVER: Color = Color::from_rgb8(0xf3, 0xf4, 0xf6);
pub const CANVAS_UNREAD: Color = Color::from_rgb8(0xf7, 0xfb, 0xff);

/// Lines.
pub const BORDER: Color = Color::from_rgb8(0xd1, 0xd9, 0xe0);
pub const BORDER_MUTED: Color = Color::from_rgb8(0xe4, 0xe8, 0xed);

/// Text.
pub const FG: Color = Color::from_rgb8(0x1f, 0x23, 0x28);
pub const FG_MUTED: Color = Color::from_rgb8(0x59, 0x63, 0x6e);
pub const FG_SUBTLE: Color = Color::from_rgb8(0x87, 0x90, 0x9b);

/// Accents.
pub const ACCENT: Color = Color::from_rgb8(0x09, 0x69, 0xda);
pub const ACCENT_SUBTLE: Color = Color::from_rgb8(0xdd, 0xf4, 0xff);
pub const ACCENT_RING: Color = Color::from_rgba8(0x09, 0x69, 0xda, 0x55);

/// Subject states.
pub const OPEN: Color = Color::from_rgb8(0x1a, 0x7f, 0x37);
pub const MERGED: Color = Color::from_rgb8(0x82, 0x50, 0xdf);
pub const CLOSED: Color = Color::from_rgb8(0xd1, 0x24, 0x2f);
pub const DRAFT: Color = Color::from_rgb8(0x59, 0x63, 0x6e);

/// Badges.
pub const BADGE_BG: Color = Color::from_rgb8(0xea, 0xee, 0xf2);
pub const BADGE_FG: Color = Color::from_rgb8(0x59, 0x63, 0x6e);
pub const SHADOW: Color = Color::from_rgba8(0x1f, 0x23, 0x28, 0x14);
pub const SHADOW_SOFT: Color = Color::from_rgba8(0x1f, 0x23, 0x28, 0x0c);

/// Avatar placeholders, cycled by name so a row is stable between launches.
pub const AVATAR_TINTS: [Color; 6] = [
    Color::from_rgb8(0xdd, 0xf4, 0xff),
    Color::from_rgb8(0xdd, 0xff, 0xe6),
    Color::from_rgb8(0xfa, 0xe7, 0xff),
    Color::from_rgb8(0xff, 0xef, 0xdb),
    Color::from_rgb8(0xff, 0xe3, 0xe3),
    Color::from_rgb8(0xe7, 0xeb, 0xff),
];

/// Type scale.
pub const TEXT_TITLE: f32 = 13.5;
pub const TEXT_BODY: f32 = 12.5;
pub const TEXT_META: f32 = 11.5;
pub const TEXT_BADGE: f32 = 11.0;
pub const TEXT_SECTION: f32 = 10.5;

/// Layout constants.
pub const SIDEBAR_WIDTH: f64 = 236.0;
pub const ROW_HEIGHT: f64 = 58.0;
pub const TOOLBAR_HEIGHT: f64 = 56.0;
pub const STATUS_HEIGHT: f64 = 30.0;
pub const RADIUS: f64 = 6.0;
pub const RADIUS_LG: f64 = 10.0;

/// macOS first, with fallbacks that exist on any machine cosmic-text can see.
pub const FONT_STACK: &str = "SF Pro Text, SF Pro Display, Helvetica Neue, Helvetica, Arial";

/// A stable tint for a login, so the same person keeps the same placeholder.
pub fn tint_for(seed: &str) -> Color {
    let sum: u32 = seed.bytes().map(u32::from).sum();
    AVATAR_TINTS[(sum as usize) % AVATAR_TINTS.len()]
}
