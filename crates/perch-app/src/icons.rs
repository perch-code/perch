//! Vector icons, built from primitives rather than pasted path data.
//!
//! Colour is baked into each string at build time, which avoids depending on
//! how `currentColor` resolves inside floem's SVG view.

use floem::peniko::Color;
use perch_github::{SubjectKind, SubjectState};

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

/// Icon for a thread whose detail has not arrived, or whose subject is not an
/// issue or pull request.
pub fn kind_icon(kind: &SubjectKind) -> String {
    let c = hex(theme::FG_SUBTLE);
    let body = match kind {
        // Detail pending. A dashed ring reads as "not known yet" without
        // implying a state the row does not have.
        SubjectKind::Issue | SubjectKind::PullRequest => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.4"
                       stroke-dasharray="2.4 2.6" stroke-linecap="round"/>"#
        ),
        SubjectKind::Discussion => format!(
            r#"<path d="M2.4 3.6 h11.2 v7 h-5.4 l-3.2 2.6 v-2.6 h-2.6 z" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        SubjectKind::Commit => format!(
            r#"<circle cx="8" cy="8" r="3" fill="none" stroke="{c}" stroke-width="1.5"/>
               <path d="M1.8 8 H5 M11 8 H14.2" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>"#
        ),
        SubjectKind::Release => format!(
            r#"<path d="M8.4 1.9 H14.1 V7.6 L7.9 13.8 L2.2 8.1 Z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <circle cx="11.3" cy="4.7" r="1.15" fill="{c}"/>"#
        ),
        SubjectKind::CheckSuite => format!(
            r#"<circle cx="8" cy="8" r="6.3" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M5.2 8.2 L7.2 10.2 L10.9 6.1" fill="none" stroke="{c}"
                     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SubjectKind::RepositoryVulnerabilityAlert => format!(
            r#"<path d="M8 1.8 L13.4 3.7 V8 C13.4 11 11 13.4 8 14.2 C5 13.4 2.6 11 2.6 8 V3.7 Z"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M8 5.6 V8.6" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="8" cy="10.8" r="0.95" fill="{c}"/>"#
        ),
        SubjectKind::Other(_) => format!(r#"<circle cx="8" cy="8" r="3.2" fill="{c}"/>"#),
    };
    svg16(&body)
}

/// Sidebar glyphs. Drawn at 16px, tinted by the caller's colour.
pub fn sidebar_icon(name: SidebarIcon, color: Color) -> String {
    let c = hex(color);
    let body = match name {
        SidebarIcon::Inbox => format!(
            r#"<path d="M1.9 9.2 L3.6 3.1 h8.8 l1.7 6.1 v3.7 h-12.2 z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M1.9 9.2 h3.4 a2.7 2.7 0 0 0 5.4 0 h3.4" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Bell => format!(
            r#"<path d="M8 2.2 C5.7 2.2 4.3 3.9 4.3 6.1 V9.3 L3.1 11.3 H12.9 L11.7 9.3 V6.1
                       C11.7 3.9 10.3 2.2 8 2.2 Z" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linejoin="round"/>
               <path d="M6.5 13.1 A1.6 1.6 0 0 0 9.5 13.1" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>"#
        ),
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
        // The partial state of a checkbox: some of these, not all.
        SidebarIcon::Dash => format!(
            r#"<path d="M4.2 8 h7.6" fill="none" stroke="{c}" stroke-width="1.8"
                     stroke-linecap="round"/>"#
        ),
        SidebarIcon::Check => format!(
            r#"<path d="M3.2 8.4 L6.4 11.6 L12.8 4.6" fill="none" stroke="{c}"
                     stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Target => format!(
            r#"<circle cx="8" cy="8" r="6.1" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="8" cy="8" r="2.7" fill="none" stroke="{c}" stroke-width="1.4"/>
               <circle cx="8" cy="8" r="0.9" fill="{c}"/>"#
        ),
        SidebarIcon::Comment => format!(
            r#"<path d="M2.4 3.4 h11.2 v7.1 h-5.5 l-3.1 2.6 v-2.6 h-2.6 z" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Mention => format!(
            r#"<circle cx="8" cy="8" r="2.6" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M10.6 5.8 v3.3 a1.9 1.9 0 0 0 3.1 1.4 A6.2 6.2 0 1 0 11.4 13.4"
                     fill="none" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::People => format!(
            r#"<circle cx="6" cy="5.6" r="2.4" fill="none" stroke="{c}" stroke-width="1.4"/>
               <path d="M1.9 13.3 a4.1 4.1 0 0 1 8.2 0" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>
               <path d="M10.7 3.6 a2.4 2.4 0 0 1 0 4.5" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>
               <path d="M11.6 9.6 a4.1 4.1 0 0 1 2.6 3.7" fill="none" stroke="{c}"
                     stroke-width="1.4" stroke-linecap="round"/>"#
        ),
        SidebarIcon::Eye => format!(
            r#"<path d="M1.4 8 C3.4 4.6 5.6 3.2 8 3.2 s4.6 1.4 6.6 4.8 C12.6 11.4 10.4 12.8 8 12.8
                       S3.4 11.4 1.4 8 Z" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round"/>
               <circle cx="8" cy="8" r="2.1" fill="none" stroke="{c}" stroke-width="1.4"/>"#
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
        SidebarIcon::Archive => format!(
            r#"<rect x="2" y="2.8" width="12" height="3.2" rx="0.8" fill="none" stroke="{c}"
                     stroke-width="1.4"/>
               <path d="M3.2 6 V12.4 a1 1 0 0 0 1 1 H11.8 a1 1 0 0 0 1 -1 V6" fill="none"
                     stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>
               <path d="M6.6 9 H9.4" stroke="{c}" stroke-width="1.4" stroke-linecap="round"/>"#
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
        SidebarIcon::Thread => format!(
            r#"<path d="M2.4 3.4 h11.2 v7.1 h-5.5 l-3.1 2.6 v-2.6 h-2.6 z" fill="{c}"
                     stroke="{c}" stroke-width="1.2" stroke-linejoin="round"/>"#
        ),
        SidebarIcon::Warning => format!(
            r#"<path d="M8 2.2 L14.6 13.4 H1.4 Z" fill="none" stroke="{c}" stroke-width="1.4"
                     stroke-linejoin="round"/>
               <path d="M8 6.2 V9.3" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>
               <circle cx="8" cy="11.3" r="0.95" fill="{c}"/>"#
        ),
    };
    svg16(&body)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarIcon {
    Inbox,
    Bell,
    Bookmark,
    BookmarkFilled,
    Check,
    Dash,
    Undo,
    Target,
    Comment,
    Mention,
    People,
    Eye,
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
    /// A box with a lid: put away, keep.
    Archive,
    Trash,
    /// Three dots: more actions.
    Ellipsis,
    Folder,
    Plus,
    /// A filled square: stop.
    Stop,
    /// A speech bubble with a dot: a thread anchored here.
    Thread,
}

impl SidebarIcon {
    /// Every glyph, so a test can check them all and a new one cannot be
    /// added without being drawn.
    #[cfg(test)]
    pub const ALL: [SidebarIcon; 28] = [
        SidebarIcon::Plus,
        SidebarIcon::Stop,
        SidebarIcon::Thread,
        SidebarIcon::Inbox,
        SidebarIcon::Bell,
        SidebarIcon::Branch,
        SidebarIcon::Archive,
        SidebarIcon::Trash,
        SidebarIcon::Ellipsis,
        SidebarIcon::Folder,
        SidebarIcon::Bookmark,
        SidebarIcon::BookmarkFilled,
        SidebarIcon::Check,
        SidebarIcon::Dash,
        SidebarIcon::Undo,
        SidebarIcon::Target,
        SidebarIcon::Comment,
        SidebarIcon::Mention,
        SidebarIcon::People,
        SidebarIcon::Eye,
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
    fn unknown_subject_kinds_still_get_an_icon() {
        let svg = kind_icon(&SubjectKind::Other("Sponsorship".into()));
        assert!(svg.starts_with("<svg"));
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
