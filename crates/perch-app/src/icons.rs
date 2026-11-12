//! Vector icons, built from primitives rather than pasted path data.
//!
//! Colour is baked into each string at build time, which avoids depending on
//! how `currentColor` resolves inside floem's SVG view.

use floem::peniko::Color;
use perch_github::SubjectState;

use crate::theme;

/// Wrap a fragment in a 16x16 SVG envelope.
/// Every icon is drawn in a single colour.
///
/// floem paints an SVG with the view's inherited text colour, overriding any
/// `fill` in the markup, so a two-colour glyph collapses into one. The colour
/// is still written into the markup here, because it is what the tests read
/// and what a renderer without that override would use; the view sets the
/// same colour on itself so the two agree.
fn svg16(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">{body}</svg>"#
    )
}

fn hex(c: Color) -> String {
    let [r, g, b, _] = c.to_rgba8().to_u8_array();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The colour a state's icon and its number are drawn in.
pub fn state_color(state: SubjectState) -> Color {
    match state {
        SubjectState::IssueOpen | SubjectState::PrOpen => theme::open(),
        SubjectState::IssueClosed | SubjectState::PrMerged => theme::merged(),
        SubjectState::PrClosed => theme::closed(),
        SubjectState::IssueNotPlanned | SubjectState::PrDraft => theme::draft(),
    }
}

/// Icon for a resolved issue or pull request state.
pub fn state_icon(state: SubjectState) -> String {
    let c = hex(state_color(state));
    let body = match state {
        SubjectState::IssueOpen => issue_body(&c),
        SubjectState::IssueClosed => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M5.1 8.2 L7.1 10.2 L11 5.9" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SubjectState::IssueNotPlanned => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M3.9 12.1 L12.1 3.9" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SubjectState::PrOpen | SubjectState::PrDraft => pull_request_body(&c),
        SubjectState::PrMerged => format!(
            r#"<circle cx="4.5" cy="3.4" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <circle cx="4.5" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <circle cx="11.6" cy="8" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M4.5 5.3 V10.7" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <path d="M4.9 5.9 C5.4 7.4 6.9 8 9.6 8" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SubjectState::PrClosed => format!(
            r#"<circle cx="4.5" cy="3.4" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <circle cx="4.5" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M4.5 5.3 V10.7" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="11.6" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M11.6 10.7 V8.4" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <path d="M9.9 3.0 L13.3 6.4 M13.3 3.0 L9.9 6.4" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
    };
    svg16(&body)
}

/// An open pull request: a branch with an arrow curving into it. The
/// state glyph for open and draft, and the summary's Status glyph in the
/// caller's colour.
fn pull_request_body(c: &str) -> String {
    format!(
        r#"<circle cx="4.5" cy="3.4" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
           <circle cx="4.5" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
           <path d="M4.5 5.3 V10.7" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
           <circle cx="11.6" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
           <path d="M11.6 10.7 V6.6 A2 2 0 0 0 9.6 4.6 H7.9" fill="none" stroke="{c}"
                 stroke-width="1.5" stroke-linecap="round"/>
           <path d="M9.4 3.1 L7.7 4.6 L9.4 6.1" fill="none" stroke="{c}"
                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
    )
}

/// An open issue: a ring with a dot. The state glyph for open, and the
/// summary's Closes glyph in the caller's colour.
fn issue_body(c: &str) -> String {
    format!(
        r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
           <circle cx="8" cy="8" r="2" fill="{c}"/>"#
    )
}

/// Sidebar glyphs. Drawn at 16px, tinted by the caller's colour.
pub fn sidebar_icon(name: SidebarIcon, color: Color) -> String {
    let c = hex(color);
    let body = match name {
        SidebarIcon::Bookmark => format!(
            r#"<path d="M4 2.2 h8 v11.6 l-4-3.1 -4 3.1 z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        // Refresh, mirrored: the same gesture running the other way.
        SidebarIcon::Undo => format!(
            r#"<path d="M2.8 8 a5.2 5.2 0 1 0 1.6 -3.7" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>
               <path d="M2.6 1.8 v3.1 h3.1" fill="none" stroke="{c}" stroke-width="1.5"
                     stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Check => format!(
            r#"<path d="M3.2 8.4 L6.4 11.6 L12.8 4.6" fill="none" stroke="{c}"
                     stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Comment => format!(
            r#"<path d="M2.4 3.4 h11.2 v7.1 h-5.5 l-3.1 2.6 v-2.6 h-2.6 z" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Search => format!(
            r#"<circle cx="7" cy="7" r="4.4" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M10.4 10.4 L14 14" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Refresh => format!(
            r#"<path d="M13.2 8 a5.2 5.2 0 1 1 -1.6 -3.7" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>
               <path d="M13.4 1.8 v3.1 h-3.1" fill="none" stroke="{c}" stroke-width="1.5"
                     stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        // Close, smaller and ringed: empty a field.
        SidebarIcon::Clear => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M5.8 5.8 L10.2 10.2 M10.2 5.8 L5.8 10.2" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Close => format!(
            r#"<path d="M4.4 4.4 L11.6 11.6 M11.6 4.4 L4.4 11.6" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SidebarIcon::ChevronRight => format!(
            r#"<path d="M6.2 3.6 L10.6 8 L6.2 12.4" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::ChevronUp => format!(
            r#"<path d="M3.6 9.8 L8 5.4 L12.4 9.8" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::ChevronDown => format!(
            r#"<path d="M3.6 6.2 L8 10.6 L12.4 6.2" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Branch => format!(
            r#"<circle cx="4.5" cy="3.5" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="4.5" cy="12.5" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="11.5" cy="5" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M4.5 5.2 V10.8" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>
               <path d="M11.5 6.7 C11.5 9.6 4.5 8.3 4.5 10.8" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        // An arrow curling back to the left: a reply under someone's comment.
        SidebarIcon::Reply => format!(
            r#"<path d="M6.6 3.4 L2.6 7.2 L6.6 11" fill="none" stroke="{c}" stroke-width="1.5"
                     stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M2.8 7.2 H9.2 a4 4 0 0 1 4 4 V13" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        // The text styles over a composer, drawn after GitHub's: a letter
        // for each of the three that change a word, and for the rest a
        // few lines with what starts each one.
        SidebarIcon::Heading => format!(
            r#"<path d="M3.8 2.8 V13.2 M12.2 2.8 V13.2 M3.8 8 H12.2" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Bold => format!(
            r#"<path d="M4.5 2.9 H8.7 a2.55 2.55 0 0 1 0 5.1 H4.5 Z M4.5 8 H9.4 a2.55 2.55 0 0 1 0 5.1 H4.5 Z"
                     fill="none" stroke="{c}" stroke-width="1.9" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Italic => format!(
            r#"<path d="M7 2.8 H12.2 M3.8 13.2 H9 M9.6 2.8 L6.4 13.2" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Quote => format!(
            r#"<path d="M2.8 4 V12 M6.2 4 H13.2 M6.2 8 H13.2 M6.2 12 H11" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SidebarIcon::NumberedList => format!(
            r#"<path d="M7 4 H13.5 M7 8 H13.5 M7 12 H13.5" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>
               <path d="M2.7 3.1 L3.7 2.5 V6.4" fill="none" stroke="{c}" stroke-width="1.1"
                     stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M2.5 10.1 a1.05 1.05 0 0 1 2.1 0.1 c0 0.8 -2.1 1.6 -2.1 3 H4.7" fill="none"
                     stroke="{c}" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::BulletList => format!(
            r#"<circle cx="3.3" cy="4" r="1.05" fill="{c}"/>
               <circle cx="3.3" cy="8" r="1.05" fill="{c}"/>
               <circle cx="3.3" cy="12" r="1.05" fill="{c}"/>
               <path d="M6.5 4 H13.5 M6.5 8 H13.5 M6.5 12 H13.5" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        // A page with a plus over a minus: suggest a change to the lines,
        // after GitHub's file-diff.
        SidebarIcon::Suggest => format!(
            r#"<path d="M3.2 2.8 a1 1 0 0 1 1 -1 H9.6 L12.8 5 V13.2 a1 1 0 0 1 -1 1 H4.2 a1 1 0 0 1 -1 -1 Z"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M8 4.8 V9 M5.9 6.9 H10.1 M5.9 11.3 H10.1" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::TaskList => format!(
            r#"<path d="M2.2 2.4 H6.2 V6.4 H2.2 Z M2.2 9.6 H6.2 V13.6 H2.2 Z" fill="none" stroke="{c}"
                     stroke-width="1.2" stroke-linejoin="round"/>
               <path d="M3.1 4.4 L3.9 5.2 L5.4 3.5" fill="none" stroke="{c}" stroke-width="1.1"
                     stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M8.5 4.4 H13.8 M8.5 11.6 H13.8" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        // A pencil at a slant: edit.
        SidebarIcon::Pencil => format!(
            r#"<path d="M3 13 l0.9 -3.4 l7.2 -7.2 a1.2 1.2 0 0 1 1.7 0 l0.8 0.8 a1.2 1.2 0 0 1 0 1.7 l-7.2 7.2 z"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M9.9 3.6 l2.5 2.5" fill="none" stroke="{c}" stroke-width="1.4"/>"#
        ),
        // Two pages, one behind the other: copy.
        SidebarIcon::Copy => format!(
            r#"<rect x="5.7" y="5.7" width="7.6" height="7.6" rx="1.3" fill="none" stroke="{c}"
                     stroke-width="1.4"/>
               <path d="M10.3 4.2 V3.7 a1 1 0 0 0 -1 -1 H3.7 a1 1 0 0 0 -1 1 V9.3 a1 1 0 0 0 1 1 H4.2"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Trash => format!(
            r#"<path d="M2.8 4.3 H13.2" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>
               <path d="M6.2 4.3 V2.9 a0.7 0.7 0 0 1 0.7 -0.7 H9.1 a0.7 0.7 0 0 1 0.7 0.7 V4.3"
                     fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M4 4.3 L4.7 12.6 a1 1 0 0 0 1 0.9 H10.3 a1 1 0 0 0 1 -0.9 L12 4.3"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M6.7 6.8 V11 M9.3 6.8 V11" stroke="{c}" stroke-width="1.3"
                     stroke-linecap="round"/>"#
        ),
        SidebarIcon::Ellipsis => format!(
            r#"<circle cx="3.5" cy="8" r="1.3" fill="{c}"/>
               <circle cx="8" cy="8" r="1.3" fill="{c}"/>
               <circle cx="12.5" cy="8" r="1.3" fill="{c}"/>"#
        ),
        SidebarIcon::ArrowUpRight => format!(
            r#"<path d="M4.5 11.5 L11.5 4.5 M5.8 4.5 H11.5 V10.2" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Terminal => format!(
            r#"<rect x="2" y="2.8" width="12" height="10.4" rx="2" fill="none" stroke="{c}"
                     stroke-width="1.4"/>
               <path d="M4.8 6.2 L6.8 8 L4.8 9.8 M8.2 10 H11" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Unread => format!(
            r#"<path d="M1.6 8 C3 5.2 5.3 3.7 8 3.7 S13 5.2 14.4 8 C13 10.8 10.7 12.3 8 12.3
                       S3 10.8 1.6 8 Z" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round"/>
               <circle cx="8" cy="8" r="2" fill="none" stroke="{c}" stroke-width="1.4"/>"#
        ),
        SidebarIcon::Archive => format!(
            r#"<rect x="2" y="2.8" width="12" height="3.2" rx="1" fill="none" stroke="{c}"
                     stroke-width="1.4"/>
               <path d="M3.2 6 V12 a1.2 1.2 0 0 0 1.2 1.2 H11.6 a1.2 1.2 0 0 0 1.2 -1.2 V6
                       M6.5 8.6 H9.5" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::BellOff => format!(
            r#"<path d="M4.2 10.8 V7.2 a3.8 3.8 0 0 1 6.4 -2.8 M11.8 7.2 V10.8 L12.8 11.8 H5"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linecap="round"
                     stroke-linejoin="round"/>
               <path d="M6.6 13.6 H9.4 M2.5 13.5 L13.5 2.5" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Inbox => format!(
            r#"<path d="M2 8.6 L3.8 3.6 a1 1 0 0 1 0.9 -0.6 H11.3 a1 1 0 0 1 0.9 0.6 L14 8.6 V12
                       a1 1 0 0 1 -1 1 H3 a1 1 0 0 1 -1 -1 Z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M2 8.6 H5.4 L6.4 10.2 H9.6 L10.6 8.6 H14" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Clock => format!(
            r#"<circle cx="8" cy="8" r="6.2" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M8 4.6 V8 L10.4 9.6" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Sort => format!(
            r#"<path d="M5 13 V3 M2.6 5.4 L5 3 L7.4 5.4 M11 3 V13 M8.6 10.6 L11 13 L13.4 10.6"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linecap="round"
                     stroke-linejoin="round"/>"#
        ),
        SidebarIcon::File => format!(
            r#"<path d="M4.2 2 H9.4 L12.6 5.2 V13 a1 1 0 0 1 -1 1 H4.2 a1 1 0 0 1 -1 -1 V3 a1 1 0 0 1 1 -1 Z"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M9.2 2.2 V5.4 H12.4" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Folder => format!(
            r#"<path d="M2 4.2 a1 1 0 0 1 1 -1 H6.2 L7.6 4.8 H13 a1 1 0 0 1 1 1 V12 a1 1 0 0 1 -1 1
                       H3 a1 1 0 0 1 -1 -1 Z" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Plus => format!(
            r#"<path d="M8 3.2 V12.8 M3.2 8 H12.8" stroke="{c}" stroke-width="1.7"
                     stroke-linecap="round"/>"#
        ),
        SidebarIcon::Stop => format!(
            r#"<path d="M5.5 4 H10.5 A1.5 1.5 0 0 1 12 5.5 V10.5 A1.5 1.5 0 0 1 10.5 12 H5.5
                       A1.5 1.5 0 0 1 4 10.5 V5.5 A1.5 1.5 0 0 1 5.5 4 Z" fill="{c}"/>"#
        ),
        // Three connected points: one coordinator and two workers. Kept
        // abstract so it reads as activity rather than either vendor's logo.
        SidebarIcon::Agents => format!(
            r#"<circle cx="8" cy="4" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="4" cy="11.5" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="12" cy="11.5" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M7.1 5.5 L4.9 10 M8.9 5.5 L11.1 10 M5.8 11.5 H10.2" fill="none"
                     stroke="{c}" stroke-width="1.3" stroke-linecap="round"/>"#
        ),
        // A dial's 240° arc with its needle past the middle. The hub sits
        // low so the drawing's box, caps included, is centred on 8,8.
        SidebarIcon::Gauge => format!(
            r#"<path d="M2.98 12.35 A5.8 5.8 0 1 1 13.02 12.35" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>
               <path d="M8 9.45 L10.5 6.95" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linecap="round"/>
               <circle cx="8" cy="9.45" r="1.3" fill="{c}"/>"#
        ),
        // A megaphone: a back block, a cone out to an open oval mouth, and
        // a handle under the block. The oval is what keeps it from reading
        // as a speaker. Send a bug report or an idea.
        SidebarIcon::Feedback => format!(
            r#"<ellipse cx="12.2" cy="7.4" rx="1.7" ry="4.8" fill="none" stroke="{c}"
                        stroke-width="1.4"/>
               <path d="M12.2 2.6 L5.3 5.5 H3.2 A1 1 0 0 0 2.2 6.5 V8.3 A1 1 0 0 0 3.2 9.3 H5.3
                        L12.2 12.2 M5.3 5.5 V9.3" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round" stroke-linecap="round"/>
               <path d="M5.5 9.5 L6.2 12.9 A0.9 0.9 0 0 0 7.9 12.5 L7.6 10.4" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linecap="round"
                     stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Thread => format!(
            r#"<path d="M2.4 3.4 h11.2 v7.1 h-5.5 l-3.1 2.6 v-2.6 h-2.6 z" fill="{c}"
                     stroke="{c}" stroke-width="1.2" stroke-linejoin="round"/>"#
        ),
        // The same shape, hollow: a comment from GitHub, not a thread of
        // the reader's own.
        SidebarIcon::ThreadHollow => format!(
            r#"<path d="M2.4 3.4 h11.2 v7.1 h-5.5 l-3.1 2.6 v-2.6 h-2.6 z" fill="none"
                     stroke="{c}" stroke-width="1.3" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Pin => format!(
            r#"<path d="M6 2.5 H10 L9.3 6.2 L11.6 8.6 V9.6 H8.6 V13.8 L8 14.4 L7.4 13.8 V9.6 H4.4 V8.6
                       L6.7 6.2 Z" fill="none" stroke="{c}" stroke-width="1.3" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Filter => format!(
            r#"<path d="M2.5 4 H13.5 M4.5 8 H11.5 M6.5 12 H9.5" stroke="{c}" stroke-width="1.6"
                     stroke-linecap="round"/>"#
        ),
        SidebarIcon::Warning => format!(
            r#"<path d="M8 2.2 L14.6 13.4 H1.4 Z" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round"/>
               <path d="M8 6.2 V9.3" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="8" cy="11.3" r="0.95" fill="{c}"/>"#
        ),
        // A circle with an i in it: a note alert.
        SidebarIcon::Note => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M8 7.2 V11.2" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="8" cy="4.9" r="0.95" fill="{c}"/>"#
        ),
        // A bulb on a stem: a tip alert.
        SidebarIcon::Tip => format!(
            r#"<path d="M5.2 9.6 a4 4 0 1 1 5.6 0 L10.2 10.6 V11.6 H5.8 V10.6 Z" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M6.4 13.6 H9.6" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        // A speech bubble with a mark in it: an important alert.
        SidebarIcon::Important => format!(
            r#"<path d="M2.4 2.6 h11.2 v8.2 h-4.6 l-3.1 2.8 v-2.8 h-3.5 z" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M8 4.8 V7.4" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="8" cy="9.2" r="0.95" fill="{c}"/>"#
        ),
        // An octagon with a mark in it: a caution alert.
        SidebarIcon::Caution => format!(
            r#"<path d="M5.5 1.8 H10.5 L14.2 5.5 V10.5 L10.5 14.2 H5.5 L1.8 10.5 V5.5 Z" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M8 4.8 V8.4" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="8" cy="10.9" r="0.95" fill="{c}"/>"#
        ),
        // An empty circle: a job not started yet, as GitHub draws one.
        SidebarIcon::Waiting => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>"#
        ),
        // A circle with a slash through it: a run that was called off, as
        // GitHub draws a skipped check.
        SidebarIcon::Skip => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M3.9 12.1 L12.1 3.9" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        // Two links of a chain, on the diagonal: a permalink.
        SidebarIcon::Link => format!(
            r#"<path d="M6.3 5.5 L8.4 3.4 a3 3 0 0 1 4.2 4.2 L10.5 9.7" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M9.7 10.5 L7.6 12.6 a3 3 0 0 1 -4.2 -4.2 L5.5 6.3" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M6.2 9.8 L9.8 6.2" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        // A page with a plus and a minus on it: a review that asked for
        // changes.
        SidebarIcon::ChangesRequested => format!(
            r#"<path d="M3.8 2.4 H9.6 L12.2 5 V13.6 H3.8 Z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M8 5.6 V8.4 M6.6 7 H9.4" stroke="{c}" stroke-width="1.3" stroke-linecap="round"/>
               <path d="M6.6 10.9 H9.4" stroke="{c}" stroke-width="1.3" stroke-linecap="round"/>"#
        ),
        // A signpost: a milestone.
        SidebarIcon::Milestone => format!(
            r#"<path d="M2.4 4.4 H11.2 L13.8 7 L11.2 9.6 H2.4 Z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M6.6 2 V4.4 M6.6 9.6 V14" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        // The two layouts are the same frame, cut across or cut down.
        SidebarIcon::Stacked => format!(
            r#"<path d="M3.7 2.7 H12.3 a1.5 1.5 0 0 1 1.5 1.5 V11.8 a1.5 1.5 0 0 1 -1.5 1.5 H3.7
                       a1.5 1.5 0 0 1 -1.5 -1.5 V4.2 a1.5 1.5 0 0 1 1.5 -1.5 Z" fill="none"
                     stroke="{c}" stroke-width="1.4"/>
               <path d="M2.2 8 H13.8" stroke="{c}" stroke-width="1.4"/>"#
        ),
        SidebarIcon::Split => format!(
            r#"<path d="M3.7 2.7 H12.3 a1.5 1.5 0 0 1 1.5 1.5 V11.8 a1.5 1.5 0 0 1 -1.5 1.5 H3.7
                       a1.5 1.5 0 0 1 -1.5 -1.5 V4.2 a1.5 1.5 0 0 1 1.5 -1.5 Z" fill="none"
                     stroke="{c}" stroke-width="1.4"/>
               <path d="M8 2.7 V13.3" stroke="{c}" stroke-width="1.4"/>"#
        ),
        // Two angle brackets, as github.com's code glyph: the markup
        // behind a picture.
        SidebarIcon::Source => format!(
            r#"<path d="M5.6 4.4 L2.2 8 L5.6 11.6 M10.4 4.4 L13.8 8 L10.4 11.6" fill="none"
                     stroke="{c}" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        // A rule with a chevron pointing at it from each side: the diff
        // folded to its changes, the shape github.com folds with.
        SidebarIcon::Fold => format!(
            r#"<path d="M2.5 8 H13.5" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <path d="M8 2 V5.6 M5.9 3.7 L8 5.8 L10.1 3.7" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M8 14 V10.4 M5.9 12.3 L8 10.2 L10.1 12.3" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        // A line, then one that runs to the edge and turns back under
        // itself, an arrow at its end, onto a short third: word wrap, as
        // editors draw it.
        SidebarIcon::Wrap => format!(
            r#"<path d="M2.5 4 H13.5 M2.5 12 H5.5" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <path d="M2.5 8 H11.2 a2 2 0 0 1 0 4 H8.6" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>
               <path d="M10.2 10.4 L8.6 12 L10.2 13.6" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        // The fold opened: the rule dotted, and the chevrons pointing away
        // from it, as github.com draws Show resolved.
        SidebarIcon::Unfold => format!(
            r#"<path d="M2.5 8 H3.7 M6.4 8 H9.6 M12.3 8 H13.5" stroke="{c}" stroke-width="1.5"
                     stroke-linecap="round"/>
               <path d="M8 5.6 V2.2 M5.9 4.1 L8 2 L10.1 4.1" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M8 10.4 V13.8 M5.9 11.9 L8 14 L10.1 11.9" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        // github.com's expanders in a diff's hunk rows: a dotted rule for
        // the code already shown, and an arrow away from it, stem and a
        // solid head, for the way the code grows. The dots are squares of
        // their own, since vger draws a dashed stroke solid, and the dots
        // and the stem sit on whole pixels, so a 1x screen draws them
        // sharp.
        SidebarIcon::ExpandDown => format!(
            r#"{}<path d="M8 6.5 V10.5" stroke="{c}" stroke-width="2"/>
               <path d="M4 10 H12 L8 14 Z" fill="{c}"/>"#,
            expander_dots(&c, 3.0)
        ),
        SidebarIcon::ExpandUp => format!(
            r#"{}<path d="M8 9.5 V5.5" stroke="{c}" stroke-width="2"/>
               <path d="M4 6 H12 L8 2 Z" fill="{c}"/>"#,
            expander_dots(&c, 11.0)
        ),
        SidebarIcon::ExpandAll => format!(
            r#"{}<path d="M8 6 V4 M8 10 V12" stroke="{c}" stroke-width="2"/>
               <path d="M4.5 4.5 H11.5 L8 1 Z M4.5 11.5 H11.5 L8 15 Z" fill="{c}"/>"#,
            expander_dots(&c, 7.0)
        ),
        // A frame with a rule down its left third: the drawer, as Codex and
        // the system draw the sidebar toggle.
        SidebarIcon::Drawer => format!(
            r#"<path d="M3.7 2.7 H12.3 a1.5 1.5 0 0 1 1.5 1.5 V11.8 a1.5 1.5 0 0 1 -1.5 1.5 H3.7
                       a1.5 1.5 0 0 1 -1.5 -1.5 V4.2 a1.5 1.5 0 0 1 1.5 -1.5 Z" fill="none"
                     stroke="{c}" stroke-width="1.4"/>
               <path d="M6.3 2.7 V13.3" stroke="{c}" stroke-width="1.4"/>"#
        ),
        SidebarIcon::Back => format!(
            r#"<path d="M13 8 H3.4 M7.2 4.2 L3.4 8 L7.2 11.8" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Forward => format!(
            r#"<path d="M3 8 H12.6 M8.8 4.2 L12.6 8 L8.8 11.8" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        // A box with an arrow leaving its top right corner: open elsewhere.
        SidebarIcon::External => format!(
            r#"<path d="M7 3.5 H4.2 a1.5 1.5 0 0 0 -1.5 1.5 V11.8 a1.5 1.5 0 0 0 1.5 1.5 H10.5
                       a1.5 1.5 0 0 0 1.5 -1.5 V9" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M9.4 2.5 H13.5 V6.6 M13.5 2.5 L7.6 8.4" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Person => format!(
            r#"<circle cx="8" cy="5.3" r="2.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M2.8 13.6 C2.8 10.6 5.1 9.1 8 9.1 S13.2 10.6 13.2 13.6" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Tag => format!(
            r#"<path d="M2.2 2.2 H7.6 L13.8 8.4 L8.4 13.8 L2.2 7.6 Z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <circle cx="5.4" cy="5.4" r="1.1" fill="{c}"/>"#
        ),
        // A face: leave a reaction.
        SidebarIcon::Smiley => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="5.7" cy="6.3" r="0.95" fill="{c}"/>
               <circle cx="10.3" cy="6.3" r="0.95" fill="{c}"/>
               <path d="M5.1 9.5 C5.7 11 6.9 11.7 8 11.7 C9.1 11.7 10.3 11 10.9 9.5" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::PullRequest => pull_request_body(&c),
        SidebarIcon::Issue => issue_body(&c),
        // GitHub's merge glyph: a line between two commits, and a third
        // off to the side joining it.
        SidebarIcon::Merge => format!(
            r#"<circle cx="4.5" cy="3.6" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="4.5" cy="12.4" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="11.6" cy="8.6" r="1.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M4.5 5.3 V10.7 M4.5 5.3 C4.5 7.6 6.6 8.6 9.9 8.6" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Repository => format!(
            r#"<path d="M4.6 1.9 H13 V11.6 H4.8 a1.6 1.6 0 0 0 0 3.2 H13 M3.2 13.2 V3.3
                       A1.4 1.4 0 0 1 4.6 1.9 M13 11.6 V14.8" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
    };
    svg16(&body)
}

/// The dotted rule of an expander glyph: five two-pixel dots across the
/// glyph with a pixel between, their tops at `top`.
fn expander_dots(c: &str, top: f64) -> String {
    [1.0, 4.0, 7.0, 10.0, 13.0]
        .iter()
        .map(|x| format!(r#"<rect x="{x}" y="{top}" width="2" height="2" rx="0.5" fill="{c}"/>"#))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarIcon {
    Bookmark,
    Check,
    Undo,
    Comment,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    Close,
    Search,
    Refresh,
    Warning,
    /// A branch forking off a line: the worktrees section.
    Branch,
    Trash,
    /// Three dots: more actions.
    Ellipsis,
    Folder,
    Plus,
    /// A filled square: stop.
    Stop,
    /// Three connected points: app-wide agent activity.
    Agents,
    /// A dial with its needle: how much of a plan's limits is used.
    Gauge,
    /// A speech bubble with a dot: a thread anchored here.
    Thread,
    Pin,
    /// Three lines of decreasing length: the filter menu.
    Filter,
    /// Two links of a chain: a permalink.
    Link,
    /// A speech bubble drawn hollow: a review comment from GitHub.
    ThreadHollow,
    /// A page with a plus and a minus: a review that requested changes.
    ChangesRequested,
    /// A signpost: a milestone.
    Milestone,
    /// A frame cut across: the head with the removed lines spliced in.
    Stacked,
    /// A frame cut down: base beside head.
    Split,
    /// A rule with a chevron pointing at it from above and below: a diff
    /// folded to its changes.
    Fold,
    /// The same rule dotted, with the chevrons pointing away from it:
    /// something folded, to open.
    Unfold,
    /// A line turning back under itself: long lines wrapped.
    Wrap,
    /// A dotted rule with an arrow down from it: a divider's Down, which
    /// grows the code above it into the hidden lines.
    ExpandDown,
    /// A dotted rule with an arrow up to it: a divider's Up, which grows
    /// the code below it into the hidden lines.
    ExpandUp,
    /// A dotted rule with arrows up and down from it: a divider's All,
    /// which opens every hidden line.
    ExpandAll,
    /// A frame with a rule down its left third: the drawer toggle.
    Drawer,
    /// Arrows left and right: Back and Forward.
    Back,
    Forward,
    /// A box with an arrow leaving it: open on GitHub.
    External,
    /// A head and shoulders: the reviewers and the assignees.
    Person,
    /// A luggage tag: the labels.
    Tag,
    /// The pull request glyph in the caller's colour, for a row's name
    /// rather than a state.
    PullRequest,
    /// The issue glyph in the caller's colour, for a row's name rather
    /// than a state.
    Issue,
    /// A book: a repository, for a branch's Repository row.
    Repository,
    /// A circle with an i in it: a note alert.
    Note,
    /// A bulb on a stem: a tip alert.
    Tip,
    /// A speech bubble with a mark in it: an important alert.
    Important,
    /// An octagon with a mark in it: a caution alert.
    Caution,
    /// A circle with a slash through it: a run that was cancelled.
    Skip,
    /// An empty circle: a check that has not started.
    Waiting,
    /// A face: leave a reaction.
    Smiley,
    /// Two angle brackets: a file's markup, for the tile that shows a
    /// picture's source.
    Source,
    /// A pencil at a slant: edit.
    Pencil,
    /// Two pages, one behind the other: copy.
    Copy,
    /// An arrow curling back: a reply.
    Reply,
    /// A cross in a ring: empty the search box.
    Clear,
    /// An H, a B and an I: a heading, bold and italics.
    Heading,
    Bold,
    Italic,
    /// Lines behind a rule: a quote.
    Quote,
    /// Lines with a 1 and a 2, with dots, and with boxes: the three lists.
    NumberedList,
    BulletList,
    TaskList,
    /// A page with a plus over a minus: suggest a change.
    Suggest,
    /// An arrow up and to the right: open somewhere else.
    ArrowUpRight,
    /// A prompt and a cursor in a window: a terminal.
    Terminal,
    /// An eye: mark unread, to look at again.
    Unread,
    /// A box with a lid: put a worktree away.
    Archive,
    /// A bell struck through: stop hearing about it.
    BellOff,
    /// A tray: the inbox.
    Inbox,
    /// Arrows up and down: the order.
    Sort,
    /// A clock face: snooze.
    Clock,
    /// A page with its corner turned down: a file an agent read.
    File,
    /// A megaphone: send feedback, a bug or an idea.
    Feedback,
    /// A branch joining a line: whether a pull request merges cleanly.
    Merge,
}

impl SidebarIcon {
    /// Every glyph, so a test can check them all and a new one cannot be
    /// added without being drawn.
    #[cfg(test)]
    pub const ALL: [SidebarIcon; 74] = [
        SidebarIcon::Merge,
        SidebarIcon::Feedback,
        SidebarIcon::Waiting,
        SidebarIcon::File,
        SidebarIcon::Clock,
        SidebarIcon::ArrowUpRight,
        SidebarIcon::Terminal,
        SidebarIcon::Unread,
        SidebarIcon::Archive,
        SidebarIcon::BellOff,
        SidebarIcon::Inbox,
        SidebarIcon::Sort,
        SidebarIcon::Suggest,
        SidebarIcon::Heading,
        SidebarIcon::Bold,
        SidebarIcon::Italic,
        SidebarIcon::Quote,
        SidebarIcon::NumberedList,
        SidebarIcon::BulletList,
        SidebarIcon::TaskList,
        SidebarIcon::Clear,
        SidebarIcon::Unfold,
        SidebarIcon::Wrap,
        SidebarIcon::ExpandDown,
        SidebarIcon::ExpandUp,
        SidebarIcon::ExpandAll,
        SidebarIcon::Copy,
        SidebarIcon::Reply,
        SidebarIcon::Pencil,
        SidebarIcon::Source,
        SidebarIcon::Smiley,
        SidebarIcon::Skip,
        SidebarIcon::Person,
        SidebarIcon::Tag,
        SidebarIcon::PullRequest,
        SidebarIcon::Issue,
        SidebarIcon::Repository,
        SidebarIcon::Note,
        SidebarIcon::Tip,
        SidebarIcon::Important,
        SidebarIcon::Caution,
        SidebarIcon::Drawer,
        SidebarIcon::Back,
        SidebarIcon::Forward,
        SidebarIcon::External,
        SidebarIcon::Link,
        SidebarIcon::ThreadHollow,
        SidebarIcon::ChangesRequested,
        SidebarIcon::Milestone,
        SidebarIcon::Stacked,
        SidebarIcon::Split,
        SidebarIcon::Fold,
        SidebarIcon::Pin,
        SidebarIcon::Filter,
        SidebarIcon::Plus,
        SidebarIcon::Stop,
        SidebarIcon::Agents,
        SidebarIcon::Gauge,
        SidebarIcon::Thread,
        SidebarIcon::Branch,
        SidebarIcon::Trash,
        SidebarIcon::Ellipsis,
        SidebarIcon::Folder,
        SidebarIcon::Bookmark,
        SidebarIcon::Check,
        SidebarIcon::Undo,
        SidebarIcon::Comment,
        SidebarIcon::ChevronRight,
        SidebarIcon::ChevronDown,
        SidebarIcon::ChevronUp,
        SidebarIcon::Close,
        SidebarIcon::Search,
        SidebarIcon::Refresh,
        SidebarIcon::Warning,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_produces_a_well_formed_icon() {
        for s in [
            SubjectState::IssueOpen,
            SubjectState::IssueClosed,
            SubjectState::IssueNotPlanned,
            SubjectState::PrOpen,
            SubjectState::PrDraft,
            SubjectState::PrMerged,
            SubjectState::PrClosed,
        ] {
            let svg = state_icon(s);
            assert!(svg.starts_with("<svg"), "{s:?}");
            assert!(svg.ends_with("</svg>"), "{s:?}");
            assert!(svg.contains("viewBox=\"0 0 16 16\""), "{s:?}");
        }
    }

    #[test]
    fn draft_and_open_prs_differ_only_in_colour() {
        // Same geometry, different tint, which is how GitHub distinguishes them.
        let open = state_icon(SubjectState::PrOpen);
        let draft = state_icon(SubjectState::PrDraft);
        assert_ne!(open, draft);
        assert_eq!(
            open.replace(&hex(theme::open()), "X"),
            draft.replace(&hex(theme::draft()), "X")
        );
    }

    #[test]
    fn every_sidebar_glyph_is_drawn() {
        for icon in SidebarIcon::ALL {
            let svg = sidebar_icon(icon, theme::fg());
            assert!(svg.starts_with("<svg"), "{icon:?}");
            assert!(svg.ends_with("</svg>"), "{icon:?}");
            assert!(svg.contains("viewBox=\"0 0 16 16\""), "{icon:?}");
            // A glyph with no geometry renders as an empty square, which is
            // indistinguishable from a missing icon.
            assert!(svg.contains("<path") || svg.contains("<circle"), "{icon:?}");
        }
    }

    #[test]
    fn the_sidebar_icon_list_is_complete() {
        // A duplicate here would mean a variant was added to the list twice
        // and another left out, which the length check alone would not catch.
        let mut seen = std::collections::HashSet::new();
        for icon in SidebarIcon::ALL {
            assert!(seen.insert(format!("{icon:?}")), "{icon:?} listed twice");
        }
    }

    #[test]
    fn colours_render_as_six_digit_hex() {
        assert_eq!(hex(Color::from_rgb8(0x09, 0x69, 0xda)), "#0969da");
    }
}
