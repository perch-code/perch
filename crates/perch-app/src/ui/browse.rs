//! Browse: GitHub's fixed views, and Recent.
//!
//! A chip picks the view, a second the repository By repository lists,
//! and Open · Closed which of its pull requests and issues. A GitHub view
//! is searched when it is picked, and again after each poll while it is
//! on screen; its results live in `AppState::browse`, never in the
//! database. Recent is the local list of what was opened lately.

use std::rc::Rc;

use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalWith};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::{ItemKind, SearchHit, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::items::BrowseView;
use crate::state::AppState;
use crate::theme;
use crate::ui::items::OpenItem;
use crate::ui::nav_row;
use crate::ui::sidebar::{local_item_list, set_filters};
use crate::ui::widgets::{open_in_browser, sidebar_glyph, state_glyph};

/// The Browse tab: its chips over the view's list.
pub fn browse(state: AppState, backend: Rc<Backend>, on_open: OpenItem) -> impl IntoView {
    let search_backend = backend.clone();
    // A memo, so a change of repository or of Open · Closed does not
    // build the list again: the list follows its search itself.
    let recent = Memo::new(move |_| state.item_filters.with(|f| f.browse == BrowseView::Recent));
    Stack::vertical((
        bar(state),
        open_row(state, backend.clone()),
        dyn_container(
            move || recent.get(),
            move |recent| {
                if recent {
                    local_item_list(
                        state,
                        backend.clone(),
                        on_open.clone(),
                        "Nothing opened yet",
                    )
                    .into_any()
                } else {
                    search_list(state, search_backend.clone()).into_any()
                }
            },
        )
        .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
    ))
    .style(|s| s.flex_col().width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// The view chip, the repository chip while By repository is picked, and
/// Open · Closed for GitHub's views.
fn bar(state: AppState) -> impl IntoView {
    let view = move || state.item_filters.with(|f| f.browse);
    Stack::horizontal((
        chip(
            move || view().label().to_string(),
            move || view_menu(state),
            "browse view",
        ),
        chip(
            move || {
                state
                    .item_filters
                    .with(|f| f.browse_repository.clone())
                    .unwrap_or_else(|| "Pick a repository".to_string())
            },
            move || repository_menu(state),
            "browse repository",
        )
        .style(move |s| s.apply_if(view() != BrowseView::Repository, |s| s.hide())),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        open_closed(state).style(move |s| s.apply_if(view() == BrowseView::Recent, |s| s.hide())),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .gap(6.0)
            .padding_horiz(theme::NAV_GUTTER)
            .padding_top(12.0)
    })
}

/// A chip that opens a menu, in the drawer's colours.
fn chip(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> Menu + 'static,
    name: &'static str,
) -> impl View {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::NAV_FG)
                .selectable(false)
                .pointer_events_none()
                .text_ellipsis()
                .min_width(0.0)
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::NAV_MUTED)
            .style(|s| s.size(9.0, 9.0).flex_shrink(0.0_f32).pointer_events_none()),
    ))
    .popout_menu(menu)
    .debug_name(name)
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .items_center()
            .height(28.0)
            .min_width(0.0)
            .padding_horiz(10.0)
            .background(theme::NAV_SELECTED)
            .border_radius(14.0)
            .hover(|s| s.background(theme::NAV_SELECTED_PRESSED))
    })
}

fn view_menu(state: AppState) -> Menu {
    let current = state.item_filters.with_untracked(|f| f.browse);
    let mut menu = Menu::new();
    for view in BrowseView::ALL {
        if view == BrowseView::Recent {
            menu = menu.separator();
        }
        menu = menu.item(view.label(), move |m| {
            m.checked(view == current).action(move || {
                let first = github_repositories(state).into_iter().next();
                set_filters(state, move |f| {
                    f.browse = view;
                    // By repository opens on the last one picked, else
                    // the first there is.
                    if view == BrowseView::Repository && f.browse_repository.is_none() {
                        f.browse_repository = first;
                    }
                })
            })
        });
    }
    menu
}

/// The configured repositories on GitHub, in the file's order.
fn github_repositories(state: AppState) -> Vec<String> {
    state.repos.with_untracked(|repos| {
        repos
            .iter()
            .filter(|repo| !perch_github::is_local(&repo.identity))
            .map(|repo| repo.identity.clone())
            .collect()
    })
}

fn repository_menu(state: AppState) -> Menu {
    let current = state
        .item_filters
        .with_untracked(|f| f.browse_repository.clone());
    let mut menu = Menu::new();
    for repo in github_repositories(state) {
        let checked = current
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case(&repo));
        menu = menu.item(repo.clone(), move |m| {
            let repo = repo.clone();
            m.checked(checked).action(move || {
                let repo = repo.clone();
                set_filters(state, move |f| f.browse_repository = Some(repo))
            })
        });
    }
    menu
}

/// Open · Closed, two halves of one control.
fn open_closed(state: AppState) -> impl View {
    let half = move |label: &'static str, closed: bool| {
        Label::new(label)
            .on_event_stop(el::Click, move |_, _| {
                set_filters(state, move |f| f.browse_closed = closed)
            })
            .debug_name(format!("browse {}", label.to_lowercase()))
            .style(move |s| {
                let on = state.item_filters.with(|f| f.browse_closed == closed);
                crate::ui::widgets::clickable(s)
                    .font_size(theme::TEXT_BADGE)
                    .height(24.0)
                    .items_center()
                    .padding_horiz(8.0)
                    .border_radius(12.0)
                    .color(if on { theme::NAV_FG } else { theme::NAV_MUTED })
                    .background(if on {
                        theme::NAV_CANVAS
                    } else {
                        theme::NAV_SELECTED
                    })
                    .selectable(false)
            })
    };
    Stack::horizontal((half("Open", false), half("Closed", true))).style(|s| {
        s.items_center()
            .padding(2.0)
            .border_radius(14.0)
            .background(theme::NAV_SELECTED)
            .flex_shrink(0.0_f32)
    })
}

/// The first row while the search box holds something Open… reads: a
/// URL, `owner/repo#57` or `#57`. It opens through Open…'s box, which
/// says what is wrong if it can't.
fn open_row(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let label = move || {
        let text = state.item_query.get();
        let hosts = state.github_hosts.get();
        crate::reference::parse(&text, &hosts)?;
        Some(match crate::ui::open_box::target(state, &text) {
            Ok((repo, reference)) => format!("Open {repo}#{}", reference.number),
            Err(_) => format!("Open {}", text.trim()),
        })
    };
    let row = Container::new(
        Stack::horizontal((
            nav_row::glyph_box(
                sidebar_glyph(SidebarIcon::Search, theme::NAV_MUTED).style(|s| s.size(14.0, 14.0)),
            ),
            Label::derived(move || label().unwrap_or_default()).style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .color(theme::NAV_FG)
                    .text_ellipsis()
                    .min_width(0.0)
            }),
        ))
        .style(|s| {
            s.items_center()
                .width_full()
                .min_width(0.0)
                .pointer_events_none()
        }),
    )
    .debug_name("browse open row")
    .on_event_stop(el::Click, move |_, _| {
        crate::ui::open_box::open_from_search(state, &backend, &state.item_query.get_untracked())
    })
    .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(move |s| {
        nav_row::row_gutter(s)
            .margin_top(8.0)
            .apply_if(label().is_none(), |s| s.hide())
    })
}

/// A GitHub view's results, searched when the view is picked and again
/// after each poll while it is on screen.
fn search_list(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let query = Memo::new(move |_| state.item_filters.with(|f| f.browse_query()));
    {
        let backend = backend.clone();
        Effect::new(move |_| {
            if let Some(query) = query.get() {
                backend.browse(&query, 1, false);
            }
        });
    }
    {
        let backend = backend.clone();
        Effect::new(move |seen: Option<u64>| {
            let polls = state.polls.get();
            if seen.is_some_and(|seen| seen != polls) {
                if let Some(query) = query.get_untracked() {
                    backend.browse(&query, 1, true);
                }
            }
            polls
        });
    }
    let page = move || {
        let query = query.get()?;
        state.browse.with(|all| all.get(&query).cloned())
    };
    let visible = move || {
        let hits = page().map(|page| page.hits).unwrap_or_default();
        state
            .item_query
            .with(|q| crate::items::search_hits(&hits, q))
    };
    let more_backend = backend.clone();
    Stack::vertical((
        Label::derived(move || page().and_then(|page| page.error).unwrap_or_default())
            .debug_name("browse error")
            .style(move |s| {
                let shown = page().is_some_and(|page| page.error.is_some());
                s.font_size(theme::TEXT_META)
                    .color(theme::NAV_MUTED)
                    .padding_horiz(theme::NAV_GUTTER + 8.0)
                    .padding_bottom(8.0)
                    .text_wrap()
                    .width_full()
                    .apply_if(!shown, |s| s.hide())
            }),
        Label::derived(move || {
            if query.get().is_none() {
                return "Pick a repository".to_string();
            }
            match page() {
                None => "Loading…".to_string(),
                Some(page) if page.pages == 0 && page.error.is_none() => "Loading…".to_string(),
                Some(page) if page.hits.is_empty() && page.pages > 0 => "Nothing here".to_string(),
                Some(_) if state.item_query.with(|q| !q.trim().is_empty()) => {
                    "No matching results".to_string()
                }
                Some(_) => String::new(),
            }
        })
        .debug_name("browse empty")
        .style(move |s| {
            let empty =
                visible().is_empty() && !page().is_some_and(|p| p.error.is_some() && p.pages == 0);
            s.font_size(theme::TEXT_META)
                .color(theme::NAV_SUBTLE)
                .padding(24.0)
                .apply_if(!empty, |s| s.hide())
        }),
        dyn_stack(
            visible,
            |hit: &SearchHit| {
                (
                    hit.repo_full_name.clone(),
                    hit.number,
                    hit.updated_at,
                    hit.state.clone(),
                    hit.merged,
                )
            },
            move |hit| search_row(state, backend.clone(), hit),
        )
        .style(|s| s.flex_col().width_full().gap(1.0)),
        Label::new("Show more")
            .debug_name("browse more")
            .on_event_stop(el::Click, move |_, _| {
                if let (Some(query), Some(page)) = (query.get_untracked(), page()) {
                    more_backend.browse(&query, page.pages + 1, false);
                }
            })
            .style(move |s| {
                let more = page().is_some_and(|page| page.more);
                crate::ui::widgets::clickable(s)
                    .font_size(theme::TEXT_META)
                    .color(theme::NAV_MUTED)
                    .margin_left(theme::NAV_GUTTER)
                    .margin_top(8.0)
                    .padding_horiz(10.0)
                    .padding_vert(6.0)
                    .border_radius(14.0)
                    .hover(|s| s.background(theme::NAV_HOVER).color(theme::NAV_FG))
                    .selectable(false)
                    .apply_if(!more, |s| s.hide())
            }),
    ))
    .style(nav_row::list_style)
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// One result, in the shape every row of the drawer has.
fn search_row(state: AppState, backend: Rc<Backend>, hit: SearchHit) -> impl IntoView {
    let subject_state = hit.subject_state();
    let (repo, kind, number) = (hit.repo_full_name.clone(), hit.kind, hit.number);
    let unread = {
        let repo = repo.clone();
        move || state.subject_unread(&repo, kind, number)
    };
    let url = hit.html_url.clone();
    let avatar = hit.author_avatar_url.clone();
    let note = match subject_state {
        SubjectState::PrDraft => Some("Draft"),
        SubjectState::PrMerged => Some("Merged"),
        SubjectState::PrClosed | SubjectState::IssueClosed | SubjectState::IssueNotPlanned => {
            Some("Closed")
        }
        _ => None,
    };
    let content = Stack::horizontal((
        nav_row::glyph_box(state_glyph(
            subject_state,
            icons::state_color(subject_state),
        )),
        nav_row::lines(
            nav_row::title_label(hit.title.clone(), false, unread.clone()),
            Stack::horizontal((
                nav_row::unread_dot(unread),
                nav_row::age(state, hit.updated_at),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32)),
            nav_row::byline(
                state,
                move || avatar.clone(),
                hit.author_login.clone(),
                repo.clone(),
                nav_row::reference(Some(number)),
                nav_row::note(note.map(str::to_string), theme::NAV_MUTED),
                Empty::new(),
            ),
        ),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .min_width(0.0)
            .pointer_events_none()
    });
    let open_repo = repo.clone();
    let row = Container::new(content)
        .debug_name(format!("browse row:{repo}#{number}"))
        .on_event_stop(el::Click, move |_, _| {
            let navigation_id = backend.open_subject(&open_repo, kind, number);
            state.begin_open(
                match kind {
                    ItemKind::PullRequest => "Reading the pull request",
                    ItemKind::Issue => "Reading the issue",
                },
                navigation_id,
            );
        })
        .context_menu(move || {
            let url = url.clone();
            Menu::new().item("Open on GitHub", move |m| {
                let url = url.clone();
                m.action(move || open_in_browser(&url))
            })
        })
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Command, Event};
    use crate::items::SidebarTab;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::SignalUpdate;
    use perch_github::DeclaredRepo;

    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| find(child, text))
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        find(harness.root_id(), name).is_some_and(|id| !id.get_visual_rect_no_clip().is_zero_area())
    }

    fn hit(number: u64, title: &str) -> SearchHit {
        SearchHit {
            kind: ItemKind::PullRequest,
            repo_full_name: "a/b".into(),
            number,
            title: title.into(),
            state: "open".into(),
            state_reason: None,
            draft: number == 2,
            merged: false,
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            html_url: String::new(),
        }
    }

    fn searches(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Vec<(String, u32, bool)> {
        let mut out = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::Browse { query, page, poll } = command {
                out.push((query, page, poll));
            }
        }
        out
    }

    fn repaint(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
    }

    /// Picking a GitHub view searches it; the results are rows; Show more
    /// asks for the next page, Closed for the closed ones, and a poll for
    /// the view again. By repository shows its repository chip.
    #[test]
    fn a_github_view_is_searched_listed_and_searched_again() {
        let root = TestRoot::new();
        let state = AppState::new();
        state.sidebar_width.set(theme::SIDEBAR_MIN_WIDTH);
        state.now.set("2026-09-20T12:00:00Z".parse().unwrap());
        state.repos.set(vec![DeclaredRepo {
            identity: "a/b".into(),
            name: "b".into(),
        }]);
        state.item_filters.update(|f| {
            f.tab = SidebarTab::Browse;
            f.browse = BrowseView::Yours;
        });
        let (backend, mut commands) = Backend::recording();
        let view = crate::ui::sidebar::sidebar(state, Rc::new(backend), Rc::new(|_| {}));
        let mut harness =
            HeadlessHarness::new_with_size(root, view, theme::SIDEBAR_MIN_WIDTH, 700.0);
        repaint(&mut harness);

        let query = "author:@me is:open archived:false".to_string();
        assert_eq!(searches(&mut commands), [(query.clone(), 1, false)]);
        assert!(shown(&harness, "browse empty"), "loading");
        assert!(!shown(&harness, "browse repository"));
        assert!(shown(&harness, "browse open"));

        state.apply(Event::BrowseResults {
            query: query.clone(),
            page: 1,
            poll: false,
            result: Ok(perch_github::SearchPage {
                hits: vec![hit(1, "Fence"), hit(2, "Spans")],
                more: true,
            }),
        });
        repaint(&mut harness);
        let row = find(harness.root_id(), "browse row:a/b#1").expect("no row");
        assert!(find(row, "Fence").is_some());
        assert!(find(row, "#1").is_some());
        let draft = find(harness.root_id(), "browse row:a/b#2").unwrap();
        assert!(find(draft, "Draft").is_some(), "a draft says so");
        assert!(!shown(&harness, "browse empty"));

        let more = find(harness.root_id(), "browse more")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.click(more.center().x, more.center().y);
        assert_eq!(searches(&mut commands), [(query.clone(), 2, false)]);

        state.polls.update(|n| *n += 1);
        repaint(&mut harness);
        assert_eq!(
            searches(&mut commands),
            [(query.clone(), 1, true)],
            "the poll asks again"
        );

        let closed = find(harness.root_id(), "browse closed")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.click(closed.center().x, closed.center().y);
        repaint(&mut harness);
        assert_eq!(
            searches(&mut commands),
            [("author:@me is:closed archived:false".to_string(), 1, false)]
        );

        crate::ui::sidebar::set_filters(state, |f| {
            f.browse = BrowseView::Repository;
            f.browse_repository = Some("a/b".into());
        });
        repaint(&mut harness);
        assert!(shown(&harness, "browse repository"));
        assert_eq!(
            searches(&mut commands),
            [("repo:a/b is:closed archived:false".to_string(), 1, false)]
        );

        crate::ui::sidebar::set_filters(state, |f| f.browse = BrowseView::Recent);
        repaint(&mut harness);
        assert!(
            !shown(&harness, "browse open"),
            "Recent has no Open · Closed"
        );
        assert!(searches(&mut commands).is_empty(), "Recent is local");
    }

    /// A reference in the search box is the first row, and Enter opens it
    /// through Open…'s box.
    #[test]
    fn a_reference_in_the_search_box_opens() {
        let root = TestRoot::new();
        let state = AppState::new();
        state.sidebar_width.set(theme::SIDEBAR_MIN_WIDTH);
        state.item_filters.update(|f| f.tab = SidebarTab::Browse);
        let (backend, mut commands) = Backend::recording();
        let view = crate::ui::sidebar::sidebar(state, Rc::new(backend), Rc::new(|_| {}));
        let mut harness =
            HeadlessHarness::new_with_size(root, view, theme::SIDEBAR_MIN_WIDTH, 700.0);
        repaint(&mut harness);
        assert!(!shown(&harness, "browse open row"));

        state.item_query.set("https://github.com/a/b/pull/7".into());
        repaint(&mut harness);
        let row = find(harness.root_id(), "browse open row").unwrap();
        assert!(find(row, "Open a/b#7").is_some());

        let rect = row.get_visual_rect_no_clip();
        harness.click(rect.center().x, rect.center().y);
        let opened = std::iter::from_fn(|| commands.try_recv().ok())
            .any(|c| matches!(c, Command::OpenReference { number: 7, .. }));
        assert!(opened);
        assert!(state.open_box.open.get(), "the box says how it goes");
    }
}
