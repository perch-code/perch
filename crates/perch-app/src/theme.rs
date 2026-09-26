//! Every colour, size, and type choice in one place.
//!
//! The palette follows the warm, quiet neutrals of Codex's desktop UI. Dark
//! mode is a later diff: add a second set here and select with floem's
//! `dark_mode` style selector.

use std::path::Path;
use std::sync::Arc;

use floem::peniko::{Blob, Color};
use floem::text::{FontStyle, FontWeight, FONT_CONTEXT};
use fontique::FontInfoOverride;

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
/// The title of a row whose notification has been read. GitHub's own
/// inbox greys a read row, and so does this: far enough off `NAV_FG` to
/// tell at a glance, not as far as an archived row's `NAV_SUBTLE`. The
/// weight axis cannot carry this on its own, since Regular against
/// Medium at 14px is a tenth of a pixel of stem.
pub const NAV_READ: Color = Color::from_rgb8(0x5b, 0x5b, 0x5e);
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
/// The dot on an unread notification: the blue GitHub's own inbox marks
/// one with, so it reads as the same mark here.
pub const UNREAD: Color = Color::from_rgb8(0x09, 0x69, 0xda);
/// What is on GitHub, or will be: a draft's mark and the gutter's mark
/// on a line that holds one. GitHub's own blue, the same as `UNREAD`.
pub const GITHUB: Color = Color::from_rgb8(0x09, 0x69, 0xda);
/// A link in rendered markdown: GitHub's own link blue, so a link reads
/// as one against the prose rather than as a darker word.
pub const LINK: Color = Color::from_rgb8(0x09, 0x69, 0xda);
/// The wash under a Draft pill: the blue GitHub washes its own marks
/// with, the same as `REACTED_BG`.
pub const GITHUB_BG: Color = Color::from_rgb8(0xdd, 0xf4, 0xff);
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
/// A reaction pill the reader is among: the blue wash and rule GitHub
/// draws its own with, so it reads as the same mark here.
pub const REACTED_BG: Color = Color::from_rgb8(0xdd, 0xf4, 0xff);
pub const REACTED_BORDER: Color = Color::from_rgb8(0x54, 0xae, 0xff);
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

/// Type scale, in whole pixels, as Codex's desktop window sets it: a 14px
/// base for a row's title, the prose and a summary's rows; a step down
/// for a toolbar's labels and the bar's title; two for a row's byline and
/// age, a section's heading and a chip; and up to 16 for a section's
/// title and 24 for the page's. Codex has nothing smaller than 12, so a
/// section heading and a badge are the same size and differ in weight
/// and colour. The renderer rounds the size in its glyph cache key, so
/// fractional sizes that round together share bitmaps on a non-retina
/// display.
pub const TEXT_TITLE: f32 = 14.0;
pub const TEXT_PAGE_TITLE: f32 = 24.0;
pub const TEXT_SECTION_TITLE: f32 = 16.0;
pub const TEXT_BODY: f32 = 14.0;
pub const TEXT_META: f32 = 13.0;
pub const TEXT_BADGE: f32 = 12.0;
pub const TEXT_SECTION: f32 = 12.0;

/// The weights, as Codex sets them. Its body is 430, a shade heavier
/// than Regular, which the system font can draw because it is variable:
/// asking for 430 sets its weight axis rather than rounding to a face.
/// Emphasis is Medium, for a title, a section's heading, a selected tab
/// and a button that matters; Codex keeps Semibold for prose, where a
/// heading, a bold run and a table's header row carry it, and for the
/// initial on a placeholder avatar. Nothing is Bold. Regular is the one
/// step below the body, and only a read row's title takes it, so it sits
/// as far from an unread title's Medium as the axis is worth using.
///
/// Functions rather than constants because a weight off the named
/// steps cannot be built in a `const`.
pub fn weight_read() -> FontWeight {
    FontWeight::NORMAL
}
pub fn weight_body() -> FontWeight {
    FontWeight::new(430.0)
}
pub fn weight_emphasis() -> FontWeight {
    FontWeight::MEDIUM
}
pub fn weight_strong() -> FontWeight {
    FontWeight::SEMI_BOLD
}

/// Line heights, as a multiple of the size. Prose runs at 1.625, Codex's
/// relaxed leading, which is what its description and comments read at;
/// the page title is set tight, at 1.2, so its two lines sit close.
pub const PROSE_LINE_HEIGHT: f32 = 1.625;
pub const PAGE_TITLE_LINE_HEIGHT: f32 = 1.2;

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
/// The drawer on the right, which holds the open item's files as a tree,
/// on Code. Docked beside the main pane while the window has the room, and
/// over it while it does not.
pub const DRAWER_WIDTH: f64 = 408.0;
pub const DRAWER_MIN_WIDTH: f64 = 336.0;
pub const DRAWER_MAX_WIDTH: f64 = 620.0;
/// The least the main pane is left with before the drawer stops taking
/// room from it and lies over it instead: enough for the item bar's
/// controls and the tab strip's, with the title given way.
pub const MAIN_MIN_WIDTH: f64 = 620.0;
/// The Agents dropdown under the bar's button: as wide as Review's popover,
/// and as tall as this before it scrolls.
pub const AGENTS_DROPDOWN_WIDTH: f64 = 430.0;
pub const AGENTS_DROPDOWN_MAX_HEIGHT: f64 = 460.0;
/// The strip at the top of the drawer, where the traffic lights sit.
///
/// The lights and the scope controls share this strip, and the bar over
/// the item is the same height so the two read as one title bar.
pub const TITLEBAR_HEIGHT: f64 = 56.0;
/// The bar at the top of the drawer on the right, under the item's tabs.
pub const DRAWER_HEADER_HEIGHT: f64 = 40.0;
pub const RADIUS: f64 = 6.0;
pub const RADIUS_LG: f64 = 10.0;
/// The room inside a text box, between its border and its lines.
pub const EDITOR_BOX_PADDING: f64 = 10.0;
/// The space between the drawer's edges and its search row and rows.
pub const NAV_GUTTER: f64 = 12.0;
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

/// Code metrics, as Codex sets its diffs: 12px at 1.8, which is 21.6 and
/// rounds up to a whole pixel here because the editor needs a fixed line
/// height to place the gutter.
pub const CODE_FONT_SIZE: f32 = 12.0;
pub const CODE_LINE_HEIGHT: f32 = 22.0;
/// A fenced block in prose: Codex gives its code content the greater of
/// 1.5em and 20px, which at the code size is 20px.
pub const PROSE_CODE_LINE_HEIGHT: f32 = 20.0 / 12.0;
/// SF Mono, by the name the system's copy is installed under.
///
/// Codex asks for `ui-monospace`, which is SF Mono on a Mac. There is no
/// "SF Mono" family here unless the developer download is installed, but
/// the copy the system ships is reachable as ".SF NS Mono", and it is a
/// variable font, so it draws the body weight the sans does. The
/// `monospace` generic resolves to Courier, and Menlo, which this used
/// to name, is a different design.
pub const CODE_FONT_STACK: &str = ".SF NS Mono";

/// Puts SF Mono's two files under the name `CODE_FONT_STACK` asks for,
/// each flagged with its slope. Called once, before any text is laid out.
///
/// Apple ships the italic without the italic bit in its OS/2 or head
/// table: only its `post` angle and its "Light Italic" name say what it
/// is. fontique reads the bits alone, so it files both as upright, and
/// asked for an upright at the body weight it takes the last one it
/// scanned, which is the italic. Every line of code in Perch then slants,
/// and the branch pill's arrow goes missing, since the italic has no
/// arrow glyph. A family registered by hand is found before a scanned one
/// of the same name, so this is the family the code font resolves to. A
/// file that is not there is skipped, and with neither the name falls
/// through to the scanned family as before.
pub fn install_code_font() {
    let faces = [
        ("SFNSMono.ttf", FontStyle::Normal),
        ("SFNSMonoItalic.ttf", FontStyle::Italic),
    ];
    let mut fonts = FONT_CONTEXT.lock();
    for (file, style) in faces {
        let path = Path::new("/System/Library/Fonts").join(file);
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "code font file not read");
                continue;
            }
        };
        fonts.collection.register_fonts(
            Blob::new(Arc::new(data)),
            Some(FontInfoOverride {
                family_name: Some(CODE_FONT_STACK),
                style: Some(style),
                ..Default::default()
            }),
        );
    }
}
pub const GUTTER_WIDTH: f64 = 70.0;
/// Gap between the gutter's change bar and the first character of code.
pub const CODE_GUTTER_GAP: f64 = 12.0;
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
/// A reaction pill under a comment, and the room above the row of them.
pub const REACTION_PILL_HEIGHT: f64 = 26.0;
pub const REACTIONS_GAP: f64 = 10.0;
/// A card's byline avatar.
pub const CARD_AVATAR: f64 = 22.0;
/// An event is one line with a dot where the avatars sit.
pub const EVENT_HEIGHT: f64 = 32.0;
pub const EVENT_DOT: f64 = 8.0;
/// A line of code under a thread's header: GitHub's 20px rows and its
/// 50px number column.
pub const CODE_ROW_HEIGHT: f64 = 20.0;
pub const CODE_ROW_NUMBER_WIDTH: f64 = 50.0;
/// How many lines of a review comment's hunk are shown above it, and how
/// many of a thread's anchor before the rest are counted instead.
pub const HUNK_LINES: usize = 4;
pub const ANCHOR_LINES: usize = 8;
/// A local thread's border is dashed, so it cannot be taken for a
/// GitHub comment's.
pub const THREAD_DASH: [f64; 2] = [4.0, 3.0];
/// A thread as github.com draws one, on Conversation and in the code: a
/// box with a header, the comments, the reply and a foot with Resolve.
/// The avatar beside each comment, smaller in the code, and the room
/// between it and the name, past which a reply's text is indented.
pub const THREAD_AVATAR: f64 = 24.0;
pub const INLINE_THREAD_AVATAR: f64 = 20.0;
pub const THREAD_AVATAR_GAP: f64 = 8.0;
/// The room at a thread's sides, GitHub's 16 on Conversation and less in
/// the code, and above and below each comment, so two comments are twice
/// it apart. The first comment has the side's room over it.
pub const THREAD_INSET: f64 = 16.0;
pub const INLINE_THREAD_INSET: f64 = 12.0;
pub const THREAD_COMMENT_SPACE: f64 = 12.0;
pub const INLINE_THREAD_COMMENT_SPACE: f64 = 10.0;
/// The line down the avatars, from one comment to the next.
pub const THREAD_CONNECTOR: f64 = 2.0;
/// The header across a thread on Conversation: GitHub's 8px round a 28px
/// chevron button.
pub const THREAD_HEADER_PADDING: f64 = 8.0;
pub const THREAD_HEADER_BUTTON: f64 = 28.0;
/// The header of a thread in the code, "Comment on line R12".
pub const INLINE_THREAD_HEADER: f64 = 32.0;
/// A thread in the code is no wider than github.com draws one.
pub const INLINE_THREAD_MAX_WIDTH: f64 = 780.0;
/// The Follow up box and the foot's Resolve button.
pub const THREAD_CONTROL: f64 = 32.0;
/// A label in a comment's byline, as GitHub sets Author: a pill with a
/// rule round it.
pub const BYLINE_LABEL: f64 = 20.0;
/// A line of a comment being typed, which the composer's styling gives
/// the editor and the box's heights are counted in.
pub const COMPOSER_LINE_HEIGHT: f32 = TEXT_BODY * 1.5;
/// How tall the comment box is before it is dragged: five lines of body
/// text and the room around them, which is the size GitHub gives a
/// comment. It grows with what is typed as far as
/// `COMPOSER_GROWN_HEIGHT`, and once dragged it is exactly as tall as
/// the reader left it.
pub const COMPOSER_HEIGHT: f64 = composer_lines(5.0);
/// How tall an undragged box grows before its lines scroll instead.
pub const COMPOSER_GROWN_HEIGHT: f64 = 220.0;
/// The heights the grip along the bottom edge drags between. The
/// shortest is two lines: the one being typed and the one above it.
pub const COMPOSER_MIN_HEIGHT: f64 = composer_lines(2.0);
pub const COMPOSER_MAX_HEIGHT: f64 = 520.0;
/// How tall a pull request's description box opens: ten lines, about
/// what GitHub gives one. It grows with what is typed as far as
/// `PULL_BODY_GROWN_HEIGHT`, and drags as tall as `PULL_BODY_MAX_HEIGHT`.
pub const PULL_BODY_HEIGHT: f64 = composer_lines(10.0);
pub const PULL_BODY_GROWN_HEIGHT: f64 = 420.0;
pub const PULL_BODY_MAX_HEIGHT: f64 = 900.0;
/// The band along the inside of the box's bottom edge that the drag
/// happens in, which is the room under the last line, and the grip
/// drawn in the middle of it.
pub const COMPOSER_GRIP: f64 = EDITOR_BOX_PADDING;
pub const COMPOSER_GRIP_WIDTH: f64 = 28.0;
pub const COMPOSER_GRIP_MARK: f64 = 3.0;

/// How tall a comment box is that holds this many lines: the lines, the
/// room around them and the border either side.
const fn composer_lines(lines: f64) -> f64 {
    lines * COMPOSER_LINE_HEIGHT as f64 + 2.0 * EDITOR_BOX_PADDING + 2.0
}

/// The `...` at the end of a thread's header, and the smaller one on
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
/// The Source, Show whole, Stacked and Split tiles at the right end of
/// the file tab bar.
pub const LAYOUT_TILE: f64 = 22.0;
/// The rule between the tile groups, which are kinds of preference: its
/// height, and the room on each side of it.
pub const TILE_RULE_HEIGHT: f64 = 14.0;
pub const TILE_RULE_GAP: f64 = 6.0;
/// A picture, an SVG drawn in place of its markup: the wash under it, so
/// its canvas shows and so does a shape in the pane's own white, and the
/// room between the picture and the edges of its pane.
pub const PICTURE_CANVAS: Color = CANVAS_SUBTLE;
pub const PICTURE_INSET: f64 = 24.0;
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
/// The selection in an editor, the code's and the composers' alike, and
/// over prose and a label: the system's selection blue, translucent so
/// that a selected line keeps its diff tint, the way a selection over a
/// diff on github.com does. floem's own default is black at half alpha
/// in an editor and its theme's teal over a label.
pub const SELECTION_BG: Color = Color::from_rgba8(0x00, 0x7a, 0xff, 0x4d);
/// The tint behind every match of the find bar's query, and the darker
/// one behind the match the bar is on: VS Code's, orange at a third and
/// an olive grey. The editor paints a range's tint over the selection,
/// so the current match's has to read on its own, without the blue.
pub const FIND_MATCH_BG: Color = Color::from_rgba8(0xea, 0x5c, 0x00, 0x55);
pub const FIND_CURRENT_BG: Color = Color::from_rgb8(0xa8, 0xac, 0x94);
/// The find bar: how tall it is, how wide its box is, and how far in
/// from the code's right edge it hangs, as VS Code's, which hangs it
/// two of its 14px scrollbars in.
pub const FIND_BAR_HEIGHT: f64 = 33.0;
pub const FIND_BOX_WIDTH: f64 = 221.0;
pub const FIND_BAR_INSET: f64 = 28.0;
/// Cmd-P's palette: as wide as VS Code's at most, and hung just under the
/// bar over the item, across the file tabs, as VS Code's is under its
/// title bar; the room inside its edge, where its list's bar runs; its
/// box; a row of its list; and how tall the list grows before it
/// scrolls, sixteen rows.
pub const GO_TO_WIDTH: f64 = 600.0;
pub const GO_TO_TOP: f64 = TITLEBAR_HEIGHT + 6.0;
pub const GO_TO_PADDING: f64 = 6.0;
pub const GO_TO_BOX_HEIGHT: f64 = 30.0;
pub const GO_TO_ROW_HEIGHT: f64 = 26.0;
pub const GO_TO_LIST_HEIGHT: f64 = 16.0 * GO_TO_ROW_HEIGHT;
/// The list an `@` puts up under a composer's caret: as wide as a path
/// and a name want, and ten of Cmd-P's rows before it scrolls.
pub const MENTION_WIDTH: f64 = 440.0;
pub const MENTION_ROWS: usize = 10;
/// The letters of a name that a query matched, in Cmd-P's list: blue, as
/// VS Code draws them, and GitHub's blue so it is one Perch already uses.
pub const MATCH_FG: Color = Color::from_rgb8(0x09, 0x69, 0xda);

#[cfg(test)]
mod tests {
    use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};

    use super::*;

    /// Every glyph `text` shapes to in the code font at the body weight.
    fn code_glyphs(text: &str) -> Vec<u32> {
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list(CODE_FONT_STACK).collect();
        let attrs = Attrs::new()
            .font_size(TEXT_BADGE)
            .family(&family)
            .weight(weight_body());
        let layout = TextLayout::new_with_text(text, AttrsList::new(attrs), None);
        let mut ids = Vec::new();
        for line in layout.parley_layout().lines() {
            for run in line.runs() {
                for cluster in run.clusters() {
                    ids.extend(cluster.glyphs().map(|g| g.id));
                }
            }
        }
        ids
    }

    /// The branch pill's arrow is set in the code font, and the italic
    /// file, which fontique takes for the upright, has no glyph for it.
    #[test]
    fn code_font_draws_the_arrow() {
        install_code_font();
        let ids = code_glyphs("main \u{2190} topic");
        assert_eq!(ids.len(), 12, "one glyph a character: {ids:?}");
        assert!(ids.iter().all(|&id| id != 0), "a .notdef among {ids:?}");
    }
}
