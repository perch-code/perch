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
        SubjectState::IssueOpen | SubjectState::PrOpen => theme::OPEN,
        SubjectState::IssueClosed | SubjectState::PrMerged => theme::MERGED,
        SubjectState::PrClosed => theme::CLOSED,
        SubjectState::IssueNotPlanned | SubjectState::PrDraft => theme::DRAFT,
    }
}

/// Icon for a resolved issue or pull request state.
pub fn state_icon(state: SubjectState) -> String {
    let c = hex(state_color(state));
    let body = match state {
        SubjectState::IssueOpen => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <circle cx="8" cy="8" r="2" fill="{c}"/>"#
        ),
        SubjectState::IssueClosed => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M5.1 8.2 L7.1 10.2 L11 5.9" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SubjectState::IssueNotPlanned => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M3.9 12.1 L12.1 3.9" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SubjectState::PrOpen | SubjectState::PrDraft => format!(
            r#"<circle cx="4.5" cy="3.4" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <circle cx="4.5" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M4.5 5.3 V10.7" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="11.6" cy="12.6" r="1.9" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M11.6 10.7 V6.6 A2 2 0 0 0 9.6 4.6 H7.9" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round"/>
               <path d="M9.4 3.1 L7.7 4.6 L9.4 6.1" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
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

/// Sidebar glyphs. Drawn at 16px, tinted by the caller's colour.
pub fn sidebar_icon(name: SidebarIcon, color: Color) -> String {
    let c = hex(color);
    let body = match name {
        SidebarIcon::Bookmark => format!(
            r#"<path d="M4 2.2 h8 v11.6 l-4-3.1 -4 3.1 z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        // The saved state of a row, as opposed to the Saved destination.
        SidebarIcon::BookmarkFilled => format!(
            r#"<path d="M4 2.2 h8 v11.6 l-4-3.1 -4 3.1 z" fill="{c}" stroke="{c}"
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
        SidebarIcon::ChevronLeft => format!(
            r#"<path d="M9.8 3.6 L5.4 8 L9.8 12.4" fill="none" stroke="{c}"
                     stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
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
        // A rule with a chevron pointing at it from each side: the diff
        // folded to its changes, the shape github.com folds with.
        SidebarIcon::Fold => format!(
            r#"<path d="M2.5 8 H13.5" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <path d="M8 2 V5.6 M5.9 3.7 L8 5.8 L10.1 3.7" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
               <path d="M8 14 V10.4 M5.9 12.3 L8 10.2 L10.1 12.3" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
    };
    svg16(&body)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarIcon {
    Bookmark,
    BookmarkFilled,
    Check,
    Undo,
    Comment,
    ChevronRight,
    ChevronLeft,
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
}

impl SidebarIcon {
    /// Every glyph, so a test can check them all and a new one cannot be
    /// added without being drawn.
    #[cfg(test)]
    pub const ALL: [SidebarIcon; 30] = [
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
        SidebarIcon::Thread,
        SidebarIcon::Branch,
        SidebarIcon::Trash,
        SidebarIcon::Ellipsis,
        SidebarIcon::Folder,
        SidebarIcon::Bookmark,
        SidebarIcon::BookmarkFilled,
        SidebarIcon::Check,
        SidebarIcon::Undo,
        SidebarIcon::Comment,
        SidebarIcon::ChevronRight,
        SidebarIcon::ChevronLeft,
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
            open.replace(&hex(theme::OPEN), "X"),
            draft.replace(&hex(theme::DRAFT), "X")
        );
    }

    #[test]
    fn every_sidebar_glyph_is_drawn() {
        for icon in SidebarIcon::ALL {
            let svg = sidebar_icon(icon, theme::FG);
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
