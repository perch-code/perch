//! Every colour, size, and type choice in one place.
//!
//! The palette follows the warm, quiet neutrals of Codex's desktop UI. Dark
//! mode is a later diff: add a second set here and select with floem's
//! `dark_mode` style selector.

use floem::peniko::Color;

/// Surfaces.
pub const CANVAS: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub const CANVAS_SUBTLE: Color = Color::from_rgb8(0xf7, 0xf7, 0xf5);
pub const CANVAS_SIDEBAR: Color = Color::from_rgb8(0xf8, 0xf8, 0xf6);
pub const CANVAS_HOVER: Color = Color::from_rgb8(0xee, 0xee, 0xeb);

/// The left navigation uses Codex's quieter, neutral surface treatment.
/// These stay separate from the shared app palette so the dock can keep its
/// precise contrast and interaction states.
pub const NAV_CANVAS: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub const NAV_CONTROL_BORDER: Color = Color::from_rgb8(0xe5, 0xe4, 0xe5);
pub const NAV_DIVIDER: Color = Color::from_rgb8(0xed, 0xed, 0xed);
pub const NAV_HOVER: Color = Color::from_rgb8(0xf8, 0xf8, 0xf8);
pub const NAV_SELECTED: Color = Color::from_rgb8(0xf4, 0xf4, 0xf4);
pub const NAV_SELECTED_PRESSED: Color = Color::from_rgb8(0xee, 0xee, 0xee);
pub const NAV_FG: Color = Color::from_rgb8(0x19, 0x1a, 0x1c);
pub const NAV_MUTED: Color = Color::from_rgb8(0x88, 0x88, 0x8a);
pub const NAV_SUBTLE: Color = Color::from_rgb8(0xa3, 0xa3, 0xa5);
/// A row's churn: the additions and the deletions, in the quiet green and
/// red Codex tints its figures with, softer than the diff's own colours so
/// they sit beside the muted byline rather than shouting over it.
pub const NAV_ADDED: Color = Color::from_rgb8(0x3b, 0x8f, 0x55);
pub const NAV_DELETED: Color = Color::from_rgb8(0xc9, 0x4a, 0x4f);

/// Lines.
pub const BORDER: Color = Color::from_rgb8(0xdd, 0xdd, 0xd9);
pub const BORDER_MUTED: Color = Color::from_rgb8(0xeb, 0xeb, 0xe8);

/// Text.
pub const FG: Color = Color::from_rgb8(0x25, 0x25, 0x23);
pub const FG_MUTED: Color = Color::from_rgb8(0x68, 0x68, 0x63);
pub const FG_SUBTLE: Color = Color::from_rgb8(0x98, 0x98, 0x92);

/// Accents.
pub const ACCENT: Color = Color::from_rgb8(0x32, 0x32, 0x2f);
pub const ACCENT_SUBTLE: Color = Color::from_rgb8(0xe9, 0xe9, 0xe6);
pub const ACCENT_RING: Color = Color::from_rgba8(0x32, 0x32, 0x2f, 0x4d);
pub const ACTION_HOVER: Color = Color::from_rgb8(0x43, 0x43, 0x3f);

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

/// GitHub's alert callouts in a blockquote: `> [!NOTE]` and its kin, each
/// with the colour the site gives its border and title.
pub const ALERT_NOTE: Color = Color::from_rgb8(0x09, 0x69, 0xda);
pub const ALERT_TIP: Color = Color::from_rgb8(0x1a, 0x7f, 0x37);
pub const ALERT_IMPORTANT: Color = Color::from_rgb8(0x82, 0x50, 0xdf);
pub const ALERT_WARNING: Color = Color::from_rgb8(0x9a, 0x67, 0x00);
pub const ALERT_CAUTION: Color = Color::from_rgb8(0xcf, 0x22, 0x2e);

pub const BADGE_BG: Color = Color::from_rgb8(0xed, 0xed, 0xea);
pub const SHADOW: Color = Color::from_rgba8(0x20, 0x20, 0x1e, 0x12);

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
pub const TEXT_PAGE_TITLE: f32 = 26.0;
pub const TEXT_BODY: f32 = 15.0;
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
/// The navigation panel, matching the roomy Codex dock at its default size.
pub const SIDEBAR_WIDTH: f64 = 442.0;
pub const SIDEBAR_MIN_WIDTH: f64 = 320.0;
pub const SIDEBAR_MAX_WIDTH: f64 = 520.0;
/// The lane the traffic lights sit in, at the left of whichever bar is at
/// the top left: the drawer's strip, or the main pane's bar while the
/// drawer is shut.
pub const ACTIVITY_RAIL_WIDTH: f64 = 72.0;
/// An item row in the sidebar: two lines of text and the glyph beside them.
pub const ROW_HEIGHT: f64 = 68.0;
pub const STATUS_HEIGHT: f64 = 34.0;
/// The app-wide Agents inspector. It overlays the workspace so opening it
/// never rebuilds or reflows the file the reader is looking at.
pub const AGENTS_WIDTH: f64 = 408.0;
pub const AGENTS_MIN_WIDTH: f64 = 336.0;
pub const AGENTS_MAX_WIDTH: f64 = 620.0;
pub const AGENTS_HEADER_HEIGHT: f64 = 46.0;
pub const AGENT_ROW_MIN_HEIGHT: f64 = 88.0;
/// The strip at the top of the drawer, where the traffic lights sit.
///
/// The lights and the scope controls share this strip, and the bar over
/// the item is the same height so the two read as one title bar.
pub const TITLEBAR_HEIGHT: f64 = 56.0;
pub const RADIUS: f64 = 6.0;
pub const RADIUS_LG: f64 = 10.0;
pub const NAV_ROW_RADIUS: f64 = 14.0;
pub const NAV_CONTROL_HEIGHT: f64 = 36.0;
pub const NAV_CONTROL_RADIUS: f64 = NAV_CONTROL_HEIGHT / 2.0;
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
pub const REVIEW_HEADER_HEIGHT: f64 = TITLEBAR_HEIGHT;
/// The room at either end of the bar over the item.
pub const REVIEW_HEADER_PADDING: f64 = 14.0;
pub const TAB_HEIGHT: f64 = 42.0;
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
// The item view: the summary's rows and pills, and Conversation's cards
// ---------------------------------------------------------------------------

/// The state pill in the summary's Status row: the state's colour as
/// text on a wash of the same colour.
pub const STATE_PILL_HEIGHT: f64 = 20.0;
pub const STATE_WASH: f32 = 0.12;
/// A row of the summary under Conversation's title: its name in a column
/// at the left, then its value. A value that wraps, reviewers or labels
/// past the column's edge, makes its row taller; the name stays on the
/// first line.
pub const SUMMARY_ROW: f64 = 32.0;
/// The name column: the glyph, its gap, and the longest name with room
/// to spare.
pub const SUMMARY_NAME_WIDTH: f64 = 124.0;
pub const SUMMARY_GLYPH: f64 = 14.0;
/// Between the parts of a value: the branches and their churn, the pill
/// and the number.
pub const SUMMARY_GAP: f64 = 12.0;
pub const SUMMARY_AVATAR: f64 = 16.0;
/// A small chip: a label, a linked issue, the Local mark on a thread.
pub const CHIP_HEIGHT: f64 = 18.0;

/// How wide the reading column is. Prose past this is hard to follow.
pub const CONVERSATION_WIDTH: f64 = 845.0;
pub const CONVERSATION_PADDING: f64 = 40.0;
/// Breathing room under the in-page title and around a flat timeline entry.
pub const CONVERSATION_TITLE_GAP: f64 = 28.0;
pub const CONVERSATION_ENTRY_PADDING: f64 = 24.0;
/// Room within entries, and between an event line and its neighbours.
pub const CARD_GAP: f64 = 12.0;
pub const EVENT_GAP: f64 = 4.0;
/// The compact byline at the top of a Conversation entry.
pub const BYLINE_HEIGHT: f64 = 32.0;
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
/// A thread in the column, and inline in the code: the room inside its
/// border and between its parts. Denser than a Conversation card, since
/// it sits beside or among the code.
pub const THREAD_PADDING: f64 = 12.0;
pub const THREAD_GAP: f64 = 10.0;
/// The `...` at the end of a thread's first row, and the smaller one on
/// a collapsed card, whose one row a 22px button would not fit.
pub const THREAD_MENU: f64 = 22.0;
pub const COLLAPSED_CARD_MENU: f64 = 18.0;
/// A card drawn over its spacer rows in the code sits this far inside
/// its first and last rows, so its border never touches a tinted line
/// above or a second card below. A collapsed card is one row, which has
/// no room to spare, and takes its row whole.
pub const CARD_INSET: f64 = 3.0;
/// The collapsed card's row: the room at either end, and the pill that
/// says how many messages follow the first, two pixels shorter than the
/// strip's count pill so it sits inside the borders, with this much room
/// before it and before the `...` after it, so no two fills touch.
pub const COLLAPSED_CARD_PADDING: f64 = 8.0;
pub const COLLAPSED_CARD_PILL: f64 = 16.0;
pub const COLLAPSED_CARD_PILL_GAP: f64 = 8.0;
/// The bar a card's messages scroll by once the card is at its cap,
/// floem's own width, drawn in the card's right padding rather than
/// taking room from the messages.
pub const CARD_SCROLLBAR: f64 = 8.0;
/// The slot the card's chevron sits in, and the glyph inside it, the
/// sizes the gutter's controls use.
pub const CHEVRON_SLOT: f64 = 16.0;
pub const CHEVRON_GLYPH: f64 = 12.0;
/// How long a card's border stays in the accent after its marker is
/// clicked.
pub const CARD_FLASH_MILLIS: u64 = 900;
/// The box that opens the composer, and the composer in its place.
pub const ASK_BOX_HEIGHT: f64 = 54.0;
/// The scope button at the top of the rail, and the Viewed count row
/// between it and the tree.
pub const SCOPE_BUTTON_HEIGHT: f64 = 34.0;
pub const VIEWED_ROW_HEIGHT: f64 = 28.0;
/// A row of the rail's tree. The Viewed check is 26 high and sits inside it.
pub const TREE_ROW_HEIGHT: f64 = 28.0;
/// A split's filler row: a blank on the side that has no line where the
/// other side does. Grey rather than a diff tint, because nothing is there.
pub const DIFF_FILLER_BG: Color = Color::from_rgb8(0xf1, 0xf3, 0xf5);
/// A divider row, standing for a run of unchanged lines folded away: the
/// accent's wash, which is what github.com paints its expand rows in.
pub const DIFF_DIVIDER_BG: Color = Color::from_rgb8(0xdd, 0xf4, 0xff);
/// The divider's count, muted like the gutter's numbers so the bar reads
/// as a seam in the file and not as a line of it.
pub const DIFF_DIVIDER_FG: Color = FG_MUTED;
/// The Show whole, Stacked and Split tiles at the right end of the file
/// tab bar.
pub const LAYOUT_TILE: f64 = 22.0;
/// The rule between Show whole and the layout tiles, which are two kinds
/// of preference: its height, and the room on each side of it.
pub const TILE_RULE_HEIGHT: f64 = 14.0;
pub const TILE_RULE_GAP: f64 = 6.0;
/// The references panel under the editor: its header, a file's heading
/// row, the rows under it, and the line number column at their left.
pub const REFERENCES_HEADER_HEIGHT: f64 = 30.0;
pub const REFERENCES_GROUP_HEIGHT: f64 = 28.0;
pub const REFERENCES_ROW_HEIGHT: f64 = 22.0;
pub const REFERENCES_LINE_WIDTH: f64 = 48.0;
/// The chip on a tab, and on a panel's file row, naming where a file
/// outside the worktree is from.
pub const ORIGIN_CHIP_HEIGHT: f64 = 16.0;
/// The tint behind every other use of the symbol the caret rests on: the
/// accent at low alpha, so it reads through a diff's green and under a
/// selection alike.
pub const SYMBOL_HIGHLIGHT_BG: Color = Color::from_rgba8(0x32, 0x32, 0x2f, 0x1c);
