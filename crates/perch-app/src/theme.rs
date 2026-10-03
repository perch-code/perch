//! Every colour, size, and type choice in one place.
//!
//! The palette follows the warm, quiet neutrals of Codex's desktop UI, and
//! each colour has a light and a dark value. The dark ones are Codex's dark
//! surfaces and text, and GitHub's dark Primer for what GitHub colours: a
//! diff, the syntax, a state, a link.

use std::path::Path;
use std::sync::Arc;

use floem::peniko::{Blob, Color};
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::text::{FontStyle, FontWeight, FONT_CONTEXT};
use fontique::{FallbackKey, FamilyId, FontInfoOverride, Script};

/// Light or dark, as macOS has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

thread_local! {
    /// The appearance every colour is picked by. Its own scope, so no
    /// view's going takes it with it.
    static APPEARANCE: RwSignal<Appearance> = Scope::new().create_rw_signal(Appearance::Light);
}

/// The appearance the colours are in, tracked: a style or an effect that
/// reads a colour runs again when the appearance changes, so a view that
/// names its colours in its style restyles itself.
pub fn appearance() -> Appearance {
    APPEARANCE.with(|a| a.get())
}

/// Switches every colour, from the window's theme. A no-op when nothing
/// changes, since every style that read a colour would run again.
pub fn set_appearance(to: Appearance) {
    APPEARANCE.with(|a| {
        if a.with_untracked(|now| *now != to) {
            a.set(to);
        }
    });
}

impl From<floem::window::Theme> for Appearance {
    fn from(theme: floem::window::Theme) -> Self {
        match theme {
            floem::window::Theme::Light => Appearance::Light,
            floem::window::Theme::Dark => Appearance::Dark,
        }
    }
}

/// A colour read where it is used rather than where it is named: a
/// colour function, `theme::fg_muted`, or a closure that picks one. A
/// view handed a `Color` keeps it through a change of appearance; one
/// handed an `Ink` reads it again in its style.
pub trait Ink: Fn() -> Color + Copy + 'static {}
impl<T: Fn() -> Color + Copy + 'static> Ink for T {}

fn pick(light: Color, dark: Color) -> Color {
    #[cfg(test)]
    dark_check::served(light, dark);
    match appearance() {
        Appearance::Light => light,
        Appearance::Dark => dark,
    }
}

/// A text editor's styling id, with the appearance in it. The id keys the
/// editor's cache of laid-out lines, and the lines carry their colours, so
/// a change of appearance lays them out again.
pub fn styling_id(id: u64) -> u64 {
    id.wrapping_mul(2) + u64::from(appearance() == Appearance::Dark)
}

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn rgba(hex: u32, alpha: u8) -> Color {
    Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, alpha)
}

/// How round an app's face is, as a share of its side: GitHub's 4px on
/// a 16px bot.
pub const APP_AVATAR_CORNER: f64 = 0.25;

/// Surfaces.
pub fn canvas() -> Color {
    pick(rgb(0xffffff), rgb(0x181818))
}
pub fn canvas_subtle() -> Color {
    pick(rgb(0xf7f7f5), rgb(0x1f1f1f))
}
pub fn canvas_hover() -> Color {
    pick(rgb(0xeeeeeb), rgb(0x282828))
}
/// What only Perch has, set off from GitHub's white as Codex sets off
/// its own: the reader's asks and an agent's answers, a local thread,
/// the Agents and Reviews sections. White is what GitHub has.
pub fn perch_bg() -> Color {
    pick(rgb(0xf4f4f2), rgb(0x212121))
}

/// The left navigation uses Codex's quieter, neutral surface treatment.
/// These stay separate from the shared app palette so the dock can keep its
/// precise contrast and interaction states.
pub fn nav_canvas() -> Color {
    pick(rgb(0xffffff), rgb(0x181818))
}
pub fn nav_control_border() -> Color {
    pick(rgb(0xe5e4e5), rgb(0x333333))
}
pub fn nav_divider() -> Color {
    pick(rgb(0xededed), rgb(0x2b2b2b))
}
pub fn nav_hover() -> Color {
    pick(rgb(0xf8f8f8), rgb(0x1f1f1f))
}
pub fn nav_selected() -> Color {
    pick(rgb(0xf4f4f4), rgb(0x242424))
}
pub fn nav_selected_pressed() -> Color {
    pick(rgb(0xeeeeee), rgb(0x2b2b2b))
}
pub fn nav_fg() -> Color {
    pick(rgb(0x191a1c), rgb(0xdfdfdf))
}
/// The title of a row whose notification has been read. GitHub's own
/// inbox greys a read row, and so does this: far enough off `nav_fg()` to
/// tell at a glance, not as far as an archived row's `nav_subtle()`. The
/// weight axis cannot carry this on its own, since Regular against
/// Medium at 14px is a tenth of a pixel of stem.
pub fn nav_read() -> Color {
    pick(rgb(0x5b5b5e), rgb(0xbababa))
}
pub fn nav_muted() -> Color {
    pick(rgb(0x88888a), rgb(0x8c8c8c))
}
pub fn nav_subtle() -> Color {
    pick(rgb(0xa3a3a5), rgb(0x747474))
}
/// A row's churn: the additions and the deletions, in the quiet green and
/// red Codex tints its figures with, softer than the diff's own colours so
/// they sit beside the muted byline rather than shouting over it.
pub fn nav_added() -> Color {
    pick(rgb(0x3b8f55), rgb(0x5cb877))
}
pub fn nav_deleted() -> Color {
    pick(rgb(0xc94a4f), rgb(0xe5676c))
}

/// Lines.
pub fn border() -> Color {
    pick(rgb(0xddddd9), rgb(0x3a3a3a))
}
pub fn border_muted() -> Color {
    pick(rgb(0xebebe8), rgb(0x2b2b2b))
}

/// Text.
pub fn fg() -> Color {
    pick(rgb(0x252523), rgb(0xdcdcd9))
}
pub fn fg_muted() -> Color {
    pick(rgb(0x686863), rgb(0xa9a9a4))
}
pub fn fg_subtle() -> Color {
    pick(rgb(0x989892), rgb(0x7c7c78))
}

/// Accents.
pub fn accent() -> Color {
    pick(rgb(0x32322f), rgb(0xdcdcd9))
}
/// The dot on an unread notification: the blue GitHub's own inbox marks
/// one with, so it reads as the same mark here.
pub fn unread() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}
/// What is on GitHub, or will be: a draft's mark and the gutter's mark
/// on a line that holds one. GitHub's own blue, the same as `unread()`.
pub fn github() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}
/// A link in rendered markdown: GitHub's own link blue, so a link reads
/// as one against the prose rather than as a darker word.
pub fn link() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}
/// The wash under a Draft pill: the blue GitHub washes its own marks
/// with, the same as `reacted_bg()`.
pub fn github_bg() -> Color {
    pick(rgb(0xddf4ff), rgb(0x1c2635))
}
pub fn accent_subtle() -> Color {
    pick(rgb(0xe9e9e6), rgb(0x2e2e2c))
}
pub fn accent_ring() -> Color {
    pick(rgba(0x32322f, 0x4d), rgba(0xdcdcd9, 0x4d))
}
pub fn action_hover() -> Color {
    pick(rgb(0x43433f), rgb(0xf0f0ed))
}

/// Subject states.
pub fn open() -> Color {
    pick(rgb(0x1a7f37), rgb(0x3fb950))
}
pub fn merged() -> Color {
    pick(rgb(0x8250df), rgb(0xab7df8))
}
pub fn closed() -> Color {
    pick(rgb(0xd1242f), rgb(0xf85149))
}
pub fn draft() -> Color {
    pick(rgb(0x59636e), rgb(0x9198a1))
}
/// An agent at work: a blue that reads as live against the grey it sits
/// on, as the accent's near-black did not.
pub fn running() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}
/// A check still running: GitHub's pending yellow.
pub fn draft_yellow() -> Color {
    pick(rgb(0xbf8700), rgb(0xd29922))
}

/// Badges.
/// A caution pill: the amber GitHub uses for an attention banner.
pub fn notice_bg() -> Color {
    pick(rgb(0xfff8c5), rgb(0x302816))
}
pub fn notice_border() -> Color {
    pick(rgb(0xd4a72c), rgb(0x9e6a03))
}
pub fn notice_fg() -> Color {
    pick(rgb(0x7d4e00), rgb(0xd29922))
}

/// GitHub's alert callouts in a blockquote: `> [!NOTE]` and its kin, each
/// with the colour the site gives its border and title.
pub fn alert_note() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}
pub fn alert_tip() -> Color {
    pick(rgb(0x1a7f37), rgb(0x3fb950))
}
pub fn alert_important() -> Color {
    pick(rgb(0x8250df), rgb(0xab7df8))
}
pub fn alert_warning() -> Color {
    pick(rgb(0x9a6700), rgb(0xd29922))
}
pub fn alert_caution() -> Color {
    pick(rgb(0xcf222e), rgb(0xf85149))
}

pub fn badge_bg() -> Color {
    pick(rgb(0xededea), rgb(0x2e2e2c))
}
/// A reaction pill the reader is among: the blue wash and rule GitHub
/// draws its own with, so it reads as the same mark here.
pub fn reacted_bg() -> Color {
    pick(rgb(0xddf4ff), rgb(0x1c2635))
}
pub fn reacted_border() -> Color {
    pick(rgb(0x54aeff), rgb(0x1f6feb))
}
/// The pill saying GitHub has something newer: GitHub's blue wash and
/// rule, so it stands out from the grey list it floats over. Refresh sits
/// on white inside it and deepens to the blue GitHub hovers with.
pub fn news_bg() -> Color {
    pick(rgb(0xddf4ff), rgb(0x1c2635))
}
pub fn news_border() -> Color {
    pick(rgb(0x54aeff), rgb(0x1f6feb))
}
pub fn news_fg() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}
pub fn news_button_hover() -> Color {
    pick(rgb(0xb6e3ff), rgb(0x203551))
}
pub fn shadow() -> Color {
    pick(rgba(0x20201e, 0x12), rgba(0x000000, 0x40))
}

/// Menus, as Codex draws its dropdowns: a white panel in a faint ring
/// with a soft shadow under it, a grey wash on the row under the pointer
/// or the keyboard, and hairlines between groups.
pub fn menu_bg() -> Color {
    pick(rgb(0xffffff), rgb(0x212121))
}
pub fn menu_ring() -> Color {
    pick(rgba(0x000000, 0x17), rgba(0xffffff, 0x1a))
}
pub fn menu_shadow() -> Color {
    pick(rgba(0x000000, 0x1f), rgba(0x000000, 0x66))
}
/// A panel's deeper shadow, Codex's `shadow-2xl`.
pub fn panel_shadow() -> Color {
    pick(rgba(0x000000, 0x30), rgba(0x000000, 0x80))
}
/// A list row under the pointer, lighter than the one the keyboard is on.
pub fn menu_pointed() -> Color {
    pick(rgb(0xf7f7f7), rgb(0x2a2a2a))
}
pub fn menu_hover() -> Color {
    pick(rgb(0xf2f2f2), rgb(0x2f2f2f))
}
pub fn menu_separator() -> Color {
    pick(rgb(0xededed), rgb(0x333333))
}

/// Avatar placeholders, cycled by name so a row is stable between launches.
pub fn avatar_tints() -> [Color; 6] {
    [
        (0xddf4ff, 0x1c2b3a),
        (0xddffe6, 0x1b3324),
        (0xfae7ff, 0x2e2238),
        (0xffefdb, 0x3a2a1a),
        (0xffe3e3, 0x3a2020),
        (0xe7ebff, 0x23263d),
    ]
    .map(|(light, dark)| pick(rgb(light), rgb(dark)))
}

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
/// The plan usage popover under the bar's gauge.
pub const USAGE_POPOVER_WIDTH: f64 = 320.0;
/// Send Feedback's dropdown, a little wider than Agents': the preview's
/// text wants the room.
pub const FEEDBACK_DROPDOWN_WIDTH: f64 = 440.0;
/// Past this the dropdown scrolls: the preview, with similar issues, is
/// taller.
pub const FEEDBACK_DROPDOWN_MAX_HEIGHT: f64 = 640.0;
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
    let tints = avatar_tints();
    tints[(sum as usize) % tints.len()]
}

// ---------------------------------------------------------------------------
// Review view
// ---------------------------------------------------------------------------

/// Code surfaces.
pub fn code_bg() -> Color {
    pick(rgb(0xffffff), rgb(0x181818))
}
pub fn gutter_bg() -> Color {
    pick(rgb(0xfcfdfd), rgb(0x1a1a1a))
}
pub fn gutter_fg() -> Color {
    pick(rgb(0x8c959f), rgb(0x6e7681))
}
pub fn rail_bg() -> Color {
    pick(rgb(0xfafbfc), rgb(0x1b1b1b))
}

/// Diff line backgrounds, from GitHub's light diff.
pub fn diff_added_bg() -> Color {
    pick(rgb(0xe6ffec), rgb(0x1b2c1e))
}
pub fn diff_modified_bg() -> Color {
    pick(rgb(0xe6ffec), rgb(0x1b2c1e))
}
pub fn diff_deleted_bg() -> Color {
    pick(rgb(0xffebe9), rgb(0x3a2120))
}

/// The bars drawn in the gutter beside a changed line.
pub fn diff_added_bar() -> Color {
    pick(rgb(0x2da44e), rgb(0x3fb950))
}
pub fn diff_modified_bar() -> Color {
    pick(rgb(0xbf8700), rgb(0xd29922))
}
pub fn diff_deleted_bar() -> Color {
    pick(rgb(0xcf222e), rgb(0xf85149))
}

/// Deleted code is dimmed, since it is context rather than the thing under
/// review.
pub fn diff_deleted_fg() -> Color {
    pick(rgb(0x825a57), rgb(0xa8908d))
}

/// Status letters in the changed-files rail.
pub fn status_added() -> Color {
    pick(rgb(0x1a7f37), rgb(0x3fb950))
}
pub fn status_modified() -> Color {
    pick(rgb(0x9a6700), rgb(0xd29922))
}
pub fn status_deleted() -> Color {
    pick(rgb(0xcf222e), rgb(0xf85149))
}
pub fn status_renamed() -> Color {
    pick(rgb(0x8250df), rgb(0xab7df8))
}

/// Syntax colours, indexed to match `perch_syntax::CAPTURES`.
///
/// GitHub's light theme, so code in Perch looks like code on the site the
/// pull request came from.
pub fn syntax_default() -> Color {
    pick(rgb(0x1f2328), rgb(0xe6edf3))
}
fn syntax_keyword() -> Color {
    pick(rgb(0xcf222e), rgb(0xff7b72))
}
pub fn syntax_string() -> Color {
    pick(rgb(0x0a3069), rgb(0xa5d6ff))
}
fn syntax_comment() -> Color {
    pick(rgb(0x6e7781), rgb(0x8b949e))
}
fn syntax_function() -> Color {
    pick(rgb(0x8250df), rgb(0xd2a8ff))
}
fn syntax_type() -> Color {
    pick(rgb(0x953800), rgb(0xffa657))
}
fn syntax_number() -> Color {
    pick(rgb(0x0550ae), rgb(0x79c0ff))
}
fn syntax_variable() -> Color {
    pick(rgb(0x1f2328), rgb(0xe6edf3))
}
fn syntax_punct() -> Color {
    pick(rgb(0x57606a), rgb(0xb1bac4))
}
fn syntax_tag() -> Color {
    pick(rgb(0x116329), rgb(0x7ee787))
}

/// Colour for a capture index from `perch_syntax::CAPTURES`.
pub fn syntax_color(capture: usize) -> Color {
    // The order here mirrors CAPTURES exactly.
    const COLORS: &[fn() -> Color] = &[
        syntax_function,   // attribute
        syntax_comment,    // comment
        syntax_number,     // constant
        syntax_number,     // constant.builtin
        syntax_type,       // constructor
        syntax_string,     // escape
        syntax_function,   // function
        syntax_function,   // function.builtin
        syntax_function,   // function.macro
        syntax_function,   // function.method
        syntax_keyword,    // keyword
        syntax_type,       // label
        syntax_number,     // number
        syntax_keyword,    // operator
        syntax_variable,   // property
        syntax_punct,      // punctuation
        syntax_punct,      // punctuation.bracket
        syntax_punct,      // punctuation.delimiter
        syntax_string,     // string
        syntax_string,     // string.special
        syntax_tag,        // tag
        syntax_type,       // type
        syntax_type,       // type.builtin
        syntax_variable,   // variable
        syntax_keyword,    // variable.builtin
        syntax_variable,   // variable.parameter
        syntax_punct,      // punctuation.special
        syntax_default,    // text.emphasis
        syntax_string,     // text.literal
        accent,            // text.reference
        syntax_default,    // text.strong
        syntax_default,    // text.title
        accent,            // text.uri
    ];
    COLORS.get(capture).map_or_else(syntax_default, |color| color())
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

/// Where a symbol neither SF Pro nor SF Mono has is drawn from, in the
/// order macOS's own cascade list names them: Apple Symbols for set and
/// logic signs (∪, ∀, ⊢), Menlo for letterlike ones (ℕ), and STIX for
/// the rest of mathematics.
const SYMBOL_FONTS: [&str; 3] = ["Apple Symbols", "Menlo", "STIX Two Math"];

/// The scripts whose text gets `SYMBOL_FONTS`: Latin, which a line of
/// symbols alone falls back as too, and the two whose letters stand in
/// formulas beside Latin ones. Each costs a CoreText lookup at startup,
/// about 3ms, so not all 161 fontique knows.
const SYMBOL_SCRIPTS: [[u8; 4]; 3] = [*b"Latn", *b"Grek", *b"Cyrl"];

/// Adds `SYMBOL_FONTS` to the end of `SYMBOL_SCRIPTS`' fallback families.
/// Called once, before any text is laid out.
///
/// parley falls back by script, not by character: a character the text's
/// fonts lack goes to the one family the system picks for the script's
/// sample letters, which for Latin is Helvetica. A symbol has no script of
/// its own and takes its neighbours', so ∪ in English went to Helvetica,
/// which lacks it too, and drew as SF's .notdef, a stack of bars. The
/// system's own family stays first, so nothing it drew changes.
pub fn install_symbol_fallbacks() {
    let mut fonts = FONT_CONTEXT.lock();
    let collection = &mut fonts.collection;
    let symbols: Vec<FamilyId> = SYMBOL_FONTS
        .iter()
        .filter_map(|name| collection.family_id(name))
        .collect();
    for script in SYMBOL_SCRIPTS {
        let key = FallbackKey::new(Script(script), None);
        let mut families: Vec<FamilyId> = collection.fallback_families(key).collect();
        for &family in &symbols {
            if !families.contains(&family) {
                families.push(family);
            }
        }
        collection.set_fallbacks(key, families.into_iter());
    }
}
/// The line-number gutter, left to right: the room before the slot a
/// thread's mark or the `+` sits in, the slot, the room between the slot
/// and the widest number, the room after the numbers, and the change bar.
/// The numbers take what the file's widest needs, and never less than
/// three digits, so most files' gutters are one width; see
/// `review::gutter::width`. A divider's expanders sit in the gutter too,
/// a row each, so it needs no room for a row of controls.
pub const GUTTER_SLOT_INSET: f64 = 2.0;
pub const GUTTER_SLOT: f64 = 16.0;
pub const GUTTER_NUMBER_GAP: f64 = 4.0;
pub const GUTTER_NUMBER_PADDING: f64 = 6.0;
pub const GUTTER_BAR: f64 = 3.0;
pub const GUTTER_MIN_DIGITS: usize = 3;
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
        Some(Language::Go) => Color::from_rgb8(0x00, 0x9c, 0xc4),
        Some(Language::JavaScript) => Color::from_rgb8(0xd4, 0xb8, 0x30),
        Some(Language::TypeScript | Language::Tsx) => Color::from_rgb8(0x31, 0x78, 0xc6),
        Some(Language::Java) => Color::from_rgb8(0xb0, 0x72, 0x19),
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
/// A badge: a label, a linked issue, a grant, the Local, Outdated and
/// Draft marks on a thread, Author in a byline. Every one is this tall
/// and fully round, with GitHub's padding either side, so two side by
/// side line up. See `widgets::badge`.
pub const CHIP_HEIGHT: f64 = 18.0;
pub const CHIP_PADDING: f64 = 7.0;

/// How wide the reading column is. Prose past this is hard to follow.
pub const CONVERSATION_WIDTH: f64 = 845.0;
pub const CONVERSATION_PADDING: f64 = 40.0;
/// Breathing room under the in-page title, and between the timeline and
/// the sections after it.
pub const CONVERSATION_TITLE_GAP: f64 = 28.0;
pub const CONVERSATION_ENTRY_PADDING: f64 = 24.0;
/// Room between two cards, and between an event line and its neighbours.
pub const CARD_GAP: f64 = 12.0;
pub const EVENT_GAP: f64 = 6.0;
/// Every box on Conversation, a comment's, a thread's, a section's, has
/// these corners and this room at its sides. See `cards::github_surface`
/// and `cards::perch_surface`.
pub const CARD_RADIUS: f64 = RADIUS_LG;
pub const CARD_PADDING: f64 = 16.0;
/// The header of a section after the timeline: Checks, Agents, Reviews.
pub const SECTION_HEADER_HEIGHT: f64 = 44.0;
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

/// The `...` at the end of a thread's header, open or collapsed.
pub const THREAD_MENU: f64 = 22.0;
pub const COLLAPSED_CARD_MENU: f64 = THREAD_MENU;
/// A card drawn over its spacer rows in the code sits this far inside
/// its first and last rows, so its border never touches a tinted line
/// above or a second card below.
pub const CARD_INSET: f64 = 3.0;
/// A collapsed card sits further inside its two rows than an open one,
/// which leaves it about as tall as github.com draws a folded thread.
pub const COLLAPSED_CARD_INSET: f64 = 5.0;
/// The collapsed card's row: the room at either end, and the pill that
/// says how many messages follow the first, with this much room before
/// it and before the `...` after it, so no two fills touch.
pub const COLLAPSED_CARD_PADDING: f64 = 10.0;
pub const COLLAPSED_CARD_PILL_GAP: f64 = 8.0;
/// The face beside the first author on a collapsed card.
pub const COLLAPSED_CARD_AVATAR: f64 = 20.0;
/// The slot a chevron sits in, and the glyph inside it, the sizes the
/// gutter's controls use, and the larger ones at a card's left.
pub const CHEVRON_SLOT: f64 = 16.0;
pub const CHEVRON_GLYPH: f64 = 12.0;
pub const CARD_CHEVRON_SLOT: f64 = 20.0;
pub const CARD_CHEVRON_GLYPH: f64 = 14.0;
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
pub fn diff_filler_bg() -> Color {
    pick(rgb(0xf1f3f5), rgb(0x212121))
}
/// A divider row, standing for a run of unchanged lines folded away: the
/// blue github.com paints its hunk rows in, Primer's accent subtle.
pub fn diff_divider_bg() -> Color {
    pick(rgb(0xddf4ff), rgb(0x1b2533))
}
/// The divider's hunk header, muted as github.com sets it, so the row
/// reads as a seam in the file and not as a line of it.
pub fn diff_divider_fg() -> Color {
    fg_muted()
}
/// A divider's expanders, in the gutter beside its row: the stronger
/// blue of github.com's hunk number cell (Primer's blue 1, which its
/// diff blob tokens give the cell), the glyph muted as GitHub draws it,
/// and while the pointer is on one, GitHub's own blue under a white glyph
/// (Primer's accent emphasis and on-emphasis), as its expanders light up.
pub fn diff_expander_bg() -> Color {
    pick(rgb(0xb6e3ff), rgb(0x254674))
}
pub fn diff_expander_fg() -> Color {
    fg_muted()
}
pub fn diff_expander_hover_bg() -> Color {
    pick(github(), rgb(0x1f6feb))
}
pub fn diff_expander_hover_fg() -> Color {
    rgb(0xffffff)
}
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
pub fn picture_canvas() -> Color {
    canvas_subtle()
}
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
pub fn symbol_highlight_bg() -> Color {
    pick(rgba(0x32322f, 0x1c), rgba(0xdcdcd9, 0x24))
}
/// The selection in an editor, the code's and the composers' alike, and
/// over prose and a label: the system's selection blue, translucent so
/// that a selected line keeps its diff tint, the way a selection over a
/// diff on github.com does. floem's own default is black at half alpha
/// in an editor and its theme's teal over a label.
pub fn selection_bg() -> Color {
    pick(rgba(0x007aff, 0x4d), rgba(0x267dff, 0x59))
}
/// The tint behind every match of the find bar's query, and the darker
/// one behind the match the bar is on: VS Code's, orange at a third and
/// an olive grey. The editor paints a range's tint over the selection,
/// so the current match's has to read on its own, without the blue.
pub fn find_match_bg() -> Color {
    rgba(0xea5c00, 0x55)
}
pub fn find_current_bg() -> Color {
    pick(rgb(0xa8ac94), rgb(0x515c6a))
}
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
pub const GO_TO_PADDING: f64 = crate::ui::menu::GUTTER;
pub const GO_TO_BOX_HEIGHT: f64 = 30.0;
pub const GO_TO_ROW_HEIGHT: f64 = 26.0;
pub const GO_TO_LIST_HEIGHT: f64 = 16.0 * GO_TO_ROW_HEIGHT;
/// The list an `@` puts up under a composer's caret: as wide as a path
/// and a name want, and ten of Cmd-P's rows before it scrolls.
pub const MENTION_WIDTH: f64 = 440.0;
pub const MENTION_ROWS: usize = 10;
/// The letters of a name that a query matched, in Cmd-P's list: blue, as
/// VS Code draws them, and GitHub's blue so it is one Perch already uses.
pub fn match_fg() -> Color {
    pick(rgb(0x0969da), rgb(0x4493f8))
}

/// A test's check that a view goes dark with the system: that after a
/// switch, nothing on screen still holds a colour only the light palette
/// has. A view that took a `Color` when it was built, where it wanted an
/// `Ink`, is what this catches.
#[cfg(test)]
pub(crate) mod dark_check {
    use std::cell::RefCell;
    use std::collections::HashSet;

    use floem::headless::HeadlessHarness;
    use floem::peniko::{Brush, Color};
    use floem::style::{
        Background, BorderBottomColor, BorderLeftColor, BorderRightColor, BorderTopColor,
        BoxShadowProp, OutlineColor, Style, TextColor,
    };
    use floem::ViewId;

    use super::{set_appearance, Appearance};

    type Rgba = [u8; 4];

    thread_local! {
        /// Every light and dark pair a colour has been picked from that
        /// differ, as `pick` served them.
        static SERVED: RefCell<HashSet<(Rgba, Rgba)>> = RefCell::new(HashSet::new());
    }

    fn rgba(color: Color) -> Rgba {
        color.to_rgba8().to_u8_array()
    }

    pub(super) fn served(light: Color, dark: Color) {
        let (light, dark) = (rgba(light), rgba(dark));
        if light != dark {
            SERVED.with(|served| served.borrow_mut().insert((light, dark)));
        }
    }

    /// Puts the colours back to light when the test is done with them,
    /// so a test run on the same thread after it starts in light.
    struct Restore;

    impl Drop for Restore {
        fn drop(&mut self) {
            set_appearance(Appearance::Light);
        }
    }

    /// Switches to dark and lists every view, by its debug names, still
    /// holding a colour that only the light palette has.
    pub(crate) fn light_left(harness: &mut HeadlessHarness) -> Vec<String> {
        let _restore = Restore;
        harness.rebuild();
        harness.paint();
        set_appearance(Appearance::Dark);
        harness.rebuild();
        harness.paint();
        let (lights, darks): (HashSet<Rgba>, HashSet<Rgba>) =
            SERVED.with(|served| served.borrow().iter().copied().unzip());
        let light_only: HashSet<Rgba> = lights.difference(&darks).copied().collect();
        let mut found = Vec::new();
        walk(harness, harness.root_id(), &mut Vec::new(), &light_only, &mut found);
        found
    }

    fn walk(
        harness: &HeadlessHarness,
        id: ViewId,
        names: &mut Vec<String>,
        light_only: &HashSet<Rgba>,
        found: &mut Vec<String>,
    ) {
        let name = id.debug_name();
        let named = !name.is_empty();
        if named {
            names.push(name);
        }
        for (prop, color) in colors(&harness.get_computed_style(id)) {
            let [r, g, b, a] = rgba(color);
            if light_only.contains(&[r, g, b, a]) {
                let at = names.iter().rev().take(4).rev().cloned().collect::<Vec<_>>().join(" > ");
                found.push(format!("{at} [{id:?}]: {prop} #{r:02x}{g:02x}{b:02x}{a:02x}"));
            }
        }
        for child in id.children() {
            walk(harness, child, names, light_only, found);
        }
        if named {
            names.pop();
        }
    }

    fn colors(style: &Style) -> Vec<(&'static str, Color)> {
        let solid = |brush: Option<Brush>| match brush {
            Some(Brush::Solid(color)) => Some(color),
            _ => None,
        };
        let mut out = Vec::new();
        let mut add = |prop, color: Option<Color>| out.extend(color.map(|c| (prop, c)));
        add("background", solid(style.get(Background)));
        add("color", style.get(TextColor));
        add("border-left", solid(style.get(BorderLeftColor)));
        add("border-top", solid(style.get(BorderTopColor)));
        add("border-right", solid(style.get(BorderRightColor)));
        add("border-bottom", solid(style.get(BorderBottomColor)));
        add("outline", solid(Some(style.get(OutlineColor))));
        for shadow in style.get(BoxShadowProp) {
            add("shadow", Some(shadow.color));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};

    use super::*;

    /// Every glyph `text` shapes to in the code font at the body weight.
    fn code_glyphs(text: &str) -> Vec<u32> {
        glyphs(CODE_FONT_STACK, text)
    }

    /// Every glyph `text` shapes to in `stack` at the body weight.
    fn glyphs(stack: &str, text: &str) -> Vec<u32> {
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list(stack).collect();
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

    /// Neither SF face has ∪, and the system's fallback for Latin text,
    /// Helvetica, has none either.
    #[test]
    fn symbols_neither_sf_face_has_are_drawn() {
        install_code_font();
        install_symbol_fallbacks();
        for stack in [FONT_STACK, CODE_FONT_STACK] {
            let ids = glyphs(stack, "A \u{222a} B, \u{2200}x \u{2115}");
            assert_eq!(ids.len(), 11, "{stack}: one glyph a character: {ids:?}");
            assert!(ids.iter().all(|&id| id != 0), "{stack}: a .notdef among {ids:?}");
        }
        let ids = glyphs(FONT_STACK, "\u{3b1} \u{222a} \u{3b2}");
        assert!(ids.iter().all(|&id| id != 0), "Greek: a .notdef among {ids:?}");
    }
}
