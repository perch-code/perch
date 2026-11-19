use floem::peniko::Color;
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use perch_github::config::Theme;

#[cfg(test)]
use super::pick;
use super::{appearance, rgb, Appearance};

thread_local! {
    static THEME: RwSignal<Theme> = Scope::new().create_rw_signal(Theme::Classic);
}

pub(crate) fn current_palette() -> (Theme, Appearance) {
    (THEME.with(|theme| theme.get()), appearance())
}

/// Restyles all views that read a palette color.
pub fn set_theme(to: Theme) {
    THEME.with(|theme| {
        if theme.with_untracked(|now| *now != to) {
            theme.set(to);
        }
    });
}

// Keep the six versions of each role together. The same table supplies both
// the live style functions and the explicit palette used by Settings previews.
macro_rules! palettes {
    ($( $(#[$doc:meta])* $role:ident: $cl:literal, $cd:literal; $wl:literal, $wd:literal; $bl:literal, $bd:literal; )*) => {
        pub struct Palette {
            $(pub $role: Color,)*
        }

        const CLASSIC_LIGHT: Palette = Palette { $($role: rgb($cl),)* };
        const CLASSIC_DARK: Palette = Palette { $($role: rgb($cd),)* };
        const LINEN_LIGHT: Palette = Palette { $($role: rgb($wl),)* };
        const LINEN_DARK: Palette = Palette { $($role: rgb($wd),)* };
        const MIST_LIGHT: Palette = Palette { $($role: rgb($bl),)* };
        const MIST_DARK: Palette = Palette { $($role: rgb($bd),)* };

        $(
            $(#[$doc])*
            pub fn $role() -> Color {
                #[cfg(test)]
                {
                    let theme = THEME.with(|theme| theme.get());
                    pick(
                        colors(theme, Appearance::Light).$role,
                        colors(theme, Appearance::Dark).$role,
                    )
                }
                #[cfg(not(test))]
                {
                    let (theme, appearance) = current_palette();
                    colors(theme, appearance).$role
                }
            }
        )*
    };
}

palettes! {
    //                     Classic light/dark    Warm Linen light/dark   Blue Mist light/dark

    // Main surfaces
    canvas:                0xffffff, 0x181818;    0xfffdf7, 0x24231f;      0xfafcfc, 0x1d252c;
    canvas_subtle:         0xf7f7f5, 0x1f1f1f;    0xf8f4ea, 0x2b2923;      0xf0f5f7, 0x222e37;
    canvas_hover:          0xeeeeeb, 0x282828;    0xeeeadf, 0x353228;      0xe5eef2, 0x293a47;
    /// A local Perch thread, or a private comment inside a GitHub thread.
    perch_bg:              0xf4f4f2, 0x212121;    0xf6f1e2, 0x352f24;      0xecf2f4, 0x2b3943;
    /// An inline review thread on GitHub.
    review_comment_bg:     0xffffff, 0x181818;    0xfaf5e4, 0x302b22;      0xf8f4e9, 0x2c2e2b;
    border:                0xddddd9, 0x3a3a3a;    0xdfdaca, 0x494338;      0xd7e2e7, 0x354754;
    border_muted:          0xebebe8, 0x2b2b2b;    0xece6d8, 0x38342c;      0xe5edf0, 0x2a3a46;
    fg:                    0x252523, 0xdcdcd9;    0x343b3b, 0xe5e0d6;      0x2b3d48, 0xdde8ef;
    fg_muted:              0x686863, 0xa9a9a4;    0x5c6764, 0xbcb5a9;      0x506773, 0xb0c0ca;
    fg_subtle:             0x989892, 0x7c7c78;    0x717c78, 0xa8a095;      0x657c88, 0x94a8b4;
    accent:                0x32322f, 0xdcdcd9;    0x486577, 0xc2d7e3;      0x375d74, 0xbad8ea;
    accent_subtle:         0xe9e9e6, 0x2e2e2c;    0xdfebef, 0x344651;      0xdbe8ef, 0x314959;
    action_hover:          0x43433f, 0xf0f0ed;    0x345569, 0xd6e7ef;      0x274c62, 0xcfe7f5;
    badge_bg:              0xededea, 0x2e2e2c;    0xf1ebde, 0x3a352c;      0xe7eff3, 0x2b3d49;

    // Navigation
    nav_canvas:            0xffffff, 0x181818;    0xf4f0e4, 0x2b2822;      0xeaf1f5, 0x24333f;
    nav_control_border:    0xe5e4e5, 0x333333;    0xdfd9c9, 0x494337;      0xd2e0e7, 0x3b4d5a;
    nav_divider:           0xededed, 0x2b2b2b;    0xe7e1d3, 0x413a2f;      0xdde7ec, 0x314350;
    nav_hover:             0xf8f8f8, 0x1f1f1f;    0xeee9dc, 0x343026;      0xe1ebf0, 0x2b3d4a;
    nav_selected:          0xf4f4f4, 0x242424;    0xdfebef, 0x344654;      0xd5e5ee, 0x304757;
    nav_selected_pressed:  0xeeeeee, 0x2b2b2b;    0xd4e4eb, 0x384a58;      0xcbdde8, 0x344b5b;
    nav_fg:                0x191a1c, 0xdfdfdf;    0x303b3c, 0xe9e3d8;      0x283e4a, 0xe4edf3;
    /// A read notification's title is quieter than an unread notification.
    nav_read:              0x5b5b5e, 0xbababa;    0x515c5a, 0xc7c6be;      0x4a606d, 0xbeced8;
    nav_muted:             0x88888a, 0x8c8c8c;    0x59645f, 0xbebab0;      0x4c636f, 0xb7cbd7;
    nav_subtle:            0xa3a3a5, 0x747474;    0x5c655e, 0xbcb6ac;      0x4d626e, 0xb2c5d1;
    nav_added:             0x3b8f55, 0x5cb877;    0x3c6e50, 0x9acaac;      0x36684d, 0x9cceb0;
    nav_deleted:           0xc94a4f, 0xe5676c;    0x924d4d, 0xeba9a1;      0x8d484e, 0xecaaa9;

    // Menu surfaces
    menu_bg:               0xffffff, 0x212121;    0xfffdf7, 0x2b2923;      0xfafcfc, 0x283641;
    menu_pointed:          0xf7f7f7, 0x2a2a2a;    0xf4f0e6, 0x363227;      0xebf2f5, 0x304451;
    menu_hover:            0xf2f2f2, 0x2f2f2f;    0xdfebef, 0x354958;      0xdbe9f0, 0x365266;
    menu_separator:        0xededed, 0x333333;    0xeae4d5, 0x433d31;      0xe2ebef, 0x3b4d5a;

    // Code and files
    code_bg:               0xffffff, 0x181818;    0xfffdf7, 0x24231f;      0xfafcfc, 0x1d252c;
    gutter_bg:             0xfcfdfd, 0x1a1a1a;    0xfcf9f0, 0x282620;      0xf2f6f8, 0x222d36;
    gutter_fg:             0x8c959f, 0x6e7681;    0x66706b, 0xada79a;      0x58707d, 0x9eb2bf;
    rail_bg:               0xfafbfc, 0x1b1b1b;    0xf9f6ec, 0x2b2822;      0xf0f5f8, 0x24333f;
    syntax_default:        0x1f2328, 0xe6edf3;    0x333f43, 0xe9e3d8;      0x2e414d, 0xe2edf3;
    diff_added_bg:         0xe6ffec, 0x1b2c1e;    0xedf4ee, 0x26372a;      0xe9f3ed, 0x253b32;
    diff_deleted_bg:       0xffebe9, 0x3a2120;    0xfaeee8, 0x3a2927;      0xf8ece9, 0x3d2f32;
    diff_added_bar:        0x2da44e, 0x3fb950;    0x588766, 0x86b895;      0x518168, 0x86bca0;
    diff_deleted_bar:      0xcf222e, 0xf85149;    0xb06a62, 0xd29287;      0xa96060, 0xd59494;
    diff_deleted_fg:       0x825a57, 0xa8908d;    0x815853, 0xc9a59e;      0x7f5458, 0xcaa6a7;
    diff_filler_bg:        0xf1f3f5, 0x212121;    0xf4f0e5, 0x2b2923;      0xeaf0f3, 0x25323c;
    diff_divider_bg:       0xddf4ff, 0x1b2533;    0xe7eef0, 0x2b3842;      0xe3edf4, 0x2b3b48;
    diff_expander_bg:      0xb6e3ff, 0x254674;    0xd7e6ec, 0x354958;      0xd3e4ee, 0x345063;
}

/// Read a particular palette without changing the app's theme.
pub fn colors(theme: Theme, appearance: Appearance) -> &'static Palette {
    match (theme, appearance) {
        (Theme::Classic, Appearance::Light) => &CLASSIC_LIGHT,
        (Theme::Classic, Appearance::Dark) => &CLASSIC_DARK,
        (Theme::WarmLinen, Appearance::Light) => &LINEN_LIGHT,
        (Theme::WarmLinen, Appearance::Dark) => &LINEN_DARK,
        (Theme::BlueMist, Appearance::Light) => &MIST_LIGHT,
        (Theme::BlueMist, Appearance::Dark) => &MIST_DARK,
    }
}
