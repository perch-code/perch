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
/// The activity rail sits a shade deeper than the panel beside it.
pub const CANVAS_RAIL: Color = Color::from_rgb8(0xf2, 0xf4, 0xf6);
pub const CANVAS_HOVER: Color = Color::from_rgb8(0xf3, 0xf4, 0xf6);

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
/// A check still running: GitHub's pending yellow.
pub const DRAFT_YELLOW: Color = Color::from_rgb8(0xbf, 0x87, 0x00);

/// Badges.
/// A caution pill: the amber GitHub uses for an attention banner.
pub const NOTICE_BG: Color = Color::from_rgb8(0xff, 0xf8, 0xc5);
pub const NOTICE_BORDER: Color = Color::from_rgb8(0xd4, 0xa7, 0x2c);
pub const NOTICE_FG: Color = Color::from_rgb8(0x7d, 0x4e, 0x00);

pub const BADGE_BG: Color = Color::from_rgb8(0xea, 0xee, 0xf2);
pub const SHADOW: Color = Color::from_rgba8(0x1f, 0x23, 0x28, 0x14);

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
pub const TEXT_TITLE: f32 = 15.0;
pub const TEXT_BODY: f32 = 14.0;
pub const TEXT_META: f32 = 13.0;
pub const TEXT_BADGE: f32 = 12.0;
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
/// The navigation panel beside the activity rail.
pub const SIDEBAR_WIDTH: f64 = 212.0;
pub const SIDEBAR_MIN_WIDTH: f64 = 160.0;
pub const SIDEBAR_MAX_WIDTH: f64 = 520.0;
/// The always-present rail of destination icons.
///
/// Wider than an editor's activity bar because the traffic lights sit at the
/// top of it, and they need the room.
pub const ACTIVITY_RAIL_WIDTH: f64 = 72.0;
/// An item row in the sidebar: two lines of text and the glyph beside them.
pub const ROW_HEIGHT: f64 = 46.0;
pub const STATUS_HEIGHT: f64 = 34.0;
/// Room above the rail's first entry, where the traffic lights sit.
///
/// The lights end at y=23; this leaves a little air below them without
/// the entry drifting down the rail.
pub const TITLEBAR_HEIGHT: f64 = 36.0;
pub const RADIUS: f64 = 6.0;
pub const RADIUS_LG: f64 = 10.0;
/// The references panel under the editor, and how far it can be dragged.
pub const REFERENCES_HEIGHT: f64 = 220.0;
pub const REFERENCES_MIN_HEIGHT: f64 = 100.0;
pub const REFERENCES_MAX_HEIGHT: f64 = 600.0;

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
pub const SYNTAX_STRING: Color = Color::from_rgb8(0x0a, 0x30, 0x69);
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
        SYNTAX_PUNCT,    // punctuation.special
        SYNTAX_DEFAULT,  // text.emphasis
        SYNTAX_STRING,   // text.literal
        ACCENT,          // text.reference
        SYNTAX_DEFAULT,  // text.strong
        SYNTAX_DEFAULT,  // text.title
        ACCENT,          // text.uri
    ];
    COLORS.get(capture).copied().unwrap_or(SYNTAX_DEFAULT)
}

/// Code metrics. The editor needs a fixed line height to place the gutter.
pub const CODE_FONT_SIZE: f32 = 13.0;
pub const CODE_LINE_HEIGHT: f32 = 21.0;
/// Menlo by name, and nothing else.
///
/// "SF Mono" and "Consolas" are not installed here, and the `monospace`
/// generic resolves to Courier rather than to anything a reviewer wants to
/// read code in.
pub const CODE_FONT_STACK: &str = "Menlo";
pub const GUTTER_WIDTH: f64 = 70.0;
/// Gap between the gutter's change bar and the first character of code.
pub const CODE_GUTTER_GAP: f64 = 12.0;
pub const RAIL_WIDTH: f64 = 312.0;
pub const REVIEW_HEADER_HEIGHT: f64 = 60.0;
/// The thread column beside the editor.
pub const THREADS_WIDTH: f64 = 360.0;
pub const THREADS_MIN_WIDTH: f64 = 260.0;
pub const THREADS_MAX_WIDTH: f64 = 640.0;
pub const TAB_HEIGHT: f64 = 38.0;
/// Widest a tab grows before its name is ellipsized.
pub const TAB_MAX_WIDTH: f64 = 210.0;

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

// ---------------------------------------------------------------------------
// The item view: the header's pills and strip, and Conversation's cards
// ---------------------------------------------------------------------------

/// The state pill before the title: the state's colour as text on a wash
/// of the same colour.
pub const STATE_PILL_HEIGHT: f64 = 20.0;
pub const STATE_WASH: f32 = 0.12;
/// The strip under the header bar: reviewers, labels, assignees, milestone.
pub const METADATA_HEIGHT: f64 = 30.0;
pub const METADATA_GAP: f64 = 16.0;
pub const METADATA_AVATAR: f64 = 16.0;
/// A small chip: a label, a linked issue, the Local mark on a thread.
pub const CHIP_HEIGHT: f64 = 18.0;

/// How wide the reading column is. Prose past this is hard to follow.
pub const CONVERSATION_WIDTH: f64 = 820.0;
pub const CONVERSATION_PADDING: f64 = 20.0;
/// Room between cards, and between an event line and its neighbours.
pub const CARD_GAP: f64 = 12.0;
pub const EVENT_GAP: f64 = 4.0;
/// The byline strip across the top of a card, and the text under it.
pub const BYLINE_HEIGHT: f64 = 40.0;
pub const CARD_PADDING: f64 = 16.0;
/// A card's byline avatar, and the smaller one on a line comment nested
/// inside a review.
pub const CARD_AVATAR: f64 = 22.0;
pub const NESTED_AVATAR: f64 = 18.0;
/// An event is one line with a dot where the avatars sit.
pub const EVENT_HEIGHT: f64 = 32.0;
pub const EVENT_DOT: f64 = 8.0;
/// A line of code shown under a review comment or a thread's anchor.
pub const CODE_ROW_HEIGHT: f64 = 22.0;
pub const CODE_ROW_NUMBER_WIDTH: f64 = 40.0;
/// How many lines of a review comment's hunk are shown above it, and how
/// many of a thread's anchor before the rest are counted instead.
pub const HUNK_LINES: usize = 4;
pub const ANCHOR_LINES: usize = 8;
/// The rule down the left of a reply under a line comment.
pub const REPLY_RULE: f64 = 2.0;
/// A local thread's border is dashed, so it cannot be taken for a
/// GitHub comment's.
pub const THREAD_DASH: [f64; 2] = [4.0, 3.0];
/// The box that opens the composer, and the composer in its place.
pub const ASK_BOX_HEIGHT: f64 = 48.0;
/// The scope button at the top of the rail, and the Viewed count row
/// between it and the tree.
pub const SCOPE_BUTTON_HEIGHT: f64 = 34.0;
pub const VIEWED_ROW_HEIGHT: f64 = 28.0;
/// A row of the rail's tree. The Viewed check is 26 high and sits inside it.
pub const TREE_ROW_HEIGHT: f64 = 28.0;
/// A split's filler row: a blank on the side that has no line where the
/// other side does. Grey rather than a diff tint, because nothing is there.
pub const DIFF_FILLER_BG: Color = Color::from_rgb8(0xf1, 0xf3, 0xf5);
/// The Stacked and Split tiles at the right end of the file tab bar.
pub const LAYOUT_TILE: f64 = 22.0;
/// The references panel under the editor: its header, a file's heading
/// row, the rows under it, and the line number column at their left.
pub const REFERENCES_HEADER_HEIGHT: f64 = 30.0;
pub const REFERENCES_GROUP_HEIGHT: f64 = 28.0;
pub const REFERENCES_ROW_HEIGHT: f64 = 22.0;
pub const REFERENCES_LINE_WIDTH: f64 = 48.0;
