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

/// Type scale, in whole pixels. The renderer rounds the size in its glyph
/// cache key, so fractional sizes that round together share bitmaps on a
/// non-retina display.
pub const TEXT_TITLE: f32 = 14.0;
pub const TEXT_BODY: f32 = 13.0;
pub const TEXT_META: f32 = 12.0;
pub const TEXT_BADGE: f32 = 11.0;
pub const TEXT_SECTION: f32 = 11.0;

/// How far to lift text that is centred inside a pill or a button.
///
/// Centring a label centres its line box, and San Francisco's box is not
/// symmetric about the glyphs: measured from the font, ascent is 0.967em and
/// descent 0.211em while cap height is 0.705em, which leaves a digit sitting
/// 0.026em below the box centre. A margin below the label pushes it back up
/// by half that margin.
pub const OPTICAL_TEXT_LIFT: f64 = 1.0;

/// Layout constants.
pub const SIDEBAR_WIDTH: f64 = 244.0;
pub const ROW_HEIGHT: f64 = 62.0;
pub const TOOLBAR_HEIGHT: f64 = 58.0;
pub const STATUS_HEIGHT: f64 = 32.0;
/// Height of the strip that replaces the title bar.
pub const TITLEBAR_HEIGHT: f64 = 38.0;
/// Room the traffic lights need before anything else may sit beside them.
///
/// The three buttons end around x=71 given the offset they are placed at;
/// the rest is breathing room so the drawer toggle does not crowd the green
/// one.
pub const TRAFFIC_LIGHTS_WIDTH: f64 = 88.0;
pub const RADIUS: f64 = 6.0;
/// Pills get a fixed height so their text can be centred by layout rather
/// than by padding, which the font's uneven leading throws off.
pub const BADGE_HEIGHT: f64 = 18.0;
pub const PILL_HEIGHT: f64 = 20.0;
pub const RADIUS_LG: f64 = 10.0;

/// The system UI font, by the one generic that reaches it.
///
/// It has to be a single entry: floem hands a lone family straight to
/// parley, which understands `system-ui`, but a comma-separated list takes a
/// different path where the generic is never resolved. "SF Pro Text" and
/// "SF Pro Display", which this used to name, are not installed families at
/// all and resolved to nothing.
pub const FONT_STACK: &str = "system-ui";

/// A stable tint for a login, so the same person keeps the same placeholder.
pub fn tint_for(seed: &str) -> Color {
    let sum: u32 = seed.bytes().map(u32::from).sum();
    AVATAR_TINTS[(sum as usize) % AVATAR_TINTS.len()]
}

// ---------------------------------------------------------------------------
// Review view
// ---------------------------------------------------------------------------

/// Code surfaces.
pub const CODE_BG: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub const GUTTER_BG: Color = Color::from_rgb8(0xfc, 0xfd, 0xfd);
pub const GUTTER_FG: Color = Color::from_rgb8(0x8c, 0x95, 0x9f);
pub const RAIL_BG: Color = Color::from_rgb8(0xfa, 0xfb, 0xfc);

/// Diff line backgrounds, from GitHub's light diff.
pub const DIFF_ADDED_BG: Color = Color::from_rgb8(0xe6, 0xff, 0xec);
pub const DIFF_MODIFIED_BG: Color = Color::from_rgb8(0xe6, 0xff, 0xec);
pub const DIFF_DELETED_BG: Color = Color::from_rgb8(0xff, 0xeb, 0xe9);

/// The bars drawn in the gutter beside a changed line.
pub const DIFF_ADDED_BAR: Color = Color::from_rgb8(0x2d, 0xa4, 0x4e);
pub const DIFF_MODIFIED_BAR: Color = Color::from_rgb8(0xbf, 0x87, 0x00);
pub const DIFF_DELETED_BAR: Color = Color::from_rgb8(0xcf, 0x22, 0x2e);

/// Deleted code is dimmed, since it is context rather than the thing under
/// review.
pub const DIFF_DELETED_FG: Color = Color::from_rgb8(0x82, 0x5a, 0x57);

/// Status letters in the changed-files rail.
pub const STATUS_ADDED: Color = Color::from_rgb8(0x1a, 0x7f, 0x37);
pub const STATUS_MODIFIED: Color = Color::from_rgb8(0x9a, 0x67, 0x00);
pub const STATUS_DELETED: Color = Color::from_rgb8(0xcf, 0x22, 0x2e);
pub const STATUS_RENAMED: Color = Color::from_rgb8(0x82, 0x50, 0xdf);

/// Syntax colours, indexed to match `perch_syntax::CAPTURES`.
///
/// GitHub's light theme, so code in Perch looks like code on the site the
/// pull request came from.
pub const SYNTAX_DEFAULT: Color = Color::from_rgb8(0x1f, 0x23, 0x28);
const SYNTAX_KEYWORD: Color = Color::from_rgb8(0xcf, 0x22, 0x2e);
const SYNTAX_STRING: Color = Color::from_rgb8(0x0a, 0x30, 0x69);
const SYNTAX_COMMENT: Color = Color::from_rgb8(0x6e, 0x77, 0x81);
const SYNTAX_FUNCTION: Color = Color::from_rgb8(0x82, 0x50, 0xdf);
const SYNTAX_TYPE: Color = Color::from_rgb8(0x95, 0x38, 0x00);
const SYNTAX_NUMBER: Color = Color::from_rgb8(0x05, 0x50, 0xae);
const SYNTAX_VARIABLE: Color = Color::from_rgb8(0x1f, 0x23, 0x28);
const SYNTAX_PUNCT: Color = Color::from_rgb8(0x57, 0x60, 0x6a);
const SYNTAX_TAG: Color = Color::from_rgb8(0x11, 0x63, 0x29);

/// Colour for a capture index from `perch_syntax::CAPTURES`.
pub fn syntax_color(capture: usize) -> Color {
    // The order here mirrors CAPTURES exactly.
    const COLORS: &[Color] = &[
        SYNTAX_FUNCTION, // attribute
        SYNTAX_COMMENT,  // comment
        SYNTAX_NUMBER,   // constant
        SYNTAX_NUMBER,   // constant.builtin
        SYNTAX_TYPE,     // constructor
        SYNTAX_STRING,   // escape
        SYNTAX_FUNCTION, // function
        SYNTAX_FUNCTION, // function.builtin
        SYNTAX_FUNCTION, // function.macro
        SYNTAX_FUNCTION, // function.method
        SYNTAX_KEYWORD,  // keyword
        SYNTAX_TYPE,     // label
        SYNTAX_NUMBER,   // number
        SYNTAX_KEYWORD,  // operator
        SYNTAX_VARIABLE, // property
        SYNTAX_PUNCT,    // punctuation
        SYNTAX_PUNCT,    // punctuation.bracket
        SYNTAX_PUNCT,    // punctuation.delimiter
        SYNTAX_STRING,   // string
        SYNTAX_STRING,   // string.special
        SYNTAX_TAG,      // tag
        SYNTAX_TYPE,     // type
        SYNTAX_TYPE,     // type.builtin
        SYNTAX_VARIABLE, // variable
        SYNTAX_KEYWORD,  // variable.builtin
        SYNTAX_VARIABLE, // variable.parameter
    ];
    COLORS.get(capture).copied().unwrap_or(SYNTAX_DEFAULT)
}

/// Code metrics. The editor needs a fixed line height to place the gutter.
pub const CODE_FONT_SIZE: f32 = 13.0;
pub const CODE_LINE_HEIGHT: f32 = 20.0;
/// Menlo by name, and nothing else.
///
/// "SF Mono" and "Consolas" are not installed here, and the `monospace`
/// generic resolves to Courier rather than to anything a reviewer wants to
/// read code in.
pub const CODE_FONT_STACK: &str = "Menlo";
pub const GUTTER_WIDTH: f64 = 66.0;
/// Gap between the gutter's change bar and the first character of code.
pub const CODE_GUTTER_GAP: f64 = 12.0;
pub const RAIL_WIDTH: f64 = 300.0;
pub const REVIEW_HEADER_HEIGHT: f64 = 58.0;
pub const TAB_HEIGHT: f64 = 37.0;
/// Widest a tab grows before its name is ellipsized.
pub const TAB_MAX_WIDTH: f64 = 190.0;

/// A dot beside a tab name, coloured by language.
pub fn language_color(language: Option<perch_syntax::Language>) -> Color {
    use perch_syntax::Language;
    match language {
        Some(Language::Rust) => Color::from_rgb8(0xce, 0x69, 0x3a),
        Some(Language::Toml) => Color::from_rgb8(0x9c, 0x7a, 0x4f),
        Some(Language::Json) => Color::from_rgb8(0xc6, 0xa1, 0x00),
        Some(Language::Markdown) => Color::from_rgb8(0x51, 0x7f, 0xbf),
        Some(Language::Yaml) => Color::from_rgb8(0x8a, 0x6d, 0xbd),
        None => Color::from_rgb8(0xa8, 0xb0, 0xb8),
    }
}
