//! The new-work view: a box for what the work is, and under it the
//! repository, the branch to start from, and the new branch's name.
//!
//! `+` at the top of the sidebar and `Cmd-N` open it in the main pane,
//! under a bar titled New worktree whose X shuts it. Start cuts the new
//! branch into a worktree and opens it. What was typed is kept when the
//! view is left without starting, until Perch quits.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, text_editor, Container, Decorators, Empty, Label, Stack, TextInput,
};
use perch_git::BaseBranch;

use crate::backend::{Backend, FirstRun, NewWork};
use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::review::model::{AgentChoice, Destination};
use crate::review::review_launch::{action_button, menu_button};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{editor_box, icon_button, lay_out_every_line, sidebar_glyph};

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
    /// The new branch's name.
    pub name: RwSignal<String>,
    /// The reader has typed in the name box, so it is no longer filled
    /// in from the message.
    pub name_typed: RwSignal<bool>,
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
            name: RwSignal::new(String::new()),
            name_typed: RwSignal::new(false),
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

/// The new branch's name as the message suggests it: the prefix, then
/// the first five words, lower case, joined by hyphens, with anything
/// git would object to left out. `work` when there are no words.
pub fn suggested_branch(prefix: &str, text: &str) -> String {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .take(5)
        .collect();
    let slug = if words.is_empty() {
        "work".to_string()
    } else {
        words.join("-")
    };
    format!("{prefix}{slug}")
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
    let branch = nw.name.get().trim().to_string();
    if branch.is_empty() {
        return Err("Name the new branch".to_string());
    }
    if branch.contains(char::is_whitespace) {
        return Err("A branch name has no spaces".to_string());
    }
    let text = nw.text.get();
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
        typed: nw.name_typed.get(),
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

/// `Claude · Opus`, and the effort, as the composers pick them; never
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
    nw.name_typed.set(false);
    state.begin_open("Adding the worktree", navigation_id);
}

/// The view, in the main pane.
pub fn new_work_view(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let nw = state.new_work;

    // The name follows the message until the reader types in it.
    Effect::new(move |_| {
        let text = nw.text.get();
        let prefix = nw.prefix.get();
        if !nw.name_typed.get_untracked() {
            nw.name.set(suggested_branch(&prefix, &text));
        }
    });

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
            branch_chip(state),
        ))
        .style(|s| {
            s.items_center()
                .gap(8.0)
                .width_full()
                .flex_wrap(floem::style::FlexWrap::Wrap)
        }),
        Stack::horizontal((
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
            dyn_container(
                move || nw.error.get(),
                |error| match error {
                    Some(error) => Label::derived(move || error.clone())
                        .style(|s| {
                            s.font_size(theme::TEXT_META)
                                .color(theme::CLOSED)
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
            .background(theme::CANVAS)
    })
}

/// The view's title in the bar, where an item's goes: the branch glyph and
/// New worktree.
pub fn bar_title() -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Branch, theme::FG_MUTED).style(|s| s.size(16.0, 16.0)),
        Label::derived(|| "New worktree".to_string())
            .debug_name("new-work-title")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG)
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
            || theme::FG_MUTED,
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
    let editor = text_editor(nw.text.get_untracked())
        .placeholder("Describe the work, or leave it empty for just the worktree".to_string())
        .editor_style(|s| s.hide_gutter(true).selection_color(theme::SELECTION_BG));
    lay_out_every_line(editor.editor());
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

/// The repository chip: every `[[repo]]`, then Add repository….
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
                    let backend = backend.clone();
                    let identity = identity.clone();
                    item.checked(checked)
                        .action(move || choose_repo(state, &backend, &identity))
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

/// The new branch's name.
fn branch_chip(state: AppState) -> impl IntoView {
    let nw = state.new_work;
    Stack::horizontal((
        Label::derived(|| "New branch".to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG)
                .selectable(false)
                .margin_right(8.0)
        }),
        TextInput::new(nw.name)
            .placeholder("branch name")
            .debug_name("new-work-branch")
            .on_event_cont(el::KeyDown, move |_, _| nw.name_typed.set(true))
            .on_event_cont(el::FocusGained, move |_, _| state.search_focused.set(true))
            .on_event_cont(el::FocusLost, move |_, _| state.search_focused.set(false))
            .style(|s| {
                s.width(220.0)
                    .height(26.0)
                    .font_size(theme::TEXT_BADGE)
                    .padding_horiz(6.0)
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER)
                    .border_radius(theme::RADIUS)
                    .focus(|s| s.border_color(theme::ACCENT))
            }),
    ))
    .style(|s| s.items_center())
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
        app.new_work.name.set("chris/readme".into());
        let work = request(app).unwrap();
        assert_eq!(work.repo, "local/notes");
        assert_eq!(work.base, BaseBranch::local("main"));
        assert_eq!(work.branch, "chris/readme");
        assert!(!work.typed);

        app.new_work.name.set("two words".into());
        assert_eq!(request(app).unwrap_err(), "A branch name has no spaces");
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

        let rect = |name: &str| {
            named(harness.root_id(), name)
                .unwrap_or_else(|| panic!("{name} is drawn"))
                .get_visual_rect_no_clip()
        };
        let message = rect("new-work-message");
        let repo = rect("new-work-repo");
        let branch = rect("new-work-branch");
        let start = rect("new-work-start");
        assert!(repo.y0 >= message.y1, "the chips are under the box");
        assert!(start.y0 >= repo.y1, "Start is under the chips");
        assert!(branch.x1 <= 1100.0 && start.x1 <= 1100.0, "{branch:?} {start:?}");
        assert!(message.width() <= theme::CONVERSATION_WIDTH + 1.0);

        // The name follows the message until it is typed in.
        app.new_work.text.set("Add a README".into());
        harness.rebuild();
        assert_eq!(app.new_work.name.get_untracked(), "chris/add-a-readme");
    }

    #[test]
    fn a_branch_name_is_the_prefix_and_the_first_five_words() {
        assert_eq!(
            suggested_branch("chris/", "Add a README with a one-line description"),
            "chris/add-a-readme-with-a"
        );
        assert_eq!(
            suggested_branch("", "Fix `slugify`'s edge-case: émoji!"),
            "fix-slugifys-edgecase-moji"
        );
        assert_eq!(suggested_branch("chris/", "   "), "chris/work");
        assert_eq!(suggested_branch("", "-- !!"), "work");
    }
}
