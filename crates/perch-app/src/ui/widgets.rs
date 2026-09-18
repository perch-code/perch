//! Small building blocks shared by the panels.

use floem::peniko::Color;
use floem::prelude::*;
use floem::text::FontWeight;
use floem::views::{ClipExt, Container, Decorators, Empty, Label, Stack, dyn_container, img, svg};

use crate::icons::{self, SidebarIcon};
use crate::state::{AppState, Checked};
use floem::style::ObjectFit;

use crate::theme;

/// Open a link in the user's default browser.
///
/// Spawned and forgotten: waiting on `open` would block the UI thread for as
/// long as the browser takes to come to the front.
pub fn open_in_browser(url: &str) {
    // Only ever a github.com page derived from an API url, never anything a
    // notification's contents could steer.
    if !url.starts_with("https://github.com/") {
        tracing::warn!(url, "refusing to open a non-GitHub link");
        return;
    }
    if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
        tracing::warn!(url, error = %e, "could not open link");
    }
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
            .item("Archive Worktree", move |item| item.action(move || archive()));
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

/// A checkbox, in the three states a set can be in.
///
/// Toggling happens on pointer-down rather than on click, because that is the
/// event that carries the shift key, and a shift-click has to extend the run
/// rather than check one row. The click is swallowed as well, so checking a
/// row inside a row-shaped button does not also open it.
pub fn checkbox(
    checked: impl Fn() -> Checked + 'static,
    on_toggle: impl Fn(bool) + 'static,
) -> impl IntoView {
    // Two readers, one closure: the container picks the glyph and the style
    // picks the fill, and a closure that captures a thread id is not Copy.
    let checked = std::rc::Rc::new(checked);
    let for_glyph = checked.clone();

    let box_ = Container::new(
        dyn_container(move || for_glyph(), |checked| match checked {
            Checked::Empty => Empty::new().into_any(),
            Checked::Partial => sidebar_glyph(SidebarIcon::Dash, theme::CANVAS).into_any(),
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
    // as a click on the row, which opens the pull request.
    Container::new(box_)
        .on_event_stop(
            floem::event::listener::PointerDown,
            move |_, e: &PointerButtonEvent| {
                on_toggle(e.state.modifiers.contains(Modifiers::SHIFT));
            },
        )
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

/// A small bordered text button, as used by the selection bar and the group
/// headers.
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

/// A small round icon button, as sits at the end of a notification row.
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

/// A rounded count pill, as used beside every sidebar row.
///
/// The pill has a fixed height and centres the label inside it, rather than
/// growing from the label's own padding. A label's box carries the font's
/// leading, which is not shared evenly above and below the glyph, so padding
/// alone leaves the number sitting low in the pill.
pub fn count_badge(count: impl Fn() -> usize + 'static) -> impl IntoView {
    Container::new(
        Label::derived(move || {
            let n = count();
            if n > 999 { "999+".to_string() } else { n.to_string() }
        })
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(FontWeight::MEDIUM)
                .color(theme::BADGE_FG)
                .selectable(false)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
    )
    .style(|s| {
        s.height(theme::BADGE_HEIGHT)
            .min_width(theme::BADGE_HEIGHT + 2.0)
            .padding_horiz(6.0)
            .border_radius(theme::BADGE_HEIGHT / 2.0)
            .background(theme::BADGE_BG)
            .items_center()
            .justify_center()
            .flex_shrink(0.0_f32)
    })
}

/// An uppercase, letter-spaced section heading.
pub fn section_heading(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_uppercase()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(theme::FG_SUBTLE)
            .padding_horiz(10.0)
            .padding_top(14.0)
            .padding_bottom(6.0)
    })
}

/// A one pixel rule.
pub fn hairline() -> impl IntoView {
    Empty::new().style(|s| s.height(1.0).width_full().background(theme::BORDER_MUTED))
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

/// A muted pill, used for the reason badge on a row.
pub fn reason_pill(text: impl Fn() -> String + 'static) -> impl IntoView {
    Container::new(Label::derived(text).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::FG_MUTED)
            .selectable(false)
            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
    }))
    .style(|s| {
        s.height(theme::PILL_HEIGHT)
            .padding_horiz(8.0)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::PILL_HEIGHT / 2.0)
            .items_center()
            .justify_center()
            .flex_shrink(0.0_f32)
    })
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
                s.font_size(15.0)
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

/// Row used by the reason filters in the sidebar.
pub fn sidebar_row(
    glyph: SidebarIcon,
    text: String,
    selected: impl Fn() -> bool + 'static + Copy,
    enabled: bool,
    badge: Option<Box<dyn Fn() -> usize>>,
) -> impl IntoView {
    let fg = move || {
        if !enabled {
            theme::FG_SUBTLE
        } else if selected() {
            theme::FG
        } else {
            theme::FG_MUTED
        }
    };

    let content = Stack::horizontal((
        dyn_container(fg, move |c| sidebar_glyph(glyph, c).into_any())
            .style(|s| s.size(16.0, 16.0).margin_right(9.0)),
        Label::derived(move || text.clone()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .color(fg())
                .font_weight(if selected() {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_ellipsis()
                .flex_grow(1.0_f32)
                .min_width(0.0)
        }),
        dyn_container(
            move || badge.as_ref().map(|b| b()).unwrap_or(0),
            move |n| {
                if n == 0 {
                    Empty::new().into_any()
                } else {
                    count_badge(move || n).into_any()
                }
            },
        ),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0));

    Container::new(content).style(move |s| {
        // Selectable is cleared either way, so a label never shows an I-beam
        // over a row. Only an enabled row offers a pointer.
        let base = if enabled { clickable(s) } else { s.selectable(false) }
            .width_full()
            .height(34.0)
            .items_center()
            .padding_horiz(10.0)
            .border_radius(theme::RADIUS)
            .margin_horiz(6.0);
        let base = if selected() {
            base.background(theme::ACCENT_SUBTLE)
                .border(1.0)
                .border_color(theme::ACCENT_RING)
        } else {
            base.border(1.0).border_color(Color::TRANSPARENT)
        };
        if enabled && !selected() {
            base.hover(|s| s.background(theme::CANVAS_HOVER))
        } else {
            base
        }
    })
}
