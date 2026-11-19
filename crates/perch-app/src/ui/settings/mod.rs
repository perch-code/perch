//! Settings: a page over the whole window that edits config.toml.
//!
//! The file stays the one place settings live. Each control asks the
//! backend to write its key (`Backend::write_settings`), and what it shows
//! comes back from the file as `Event::Settings`, which also brings in
//! edits made by hand while Perch runs.
//!
//! A section lists its rows as data, each a title, a description, a few
//! words to be found by and a control, so the search box can look through
//! every section's rows at once and the page draws them all one way.

mod agents;
mod appearance;
pub(crate) mod controls;
mod environment;
mod general;
mod keyboard;
mod knowledge;
mod language_servers;
mod repositories;
mod work;

use std::rc::Rc;

use floem::context::VisualChanged;
use floem::event::EventPropagation;
use floem::peniko::Brush;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::editor::SelectionColor;
use floem::views::{
    dyn_container, Container, Decorators, Empty, Label, PlaceholderTextClass, Scroll, Stack,
    TextInput,
};
use floem::AnyView;
use perch_github::{Config, Setting};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{action_button, clickable, icon_action, icon_in, sidebar_glyph};

/// How wide the column of sections is.
pub(crate) const NAV_WIDTH: f64 = 260.0;
/// How wide the groups grow in the pane, as wide as Conversation's page.
const CONTENT_WIDTH: f64 = theme::CONVERSATION_WIDTH;
/// The room between the pane's edges and the groups.
const PANE_PADDING: f64 = 32.0;

/// The page's state, in each window's app state. Signals, so it is `Copy`.
#[derive(Clone, Copy)]
pub struct SettingsState {
    pub open: RwSignal<bool>,
    show_checks: RwSignal<bool>,
    ssh_draft: RwSignal<String>,
    /// The section on show; kept from one opening to the next.
    pub section: RwSignal<Section>,
    /// What the search box holds. While it holds anything, the pane lists
    /// the rows that match from every section.
    pub query: RwSignal<String>,
    /// Whatever had the keyboard when the page opened, to have it back.
    pub opener: RwSignal<Option<floem::ViewId>>,
    /// The setting's text box the keyboard is in. Escape there puts back
    /// what the box held, rather than shutting the page.
    pub(crate) editing: RwSignal<Option<floem::ViewId>>,
    /// The shortcut control waiting for keys to be pressed. Every key is
    /// its while it waits, Tab, Escape and Cmd-W too.
    pub(crate) capturing: RwSignal<Option<floem::ViewId>>,
    /// The key a shortcut was just given, until another key goes down. Held
    /// past the wait, it repeats, and its repeats are not keys for the
    /// page: a Cmd-W given to a shortcut doesn't then shut it.
    pub(crate) held: RwSignal<Option<Code>>,
    /// The Perch menu's keys, let through from the moment a shortcut
    /// waits until the key it was given comes back up, so neither the key
    /// nor its repeats run a menu item. Kept here rather than in the
    /// control so that a window shut while one waits gives them back.
    pub(crate) menu_keys: RwSignal<Option<crate::app_menu::KeysLetThrough>>,
    search_box: RwSignal<Option<floem::ViewId>>,
    /// What the Add repository box holds, with the engine on a host. Kept
    /// here and not in the box, which is built again whenever the list of
    /// repositories changes, as it does when one typed earlier is added.
    pub(crate) adding: RwSignal<String>,
    /// What that box last sent and has not heard back about. It is not
    /// sent again meanwhile, and the box empties when it is added only if
    /// it still holds it.
    pub(crate) adding_sent: RwSignal<Option<String>>,
    /// The page while it is up, for Tab to go round its controls only.
    page: RwSignal<Option<floem::ViewId>>,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            show_checks: RwSignal::new(false),
            ssh_draft: RwSignal::new(String::new()),
            section: RwSignal::new(Section::General),
            query: RwSignal::new(String::new()),
            opener: RwSignal::new(None),
            editing: RwSignal::new(None),
            capturing: RwSignal::new(None),
            held: RwSignal::new(None),
            menu_keys: RwSignal::new(None),
            search_box: RwSignal::new(None),
            adding: RwSignal::new(String::new()),
            adding_sent: RwSignal::new(None),
            page: RwSignal::new(None),
        }
    }

    /// The key given to a shortcut is up, or another went down: its
    /// repeats are over, and the menu has its keys back.
    pub(crate) fn let_go(&self) {
        if self.held.get_untracked().is_some() {
            self.held.set(None);
        }
        self.let_menu_keys_back();
    }

    /// The window is shutting: no shortcut waits in it any more, and the
    /// Perch menu, which outlives it, has its keys back. floem takes no
    /// view away as a window shuts, so the controls can't say so.
    pub(crate) fn window_closed(&self) {
        if self.capturing.try_get_untracked().flatten().is_some() {
            self.capturing.set(None);
        }
        self.let_go();
    }

    /// The Perch menu holds its keys again, unless a shortcut is waiting.
    pub(crate) fn let_menu_keys_back(&self) {
        if self.capturing.try_get_untracked().flatten().is_none()
            && self
                .menu_keys
                .try_with_untracked(|k| k.is_some_and(Option::is_some))
        {
            self.menu_keys.set(None);
        }
    }
}

/// The page's sections, in the order the column lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    General,
    Environment,
    Appearance,
    KeyboardShortcuts,
    Agents,
    Work,
    Repositories,
    Knowledge,
    LanguageServers,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::General,
        Section::Environment,
        Section::Appearance,
        Section::KeyboardShortcuts,
        Section::Agents,
        Section::Work,
        Section::Repositories,
        Section::Knowledge,
        Section::LanguageServers,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Environment => "Environment",
            Section::Appearance => "Appearance",
            Section::KeyboardShortcuts => "Keyboard shortcuts",
            Section::Agents => "Agents",
            Section::Work => "Work",
            Section::Repositories => "Repositories",
            Section::Knowledge => "Knowledge",
            Section::LanguageServers => "Language servers",
        }
    }

    pub fn icon(self) -> SidebarIcon {
        match self {
            Section::General => SidebarIcon::Gear,
            Section::Environment => SidebarIcon::Terminal,
            Section::Appearance => SidebarIcon::Palette,
            Section::KeyboardShortcuts => SidebarIcon::Keyboard,
            Section::Agents => SidebarIcon::Agents,
            Section::Work => SidebarIcon::Branch,
            Section::Repositories => SidebarIcon::Repository,
            Section::Knowledge => SidebarIcon::Folder,
            Section::LanguageServers => SidebarIcon::Server,
        }
    }

    /// The section's groups as config.toml has them now. Read inside a
    /// reactive scope, the rows that come and go with the file (a card per
    /// repository, say) are tracked.
    fn groups(self, page: &Page) -> Vec<Group> {
        match self {
            Section::General => general::groups(page),
            Section::Environment => environment::groups(page),
            Section::Appearance => appearance::groups(page),
            Section::KeyboardShortcuts => keyboard::groups(page),
            Section::Agents => agents::groups(page),
            Section::Work => work::groups(page),
            Section::Repositories => repositories::groups(page),
            Section::Knowledge => knowledge::groups(page),
            Section::LanguageServers => language_servers::groups(page),
        }
    }
}

/// What a section's rows need: the window's state, for the file as the
/// backend last read it, and the backend, to write to it.
#[derive(Clone)]
pub(crate) struct Page {
    pub state: AppState,
    pub backend: Rc<Backend>,
    /// Whether the rows are the client's own settings, which this machine's
    /// file holds whatever the engine.
    client: bool,
}

/// Whether a page's rows can be changed: see [`Page::writable`]. With the
/// engine on a host, `file` is the host's with this machine's tables put
/// in, and its error the host's; `client_broken` says whether this
/// machine's own file parses.
fn writable(
    file: Option<&crate::backend::SettingsFile>,
    client: bool,
    remote: bool,
    client_broken: bool,
    engine_heard: bool,
) -> bool {
    let Some(file) = file.filter(|file| file.config.is_some()) else {
        return false;
    };
    if client && remote {
        return !client_broken;
    }
    file.error.is_none() && (client || engine_heard)
}

thread_local! {
    /// What a file with nothing in it reads as, for the page to show before
    /// the backend has said what the file holds.
    static DEFAULTS: Config = Config::parse_text("").unwrap_or_default();
}

impl Page {
    pub(crate) fn new(state: AppState, backend: Rc<Backend>) -> Self {
        Page {
            state,
            backend,
            client: false,
        }
    }

    /// The page for rows of the client's own settings: with the engine on
    /// a host they can be changed before it has sent its own.
    pub(crate) fn here(&self) -> Self {
        Page {
            client: true,
            ..self.clone()
        }
    }

    /// Read the file's settings, tracked. Before the backend has said,
    /// the defaults.
    pub(crate) fn with_config<T>(&self, read: impl FnOnce(&Config) -> T) -> T {
        let config = self
            .state
            .settings_file
            .with(|file| file.as_ref().and_then(|file| file.config.clone()));
        match config {
            Some(config) => read(&config),
            None => DEFAULTS.with(read),
        }
    }

    /// Whether the controls can change anything: the file has been read,
    /// and it parses. With the engine on a host, only the client's own
    /// until the engine has sent its settings: the rest show defaults
    /// until then, not what the host's file holds. The client's own go by
    /// whether this machine's file parses, not the host's.
    pub(crate) fn writable(&self) -> bool {
        self.state.settings_file.with(|file| {
            writable(
                file.as_ref(),
                self.client,
                self.state.place.get().host().is_some(),
                crate::windows::client_file_broken(self.state),
                crate::windows::engine_settings_heard(self.state),
            )
        })
    }

    /// Whether the file sets `key`, in the dotted form `present_keys`
    /// gives, such as `display.zoom`.
    pub(crate) fn present(&self, key: &str) -> bool {
        self.state
            .settings_file
            .with(|file| file.as_ref().is_some_and(|file| file.present.contains(key)))
    }

    /// Write one setting to the file. The page shows the new value once
    /// the backend reads the file again. With the engine on a host, the
    /// client's own settings go to this machine's file instead (see
    /// `client_settings`).
    pub(crate) fn write(&self, setting: Setting) {
        let (here, engine) =
            crate::client_settings::route(vec![setting], self.state.place.get().host().is_some());
        if !here.is_empty() {
            match crate::client_settings::write_here(&here) {
                Ok(()) => crate::windows::client_settings_written(),
                Err(message) => self.state.complain(message),
            }
        }
        if !engine.is_empty() {
            self.backend.write_settings(engine);
        }
    }

    /// Where config.toml is.
    pub(crate) fn path(&self) -> Option<std::path::PathBuf> {
        self.state
            .settings_file
            .with(|file| file.as_ref().map(|file| file.path.clone()))
            .or_else(perch_github::config::config_path)
    }
}

/// A headed card of rows.
pub(crate) struct Group {
    pub title: String,
    /// A line under the title, such as a repository's root.
    pub subtitle: Option<String>,
    pub rows: Vec<Row>,
}

impl Group {
    pub(crate) fn new(title: impl Into<String>, rows: Vec<Row>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            rows,
        }
    }

    pub(crate) fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
}

/// One setting: what search reads and what the page draws.
#[derive(Clone)]
pub(crate) struct Row {
    /// Unique on the page: the row is named `settings-row:<name>`.
    pub name: String,
    pub title: String,
    /// The line under the title. A function, since some say what Perch
    /// found, or what the file holds.
    pub description: Rc<dyn Fn() -> String>,
    /// Other words the row is found by, such as its key in the file.
    pub keywords: Vec<&'static str>,
    /// The key the reset arrow takes out of the file, and the setting
    /// that does it. The arrow shows while the file sets the key.
    pub reset: Option<Reset>,
    /// Whether the file now holds a value Perch takes up only when it
    /// starts again. `None` for a row that applies at once.
    pub restart: Option<Rc<dyn Fn() -> bool>>,
    pub control: Rc<dyn Fn() -> AnyView>,
    /// A full-width preview below the row's control.
    preview: Option<Rc<dyn Fn() -> AnyView>>,
}

#[derive(Clone)]
pub(crate) struct Reset {
    pub key: String,
    pub setting: Setting,
}

impl Row {
    pub(crate) fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        description: impl Fn() -> String + 'static,
        control: impl Fn() -> AnyView + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            description: Rc::new(description),
            keywords: Vec::new(),
            reset: None,
            restart: None,
            control: Rc::new(control),
            preview: None,
        }
    }

    /// Words to be found by, on top of any already given.
    pub(crate) fn keywords(mut self, keywords: &[&'static str]) -> Self {
        self.keywords.extend_from_slice(keywords);
        self
    }

    pub(crate) fn reset(mut self, key: impl Into<String>, setting: Setting) -> Self {
        self.reset = Some(Reset {
            key: key.into(),
            setting,
        });
        self
    }

    pub(crate) fn restart(mut self, pending: impl Fn() -> bool + 'static) -> Self {
        self.restart = Some(Rc::new(pending));
        self
    }

    fn preview(mut self, preview: impl Fn() -> AnyView + 'static) -> Self {
        self.preview = Some(Rc::new(preview));
        self
    }

    /// Whether every word of a query is in the row's title, description,
    /// keywords, or the names of its group and section.
    fn matches(&self, words: &[String], section: Section, group: &str) -> bool {
        let text = format!(
            "{} {} {} {} {}",
            self.title,
            (self.description)(),
            self.keywords.join(" "),
            group,
            section.label(),
        )
        .to_lowercase();
        words.iter().all(|word| text.contains(word.as_str()))
    }
}

/// A query's words, in lower case.
fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// What the pane shows, for knowing when to build it again: the section
/// and the names of its groups and rows, or the rows a search found. Not
/// the values, which every control follows on its own, so a write
/// doesn't build the page again under the keyboard.
#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Section(Section, Outline),
    Search(Vec<String>, Vec<(Section, Outline)>),
}

/// Each group's title, with the names of its rows.
type Outline = Vec<(String, Vec<String>)>;

/// The rows of every section that match `words`, in their groups, by
/// section, leaving out groups and sections with none. A group keeps its
/// title and subtitle, so two repositories' rows of the same name can be
/// told apart.
fn found(page: &Page, words: &[String]) -> Vec<(Section, Vec<Group>)> {
    Section::ALL
        .into_iter()
        .filter_map(|section| {
            let groups: Vec<Group> = section
                .groups(page)
                .into_iter()
                .filter_map(|group| {
                    let Group {
                        title,
                        subtitle,
                        rows,
                    } = group;
                    let rows: Vec<Row> = rows
                        .into_iter()
                        .filter(|row| row.matches(words, section, &title))
                        .collect();
                    (!rows.is_empty()).then_some(Group {
                        title,
                        subtitle,
                        rows,
                    })
                })
                .collect();
            (!groups.is_empty()).then_some((section, groups))
        })
        .collect()
}

/// How a group found by a search is headed: under its section's name,
/// since the page shows rows from every section at once.
fn found_title(section: Section, group: &str) -> String {
    format!("{} · {group}", section.label())
}

/// Open the page with the keyboard in its search box, on the section it
/// was left on. `opener` gets the keyboard back when it shuts.
///
/// What hangs over the window's panes goes away first: a run's raw output
/// sits above the page and would cover it, and the bar's popovers and the
/// close button's menu would go on taking keys under it.
pub fn show(state: AppState, opener: Option<floem::ViewId>) {
    let settings = state.settings;
    if let Some(review) = state.review.get_untracked() {
        crate::ui::agents::detail::close(review);
        if review.close_menu.with_untracked(Option::is_some) {
            review.close_menu.set(None);
        }
        crate::ui::agents::dropdown::close(review, false);
        crate::ui::usage::close(review, false);
        crate::ui::feedback::close(review, false);
    }
    if settings.open.get_untracked() {
        if let Some(id) = settings.search_box.get_untracked() {
            id.request_focus();
        }
        return;
    }
    settings.opener.set(opener);
    settings.query.set(String::new());
    settings.open.set(true);
}

/// Shut the page. With `restore_focus`, the keyboard goes back to what
/// had it.
pub fn close(state: AppState, restore_focus: bool) {
    let settings = state.settings;
    if !settings.open.get_untracked() {
        return;
    }
    let opener = settings.opener.get_untracked();
    settings.opener.set(None);
    settings.open.set(false);
    if let Some(opener) = opener.filter(|_| restore_focus) {
        opener.request_focus();
    }
}

/// The page's keys, from the window while it is up: Escape clears the
/// search, then shuts the page; Cmd-[ and Cmd-W, or whatever the keymap
/// gives back and close, shut it; Tab and Shift-Tab go round the page's
/// own controls. Everything else is for whatever has the keyboard on the
/// page. While a shortcut waits for keys, every key is its. `typing` is
/// whether a key that types goes into a box.
pub fn key(
    state: AppState,
    cx: &mut floem::context::EventCx<'_>,
    key: &KeyboardEvent,
    typing: bool,
) -> EventPropagation {
    let settings = state.settings;
    // A menu hung from a control has keys of its own, and a shortcut
    // waiting for keys takes every one.
    if key.is_composing
        || crate::ui::menu::is_open()
        || settings.capturing.get_untracked().is_some()
    {
        return EventPropagation::Continue;
    }
    if let Some(code) = settings.held.get_untracked() {
        if key.repeat && key.code == code {
            cx.prevent_default();
            return EventPropagation::Stop;
        }
        if !key.repeat {
            settings.let_go();
        }
    }
    if key.code == Code::Tab && (key.modifiers.is_empty() || key.modifiers == Modifiers::SHIFT) {
        if let Some(page) = settings.page.get_untracked() {
            tab(cx, page, key.modifiers == Modifiers::SHIFT);
            return EventPropagation::Stop;
        }
    }
    if key.code == Code::Escape && key.modifiers.is_empty() {
        // A setting's box puts back what it held.
        if settings.editing.get_untracked().is_some() {
            return EventPropagation::Continue;
        }
        if !settings.query.with_untracked(String::is_empty) {
            settings.query.set(String::new());
        } else {
            close(state, true);
        }
        return EventPropagation::Stop;
    }
    use crate::keymap::{pressed, Action};
    if pressed(Action::Back, key, typing) || pressed(Action::Close, key, typing) {
        close(state, true);
        return EventPropagation::Stop;
    }
    // Already open.
    if pressed(Action::Settings, key, typing) {
        return EventPropagation::Stop;
    }
    EventPropagation::Continue
}

/// The page over the window while it is open. It covers everything but the
/// toasts, which say why a write failed.
pub fn overlay(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let shown = Memo::new(move |_| state.settings.open.get());
    dyn_container(
        move || shown.get(),
        move |open| {
            if open {
                page_view(Page::new(state, backend.clone())).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .debug_name("settings")
    .style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            // Under the toasts (80), over the window's panes.
            .z_index(70)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

fn page_view(page: Page) -> impl IntoView {
    let settings = page.state.settings;
    let view = Stack::horizontal((nav(page.clone()), pane(page)));
    let id = view.view_id();
    settings.page.set(Some(id));
    view.on_cleanup(move || {
        if settings.page.try_get_untracked() == Some(Some(id)) {
            settings.page.set(None);
        }
    })
    .debug_name("settings-page")
    .style(|s| s.size_full().background(theme::canvas()))
}

/// Move the keyboard to the next of the page's controls, or the one before,
/// round from the last to the first. floem's own Tab would walk on into
/// the window under the page, where nothing typed can be seen.
fn tab(cx: &mut floem::context::EventCx<'_>, page: floem::ViewId, backwards: bool) {
    cx.prevent_default();
    let mut stops = Vec::new();
    tab_stops(page, &mut stops);
    if stops.is_empty() {
        return;
    }
    let at = stops.iter().position(|id| cx.window_state.is_focused(*id));
    let last = stops.len() - 1;
    let next = match (at, backwards) {
        (None, false) => 0,
        (None, true) => last,
        (Some(at), false) if at == last => 0,
        (Some(at), false) => at + 1,
        (Some(0), true) => last,
        (Some(at), true) => at - 1,
    };
    stops[next].request_focus();
}

/// The views under `id` that Tab stops at, in the order they are built,
/// which is the order they read: the column, then the pane top to bottom.
fn tab_stops(id: floem::ViewId, stops: &mut Vec<floem::ViewId>) {
    if id.is_hidden() {
        return;
    }
    let stop = id
        .focus_nav_meta_for_element(id.get_element_id())
        .is_some_and(|meta| meta.is_keyboard_navigable() && meta.enabled);
    if stop && !id.is_disabled() {
        stops.push(id);
    }
    for child in id.children() {
        tab_stops(child, stops);
    }
}

// ---------------------------------------------------------------------
// The column of sections
// ---------------------------------------------------------------------

fn nav(page: Page) -> impl IntoView {
    let settings = page.state.settings;
    // The sections a search finds anything in, for the others to grey.
    let matched = {
        let page = page.clone();
        Memo::new(move |_| {
            let words = words(&settings.query.get());
            if words.is_empty() {
                return Section::ALL.to_vec();
            }
            found(&page, &words)
                .into_iter()
                .map(|(section, _)| section)
                .collect::<Vec<_>>()
        })
    };
    let rows = Section::ALL.map(|section| nav_row(page.state, section, matched).into_any());
    Stack::vertical((
        // The strip the traffic lights sit over, which drags the window.
        crate::ui::titlebar::title_bar(Empty::new().style(|s| s.size_full())).style(|s| {
            s.height(theme::TITLEBAR_HEIGHT)
                .width_full()
                .flex_shrink(0.0_f32)
        }),
        search_box(page.state),
        Stack::vertical_from_iter(rows).style(|s| {
            s.flex_col()
                .gap(2.0)
                .padding_horiz(theme::NAV_GUTTER)
                .padding_top(12.0)
                .width_full()
        }),
    ))
    .debug_name("settings-nav")
    .style(|s| {
        s.width(NAV_WIDTH)
            .height_full()
            .flex_shrink(0.0_f32)
            .flex_col()
            .background(theme::nav_canvas())
            .border_right(1.0)
            .border_color(theme::nav_divider())
    })
}

fn nav_row(state: AppState, section: Section, matched: Memo<Vec<Section>>) -> impl IntoView {
    let settings = state.settings;
    let searching = move || !settings.query.with(|q| q.trim().is_empty());
    let selected = move || !searching() && settings.section.get() == section;
    let greyed = move || matched.with(|m| !m.contains(&section));
    let ink = move || {
        if greyed() {
            theme::nav_subtle()
        } else if selected() {
            theme::nav_fg()
        } else {
            theme::nav_muted()
        }
    };
    let pick = move || {
        settings.query.set(String::new());
        settings.section.set(section);
    };
    let glyph = icon_in(move || icons::sidebar_icon(section.icon(), ink()), ink)
        .style(|s| s.margin_right(10.0).flex_shrink(0.0_f32));
    Stack::horizontal((
        glyph,
        Label::new(section.label()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .color(ink())
                .text_ellipsis()
                .min_width(0.0)
                .pointer_events_none()
        }),
    ))
    .on_event_stop(el::Click, move |_, _| pick())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) && key.modifiers.is_empty() {
            pick();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name(format!("settings-nav:{}", section.label()))
    .style(move |s| {
        clickable(s)
            .items_center()
            .height(34.0)
            .padding_horiz(10.0)
            .border_radius(10.0)
            .width_full()
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::nav_control_border()))
            .background(if selected() {
                theme::nav_selected()
            } else {
                theme::nav_canvas()
            })
            .hover(move |s| {
                s.background(if selected() {
                    theme::nav_selected()
                } else {
                    theme::nav_hover()
                })
            })
    })
}

/// The search box at the column's head. It takes the keyboard as the page
/// opens.
fn search_box(state: AppState) -> impl IntoView {
    let settings = state.settings;
    let input = TextInput::new(settings.query)
        .placeholder("Search settings")
        .debug_name("settings-search")
        .style(|s| {
            s.width_full()
                .min_width(0.0)
                .height(theme::NAV_CONTROL_HEIGHT)
                .font_size(theme::TEXT_BODY)
                .padding_left(34.0)
                .padding_right(12.0)
                .background(theme::nav_canvas())
                .border(1.0)
                .border_color(theme::nav_control_border())
                .border_radius(theme::NAV_CONTROL_RADIUS)
                .focus(|s| s.border_color(theme::nav_muted()))
                .set(SelectionColor, Brush::Solid(theme::selection_bg()))
                .class(PlaceholderTextClass, |s| s.color(theme::nav_subtle()))
        });
    let id = input.view_id();
    settings.search_box.set(Some(id));
    // Once it is styled and placed: a view new this frame can't take the
    // keyboard yet.
    let asked = std::cell::Cell::new(false);
    let input = input
        .on_event_cont(VisualChanged::listener(), move |_, _| {
            if !asked.replace(true) {
                id.request_focus();
            }
        })
        .on_cleanup(move || {
            if settings.search_box.get_untracked() == Some(id) {
                settings.search_box.set(None);
            }
        });
    let glyph = sidebar_glyph(SidebarIcon::Search, theme::nav_subtle)
        .style(|s| {
            s.size(15.0, 15.0)
                .position(Position::Absolute)
                .inset_left(10.0)
                .pointer_events_none()
        })
        .debug_name("settings-search-glyph");
    // The gutter sits on a wrapper: an absolute child's inset counts from
    // its parent's padding edge, so padding on the glyph's own stack would
    // put it on the box's border.
    let field = Stack::horizontal((input, glyph)).style(|s| s.items_center().width_full());
    Container::new(field).style(|s| {
        s.width_full()
            .padding_horiz(theme::NAV_GUTTER)
            .flex_shrink(0.0_f32)
    })
}

// ---------------------------------------------------------------------
// The pane
// ---------------------------------------------------------------------

fn pane(page: Page) -> impl IntoView {
    let state = page.state;
    let settings = state.settings;
    let shown = {
        let page = page.clone();
        Memo::new(move |_| {
            let words = words(&settings.query.get());
            if words.is_empty() {
                let section = settings.section.get();
                let outline = section
                    .groups(&page)
                    .into_iter()
                    .map(|group| {
                        (
                            group.title,
                            group.rows.into_iter().map(|r| r.name).collect(),
                        )
                    })
                    .collect();
                Shown::Section(section, outline)
            } else {
                let outline = found(&page, &words)
                    .into_iter()
                    .map(|(section, groups)| {
                        let groups = groups
                            .into_iter()
                            .map(|group| {
                                (
                                    group.title,
                                    group.rows.into_iter().map(|r| r.name).collect(),
                                )
                            })
                            .collect();
                        (section, groups)
                    })
                    .collect();
                Shown::Search(words, outline)
            }
        })
    };
    let body_page = page.clone();
    let body = dyn_container(
        move || shown.get(),
        move |shown| Effect::untrack(|| body(&body_page, shown)),
    )
    .debug_name("settings-body")
    .style(|s| s.width_full().flex_col());
    let column = Stack::vertical((
        error_banner(page.clone()),
        warnings_banner(state),
        restart_banner(page.clone()),
        body,
    ))
    .debug_name("settings-column")
    .style(|s| {
        s.width(CONTENT_WIDTH)
            .max_width_full()
            .flex_col()
            .gap(theme::CARD_GAP)
    });
    let scroll = Scroll::new(
        Container::new(column)
            .style(|s| {
                s.width_full()
                    .max_width_full()
                    .flex_col()
                    .items_center()
                    .padding_horiz(PANE_PADDING)
                    .padding_top(24.0)
                    .padding_bottom(48.0)
            })
            .debug_name("settings-scroll-body"),
    )
    .debug_name("settings-scroll")
    .style(|s| {
        s.width_full()
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_height(0.0)
    });
    Stack::vertical((header(state), scroll))
        .debug_name("settings-pane")
        .style(|s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height_full()
                .flex_col()
        })
}

/// "Settings / General", and the X that shuts the page.
fn header(state: AppState) -> impl IntoView {
    let settings = state.settings;
    let crumb = move || {
        if settings.query.with(|q| q.trim().is_empty()) {
            settings.section.get().label()
        } else {
            "Search"
        }
    };
    crate::ui::titlebar::title_bar(
        Stack::horizontal((
            Label::new("Settings").style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .color(theme::fg_muted())
                    .pointer_events_none()
            }),
            Label::new("/").style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .color(theme::fg_subtle())
                    .margin_horiz(8.0)
                    .pointer_events_none()
            }),
            Label::derived(move || crumb().to_string())
                .debug_name("settings-crumb")
                .style(|s| {
                    s.font_size(theme::TEXT_TITLE)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .pointer_events_none()
                }),
            Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
            icon_action(
                SidebarIcon::Close,
                "settings-close",
                || true,
                move || close(state, true),
            ),
        ))
        .debug_name("settings-header"),
    )
    .style(|s| {
        s.height(theme::TITLEBAR_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(20.0)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    })
}

fn body(page: &Page, shown: Shown) -> AnyView {
    match shown {
        Shown::Section(section, _) => {
            let views: Vec<AnyView> = section
                .groups(page)
                .into_iter()
                .map(|group| {
                    let rows = group
                        .rows
                        .iter()
                        .map(|row| controls::row(page, row))
                        .collect();
                    controls::group(&group.title, group.subtitle.as_deref(), rows).into_any()
                })
                .collect();
            Stack::vertical_from_iter(views)
                .style(|s| s.flex_col().width_full().gap(28.0))
                .into_any()
        }
        Shown::Search(words, _) => {
            let results = found(page, &words);
            if results.is_empty() {
                let query = page.state.settings.query.get_untracked();
                return Label::new(format!("No settings match “{}”.", query.trim()))
                    .debug_name("settings-no-match")
                    .style(|s| {
                        s.font_size(theme::TEXT_BODY)
                            .color(theme::fg_muted())
                            .padding_top(24.0)
                            .width_full()
                            .justify_center()
                    })
                    .into_any();
            }
            let views: Vec<AnyView> = results
                .into_iter()
                .flat_map(|(section, groups)| {
                    groups.into_iter().map(move |group| {
                        let rows = group
                            .rows
                            .iter()
                            .map(|row| controls::row(page, row))
                            .collect();
                        controls::group(
                            &found_title(section, &group.title),
                            group.subtitle.as_deref(),
                            rows,
                        )
                        .into_any()
                    })
                })
                .collect();
            Stack::vertical_from_iter(views)
                .style(|s| s.flex_col().width_full().gap(28.0))
                .into_any()
        }
    }
}

/// Why the file can't be used, over every section, while it can't.
fn error_banner(page: Page) -> impl IntoView {
    let state = page.state;
    let error = move || {
        state
            .settings_file
            .with(|file| file.as_ref().and_then(|file| file.error.clone()))
    };
    // With the engine on a host, once it has sent its settings the error
    // is its file's.
    let host_s =
        move || state.place.get().host().is_some() && crate::windows::engine_settings_heard(state);
    let text = move || {
        banner_text(
            error(),
            host_s(),
            host_s() && crate::windows::client_file_broken(state),
        )
    };
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Warning, theme::alert_caution)
            .style(|s| s.size(16.0, 16.0).flex_shrink(0.0_f32).margin_right(10.0)),
        Label::derived(text)
            .debug_name("settings-error-text")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg())
                    .text_wrap()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .flex_basis(0.0)
            }),
        controls::open_in_editor_button(page.clone(), "settings-error-open")
            .style(|s| s.margin_left(12.0)),
    ))
    .debug_name("settings-error")
    .style(move |s| {
        s.items_center()
            .width_full()
            .padding(12.0)
            .border(1.0)
            .border_color(theme::alert_caution())
            .border_radius(theme::CARD_RADIUS)
            .apply_if(error().is_none(), |s| s.hide())
    })
}

/// The error banner's words for the file's `error`. With the engine on a
/// host (`host_s`), the error is the host file's, and this Mac's rows can
/// still be changed, unless this Mac's file doesn't parse either
/// (`mine_broken`): its error is then among the warnings, and nothing can
/// be changed here.
fn banner_text(error: Option<String>, host_s: bool, mine_broken: bool) -> String {
    match error {
        Some(error) if host_s && mine_broken => format!(
            "Perch can't use the host's config.toml: {error}. {}'s doesn't parse either. \
             Fix both in your editor. Until then Perch keeps the settings it read last, and \
             they can't be changed here.",
            perch_engine::host::HERE_FIRST
        ),
        Some(error) if host_s => format!(
            "Perch can't use the host's config.toml: {error}. Fix it in your editor. Until \
             then the engine keeps the settings it read last, and only {}'s can be \
             changed here.",
            perch_engine::host::HERE
        ),
        Some(error) => format!(
            "Perch can't use config.toml: {error}. Fix it in your editor. Until then Perch \
             keeps the settings it read last, and they can't be changed here."
        ),
        None => String::new(),
    }
}

/// What loading the file had to say, such as a zoom out of range or a
/// repository left out, for as long as the file says it: the toasts that
/// said it first go.
fn warnings_banner(state: AppState) -> impl IntoView {
    let warnings = move || {
        state.settings_file.with(|file| {
            file.as_ref()
                .filter(|file| file.error.is_none())
                .map(|file| file.warnings.clone())
                .unwrap_or_default()
        })
    };
    let text = move || warnings().join("\n");
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Warning, theme::notice_fg)
            .style(|s| s.size(16.0, 16.0).flex_shrink(0.0_f32).margin_right(10.0)),
        Label::derived(text)
            .debug_name("settings-warnings-text")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg())
                    .text_wrap()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .flex_basis(0.0)
            }),
    ))
    .debug_name("settings-warnings")
    .style(move |s| {
        s.items_start()
            .width_full()
            .padding(12.0)
            .border(1.0)
            .border_color(theme::notice_border())
            .border_radius(theme::CARD_RADIUS)
            .apply_if(warnings().is_empty(), |s| s.hide())
    })
}

/// Whether any row's value waits for Perch to start again.
fn restart_pending(page: &Page) -> bool {
    Section::ALL.into_iter().any(|section| {
        section.groups(page).iter().any(|group| {
            group
                .rows
                .iter()
                .any(|row| row.restart.as_ref().is_some_and(|pending| pending()))
        })
    })
}

/// "Restart Perch", while a setting waits for it. Without an app to start
/// again, under `cargo run` say, it only says so.
fn restart_banner(page: Page) -> impl IntoView {
    let pending = {
        let page = page.clone();
        Memo::new(move |_| restart_pending(&page))
    };
    let can_restart = crate::update::running_bundle().is_some();
    let text = if can_restart {
        "Some changes apply after Perch restarts."
    } else {
        "Some changes apply after Perch restarts. Quit Perch and open it again to see them."
    };
    let button = if can_restart {
        action_button(
            || "Restart Perch".to_string(),
            "settings-restart",
            true,
            || true,
            restart,
        )
        .style(|s| s.margin_left(12.0))
        .into_any()
    } else {
        Empty::new().style(|s| s.hide()).into_any()
    };
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Refresh, theme::notice_fg)
            .style(|s| s.size(16.0, 16.0).flex_shrink(0.0_f32).margin_right(10.0)),
        Label::new(text).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::notice_fg())
                .text_wrap()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        button,
    ))
    .debug_name("settings-restart-banner")
    .style(move |s| {
        s.items_center()
            .width_full()
            .padding(12.0)
            .background(theme::notice_bg())
            .border(1.0)
            .border_color(theme::notice_border())
            .border_radius(theme::CARD_RADIUS)
            .apply_if(!pending.get(), |s| s.hide())
    })
}

/// Start Perch again from the app it runs from, as an installed update
/// does.
fn restart() {
    let Some(bundle) = crate::update::running_bundle() else {
        return;
    };
    match crate::update::relaunch_after_exit(&bundle) {
        Ok(()) => crate::windows::quit_app(),
        Err(e) => tracing::warn!(error = %e, "couldn't restart Perch"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::str::FromStr;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::kurbo::Rect;
    use perch_github::{KnowledgeSetting, RepoSetting};

    use super::*;
    use crate::backend::{Command, SettingsFile};
    use crate::review::model::ReviewState;

    /// With the engine on a host, the banner says only this Mac's rows can
    /// be changed while this Mac's file parses, and that none can when
    /// both files are broken.
    #[test]
    fn the_banner_says_what_can_still_be_changed() {
        let error = || Some("line 1".to_string());
        use perch_engine::host::{HERE, HERE_FIRST};
        assert!(banner_text(error(), true, false)
            .ends_with(&format!("only {HERE}'s can be changed here.")));
        let both = banner_text(error(), true, true);
        assert!(
            both.contains(&format!("{HERE_FIRST}'s doesn't parse either.")),
            "{both}"
        );
        assert!(both.ends_with("they can't be changed here."), "{both}");
        assert!(banner_text(error(), false, false).starts_with("Perch can't use config.toml"));
        assert_eq!(banner_text(None, true, true), "");
    }

    /// With the engine on a host, the client's rows go by this machine's
    /// file and the engine's by the host's: a broken host file leaves the
    /// Engine, appearance and keys rows on, a broken file here turns them
    /// off. In process one file decides all.
    #[test]
    fn the_client_s_rows_go_by_this_machine_s_file() {
        let file = |error: Option<&str>| SettingsFile {
            path: "/x".into(),
            config: Some(Arc::new(perch_github::Config::parse_text("").unwrap())),
            error: error.map(str::to_string),
            present: Default::default(),
            warnings: Vec::new(),
            knowledge_problems: Default::default(),
        };
        let broken_host = file(Some("line 1"));
        let whole = file(None);
        // Remote, the host's file broken.
        assert!(writable(Some(&broken_host), true, true, false, true));
        assert!(!writable(Some(&broken_host), false, true, false, true));
        // Remote, this machine's file broken and the host's whole.
        assert!(!writable(Some(&whole), true, true, true, true));
        assert!(writable(Some(&whole), false, true, true, true));
        // Remote, before the engine has sent its own.
        assert!(writable(Some(&whole), true, true, false, false));
        assert!(!writable(Some(&whole), false, true, false, false));
        assert!(!writable(None, true, true, false, false));
        // In process.
        assert!(writable(Some(&whole), true, false, false, true));
        assert!(!writable(Some(&broken_host), true, false, false, true));
    }

    pub(super) fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    pub(super) fn all_named(id: floem::ViewId, name: &str, found: &mut Vec<floem::ViewId>) {
        if id.debug_name().contains(name) {
            found.push(id);
        }
        for child in id.children() {
            all_named(child, name, found);
        }
    }

    pub(super) fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    pub(super) fn rect(harness: &HeadlessHarness, name: &str) -> Rect {
        named(harness.root_id(), name)
            .unwrap_or_else(|| panic!("no {name}"))
            .get_visual_rect_no_clip()
    }

    /// Whether `name` is in the tree and takes up room on screen.
    pub(super) fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| {
            let r = id.get_visual_rect_no_clip();
            r.width() > 0.0 && r.height() > 0.0
        })
    }

    pub(super) fn click(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, name).center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    pub(super) fn key(code: Code, key: Key, modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    pub(super) fn escape(harness: &mut HeadlessHarness) {
        harness.dispatch_event(key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        ));
        settle(harness);
    }

    pub(super) fn type_text(harness: &mut HeadlessHarness, text: &str) {
        for c in text.chars() {
            // A letter's key, or none for anything else: the box takes
            // the character either way.
            let code = Code::from_str(&format!("Key{}", c.to_ascii_uppercase()))
                .unwrap_or(Code::Unidentified);
            harness.dispatch_event(key(code, Key::Character(c.to_string()), Modifiers::empty()));
        }
        settle(harness);
    }

    /// config.toml holding `text`, as the backend sends it.
    pub(super) fn file(text: &str) -> SettingsFile {
        let config = Config::parse_text(text).unwrap();
        SettingsFile {
            path: PathBuf::from("/srv/perch/config.toml"),
            present: perch_github::present_keys(text, &config.repo),
            warnings: config.warnings.clone(),
            config: Some(Arc::new(config)),
            error: None,
            knowledge_problems: Default::default(),
        }
    }

    /// A window `width` by `height` whose config.toml holds `text`, its
    /// commands recorded.
    pub(super) fn window(
        width: f64,
        height: f64,
        text: &str,
    ) -> (
        HeadlessHarness,
        AppState,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let state = AppState::new();
        let review = ReviewState::new();
        state.review.set(Some(review));
        state.settings_file.set(Some(file(text)));
        let (backend, commands) = Backend::recording();
        let root = TestRoot::new();
        let view = crate::ui::app_view(state, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, width, height);
        settle(&mut harness);
        (harness, state, commands)
    }

    /// The settings written since the last look.
    pub(super) fn written(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Vec<Setting> {
        std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|command| match command {
                Command::WriteSettings(changes) => Some(changes),
                _ => None,
            })
            .flatten()
            .collect()
    }

    pub(super) fn open_on(harness: &mut HeadlessHarness, state: AppState, section: Section) {
        state.settings.section.set(section);
        show(state, None);
        settle(harness);
    }

    /// The view named exactly `name`, where `named` takes the first name
    /// that holds it.
    pub(super) fn exactly_named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split(" - ").any(|n| n == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| exactly_named(child, name))
    }

    /// Open the dropdown named `control` and pick its row `row`.
    pub(super) fn pick(harness: &mut HeadlessHarness, control: &str, row: &str) {
        let at = rect(harness, control).center();
        harness.pointer_down(at.x, at.y);
        settle(harness);
        harness.pointer_up(1.0, 1.0);
        settle(harness);
        assert!(crate::ui::menu::is_open(), "{control} opens a menu");
        click(harness, &format!("menu-row:{row}"));
    }

    /// Type `text` into the box named `field` and press Enter.
    pub(super) fn enter_in(harness: &mut HeadlessHarness, field: &str, text: &str) {
        click(harness, field);
        let id = named(harness.root_id(), field).unwrap();
        assert!(harness.is_focused(id), "{field} has the keyboard");
        type_text(harness, text);
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        settle(harness);
    }

    /// Every command sent since the last look.
    pub(super) fn sent(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    /// Nothing on the section on show is left light in the dark.
    pub(super) fn goes_dark(harness: &mut HeadlessHarness) {
        let left = crate::theme::dark_check::light_left(harness);
        assert!(left.is_empty(), "{left:#?}");
    }

    /// The gear in the drawer's footer opens Settings with the keyboard in
    /// its search box; Escape shuts it and gives the keyboard back to the
    /// gear. Cmd-[ and Cmd-W shut it too.
    #[test]
    fn the_footer_gear_opens_settings_and_escape_gives_the_keyboard_back() {
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        assert!(!shown(&harness, "settings-page"));

        click(&mut harness, "settings-button");
        assert!(state.settings.open.get_untracked());
        assert!(shown(&harness, "settings-page"));
        let search = named(harness.root_id(), "settings-search").unwrap();
        assert!(
            harness.is_focused(search),
            "the search box has the keyboard"
        );
        assert_eq!(state.settings.section.get_untracked(), Section::General);

        escape(&mut harness);
        assert!(!state.settings.open.get_untracked());
        assert!(named(harness.root_id(), "settings-page").is_none());
        let gear = named(harness.root_id(), "settings-button").unwrap();
        assert!(harness.is_focused(gear), "the gear has the keyboard back");

        for shut in [Code::BracketLeft, Code::KeyW] {
            click(&mut harness, "settings-button");
            assert!(state.settings.open.get_untracked());
            harness.dispatch_event(key(shut, Key::Character("[".into()), crate::ui::OS_MOD));
            settle(&mut harness);
            assert!(!state.settings.open.get_untracked(), "{shut:?} shuts it");
        }
        // Cmd-W shut Settings, not the window or an item.
        assert!(named(harness.root_id(), "settings-button").is_some());
    }

    /// Tab and Shift-Tab go round the page's controls and never reach the
    /// window under it; the mouse's back button shuts the page rather
    /// than moving what is under it.
    #[test]
    fn tab_stays_on_the_page_and_back_shuts_it() {
        use floem::ui_events::pointer::{
            PointerButton, PointerButtonEvent, PointerEvent, PointerId, PointerInfo, PointerType,
        };
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        show(state, None);
        settle(&mut harness);
        let page = named(harness.root_id(), "settings-page").unwrap();
        let on_page = |harness: &HeadlessHarness| {
            let mut all = Vec::new();
            all_named(page, "", &mut all);
            all.into_iter().any(|id| harness.is_focused(id))
        };
        let search = named(harness.root_id(), "settings-search").unwrap();
        assert!(harness.is_focused(search));
        for modifiers in [Modifiers::SHIFT, Modifiers::empty()] {
            let mut stops = 0;
            loop {
                harness.dispatch_event(key(Code::Tab, Key::Named(NamedKey::Tab), modifiers));
                settle(&mut harness);
                assert!(on_page(&harness), "{modifiers:?} after {stops} stops");
                stops += 1;
                if harness.is_focused(search) {
                    break;
                }
                assert!(stops < 100, "Tab came round to the search box");
            }
            assert!(stops > 2, "{stops}");
        }

        let at = rect(&harness, "settings-pane").center();
        let mut pointer = floem::ui_events::pointer::PointerState::default();
        pointer.position.x = at.x;
        pointer.position.y = at.y;
        pointer.count = 1;
        harness.dispatch_event(floem::event::Event::Pointer(PointerEvent::Down(
            PointerButtonEvent {
                state: pointer,
                button: Some(PointerButton::X1),
                pointer: PointerInfo {
                    pointer_id: Some(PointerId::PRIMARY),
                    persistent_device_id: None,
                    pointer_type: PointerType::Mouse,
                },
            },
        )));
        settle(&mut harness);
        assert!(!state.settings.open.get_untracked());
    }

    /// Opening the page puts away what hangs over the window, and Send
    /// Feedback, which hangs from the drawer under it, shuts the page.
    #[test]
    fn settings_and_the_popovers_under_it_take_turns() {
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        let review = state.review.get_untracked().unwrap();
        review.usage_open.set(true);
        review.feedback_open.set(true);
        review.agent_output.set(Some("run-1".into()));
        show(state, None);
        settle(&mut harness);
        assert_eq!(
            review.agent_output.get_untracked(),
            None,
            "the sheet is over the page"
        );
        assert!(!review.usage_open.get_untracked());
        assert!(!review.feedback_open.get_untracked());

        crate::ui::feedback::open(state, review, crate::feedback::FeedbackKind::Idea, None);
        settle(&mut harness);
        assert!(!state.settings.open.get_untracked());
        assert!(review.feedback_open.get_untracked());
    }

    /// Off macOS there is no Perch menu: Ctrl-, opens Settings.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn ctrl_comma_opens_settings() {
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        harness.dispatch_event(key(
            Code::Comma,
            Key::Character(",".into()),
            crate::ui::OS_MOD,
        ));
        settle(&mut harness);
        assert!(state.settings.open.get_untracked());
        let search = named(harness.root_id(), "settings-search").unwrap();
        assert!(harness.is_focused(search));
    }

    /// A section in the column shows its rows, and the page opens on it
    /// again next time.
    #[test]
    fn the_column_switches_sections() {
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        show(state, None);
        settle(&mut harness);
        assert!(shown(&harness, "settings-row:poll-interval"));
        assert!(!shown(&harness, "settings-row:zoom"));

        click(&mut harness, "settings-nav:Appearance");
        assert_eq!(state.settings.section.get_untracked(), Section::Appearance);
        assert!(shown(&harness, "settings-row:zoom"));
        assert!(shown(&harness, "settings-row:appearance"));
        assert!(!shown(&harness, "settings-row:poll-interval"));

        close(state, false);
        settle(&mut harness);
        show(state, None);
        settle(&mut harness);
        assert!(shown(&harness, "settings-row:zoom"), "back on Appearance");

        click(&mut harness, "settings-nav:Language servers");
        assert!(shown(&harness, "settings-row:lsp-memory"));
    }

    #[test]
    fn the_theme_picker_writes_previews_and_resets_the_palette() {
        use floem::style::Background;
        use perch_github::config::Theme;

        let (mut harness, state, mut commands) =
            window(1400.0, 900.0, "[ui]\ntheme = \"classic\"\n");
        open_on(&mut harness, state, Section::Appearance);
        assert!(shown(&harness, "settings-theme-preview"));
        let preview = named(harness.root_id(), "settings-theme-preview").unwrap();
        let appearance = theme::appearance();
        assert_eq!(
            harness.get_computed_style(preview).get(Background),
            Some(Brush::Solid(
                theme::colors(Theme::Classic, appearance).canvas
            ))
        );

        pick(&mut harness, "settings-theme", Theme::WarmLinen.label());
        assert!(matches!(
            written(&mut commands).as_slice(),
            [Setting::Theme(Some(Theme::WarmLinen))]
        ));
        state
            .settings_file
            .set(Some(file("[ui]\ntheme = \"warm-linen\"\n")));
        settle(&mut harness);
        assert_eq!(
            harness.get_computed_style(preview).get(Background),
            Some(Brush::Solid(
                theme::colors(Theme::WarmLinen, appearance).canvas
            ))
        );

        click(&mut harness, "settings-reset:theme");
        assert!(matches!(
            written(&mut commands).as_slice(),
            [Setting::Theme(None)]
        ));
    }

    /// The search glyph sits inside the box, clear of its border and
    /// centred on it, where the sidebar's search box has its own.
    #[test]
    fn the_search_glyph_sits_inside_the_box() {
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        show(state, None);
        settle(&mut harness);
        let field = rect(&harness, "settings-search");
        let glyph = rect(&harness, "settings-search-glyph");
        assert!(
            (glyph.x0 - field.x0 - 10.0).abs() < 0.5,
            "{glyph:?} {field:?}"
        );
        assert!(
            (glyph.center().y - field.center().y).abs() < 0.5,
            "{glyph:?} {field:?}"
        );
    }

    /// Searching lists the rows that match from every section; Escape
    /// clears the search before it shuts the page.
    #[test]
    fn search_lists_matching_rows_from_every_section() {
        let (mut harness, state, commands) = window(1400.0, 900.0, "");
        show(state, None);
        settle(&mut harness);

        type_text(&mut harness, "zoom");
        assert_eq!(state.settings.query.get_untracked(), "zoom");
        assert!(shown(&harness, "settings-row:zoom"));
        assert!(!shown(&harness, "settings-row:poll-interval"));
        assert!(shown(&harness, "settings-group:Appearance"));
        let page = Page::new(state, Rc::new(Backend::detached()));
        let sections: Vec<Section> = found(&page, &words("zoom"))
            .into_iter()
            .map(|(section, _)| section)
            .collect();
        assert_eq!(sections, vec![Section::Appearance], "the rest are greyed");

        // A group's name finds its rows.
        state.settings.query.set("github".into());
        settle(&mut harness);
        assert!(shown(&harness, "settings-row:poll-interval"));
        assert!(shown(&harness, "settings-row:github-host"));
        assert!(!shown(&harness, "settings-row:zoom"));

        state.settings.query.set("nothing like it".into());
        settle(&mut harness);
        assert!(shown(&harness, "settings-no-match"));

        escape(&mut harness);
        assert!(state.settings.open.get_untracked(), "the search goes first");
        assert_eq!(state.settings.query.get_untracked(), "");
        assert!(shown(&harness, "settings-row:poll-interval"));
        escape(&mut harness);
        assert!(!state.settings.open.get_untracked());
        drop(commands);
    }

    /// A search keeps each row under its group, so two repositories'
    /// switches of the same name say which repository they belong to.
    #[test]
    fn search_keeps_each_repositorys_rows_under_its_name() {
        let text = "[[repo]]\ngithub = \"acme/widgets\"\n\n[[repo]]\ngithub = \"acme/gadgets\"\n";
        let (mut harness, state, _commands) = window(1400.0, 900.0, text);
        show(state, None);
        settle(&mut harness);
        type_text(&mut harness, "trust");
        assert!(shown(&harness, "settings-group:Repositories · widgets"));
        assert!(shown(&harness, "settings-group:Repositories · gadgets"));
        assert!(shown(&harness, "settings-row:repo:acme/widgets:work-trust"));
        assert!(shown(&harness, "settings-row:repo:acme/gadgets:work-trust"));
    }

    #[test]
    fn knowledge_settings_add_edit_and_remove_directories() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::Knowledge);

        click(&mut harness, "settings-knowledge-add-dir");
        type_text(&mut harness, "discord/project");
        enter_in(&mut harness, "settings-knowledge-add-repos", "acme/widgets");
        let change = |change| Setting::Knowledge {
            dir: "discord/project".into(),
            change,
        };
        assert_eq!(
            written(&mut commands),
            vec![change(KnowledgeSetting::Add(Some(vec![
                "acme/widgets".into()
            ])))]
        );
        state.settings_file.set(Some(file(
            "[[knowledge]]\ndir = \"discord/project\"\nrepos = [\"acme/widgets\"]\n",
        )));
        settle(&mut harness);
        assert!(shown(&harness, "settings-group:discord/project"));
        click(
            &mut harness,
            "settings-reset:knowledge:discord/project:repos",
        );
        assert_eq!(
            written(&mut commands),
            vec![change(KnowledgeSetting::Repositories(None))]
        );
        click(&mut harness, "settings-knowledge:discord/project:remove");
        assert_eq!(
            written(&mut commands),
            vec![change(KnowledgeSetting::Remove)]
        );

        // Every repository is a switch, and the list shows only without it,
        // so emptying the list never widens the directory.
        click(&mut harness, "settings-knowledge:discord/project:all");
        assert_eq!(
            written(&mut commands),
            vec![change(KnowledgeSetting::Repositories(None))]
        );
        state
            .settings_file
            .set(Some(file("[[knowledge]]\ndir = \"discord/project\"\n")));
        settle(&mut harness);
        assert!(!shown(
            &harness,
            "settings-row:knowledge:discord/project:repos"
        ));
        click(&mut harness, "settings-knowledge:discord/project:all");
        assert_eq!(
            written(&mut commands),
            vec![change(KnowledgeSetting::Repositories(Some(Vec::new())))]
        );
        state.settings_file.set(Some(file(
            "[[knowledge]]\ndir = \"discord/project\"\nrepos = []\n",
        )));
        settle(&mut harness);
        assert!(shown(
            &harness,
            "settings-row:knowledge:discord/project:repos"
        ));
        assert!(!shown(
            &harness,
            "settings-row:knowledge:discord/project:status"
        ));

        // What the engine found wrong with the directory.
        let mut broken = file("[[knowledge]]\ndir = \"discord/project\"\n");
        broken
            .knowledge_problems
            .insert("discord/project".into(), "latest is a symlink".into());
        state.settings_file.set(Some(broken));
        settle(&mut harness);
        assert!(shown(
            &harness,
            "settings-row:knowledge:discord/project:status"
        ));
    }

    /// Picking from a dropdown writes the setting.
    #[test]
    fn a_dropdown_writes_what_is_picked() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::Appearance);

        let zoom = rect(&harness, "settings-zoom").center();
        harness.pointer_down(zoom.x, zoom.y);
        settle(&mut harness);
        harness.pointer_up(1.0, 1.0);
        settle(&mut harness);
        assert!(crate::ui::menu::is_open());
        click(&mut harness, "menu-row:125%");
        assert_eq!(written(&mut commands), vec![Setting::Zoom(Some(1.25))]);
        assert!(state.settings.open.get_untracked(), "Settings stays up");
    }

    /// A switch writes as it flips, and flips back from what the file
    /// says.
    #[test]
    fn a_switch_writes_as_it_flips() {
        let state = AppState::new();
        let (backend, mut commands) = Backend::recording();
        let page = Page::new(state, Rc::new(backend));
        let on = RwSignal::new(false);
        let root = TestRoot::new();
        let switch = controls::toggle(
            move || on.get(),
            || true,
            move |to| {
                page.write(Setting::Repo {
                    identity: "acme/widgets".into(),
                    change: RepoSetting::WorkTrust(to),
                })
            },
            "switch",
        );
        let view = Stack::new((switch,)).style(|s| s.size_full().padding(20.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 200.0, 100.0);
        settle(&mut harness);

        click(&mut harness, "switch");
        assert_eq!(
            written(&mut commands),
            vec![Setting::Repo {
                identity: "acme/widgets".into(),
                change: RepoSetting::WorkTrust(true),
            }]
        );
        on.set(true);
        settle(&mut harness);
        click(&mut harness, "switch");
        assert_eq!(
            written(&mut commands),
            vec![Setting::Repo {
                identity: "acme/widgets".into(),
                change: RepoSetting::WorkTrust(false),
            }]
        );
    }

    /// A text setting is written at Enter; Escape puts back what the file
    /// holds without writing, and a second Escape shuts the page.
    #[test]
    fn a_text_setting_writes_at_enter_and_escape_puts_it_back() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        show(state, None);
        settle(&mut harness);

        click(&mut harness, "settings-github-host");
        let field = named(harness.root_id(), "settings-github-host").unwrap();
        assert!(harness.is_focused(field));
        type_text(&mut harness, "ghe");
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert_eq!(
            written(&mut commands),
            vec![Setting::GithubHost(Some("ghe".into()))]
        );

        // The file now says so, and something else is typed and let go.
        // The page isn't built again for a new value: the box is the same
        // box, and keeps the keyboard.
        let row = named(harness.root_id(), "settings-row:github-host").unwrap();
        state
            .settings_file
            .set(Some(file("github_host = \"ghe\"\n")));
        settle(&mut harness);
        assert_eq!(
            named(harness.root_id(), "settings-row:github-host"),
            Some(row)
        );
        assert!(harness.is_focused(field));
        type_text(&mut harness, "x");
        escape(&mut harness);
        assert!(state.settings.open.get_untracked(), "the box took it");
        assert!(!harness.is_focused(field));
        assert!(written(&mut commands).is_empty(), "nothing written");
        escape(&mut harness);
        assert!(!state.settings.open.get_untracked());
    }

    /// A box that only has the keyboard follows the file and writes
    /// nothing when let go of; one whose write was refused shows the file
    /// again rather than writing it a second time.
    #[test]
    fn a_text_setting_follows_the_file_until_typed_in() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        show(state, None);
        settle(&mut harness);

        click(&mut harness, "settings-github-host");
        let field = named(harness.root_id(), "settings-github-host").unwrap();
        assert!(harness.is_focused(field));
        // Changed in an editor while the box has the keyboard.
        state
            .settings_file
            .set(Some(file("github_host = \"ghe.corp\"\n")));
        settle(&mut harness);
        click(&mut harness, "settings-search");
        assert!(!harness.is_focused(field));
        assert!(written(&mut commands).is_empty(), "the hand edit stands");

        // Something typed and sent, then refused: the backend sends the
        // file as it was.
        click(&mut harness, "settings-github-host");
        type_text(&mut harness, "x");
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert_eq!(written(&mut commands).len(), 1);
        state
            .settings_file
            .set(Some(file("github_host = \"ghe.corp\"\n")));
        settle(&mut harness);
        click(&mut harness, "settings-search");
        assert!(written(&mut commands).is_empty(), "not sent twice");
        // The box shows the file again, so Enter there has nothing to say.
        click(&mut harness, "settings-github-host");
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert!(written(&mut commands).is_empty());
    }

    /// While the file doesn't parse, every section says so and nothing
    /// can be changed; a value that waits for a restart says so too.
    #[test]
    fn a_broken_file_is_said_and_its_controls_are_off() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::Appearance);
        assert!(!shown(&harness, "settings-error"));
        assert!(!shown(&harness, "settings-restart-banner"));
        assert!(!shown(&harness, "settings-restart-note:zoom"));

        // A zoom Perch didn't start with waits for a restart.
        state
            .settings_file
            .set(Some(file("[display]\nzoom = 1.25\n")));
        settle(&mut harness);
        assert!(shown(&harness, "settings-restart-banner"));
        assert!(shown(&harness, "settings-restart-note:zoom"));
        assert!(shown(&harness, "settings-reset:zoom"));
        assert!(!shown(&harness, "settings-reset:code-font-size"));

        let mut broken = file("");
        broken.config = None;
        broken.error = Some("line 2: expected `=`".into());
        state.apply(crate::backend::Event::Settings(broken));
        settle(&mut harness);
        assert!(shown(&harness, "settings-error"));
        assert!(!shown(&harness, "settings-reset:zoom"), "no resets");
        assert!(
            shown(&harness, "settings-restart-note:zoom"),
            "the settings read last are kept"
        );
        let zoom = rect(&harness, "settings-zoom").center();
        harness.pointer_down(zoom.x, zoom.y);
        settle(&mut harness);
        harness.pointer_up(1.0, 1.0);
        settle(&mut harness);
        assert!(!crate::ui::menu::is_open(), "no menu while it's broken");
        assert!(written(&mut commands).is_empty());
    }

    #[test]
    fn settings_go_dark_with_the_system() {
        // A zoom out of range: a warning, and a restart waiting.
        let (mut harness, state, _commands) = window(
            1400.0,
            900.0,
            "[display]\nzoom = 3.0\ncode_font_size = 15\n",
        );
        show(state, None);
        settle(&mut harness);
        assert!(shown(&harness, "settings-warnings"));
        assert!(shown(&harness, "settings-restart-banner"));
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(left.is_empty(), "{left:#?}");

        click(&mut harness, "settings-nav:Appearance");
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(left.is_empty(), "{left:#?}");
    }

    /// The cards of every section sit inside the pane, inside its padding,
    /// with nothing running past their edges, in a wide window and a
    /// narrow one.
    #[test]
    fn the_cards_fit_the_pane_wide_and_narrow() {
        let text = "[[repo]]\ngithub = \"acme/widgets\"\nroot = \"/srv/a/rather/long/path/to/the/widgets/checkout\"\n\n\
                    [[repo]]\nroot = \"/srv/notes\"\n";
        for (width, height) in [(1400.0, 900.0), (900.0, 700.0)] {
            let (mut harness, state, _commands) = window(width, height, text);
            for section in Section::ALL {
                open_on(&mut harness, state, section);
                let pane = rect(&harness, "settings-pane");
                assert_eq!(pane.x0, NAV_WIDTH, "{width}");
                assert_eq!(pane.x1, width);
                // The scroll keeps a lane for its bar.
                let body = rect(&harness, "settings-scroll-body");
                assert!(body.x1 <= pane.x1 + 0.5, "{width}: {body:?} {pane:?}");
                assert!(body.width() >= pane.width() - 8.5, "{width}: {body:?}");
                let column = rect(&harness, "settings-column");
                let room = (body.width() - 2.0 * PANE_PADDING).min(CONTENT_WIDTH);
                assert!((column.width() - room).abs() < 0.5, "{width}: {column:?}");

                let mut cards = Vec::new();
                all_named(harness.root_id(), "settings-card", &mut cards);
                assert!(!cards.is_empty());
                for card in cards {
                    let card = card.get_visual_rect_no_clip();
                    assert!(card.x0 >= body.x0 + PANE_PADDING - 0.5, "{width}: {card:?}");
                    assert!(card.x1 <= body.x1 - PANE_PADDING + 0.5, "{width}: {card:?}");
                    let mut rows = Vec::new();
                    all_named(harness.root_id(), "settings-row:", &mut rows);
                    for row in rows {
                        let r = row.get_visual_rect_no_clip();
                        if r.y0 >= card.y0 && r.y1 <= card.y1 {
                            assert!(r.x1 <= card.x1 + 0.5, "{width}: {r:?} {card:?}");
                        }
                    }
                }
                for control in [
                    "settings-poll-interval",
                    "settings-github-host",
                    "settings-reveal-config",
                    "settings-zoom",
                    "settings-agent-model",
                    "settings-codex-binary",
                    "settings-work-approvals",
                    "settings-repo:acme/widgets:name",
                    "settings-repo:acme/widgets:work-trust",
                    "settings-repo:acme/widgets:remove",
                    "settings-add-repository",
                    "settings-lsp-memory",
                ] {
                    if let Some(id) = named(harness.root_id(), control) {
                        let r = id.get_visual_rect_no_clip();
                        assert!(
                            r.x1 <= body.x1 - PANE_PADDING - theme::CARD_PADDING + 0.5,
                            "{width}: {control} {r:?}"
                        );
                        assert!(r.width() > 0.0, "{width}: {control}");
                    }
                }
                close(state, false);
                settle(&mut harness);
            }
        }
    }
}
