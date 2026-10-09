//! Browse: GitHub's searches.
//!
//! The filter menu picks the view, the repository, the state and the
//! order. A view is searched when it is picked, and again after each poll
//! while it is on screen; its results live in `AppState::browse`, never in
//! the database.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalWith};
use floem::views::{dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack};
use perch_github::{ItemKind, SearchHit, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::state::AppState;
use crate::theme;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::nav_row;
use crate::ui::widgets::{open_in_browser, sidebar_glyph, state_glyph};

/// The Browse tab: the row Open… would take, over the view's results.
pub fn browse(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        open_row(state, backend.clone()),
        search_list(state, backend).style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
    ))
    .style(|s| s.flex_col().width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// The first row while the search box holds something Open… reads: a
/// URL, `owner/repo#57` or `#57`. It opens as a result's row does, under
/// the progress card, which says what is wrong if it can't.
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
                sidebar_glyph(SidebarIcon::Search, theme::nav_muted).style(|s| s.size(14.0, 14.0)),
            ),
            Label::derived(move || label().unwrap_or_default()).style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .color(theme::nav_fg())
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
    let query = Memo::new(move |_| state.browse_query());
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
                    .color(theme::nav_muted())
                    .padding_horiz(theme::NAV_GUTTER + 8.0)
                    .padding_bottom(8.0)
                    .text_wrap()
                    .width_full()
                    .apply_if(!shown, |s| s.hide())
            }),
        Label::derived(move || {
            if query.get().is_none() {
                let local = state
                    .item_filters
                    .with(|f| f.repository.as_deref().is_some_and(perch_github::is_local));
                return if local {
                    "Not a repository on GitHub"
                } else {
                    "Pick a repository"
                }
                .to_string();
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
                .color(theme::nav_subtle())
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
                    .color(theme::nav_muted())
                    .margin_left(theme::NAV_GUTTER)
                    .margin_top(8.0)
                    .padding_horiz(10.0)
                    .padding_vert(6.0)
                    .border_radius(14.0)
                    .hover(|s| s.background(theme::nav_hover()).color(theme::nav_fg()))
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
        nav_row::glyph_box(state_glyph(subject_state, move || {
            icons::state_color(subject_state)
        })),
        nav_row::lines(
            nav_row::title_label(hit.title.clone(), false, unread.clone()),
            Stack::horizontal((
                nav_row::unread_dot(unread),
                nav_row::age(state, hit.updated_at),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32)),
            nav_row::byline(
                state,
                {
                    // Who acted last once a look has found them; whoever
                    // opened it until then.
                    let author = nav_row::Face::new(avatar, hit.author_login.clone());
                    let key = (repo.clone(), number);
                    move || {
                        state
                            .participants
                            .with(|found| {
                                found
                                    .get(&key)
                                    .filter(|p| !p.is_empty())
                                    .map(|p| nav_row::participant_faces(p))
                            })
                            .unwrap_or_else(|| vec![author.clone()])
                    }
                },
                repo.clone(),
                nav_row::reference(Some(number)),
                nav_row::note(note.map(str::to_string), theme::nav_muted),
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
    let menu_backend = backend.clone();
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
            crate::review::notified::follow_subject(state, &open_repo, kind, number);
        })
        .right_click_menu(move || {
            search_menu(state, menu_backend.clone(), &repo, kind, number, &url)
        })
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
}

/// A result's menu: Move to Inbox while its notification is Done or
/// snoozed, as that view's own row offers, and Open on GitHub.
fn search_menu(
    state: AppState,
    backend: Rc<Backend>,
    repo: &str,
    kind: ItemKind,
    number: u64,
    url: &str,
) -> Menu {
    let mut menu = Menu::new();
    if let Some(id) = state.put_away_thread(repo, kind, number) {
        menu = menu
            .item("Move to Inbox", move |item| {
                let backend = backend.clone();
                let id = id.clone();
                item.icon(SidebarIcon::Inbox)
                    .action(move || backend.restore_notification(&id))
            })
            .separator();
    }
    let url = url.to_string();
    menu.item("Open on GitHub", move |m| {
        let url = url.clone();
        m.icon(SidebarIcon::External)
            .action(move || open_in_browser(&url))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Command, Event};
    use crate::items::{BrowseView, ItemSort, SidebarTab, StateFilter};
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
    /// asks for the next page, and a poll for the view again. The menu's
    /// State, Repository and Sort By each search anew.
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
            f.browse.view = BrowseView::Yours;
        });
        let (backend, mut commands) = Backend::recording();
        let view = crate::ui::sidebar::sidebar(state, Rc::new(backend), Rc::new(|_| {}));
        let mut harness =
            HeadlessHarness::new_with_size(root, view, theme::SIDEBAR_MIN_WIDTH, 700.0);
        repaint(&mut harness);

        let query = "author:@me is:open archived:false".to_string();
        assert_eq!(searches(&mut commands), [(query.clone(), 1, false)]);
        assert!(shown(&harness, "browse empty"), "loading");
        assert!(
            shown(&harness, "sidebar filter"),
            "Browse has the menu every tab has"
        );

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

        crate::ui::sidebar::set_filters(state, |f| f.browse.state = StateFilter::Closed);
        repaint(&mut harness);
        assert_eq!(
            searches(&mut commands),
            [("author:@me is:closed archived:false".to_string(), 1, false)]
        );

        crate::ui::sidebar::set_filters(state, |f| {
            f.browse.view = BrowseView::All;
            f.browse.order.sort = ItemSort::Created;
        });
        repaint(&mut harness);
        assert_eq!(
            searches(&mut commands),
            [(
                "repo:a/b is:closed archived:false sort:created-desc".to_string(),
                1,
                false
            )]
        );

        // A repository that is not on GitHub has nothing to search.
        crate::ui::sidebar::set_filters(state, |f| f.repository = Some("local/notes".into()));
        repaint(&mut harness);
        assert!(searches(&mut commands).is_empty());
        assert!(find(harness.root_id(), "Not a repository on GitHub").is_some());
    }

    /// A result whose notification is Done, or snoozed, moves it back to
    /// the inbox from its menu; one the inbox still has offers nothing.
    #[test]
    fn a_put_away_result_moves_back_to_the_inbox() {
        use perch_github::{Reason, Repository, Subject, SubjectKind, Thread};
        let thread = |id: &str, number: u64| Thread {
            id: id.into(),
            unread: false,
            local_unread: false,
            local_read: false,
            reason: Reason::Mention,
            updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: "Fence".into(),
                kind: SubjectKind::PullRequest,
                url: Some(format!("https://api.github.com/repos/a/b/pulls/{number}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "a/b".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        };
        let state = AppState::new();
        state.notifications.set(vec![thread("10", 1)]);
        state.done_notifications.set(vec![thread("20", 2)]);
        state.snoozed_notifications.set(vec![thread("30", 3)]);
        let (backend, mut commands) = Backend::recording();
        let backend = Rc::new(backend);
        let menu = |number| {
            search_menu(
                state,
                backend.clone(),
                "a/b",
                ItemKind::PullRequest,
                number,
                "",
            )
        };

        assert_eq!(menu(1).outline(), ["Open on GitHub"], "still in the inbox");
        assert_eq!(menu(4).outline(), ["Open on GitHub"], "no notification");
        assert_eq!(menu(2).outline(), ["Move to Inbox", "—", "Open on GitHub"]);
        menu(2).choose("Move to Inbox");
        menu(3).choose("Move to Inbox");
        let restored: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|c| match c {
                Command::RestoreNotification { thread_id } => Some(thread_id),
                _ => None,
            })
            .collect();
        assert_eq!(restored, ["20", "30"]);
    }

    /// A reference in the search box is the first row, and a click opens it
    /// under the progress card, with no box.
    #[test]
    fn a_reference_in_the_search_box_opens() {
        let root = TestRoot::new();
        let state = AppState::new();
        let review = crate::review::model::ReviewState::new();
        state.review.set(Some(review));
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
        assert!(!state.open_box.open.get(), "no box goes up");
        assert_eq!(review.phase.get(), crate::review::model::Phase::Preparing);
    }
}
