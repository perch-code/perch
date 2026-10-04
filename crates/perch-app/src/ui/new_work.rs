//! The new-work view: a box for what the work is, and under it the
//! repository, the branch to start from, and who the message goes to.
//!
//! `+` at the top of the sidebar and `Cmd-N` open it in the main pane,
//! under a bar titled New worktree whose X shuts it. Start cuts a new
//! branch, named from the message, into a worktree and opens it. What was typed is kept when the
//! view is left without starting, until Perch quits.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use perch_git::BaseBranch;

use crate::backend::{Backend, FirstRun, NewWork};
use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::review::model::{AgentChoice, Destination};
use crate::review::review_launch::{action_button, menu_button};
use crate::review::timeline::short_sha;
use crate::state::AppState;
use crate::theme;
use crate::ui::menu::Menu;
use crate::ui::widgets::{editor_box, icon_button, prose_editor, sidebar_glyph};

/// The branches of the chosen repository, as far as they are known.
#[derive(Debug, Clone, PartialEq)]
pub enum BaseList {
    Unasked,
    Loading {
        repo: String,
    },
    Ready {
        repo: String,
        bases: Vec<BaseBranch>,
    },
    Failed {
        repo: String,
        message: String,
    },
}

/// What the view holds between visits.
#[derive(Clone, Copy)]
pub struct NewWorkState {
    /// Whether the main pane shows the view.
    pub open: RwSignal<bool>,
    /// The `[[repo]]` identity work starts in.
    pub repo: RwSignal<Option<String>>,
    pub bases: RwSignal<BaseList>,
    pub base: RwSignal<Option<BaseBranch>>,
    /// What the work is.
    pub text: RwSignal<String>,
    /// `[work] branch_prefix`.
    pub prefix: RwSignal<String>,
    /// Why the last Start did not go.
    pub error: RwSignal<Option<String>>,
    /// Who the message goes to; `None` until picked, when the composers'
    /// default is used.
    pub agent: RwSignal<Option<AgentChoice>>,
}

impl NewWorkState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            repo: RwSignal::new(None),
            bases: RwSignal::new(BaseList::Unasked),
            base: RwSignal::new(None),
            text: RwSignal::new(String::new()),
            prefix: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            agent: RwSignal::new(None),
        }
    }

    /// The branches `repo` has, from the backend. Anything for a
    /// repository no longer chosen is dropped; the base picks the first,
    /// which is the repository's base branch, unless one is chosen.
    pub fn take_bases(&self, repo: String, bases: Result<Vec<BaseBranch>, String>) {
        if self.repo.get_untracked().as_deref() != Some(repo.as_str()) {
            return;
        }
        match bases {
            Ok(bases) => {
                let keep = self
                    .base
                    .get_untracked()
                    .filter(|base| bases.contains(base));
                self.base.set(keep.or_else(|| bases.first().cloned()));
                self.bases.set(BaseList::Ready { repo, bases });
            }
            Err(message) => {
                self.base.set(None);
                self.bases.set(BaseList::Failed { repo, message });
            }
        }
    }
}

impl Default for NewWorkState {
    fn default() -> Self {
        Self::new()
    }
}

/// Show the view, on the open item's repository when there is one, else
/// on the last one chosen, else the first declared.
pub fn open(state: AppState, backend: &Backend) {
    let nw = state.new_work;
    nw.error.set(None);
    let open_repo = state.review.get_untracked().and_then(|review| {
        review
            .item
            .with_untracked(|item| item.as_ref().map(|item| item.repo_full_name.clone()))
    });
    let declared = state.repos.with_untracked(|repos| {
        repos
            .iter()
            .map(|repo| repo.identity.clone())
            .collect::<Vec<_>>()
    });
    let known = |identity: &String| declared.iter().any(|d| d.eq_ignore_ascii_case(identity));
    let repo = open_repo
        .filter(known)
        .or_else(|| nw.repo.get_untracked().filter(known))
        .or_else(|| declared.first().cloned());
    if let Some(repo) = repo {
        choose_repo(state, backend, &repo);
    }
    nw.open.set(true);
}

/// Pick a repository and ask for its branches, unless they are in hand.
pub fn choose_repo(state: AppState, backend: &Backend, repo: &str) {
    let nw = state.new_work;
    let same = nw.repo.get_untracked().as_deref() == Some(repo);
    let listed = nw.bases.with_untracked(|bases| {
        matches!(bases, BaseList::Ready { repo: r, .. } | BaseList::Loading { repo: r } if r == repo)
    });
    if same && listed {
        return;
    }
    if !same {
        nw.base.set(None);
    }
    nw.repo.set(Some(repo.to_string()));
    nw.bases.set(BaseList::Loading {
        repo: repo.to_string(),
    });
    backend.list_bases(repo);
}

/// Take a repository out of the config. The list loses it now rather
/// than when the backend says, so taking out several in a row chooses
/// from what is left; when it was the one chosen, the first left is.
fn remove_repo(state: AppState, backend: &Backend, identity: &str) {
    backend.remove_repository(identity);
    state
        .repos
        .update(|repos| repos.retain(|r| !r.identity.eq_ignore_ascii_case(identity)));
    let nw = state.new_work;
    if !nw.repo.get_untracked().is_some_and(|r| r.eq_ignore_ascii_case(identity)) {
        return;
    }
    match state.repos.with_untracked(|repos| repos.first().map(|r| r.identity.clone())) {
        Some(next) => choose_repo(state, backend, &next),
        None => {
            nw.repo.set(None);
            nw.base.set(None);
            nw.bases.set(BaseList::Unasked);
        }
    }
}

/// Words that say nothing in a branch name.
const FILLER: &[&str] = &["a", "an", "the", "please"];

/// The new branch's name, from the message: the prefix, then its first
/// five words that say something, lower case, joined by hyphens, with
/// anything git would object to left out. A link stands for what it
/// points at, so `Fix https://github.com/o/r/pull/12` is `fix-pr-12`.
/// `work` when there are no words.
pub fn suggested_branch(prefix: &str, text: &str) -> String {
    const MAX_WORDS: usize = 5;
    const MAX_LEN: usize = 48;
    let words = text
        .split_whitespace()
        .flat_map(|word| {
            if word.starts_with("http://") || word.starts_with("https://") {
                link_words(word)
            } else {
                slug_words(&word.replace(['\'', '\u{2019}', '`', '"'], ""))
            }
        })
        .filter(|word| !FILLER.contains(&word.as_str()));
    let mut slug = String::new();
    for word in words.take(MAX_WORDS) {
        if !slug.is_empty() && slug.len() + 1 + word.len() > MAX_LEN {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    slug.truncate(MAX_LEN);
    if slug.is_empty() {
        slug.push_str("work");
    }
    format!("{prefix}{slug}")
}

/// A word's ASCII letters and digits, lower case, in the runs anything
/// else splits them into.
fn slug_words(word: &str) -> Vec<String> {
    word.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// What a link points at, in a few words: a GitHub pull request, issue,
/// commit or Actions run by its kind and number, anything else by the
/// last part of its path, or its host when it has none.
fn link_words(url: &str) -> Vec<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let host = parts.next().unwrap_or_default().to_ascii_lowercase();
    let path: Vec<&str> = parts.collect();
    if host == "github.com" || host == "www.github.com" {
        let named = match path.as_slice() {
            [_, _, "pull", n, ..] => Some(format!("pr {n}")),
            [_, _, "issues", n, ..] => Some(format!("issue {n}")),
            [_, _, "commit", sha, ..] => Some(format!("commit {}", short_sha(sha))),
            [_, _, "actions", "runs", n, ..] => Some(format!("ci {n}")),
            [_, repo, ..] => Some(repo.to_string()),
            _ => None,
        };
        if let Some(named) = named {
            return slug_words(&named);
        }
    }
    let site = host.trim_start_matches("www.");
    match path.last() {
        Some(last) => slug_words(last.rsplit_once('.').map_or(last, |(stem, _)| stem)),
        None => slug_words(site.split('.').next().unwrap_or_default()),
    }
}

/// What Start would send, or why it cannot.
fn request(state: AppState) -> Result<NewWork, String> {
    let nw = state.new_work;
    let repo = nw
        .repo
        .get()
        .ok_or_else(|| "Choose a repository".to_string())?;
    let base = nw.base.get().ok_or_else(|| {
        nw.bases.with(|bases| match bases {
            BaseList::Loading { .. } => "Listing branches…".to_string(),
            BaseList::Failed { message, .. } => message.clone(),
            _ => "Choose a branch to start from".to_string(),
        })
    })?;
    let text = nw.text.get();
    let branch = suggested_branch(&nw.prefix.get(), &text);
    let run = (!text.trim().is_empty()).then(|| {
        let (backend, model, effort) = chosen_agent(state);
        FirstRun {
            text: text.trim().to_string(),
            backend,
            model,
            effort,
        }
    });
    Ok(NewWork {
        repo,
        base,
        branch,
        run,
    })
}

/// The agent the message goes to: the one picked here, else what a new
/// composer starts on.
fn chosen_agent(state: AppState) -> AgentChoice {
    state
        .new_work
        .agent
        .get()
        .unwrap_or_else(|| crate::review::threads::defaults(state))
}

/// `Claude Opus`, and the effort, as the composers pick them; never
/// GitHub, since new work has nothing on GitHub yet.
fn agent_chips(state: AppState) -> impl IntoView {
    let nw = state.new_work;
    let model = menu_button(
        move || crate::review::threads::destination_label(&Destination::agent(chosen_agent(state))),
        move || {
            let (_, selected, effort) = floem::reactive::Effect::untrack(|| chosen_agent(state));
            let agent = state.agent.get_untracked();
            let mut menu = Menu::new();
            for (i, group) in crate::review::threads::model_groups(&agent).into_iter().enumerate() {
                if i > 0 {
                    menu = menu.separator();
                }
                for (backend, model) in group {
                    let checked = model == selected;
                    let label = crate::review::threads::model_label(&model);
                    let agent = agent.clone();
                    menu = menu.item(label, move |item| {
                        let model = model.clone();
                        let effort = crate::review::threads::nearest_effort(&agent, &model, effort);
                        item.checked(checked)
                            .action(move || nw.agent.set(Some((backend, model.clone(), effort))))
                    });
                }
            }
            menu
        },
        "new-work-agent".into(),
    );
    let effort = menu_button(
        move || chosen_agent(state).2.label().to_string(),
        move || {
            let (backend, model, selected) = floem::reactive::Effect::untrack(|| chosen_agent(state));
            let agent = state.agent.get_untracked();
            let mut menu = Menu::new();
            for effort in perch_agent::Effort::ALL {
                let takes = agent.effort_supported(&model, effort.name());
                let model = model.clone();
                menu = menu.item(effort.label(), move |item| {
                    let model = model.clone();
                    item.checked(effort == selected)
                        .enabled(takes)
                        .action(move || nw.agent.set(Some((backend, model.clone(), effort))))
                });
            }
            menu
        },
        "new-work-effort".into(),
    );
    Stack::horizontal((model, effort)).style(|s| s.items_center().gap(6.0))
}

/// Make the worktree. The draft is cleared once it has gone.
fn start(state: AppState, backend: &Backend) {
    let nw = state.new_work;
    let work = match floem::reactive::Effect::untrack(|| request(state)) {
        Ok(work) => work,
        Err(message) => {
            nw.error.set(Some(message));
            return;
        }
    };
    nw.error.set(None);
    let navigation_id = backend.create_worktree(work);
    nw.text.set(String::new());
    state.begin_open("Adding the worktree", navigation_id);
}

/// The view, in the main pane.
pub fn new_work_view(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let nw = state.new_work;

    let start_backend = backend.clone();
    let start: Rc<dyn Fn()> = Rc::new(move || start(state, &start_backend));
    // Whether there is a message, as a memo: the text changes with every
    // key, and a container keyed on the signal would be built again on
    // each.
    let empty = Memo::new(move |_| nw.text.with(|t| t.trim().is_empty()));

    let column = Stack::vertical((
        message_box(state, start.clone()),
        Stack::horizontal((
            repo_chip(state, backend.clone()),
            base_chip(state),
            dyn_container(
                move || empty.get(),
                move |empty| {
                    if empty {
                        Empty::new().into_any()
                    } else {
                        agent_chips(state).into_any()
                    }
                },
            ),
        ))
        .style(|s| {
            s.items_center()
                .gap(8.0)
                .width_full()
                .flex_wrap(floem::style::FlexWrap::Wrap)
        }),
        Stack::horizontal((
            dyn_container(
                move || nw.error.get(),
                |error| match error {
                    Some(error) => Label::derived(move || error.clone())
                        .style(|s| {
                            s.font_size(theme::TEXT_META)
                                .color(theme::closed())
                                .text_wrap()
                                .min_width(0.0)
                        })
                        .into_any(),
                    None => Empty::new().into_any(),
                },
            )
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0)),
            {
                let start = start.clone();
                dyn_container(
                    move || empty.get(),
                    move |empty| {
                        let start = start.clone();
                        action_button(
                            if empty { "Create worktree" } else { "Start" },
                            "new-work-start",
                            true,
                            move || request(state).is_ok(),
                            move || start(),
                        )
                        .into_any()
                    },
                )
            },
        ))
        .style(|s| s.items_center().width_full().gap(12.0)),
    ))
    .style(|s| {
        s.flex_col()
            .gap(12.0)
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
    });

    Container::new(column).debug_name("new-work").style(|s| {
        s.size_full()
            .justify_center()
            .padding_top(72.0)
            .padding_horiz(16.0)
            .background(theme::canvas())
    })
}

/// The view's title in the bar, where an item's goes: the branch glyph and
/// New worktree.
pub fn bar_title() -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Branch, theme::fg_muted).style(|s| s.size(16.0, 16.0)),
        Label::derived(|| "New worktree".to_string())
            .debug_name("new-work-title")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg())
                    .text_ellipsis()
                    .min_width(0.0)
            }),
    ))
    .style(|s| s.items_center().gap(8.0).min_width(0.0))
}

/// Shut the view, with what was typed kept, as Escape and Cmd-[ do.
pub fn close_button(state: AppState) -> impl IntoView {
    tip(
        icon_button(
            || SidebarIcon::Close,
            theme::fg_muted,
            move || state.new_work.open.set(false),
        ),
        "Close (⌘[)".to_string(),
    )
    .debug_name("new-work-close")
    .style(|s| s.flex_shrink(0.0_f32).pointer_events_auto())
}

/// What the work is. Cmd-Enter starts it; Escape leaves the view with
/// the text kept.
fn message_box(state: AppState, start: Rc<dyn Fn()>) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;

    use crate::review::threads::cmd_enter;

    let nw = state.new_work;
    let editor = prose_editor(
        nw.text.get_untracked(),
        "Describe the work, or leave it empty for just the worktree",
        None,
    );
    let editor = editor
        .pre_command(move |cmd| {
            if cmd_enter(&cmd) {
                start();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(editor) = update.editor {
                nw.text.set(editor.doc().text().to_string());
            }
        })
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    nw.open.set(false);
                    EventPropagation::Stop
                } else {
                    EventPropagation::Continue
                }
            },
        )
        .style(|s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
        })
        .debug_name("new-work-message");
    // The box takes the keyboard as the view opens.
    let inner = editor.editor().editor_view_id;
    let asked = std::rc::Rc::new(std::cell::Cell::new(false));
    let editor = editor.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            if let Some(id) = inner.get_untracked() {
                id.request_focus();
            }
        }
    });
    editor_box(editor, || (120.0, 320.0), true)
}

/// The repository chip: every `[[repo]]`, each with a × to take it out,
/// then Add repository….
fn repo_chip(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let nw = state.new_work;
    menu_button(
        move || {
            let chosen = nw.repo.get();
            state.repos.with(|repos| {
                chosen
                    .as_ref()
                    .and_then(|identity| {
                        repos
                            .iter()
                            .find(|r| r.identity.eq_ignore_ascii_case(identity))
                            .map(|r| r.name.clone())
                    })
                    .unwrap_or_else(|| "Choose a repository".to_string())
            })
        },
        move || {
            let chosen = nw.repo.get_untracked();
            let mut menu = Menu::new();
            let repos = state.repos.get_untracked();
            for repo in repos {
                let label = if perch_github::is_local(&repo.identity) {
                    format!("{}  (local)", repo.name)
                } else {
                    format!("{}  ({})", repo.name, repo.identity)
                };
                let checked = chosen
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&repo.identity));
                let backend = backend.clone();
                let identity = repo.identity.clone();
                menu = menu.item(label, move |item| {
                    let (pick, remove) = (backend.clone(), backend.clone());
                    let (picked, removed) = (identity.clone(), identity.clone());
                    item.checked(checked)
                        .action(move || choose_repo(state, &pick, &picked))
                        .removable(move || remove_repo(state, &remove, &removed))
                });
            }
            menu = menu.separator();
            let backend = backend.clone();
            menu.item("Add repository…", move |item| {
                let backend = backend.clone();
                item.action(move || add_repository(backend.clone()))
            })
        },
        "new-work-repo".into(),
    )
}

/// The system folder chooser, then the backend adds what was picked.
fn add_repository(backend: Rc<Backend>) {
    use floem::platform::file::FileDialogOptions;

    floem::action::open_file(
        FileDialogOptions::new()
            .select_directories()
            .title("Add repository"),
        move |picked| {
            if let Some(path) = picked.and_then(|info| info.path.into_iter().next()) {
                backend.add_repository(path);
            }
        },
    );
}

/// The base chip: the branches of the chosen repository.
fn base_chip(state: AppState) -> impl IntoView {
    let nw = state.new_work;
    menu_button(
        move || match nw.base.get() {
            Some(base) => base.label(),
            None => nw.bases.with(|bases| match bases {
                BaseList::Loading { .. } => "Listing branches…".to_string(),
                BaseList::Failed { .. } => "No branches".to_string(),
                _ => "Choose a branch".to_string(),
            }),
        },
        move || {
            let chosen = nw.base.get_untracked();
            let mut menu = Menu::new();
            match nw.bases.get_untracked() {
                BaseList::Ready { bases, .. } => {
                    for base in bases {
                        let checked = chosen.as_ref() == Some(&base);
                        let label = base.label();
                        menu = menu.item(label, move |item| {
                            let base = base.clone();
                            item.checked(checked)
                                .action(move || nw.base.set(Some(base.clone())))
                        });
                    }
                }
                BaseList::Failed { message, .. } => {
                    menu = menu.item(message, |item| item.enabled(false));
                }
                BaseList::Loading { .. } => {
                    menu = menu.item("Listing branches…", |item| item.enabled(false));
                }
                BaseList::Unasked => {
                    menu = menu.item("Choose a repository first", |item| item.enabled(false));
                }
            }
            menu
        },
        "new-work-base".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn app() -> AppState {
        let app = AppState::new();
        app.repos.set(vec![
            perch_github::DeclaredRepo {
                identity: "local/notes".into(),
                name: "notes".into(),
            },
            perch_github::DeclaredRepo {
                identity: "acme/widgets".into(),
                name: "widgets".into(),
            },
        ]);
        app
    }

    #[test]
    fn opening_chooses_the_first_repository_and_its_base_branch() {
        let app = app();
        let backend = Backend::detached();
        open(app, &backend);
        assert!(app.new_work.open.get_untracked());
        assert_eq!(app.new_work.repo.get_untracked().as_deref(), Some("local/notes"));
        assert_eq!(
            request(app).unwrap_err(),
            "Listing branches…",
            "nothing to start from yet"
        );

        // An answer for a repository no longer chosen is dropped.
        app.new_work
            .take_bases("acme/widgets".into(), Ok(vec![BaseBranch::on("origin", "main")]));
        assert_eq!(app.new_work.base.get_untracked(), None);

        app.new_work.take_bases(
            "local/notes".into(),
            Ok(vec![BaseBranch::local("main"), BaseBranch::local("feature")]),
        );
        app.new_work.prefix.set("chris/".into());
        app.new_work.text.set("Add a README".into());
        let work = request(app).unwrap();
        assert_eq!(work.repo, "local/notes");
        assert_eq!(work.base, BaseBranch::local("main"));
        assert_eq!(work.branch, "chris/add-readme");
    }

    #[test]
    fn the_view_lays_out_its_box_and_chips_in_one_column() {
        let app = app();
        app.new_work.prefix.set("chris/".into());
        app.new_work.repo.set(Some("local/notes".into()));
        app.new_work.take_bases("local/notes".into(), Ok(vec![BaseBranch::local("main")]));
        let root = TestRoot::new();
        let view = new_work_view(app, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1100.0, 700.0);
        harness.rebuild();
        harness.paint();

        let root = harness.root_id();
        let rect = |name: &str| {
            named(root, name)
                .unwrap_or_else(|| panic!("{name} is drawn"))
                .get_visual_rect_no_clip()
        };
        let message = rect("new-work-message");
        let repo = rect("new-work-repo");
        let start = rect("new-work-start");
        assert!(repo.y0 >= message.y1, "the chips are under the box");
        assert!(start.y0 >= repo.y1, "Start is under the chips");
        assert!(start.x1 <= 1100.0, "{start:?}");
        assert!(message.width() <= theme::CONVERSATION_WIDTH + 1.0);
        assert!(named(root, "new-work-agent").is_none(), "no agent without a message");

        // With a message, who it goes to sits beside the repository.
        app.new_work.text.set("Add a README".into());
        harness.rebuild();
        harness.paint();
        let agent = rect("new-work-agent");
        let effort = rect("new-work-effort");
        assert!((agent.y0 - repo.y0).abs() < 1.0, "{agent:?} {repo:?}");
        assert!((effort.y0 - repo.y0).abs() < 1.0, "{effort:?} {repo:?}");
        assert!(effort.x1 <= 1100.0, "{effort:?}");
    }

    #[test]
    fn a_branch_name_is_the_prefix_and_the_first_five_words() {
        assert_eq!(
            suggested_branch("chris/", "Add a README with a one-line description"),
            "chris/add-readme-with-one-line"
        );
        assert_eq!(
            suggested_branch("", "Fix `slugify`'s edge-case: émoji!"),
            "fix-slugifys-edge-case-moji"
        );
        assert_eq!(suggested_branch("chris/", "   "), "chris/work");
        assert_eq!(suggested_branch("", "-- !!"), "work");
        let long = "internationalization ".repeat(5);
        assert_eq!(suggested_branch("", &long), "internationalization-internationalization");
    }

    #[test]
    fn a_link_in_the_message_names_what_it_points_at() {
        assert_eq!(
            suggested_branch("", "Fix https://github.com/slatedb/slatedb/actions/runs/36595615717/job/109499634151"),
            "fix-ci-36595615717"
        );
        assert_eq!(
            suggested_branch("", "Review https://github.com/acme/widgets/pull/12#discussion_r1"),
            "review-pr-12"
        );
        assert_eq!(
            suggested_branch("", "Do https://github.com/acme/widgets/issues/7"),
            "do-issue-7"
        );
        assert_eq!(
            suggested_branch("", "Revert https://github.com/acme/widgets/commit/0123456789abcdef"),
            "revert-commit-0123456"
        );
        assert_eq!(
            suggested_branch("", "Read https://docs.rs/tokio/latest/tokio/fn.spawn.html please"),
            "read-fn-spawn"
        );
        assert_eq!(suggested_branch("", "See https://example.com/"), "see-example");
    }
}
