//! The scope menu at the top of the rail: what range the diff is over.
//!
//! A button naming the scope and its churn, and a native menu of the
//! others. Which entries are greyed, and why, is decided from a handful
//! of facts about the checkout, pulled out of the state so the rule can
//! be tested without a window. A native menu has no hover text, so a
//! greyed entry says why in its title.

use std::rc::Rc;
use std::sync::Arc;

use floem::menu::{HasCheck, ItemBuilder, Menu};
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, Empty, Label, Stack, dyn_container};
use jiff::Timestamp;
use perch_git::{Commit, Dirt, Scope};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::sidebar_glyph;

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub scope: Scope,
    /// The name, with the reason in parentheses when greyed.
    pub title: String,
    pub enabled: bool,
    /// The current scope carries the check.
    pub checked: bool,
}

/// What the entries are decided from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub current: Scope,
    /// The head at the previous open, `None` before the first.
    pub last_seen_head: Option<String>,
    pub head_sha: String,
    /// Uncommitted work is only reachable in a checkout that is on disk.
    pub on_disk: bool,
    pub dirty: Dirt,
    /// The pull request moved while the worktree had work in it, so what
    /// is on disk is against an older head. The notice in the header says
    /// so.
    pub stale_head: bool,
    /// Newest first, as the log gives them.
    pub commits: Arc<Vec<Commit>>,
    pub now: Timestamp,
}

impl Facts {
    /// Untracked reads: the menu is built from event dispatch, on every
    /// open, which is exactly when the facts have to be fresh.
    fn from_state(state: ReviewState) -> Option<Facts> {
        let checkout = state.item.with_untracked(|i| i.as_ref().and_then(|i| i.checkout.clone()))?;
        Some(Facts {
            current: state.scope.get_untracked(),
            last_seen_head: state.since_head.get_untracked(),
            head_sha: checkout.head_sha,
            on_disk: checkout.on_disk,
            dirty: state.dirty.get_untracked(),
            stale_head: state.stale_head.with_untracked(Option::is_some),
            commits: state.commits.get_untracked(),
            now: Timestamp::now(),
        })
    }
}

/// How much of a commit's subject a menu row shows.
const SUBJECT_CHARS: usize = 60;

/// The entries in menu order: since you last looked, the uncommitted
/// cuts, one per commit, then the branch. The uncommitted cuts are left
/// out, not greyed, when the checkout is not on disk.
pub fn entries(facts: &Facts) -> Vec<Entry> {
    let entry = |scope: Scope, title: String, enabled: bool| Entry {
        checked: facts.current == scope,
        scope,
        title,
        enabled,
    };
    let mut out = Vec::new();

    out.push(match &facts.last_seen_head {
        Some(from) if *from != facts.head_sha => {
            entry(Scope::SinceLastLooked { from: from.clone() }, "Since you last looked".into(), true)
        }
        Some(from) => entry(
            Scope::SinceLastLooked { from: from.clone() },
            "Since you last looked (no new commits)".into(),
            false,
        ),
        None => entry(
            Scope::SinceLastLooked { from: facts.head_sha.clone() },
            "Since you last looked (first look)".into(),
            false,
        ),
    });

    if facts.on_disk {
        // The cuts follow their parent: a child enabled under a greyed
        // Uncommitted would read as a contradiction.
        let cut = |scope: Scope, name: &str, has: bool, why: &str| {
            let (enabled, reason) = if facts.stale_head {
                (false, " (head moved)")
            } else if has {
                (true, "")
            } else {
                (false, why)
            };
            entry(scope, format!("{name}{reason}"), enabled)
        };
        out.push(cut(Scope::Uncommitted, "Uncommitted", facts.dirty.any(), " (clean)"));
        out.push(cut(Scope::Unstaged, "   Unstaged", facts.dirty.unstaged, " (nothing unstaged)"));
        out.push(cut(Scope::Staged, "   Staged", facts.dirty.staged, " (nothing staged)"));
    }

    for commit in facts.commits.iter() {
        let title = format!(
            "{}  {}  \u{00b7}  {}",
            commit.short_sha(),
            truncated(&commit.subject, SUBJECT_CHARS),
            timefmt::relative(commit.authored_at, facts.now)
        );
        out.push(entry(Scope::Commit { sha: commit.sha.clone() }, title, true));
    }

    out.push(entry(Scope::Branch, "Branch".into(), true));
    out
}

/// The first `max` characters, with an ellipsis when that is not all of
/// them.
fn truncated(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}\u{2026}", head.trim_end())
    } else {
        head
    }
}

/// The menu, built fresh on every open. Picking an entry only asks the
/// backend; the list and the scope arrive together with `ScopeReady`.
pub fn menu(state: ReviewState, backend: Rc<Backend>, item_id: String) -> Menu {
    let Some(facts) = Facts::from_state(state) else {
        return Menu::new();
    };
    let (commits, rest): (Vec<Entry>, Vec<Entry>) =
        entries(&facts).into_iter().partition(|e| matches!(e.scope, Scope::Commit { .. }));
    let pick: Rc<dyn Fn(Scope)> = Rc::new(move |scope| backend.set_scope(&item_id, scope));

    // Three groups: the range since the last look, the work on the
    // branch in its cuts, and the branch they add up to.
    let mut menu = Menu::new();
    for e in rest.iter().filter(|e| e.scope != Scope::Branch) {
        if e.scope == Scope::Uncommitted {
            menu = menu.separator();
        }
        menu = menu.item(&e.title, row(e, &pick));
    }
    let has_commits = !commits.is_empty();
    let title = if has_commits { "Committed" } else { "Committed (no commits)" };
    let commit_pick = pick.clone();
    menu = menu.submenu(title, move |mut sub| {
        for e in &commits {
            sub = sub.item(&e.title, row(e, &commit_pick));
        }
        sub.enabled(has_commits)
    });
    menu = menu.separator();
    if let Some(branch) = rest.iter().find(|e| e.scope == Scope::Branch) {
        menu = menu.item(&branch.title, row(branch, &pick));
    }
    menu
}

/// One row's properties: its check, its greying, and what picking it does.
fn row(e: &Entry, pick: &Rc<dyn Fn(Scope)>) -> impl FnOnce(ItemBuilder) -> ItemBuilder<HasCheck> {
    let (pick, scope, checked, enabled) = (pick.clone(), e.scope.clone(), e.checked, e.enabled);
    move |m| m.checked(checked).enabled(enabled).action(move || pick(scope.clone()))
}

/// The button: the scope's name, the changed list's churn, and a chevron.
/// A click opens the menu.
pub fn button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let item_id = move || state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone()));
    // Read as a pair so a scope change repaints the name once, when the
    // list lands with it, rather than once for the scope and once for the
    // commits.
    let named = floem::reactive::Memo::new(move |_| {
        let scope = state.scope.get();
        let subject = match &scope {
            Scope::Commit { sha } => {
                state.commits.with(|c| c.iter().find(|c| c.sha == *sha).map(|c| c.subject.clone()))
            }
            _ => None,
        };
        (scope, subject)
    });
    let churn = move || {
        state.changed_files.with(|files| {
            files.iter().fold((0u32, 0u32), |(a, d), f| (a + f.additions, d + f.deletions))
        })
    };

    Stack::horizontal((
        dyn_container(
            move || named.get(),
            move |(scope, subject)| match subject {
                Some(subject) => Stack::horizontal((
                    Label::derived(move || scope.label()).style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .font_family(theme::CODE_FONT_STACK.to_string())
                            .color(theme::FG_MUTED)
                            .margin_right(7.0)
                            .flex_shrink(0.0_f32)
                    }),
                    Label::derived(move || subject.clone()).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::FG)
                            .text_ellipsis()
                            .min_width(0.0)
                    }),
                ))
                .style(|s| s.items_center().min_width(0.0).pointer_events_none())
                .into_any(),
                None => Label::derived(move || scope.label())
                    .style(|s| s.font_size(theme::TEXT_META).color(theme::FG).text_ellipsis().min_width(0.0))
                    .into_any(),
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0).pointer_events_none()),
        churn_labels(churn),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
            .style(|s| s.size(10.0, 10.0).margin_left(8.0).flex_shrink(0.0_f32).pointer_events_none()),
    ))
    .on_event_cont(el::PointerEnter, {
        let backend = backend.clone();
        move |_, _| {
            // The dirt and the commits were read when the item opened;
            // the reader may have edited or committed since. A look now,
            // as the pointer arrives, lands before the menu opens.
            if let Some(id) = item_id() {
                backend.refresh_scope(&id);
            }
        }
    })
    .popout_menu(move || match item_id() {
        Some(id) => menu(state, backend.clone(), id),
        None => Menu::new(),
    })
    .style(|s| {
        s.height(theme::SCOPE_BUTTON_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(10.0)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
            .selectable(false)
            .hover(|s| s.background(theme::CANVAS_HOVER).cursor(CursorStyle::Pointer))
    })
}

/// "+98 -26" in the status colours; nothing for an empty list.
fn churn_labels(churn: impl Fn() -> (u32, u32) + 'static + Copy) -> impl IntoView {
    // A memo, so the labels are only rebuilt when the list empties or
    // fills, and repaint in place as the numbers change.
    let empty = floem::reactive::Memo::new(move |_| churn() == (0, 0));
    dyn_container(
        move || empty.get(),
        move |empty| {
            if empty {
                return Empty::new().into_any();
            }
            Stack::horizontal((
                Label::derived(move || format!("+{}", churn().0))
                    .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::STATUS_ADDED)),
                Label::derived(move || format!("-{}", churn().1))
                    .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::STATUS_DELETED).margin_left(5.0)),
            ))
            .style(|s| s.items_center())
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32).margin_left(8.0).pointer_events_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, subject: &str, ago_secs: i64, now: Timestamp) -> Commit {
        Commit {
            sha: sha.into(),
            subject: subject.into(),
            body: String::new(),
            author: "rohan".into(),
            authored_at: now - jiff::SignedDuration::from_secs(ago_secs),
        }
    }

    fn facts() -> Facts {
        let now: Timestamp = "2026-09-18T12:00:00Z".parse().unwrap();
        Facts {
            current: Scope::Branch,
            last_seen_head: Some("aaaaaaa1".into()),
            head_sha: "bbbbbbb2".into(),
            on_disk: true,
            dirty: Dirt { unstaged: true, staged: false },
            stale_head: false,
            commits: Arc::new(vec![
                commit("bbbbbbb2", "Fence the manifest", 3600, now),
                commit("ccccccc3", "Add the fence", 2 * 86_400, now),
            ]),
            now,
        }
    }

    fn find<'a>(entries: &'a [Entry], scope: &Scope) -> &'a Entry {
        entries.iter().find(|e| e.scope == *scope).unwrap_or_else(|| panic!("no entry for {scope}"))
    }

    #[test]
    fn the_menu_is_in_the_spec_order_with_the_branch_checked() {
        let entries = entries(&facts());
        let scopes: Vec<String> = entries.iter().map(|e| e.scope.to_string()).collect();
        assert_eq!(
            scopes,
            vec![
                "since:aaaaaaa1",
                "uncommitted",
                "unstaged",
                "staged",
                "commit:bbbbbbb2",
                "commit:ccccccc3",
                "branch"
            ]
        );
        let branch = find(&entries, &Scope::Branch);
        assert!(branch.enabled && branch.checked);
        assert_eq!(entries.iter().filter(|e| e.checked).count(), 1);
    }

    #[test]
    fn since_you_last_looked_needs_a_head_that_moved() {
        let moved = entries(&facts());
        let since = find(&moved, &Scope::SinceLastLooked { from: "aaaaaaa1".into() });
        assert!(since.enabled);
        assert_eq!(since.title, "Since you last looked");

        let same = entries(&Facts { last_seen_head: Some("bbbbbbb2".into()), ..facts() });
        let since = find(&same, &Scope::SinceLastLooked { from: "bbbbbbb2".into() });
        assert!(!since.enabled);
        assert_eq!(since.title, "Since you last looked (no new commits)");

        let first = entries(&Facts { last_seen_head: None, ..facts() });
        assert!(!first[0].enabled);
        assert_eq!(first[0].title, "Since you last looked (first look)");
    }

    #[test]
    fn the_uncommitted_cuts_follow_the_dirt() {
        let entries = entries(&facts());
        assert!(find(&entries, &Scope::Uncommitted).enabled);
        assert!(find(&entries, &Scope::Unstaged).enabled);
        let staged = find(&entries, &Scope::Staged);
        assert!(!staged.enabled);
        assert_eq!(staged.title, "   Staged (nothing staged)");

        let clean = super::entries(&Facts { dirty: Dirt::default(), ..facts() });
        let uncommitted = find(&clean, &Scope::Uncommitted);
        assert!(!uncommitted.enabled);
        assert_eq!(uncommitted.title, "Uncommitted (clean)");
        assert_eq!(find(&clean, &Scope::Unstaged).title, "   Unstaged (nothing unstaged)");
    }

    #[test]
    fn a_stale_head_greys_the_uncommitted_cuts_whatever_the_dirt() {
        let entries = entries(&Facts { stale_head: true, ..facts() });
        for scope in [Scope::Uncommitted, Scope::Unstaged, Scope::Staged] {
            let e = find(&entries, &scope);
            assert!(!e.enabled, "{scope} should be greyed");
            assert!(e.title.ends_with("(head moved)"), "{}", e.title);
        }
    }

    #[test]
    fn the_uncommitted_cuts_are_left_out_off_disk() {
        let entries = entries(&Facts { on_disk: false, ..facts() });
        assert!(!entries.iter().any(|e| matches!(e.scope, Scope::Uncommitted | Scope::Unstaged | Scope::Staged)));
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn commits_are_rows_with_a_sha_a_subject_and_a_time_and_none_without() {
        let entries = entries(&Facts { current: Scope::Commit { sha: "ccccccc3".into() }, ..facts() });
        let rows: Vec<&Entry> = entries.iter().filter(|e| matches!(e.scope, Scope::Commit { .. })).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "bbbbbbb  Fence the manifest  \u{00b7}  an hour ago");
        assert!(rows[0].enabled && !rows[0].checked);
        assert!(rows[1].checked);

        let none = super::entries(&Facts { commits: Arc::new(Vec::new()), ..facts() });
        assert!(!none.iter().any(|e| matches!(e.scope, Scope::Commit { .. })));
    }

    #[test]
    fn a_long_subject_is_cut_with_an_ellipsis() {
        assert_eq!(truncated("short", 60), "short");
        let long = "a".repeat(61);
        let cut = truncated(&long, 60);
        assert_eq!(cut.chars().count(), 61);
        assert!(cut.ends_with('\u{2026}'));
        // No gap before the ellipsis when the cut lands on a space.
        assert_eq!(truncated("cut here please", 9), "cut here\u{2026}");
    }
}
