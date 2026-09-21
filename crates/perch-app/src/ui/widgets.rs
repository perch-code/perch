//! Small building blocks shared by the panels.

use floem::event::EventPropagation;
use floem::peniko::Color;
use floem::prelude::*;
use floem::style::Style;
use floem::text::FontWeight;
use floem::views::{ClipExt, Container, Decorators, Empty, Label, Stack, dyn_container, img, svg};

use crate::icons::{self, SidebarIcon};
use crate::state::{AppState, Checked};
use floem::style::ObjectFit;

use crate::theme;

/// Open an HTTP(S) link supplied by GitHub in the user's default browser.
///
/// Spawned and forgotten: waiting on `open` would block the UI thread for as
/// long as the browser takes to come to the front. Restricting the scheme keeps
/// local files and application URLs out while allowing GitHub Enterprise and
/// check providers to use their own hosts.
pub fn open_in_browser(url: &str) {
    if !is_browser_url(url) {
        tracing::warn!(url, "refusing to open a non-HTTP(S) link");
        return;
    }
    #[cfg(test)]
    OPENED.with(|opened| opened.borrow_mut().push(url.to_string()));
    #[cfg(not(test))]
    if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
        tracing::warn!(url, error = %e, "could not open link");
    }
}

#[cfg(test)]
thread_local! {
    /// The links opened, in order, so a test can see where a click led
    /// without a browser coming to the front.
    static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The links opened since the last call, and none of them again: a test
/// takes them before it clicks, so nothing an earlier test on the thread
/// opened is counted as its own.
#[cfg(test)]
pub(crate) fn opened() -> Vec<String> {
    OPENED.with(|opened| std::mem::take(&mut *opened.borrow_mut()))
}

/// The browser boundary is deliberately scheme-only: URLs passed here come
/// from GitHub, whose check details may live on another host.
fn is_browser_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Where a login's page is on github.com: the profile of a person or an
/// organisation, and the app's page for a bot, whose login ends in
/// `[bot]` and whose profile path is a 404. Copilot signs as plain
/// `Copilot`, which has no page under that name either; the API sends
/// that login for its reviewer and for its coding agent alike, and Perch
/// keeps only the login, so the reviewer's page is assumed. `None` for the
/// empty login Perch keeps when GitHub sends no user at all.
pub fn profile_url(login: &str) -> Option<String> {
    if login.is_empty() {
        return None;
    }
    if login == "Copilot" {
        return Some("https://github.com/apps/copilot-pull-request-reviewer".into());
    }
    Some(match login.strip_suffix("[bot]") {
        Some(app) => format!("https://github.com/apps/{app}"),
        None => format!("https://github.com/{login}"),
    })
}

/// What a row calls someone: a login, which leads to their page on
/// GitHub, or a name that leads nowhere, an agent's model or "You"
/// before GitHub has said who the reader is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Name {
    Login(String),
    Plain(String),
}

impl Name {
    fn text(&self) -> &str {
        match self {
            Name::Login(text) | Name::Plain(text) => text,
        }
    }

    fn profile_url(&self) -> Option<String> {
        match self {
            Name::Login(login) => profile_url(login),
            Name::Plain(_) => None,
        }
    }
}

/// A name as GitHub draws one: a login is a link, and a click on it opens
/// the person's page in the browser. `style` is the label's own, since a
/// byline and the strip set a name differently; the pointer and the
/// underline under it come with the link. A name that leads nowhere is
/// the same label, plain.
///
/// The underline is the label's bottom border, kept transparent until the
/// pointer is over it, so a name is the same height whether or not it is
/// a link, and the row does not shift when the reader's login arrives.
/// The border comes out of the label's height, so a pixel of padding at
/// the top balances it and the text sits level with the words beside it.
pub fn name_label(
    name: impl Fn() -> Name + 'static,
    style: impl Fn(Style) -> Style + 'static,
) -> impl IntoView {
    let name = std::rc::Rc::new(name);
    let text = {
        let name = name.clone();
        move || name().text().to_string()
    };
    let url = {
        let name = name.clone();
        move || name().profile_url()
    };
    Label::derived(text)
        .on_event(el::Click, move |_, _| match url() {
            Some(url) => {
                open_in_browser(&url);
                EventPropagation::Stop
            }
            None => EventPropagation::Continue,
        })
        .style(move |s| {
            let s = style(s)
                .padding_top(1.0)
                .border_bottom(1.0)
                .border_color(Color::TRANSPARENT);
            if name().profile_url().is_some() {
                clickable(s).hover(|s| s.border_color(theme::FG_MUTED))
            } else {
                s
            }
        })
        .debug_name("name label")
}

/// Style a row that behaves as a button.
///
/// floem's theme turns a label's `Selectable` flag into a text cursor, and
/// cursor resolution is deepest-hovered-wins, so a label inside a row
/// overrides the row's own pointer cursor. `selectable` is inherited, so
/// clearing it once on the row covers every label inside it.
///
/// Note this deliberately does not make children transparent to the pointer:
/// a selectable label does not swallow clicks, and `pointer_events_none` on
/// the view that owns the handler would stop it receiving them at all.
/// Show a directory in the Finder.
pub fn reveal_in_finder(path: &std::path::Path) {
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
}

/// Open a Terminal window in a directory.
pub fn open_in_terminal(path: &std::path::Path) {
    let _ = std::process::Command::new("open").arg("-a").arg("Terminal").arg(path).spawn();
}

pub fn copy_to_clipboard(text: String) {
    if let Err(e) = floem::Clipboard::set_contents(text) {
        tracing::warn!(error = ?e, "could not write to the clipboard");
    }
}

/// The menu every worktree offers: where it is, and how to get there.
///
/// Archive is appended when the caller passes a way to do it; the review
/// header offers it, the archived rows of the list do not.
pub fn worktree_menu(
    path: std::path::PathBuf,
    on_archive: Option<std::rc::Rc<dyn Fn()>>,
) -> floem::menu::Menu {
    use floem::menu::Menu;

    let (reveal, terminal, copy) = (path.clone(), path.clone(), path);
    let mut menu = Menu::new()
        .item("Reveal in Finder", move |item| item.action(move || reveal_in_finder(&reveal)))
        .item("Open in Terminal", move |item| item.action(move || open_in_terminal(&terminal)))
        .item("Copy Path", move |item| {
            item.action(move || copy_to_clipboard(copy.to_string_lossy().into_owned()))
        });
    if let Some(archive) = on_archive {
        menu = menu
            .separator()
            .item("Archive", move |item| item.action(move || archive()));
    }
    menu
}

pub fn clickable(style: floem::style::Style) -> floem::style::Style {
    style
        .selectable(false)
        .hover(|s| s.cursor(floem::style::CursorStyle::Pointer))
}

/// A 16px icon in a colour of its own.
///
/// floem draws an SVG in the view's text colour rather than the markup's
/// fills, so the colour has to be set on the view or every icon comes out
/// the same near-black as the body text.
pub fn icon_in(source: impl Fn() -> String + 'static, color: impl Fn() -> Color + 'static) -> impl IntoView {
    svg(source).style(move |s| s.size(16.0, 16.0).color(color()))
}

pub fn sidebar_glyph(name: SidebarIcon, color: Color) -> impl IntoView {
    icon_in(move || icons::sidebar_icon(name, color), move || color)
}

/// A checkbox.
///
/// Toggling happens on pointer-down, and the click is swallowed, so checking
/// a box inside a row-shaped button does not also open the row.
pub fn checkbox(
    checked: impl Fn() -> Checked + 'static,
    on_toggle: impl Fn() + 'static,
) -> impl IntoView {
    // Two readers, one closure: the container picks the glyph and the style
    // picks the fill, and a closure that captures a thread id is not Copy.
    let checked = std::rc::Rc::new(checked);
    let for_glyph = checked.clone();

    let box_ = Container::new(
        dyn_container(move || for_glyph(), |checked| match checked {
            Checked::Empty => Empty::new().into_any(),
            Checked::Full => sidebar_glyph(SidebarIcon::Check, theme::CANVAS).into_any(),
        })
        .style(|s| s.size(12.0, 12.0).items_center().justify_center()),
    )
    .style(move |s| {
        let base = s
            .size(16.0, 16.0)
            .items_center()
            .justify_center()
            .border_radius(4.0)
            .border(1.5);
        if checked() == Checked::Empty {
            base.background(theme::CANVAS).border_color(theme::BORDER)
        } else {
            base.background(theme::ACCENT).border_color(theme::ACCENT)
        }
    });

    // The listeners sit on a box half again as wide as the one that is drawn.
    // A click is the view that both the press and the release land in, so a
    // press on a 16px target that drifts a few pixels before release counts
    // as a click on the row, which opens the file.
    Container::new(box_)
        .on_event_stop(floem::event::listener::PointerDown, move |_, _| on_toggle())
        // Belt and braces: the row around this one is itself a button.
        .on_event_stop(floem::event::listener::Click, |_, _| {})
        .style(|s| {
            clickable(s)
                .size(26.0, 26.0)
                .items_center()
                .justify_center()
                .hover(|s| s.background(theme::BADGE_BG))
                .border_radius(6.0)
        })
}

/// A small bordered text button, as under a Delete confirmation.
pub fn text_button(
    label: impl Fn() -> String + 'static,
    glyph: SidebarIcon,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    Container::new(
        Stack::horizontal((
            sidebar_glyph(glyph, theme::FG_MUTED).style(|s| s.margin_right(6.0)),
            Label::derived(label).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG)
                    .selectable(false)
                    .margin_bottom(theme::OPTICAL_TEXT_LIFT)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .on_event_stop(floem::event::listener::Click, move |_, _| on_click())
    .style(|s| {
        clickable(s)
            .height(28.0)
            .items_center()
            .padding_horiz(10.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

/// A small round icon button, as sits at the end of an item row.
///
/// Its own hover background, so it reads as a target inside a row that is
/// itself a button.
pub fn icon_button(
    glyph: impl Fn() -> SidebarIcon + 'static + Copy,
    color: impl Fn() -> Color + 'static + Copy,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    Container::new(icon_in(
        move || icons::sidebar_icon(glyph(), color()),
        color,
    ))
    .on_event_stop(floem::event::listener::Click, move |_, _| on_click())
    .style(|s| {
        clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(13.0)
            .hover(|s| s.background(theme::BADGE_BG))
    })
}

/// A circular avatar that falls back to a tinted initial until the image
/// arrives, so a row never reflows when the download lands.
pub fn avatar(state: AppState, url: impl Fn() -> String + 'static, size: f64, seed: String) -> impl IntoView {
    let radius = size / 2.0;
    dyn_container(
        move || state.avatar_for(&url()),
        move |bytes| match bytes {
            // An Img draws its bitmap square: border_radius alone would only
            // round the border, not the picture. Clipping is what actually
            // makes the avatar circular.
            Some(bytes) => img(move || bytes.as_ref().clone())
                .style(move |s| s.size(size, size).object_fit(ObjectFit::Cover))
                .clip()
                .style(move |s| s.size(size, size).border_radius(radius))
                .into_any(),
            None => {
                let initial = seed
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".into());
                let tint = theme::tint_for(&seed);
                Container::new(Label::derived(move || initial.clone()).style(move |s| {
                    s.font_size((size * 0.44) as f32)
                        .font_weight(FontWeight::SEMI_BOLD)
                        .color(theme::FG_MUTED)
                }))
                .style(move |s| {
                    s.size(size, size)
                        .border_radius(radius)
                        .background(tint)
                        .items_center()
                        .justify_center()
                })
                .into_any()
            }
        },
    )
    .style(move |s| s.size(size, size).border_radius(radius))
}

/// A centred message for empty and error states.
pub fn centered_message(
    glyph: SidebarIcon,
    title: impl Fn() -> String + 'static,
    body: impl Fn() -> String + 'static,
) -> impl IntoView {
    Container::new(
        Stack::vertical((
            Container::new(sidebar_glyph(glyph, theme::FG_SUBTLE)).style(|s| {
                s.size(48.0, 48.0)
                    .border_radius(24.0)
                    .background(theme::CANVAS_SUBTLE)
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(title).style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .margin_bottom(6.0)
            }),
            Label::derived(body).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_MUTED)
                    .max_width(420.0)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(|s| s.size_full().items_center().justify_center().padding(40.0))
}

/// A horizontal spacer that pushes what follows to the right.
pub fn spacer() -> impl IntoView {
    Empty::new().style(|s| s.flex_grow(1.0_f32))
}

#[cfg(test)]
mod tests {
    use super::{is_browser_url, profile_url, Name};

    #[test]
    fn a_login_leads_to_its_page_on_github() {
        for (login, url) in [
            ("criccomini", "https://github.com/criccomini"),
            // An organisation's page is at the same path as a person's.
            ("slatedb", "https://github.com/slatedb"),
            // github.com finds a profile whatever the case; the login is
            // passed through as GitHub spelt it.
            ("CRICCOMINI", "https://github.com/CRICCOMINI"),
            ("a-b-c", "https://github.com/a-b-c"),
            // GitHub's stand-in for a deleted account is a login like any
            // other.
            ("ghost", "https://github.com/ghost"),
            // An enterprise-managed user's suffix, and an imported
            // mannequin's UUID, are plain path characters.
            ("mona-cat_octo", "https://github.com/mona-cat_octo"),
            (
                "ebd80758-eb43-4a34-86bb-a2b2a2196e69",
                "https://github.com/ebd80758-eb43-4a34-86bb-a2b2a2196e69",
            ),
        ] {
            assert_eq!(profile_url(login).as_deref(), Some(url), "{login}");
        }
        assert_eq!(
            Name::Login("rohan".into()).profile_url().as_deref(),
            Some("https://github.com/rohan")
        );
    }

    #[test]
    fn a_bot_leads_to_its_app_page() {
        for (login, url) in [
            ("dependabot[bot]", "https://github.com/apps/dependabot"),
            (
                "github-actions[bot]",
                "https://github.com/apps/github-actions",
            ),
            ("renovate[bot]", "https://github.com/apps/renovate"),
            (
                "copilot-pull-request-reviewer[bot]",
                "https://github.com/apps/copilot-pull-request-reviewer",
            ),
            (
                "Copilot",
                "https://github.com/apps/copilot-pull-request-reviewer",
            ),
        ] {
            assert_eq!(profile_url(login).as_deref(), Some(url), "{login}");
        }
        // The match is exact: `copilot` falls through to the profile path.
        assert_eq!(
            profile_url("copilot").as_deref(),
            Some("https://github.com/copilot")
        );
    }

    #[test]
    fn nobody_has_no_page() {
        // Not github.com's front page.
        assert_eq!(profile_url(""), None);
        assert_eq!(Name::Login(String::new()).profile_url(), None);
        assert_eq!(Name::Plain("You".into()).profile_url(), None);
        assert_eq!(Name::Plain("Sonnet".into()).profile_url(), None);
    }

    #[test]
    fn browser_links_accept_explicit_http_schemes_only() {
        for url in [
            "https://github.example.com/acme/widgets/pull/7",
            "https://checks.example.net/runs/42",
            "http://github.enterprise.test/acme/widgets/pull/7",
        ] {
            assert!(is_browser_url(url), "rejected {url}");
        }

        for url in [
            "//github.com/acme/widgets/pull/7",
            "file:///tmp/report.html",
            "javascript:alert(1)",
            "https:github.com/acme/widgets/pull/7",
            " HTTPS://github.com/acme/widgets/pull/7",
        ] {
            assert!(!is_browser_url(url), "accepted {url}");
        }
    }
}
