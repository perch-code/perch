//! Open…: a one-line box over the window that opens any pull request or
//! issue from its URL, `owner/repo#57`, or `#57`. The Perch menu puts it
//! up, with `Cmd-O`.

use floem::event::EventPropagation;
use floem::peniko::Brush;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::editor::SelectionColor;
use floem::views::{
    dyn_container, Container, Decorators, Empty, Label, PlaceholderTextClass, Stack, TextInput,
};
use perch_github::ItemKind;

use crate::backend::Backend;
use crate::reference::{self, Reference};
use crate::state::AppState;
use crate::theme;

/// The box's state, in the app state. Signals, so it is `Copy`.
#[derive(Clone, Copy)]
pub struct OpenBox {
    pub open: RwSignal<bool>,
    pub text: RwSignal<String>,
    /// Why the last Enter didn't open anything.
    pub error: RwSignal<Option<String>>,
    /// The open asked for, while GitHub is asked what the number is.
    pub waiting: RwSignal<Option<u64>>,
    /// Whatever had the keyboard, for Escape to give it back to.
    pub opener: RwSignal<Option<floem::ViewId>>,
    text_box: RwSignal<Option<floem::ViewId>>,
}

impl Default for OpenBox {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenBox {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            text: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            waiting: RwSignal::new(None),
            opener: RwSignal::new(None),
            text_box: RwSignal::new(None),
        }
    }
}

/// Put the box up, empty, with the keyboard.
pub fn show(state: AppState, opener: Option<floem::ViewId>) {
    let open_box = state.open_box;
    open_box.opener.set(opener);
    open_box.text.set(String::new());
    open_box.error.set(None);
    open_box.waiting.set(None);
    open_box.open.set(true);
}

/// Put the box away. With `restore_focus`, the keyboard goes back to
/// what had it.
pub fn close(state: AppState, restore_focus: bool) {
    let open_box = state.open_box;
    let opener = open_box.opener.get_untracked();
    let text_box = open_box.text_box.get_untracked();
    open_box.opener.set(None);
    open_box.waiting.set(None);
    open_box.open.set(false);
    match opener.filter(|_| restore_focus) {
        Some(opener) => opener.request_focus(),
        None => {
            if let Some(id) = text_box {
                id.clear_focus();
            }
        }
    }
}

/// The repository a bare `#57` is in: the open item's, else the one
/// Browse › By repository is showing, else the only GitHub repository
/// configured.
pub fn default_repo(state: AppState) -> Option<String> {
    let open = state.review.get_untracked().and_then(|review| {
        review
            .item
            .with_untracked(|item| item.as_ref().map(|item| item.repo_full_name.clone()))
    });
    if let Some(repo) = open.filter(|repo| !perch_github::is_local(repo)) {
        return Some(repo);
    }
    if let Some(repo) = state
        .item_filters
        .with_untracked(|f| f.browse_repository().map(str::to_string))
    {
        return Some(repo);
    }
    let github: Vec<String> = state.repos.with_untracked(|repos| {
        repos
            .iter()
            .filter(|repo| !perch_github::is_local(&repo.identity))
            .map(|repo| repo.identity.clone())
            .collect()
    });
    match github.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// What the text names, with its repository filled in, or why not.
pub fn target(state: AppState, text: &str) -> Result<(String, Reference), String> {
    let hosts = state.github_hosts.get_untracked();
    let Some(reference) = reference::parse(text, &hosts) else {
        return Err("Paste a pull request or issue URL, or type owner/repo#57".into());
    };
    let repo = match reference.repo.clone() {
        Some(repo) => repo,
        None => default_repo(state).ok_or_else(|| "Which repository?".to_string())?,
    };
    Ok((repo, reference))
}

/// Open what `text` names. The box stays up, saying so, until GitHub has
/// said what the number is; see `resolved` and `failed`.
pub fn open_text(state: AppState, backend: &Backend, text: &str) -> Result<u64, String> {
    let (repo, reference) = target(state, text)?;
    Ok(backend.open_reference(&repo, reference.number, reference.kind))
}

/// The Browse search box's Open row: the box, with what was typed in
/// it, asked to open it, so it says what is wrong if it can't.
pub fn open_from_search(state: AppState, backend: &Backend, text: &str) {
    show(state, None);
    state.open_box.text.set(text.trim().to_string());
    submit(state, backend);
}

/// Enter.
fn submit(state: AppState, backend: &Backend) {
    if state.open_box.waiting.get_untracked().is_some() {
        return;
    }
    let text = state.open_box.text.get_untracked();
    match open_text(state, backend, &text) {
        Ok(navigation_id) => {
            state.open_box.error.set(None);
            state.open_box.waiting.set(Some(navigation_id));
        }
        Err(message) => state.open_box.error.set(Some(message)),
    }
}

/// GitHub has it: the box goes, and the open it asked for goes ahead.
pub fn resolved(state: AppState, navigation_id: u64, kind: ItemKind) {
    if state.open_box.waiting.get_untracked() != Some(navigation_id) {
        return;
    }
    close(state, false);
    state.begin_open(
        match kind {
            ItemKind::PullRequest => "Reading the pull request",
            ItemKind::Issue => "Reading the issue",
        },
        navigation_id,
    );
}

/// It can't be opened: the box says why and keeps what was typed.
pub fn failed(state: AppState, navigation_id: u64, message: String) {
    if state.open_box.waiting.get_untracked() != Some(navigation_id) {
        return;
    }
    state.open_box.waiting.set(None);
    state.open_box.error.set(Some(message));
}

/// The box's keys, from the window while it is up: Escape and Enter. The
/// rest are the text box's.
pub fn key(state: AppState, backend: &Backend, key: &KeyboardEvent) -> EventPropagation {
    if key.is_composing || !key.modifiers.is_empty() {
        return EventPropagation::Continue;
    }
    match key.code {
        Code::Escape => close(state, true),
        Code::Enter | Code::NumpadEnter => submit(state, backend),
        Code::Tab => {}
        _ => return EventPropagation::Continue,
    }
    EventPropagation::Stop
}

/// The box over the window, while it is up. A press outside it puts it
/// away, as one outside Cmd-P's palette does.
pub fn overlay(state: AppState) -> impl IntoView {
    let shown = Memo::new(move |_| state.open_box.open.get());
    dyn_container(
        move || shown.get(),
        move |open| {
            if !open {
                return Empty::new().into_any();
            }
            Container::new(panel(state))
                .on_event_stop(el::PointerDown, move |_, _| close(state, true))
                .style(|s| s.size_full())
                .into_any()
        },
    )
    .debug_name("open-box")
    .style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(100)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

fn panel(state: AppState) -> impl IntoView {
    let line = Label::derived(move || {
        if state.open_box.waiting.get().is_some() {
            return "Opening…".to_string();
        }
        state.open_box.error.get().unwrap_or_default()
    })
    .debug_name("open-box-line")
    .style(move |s| {
        let failed = state.open_box.error.with(Option::is_some)
            && state.open_box.waiting.with(Option::is_none);
        let quiet = state.open_box.error.with(Option::is_none)
            && state.open_box.waiting.with(Option::is_none);
        s.font_size(theme::TEXT_META)
            .color(if failed {
                theme::CLOSED
            } else {
                theme::FG_MUTED
            })
            .padding_horiz(4.0)
            .margin_top(6.0)
            .apply_if(quiet, |s| s.hide())
    });
    Stack::vertical((text_box(state), line))
        .on_event_stop(el::PointerDown, move |_, _| {
            if let Some(id) = state.open_box.text_box.get_untracked() {
                id.request_focus();
            }
        })
        .debug_name("open-box-panel")
        .style(move |s| {
            let (left, width) =
                crate::review::go_to::placement(state.main_pane.get(), state.window_width.get());
            s.position(Position::Absolute)
                .inset_top(theme::GO_TO_TOP)
                .inset_left(left)
                .width(width)
                .flex_col()
                .padding(theme::GO_TO_PADDING)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS_LG)
                .box_shadow_blur(22.0)
                .box_shadow_v_offset(7.0)
                .box_shadow_color(theme::SHADOW)
        })
}

fn text_box(state: AppState) -> impl IntoView {
    let input = TextInput::new(state.open_box.text)
        .placeholder("Pull request or issue URL, or owner/repo#57")
        .debug_name("open-box-text")
        .style(|s| {
            s.width_full()
                .height(theme::GO_TO_BOX_HEIGHT)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(8.0)
                .background(theme::CANVAS)
                .hover(|s| s.background(theme::CANVAS))
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::ACCENT))
                .cursor_color(theme::FG)
                .set(SelectionColor, Brush::Solid(theme::SELECTION_BG))
                .class(PlaceholderTextClass, |s| s.color(theme::FG_SUBTLE))
        });
    let id = input.view_id();
    state.open_box.text_box.set(Some(id));
    // Typing again after a complaint takes it away.
    let typed = state.open_box.text;
    floem::reactive::Effect::new(move |_| {
        typed.track();
        if state.open_box.error.get_untracked().is_some() {
            state.open_box.error.set(None);
        }
    });
    let taken = RwSignal::new(false);
    input
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if !taken.get_untracked() {
                taken.set(true);
                id.request_focus();
            }
        })
        .on_cleanup(move || {
            if state.open_box.text_box.get_untracked() == Some(id) {
                state.open_box.text_box.set(None);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Command;
    use perch_github::{DeclaredRepo, Item};

    fn declared(identities: &[&str]) -> Vec<DeclaredRepo> {
        identities
            .iter()
            .map(|identity| DeclaredRepo {
                identity: identity.to_string(),
                name: identity.to_string(),
            })
            .collect()
    }

    #[test]
    fn a_bare_number_takes_the_open_item_then_the_one_repository() {
        let state = AppState::new();
        state
            .repos
            .set(declared(&["a/one", "a/two", "local/notes"]));
        assert_eq!(target(state, "#5"), Err("Which repository?".to_string()));

        state.repos.set(declared(&["a/one", "local/notes"]));
        let (repo, reference) = target(state, "#5").unwrap();
        assert_eq!(
            (repo.as_str(), reference.number),
            ("a/one", 5),
            "the only GitHub repository"
        );

        state.item_filters.update(|f| {
            f.tab = crate::items::SidebarTab::Browse;
            f.browse = crate::items::BrowseView::Repository;
            f.browse_repository = Some("c/picked".into());
        });
        assert_eq!(target(state, "#5").unwrap().0, "c/picked", "the one Browse is showing");

        let review = crate::review::model::ReviewState::new();
        review.item.set(Some(std::sync::Arc::new(Item::new(
            "b/open",
            "2026-09-20T12:00:00Z".parse().unwrap(),
        ))));
        state.review.set(Some(review));

        assert_eq!(target(state, "57").unwrap().0, "b/open", "the open item's");

        assert_eq!(
            target(state, "x/y#3").unwrap().0,
            "x/y",
            "one that names its own"
        );
        assert!(target(state, "fence").is_err());
    }

    #[test]
    fn enter_waits_for_github_then_the_box_goes_or_says_why() {
        let state = AppState::new();
        let review = crate::review::model::ReviewState::new();
        state.review.set(Some(review));
        let (backend, mut commands) = Backend::recording();

        show(state, None);
        state.open_box.text.set("nonsense".into());
        submit(state, &backend);
        assert!(state.open_box.error.get().is_some());
        assert!(commands.try_recv().is_err(), "nothing asked");

        state
            .open_box
            .text
            .set("https://github.com/a/b/pull/7".into());
        submit(state, &backend);
        let Ok(Command::OpenReference {
            repo_full_name,
            number,
            kind,
            navigation_id,
        }) = commands.try_recv()
        else {
            panic!("no OpenReference");
        };
        assert_eq!(
            (repo_full_name.as_str(), number, kind),
            ("a/b", 7, Some(ItemKind::PullRequest))
        );
        assert_eq!(state.open_box.waiting.get(), Some(navigation_id));
        assert!(state.open_box.error.get().is_none());

        failed(state, navigation_id + 1, "not this one".into());
        assert!(
            state.open_box.error.get().is_none(),
            "another open's failure is not this box's"
        );
        failed(state, navigation_id, "No #7 in a/b".into());
        assert_eq!(state.open_box.error.get().as_deref(), Some("No #7 in a/b"));
        assert!(state.open_box.open.get(), "the box stays up");
        assert_eq!(
            state.open_box.text.get(),
            "https://github.com/a/b/pull/7",
            "with what was typed"
        );

        submit(state, &backend);
        let Ok(Command::OpenReference { navigation_id, .. }) = commands.try_recv() else {
            panic!("no OpenReference");
        };
        resolved(state, navigation_id, ItemKind::PullRequest);
        assert!(!state.open_box.open.get(), "the box goes");
        assert!(
            state.accepts_navigation(navigation_id),
            "and the open is the window's"
        );
    }
}
