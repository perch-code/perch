//! Which keys run which of Perch's commands.
//!
//! Every command a key can run is an [`Action`], with the keys it has by
//! default. `[keybindings]` in config.toml gives an action other keys, or
//! none, by its id. The keymap says only which keys run an action; whether
//! the action can run just then, with a dialog up or no item open, is still
//! the caller's to say.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::ui_events::keyboard::KeyboardEvent;

use crate::ui::menu::Shortcut;

/// Where on the Settings page, and in the docs, an action is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    General,
    Sidebar,
    Item,
    ConversationAndCode,
    Code,
    Find,
    Composer,
}

impl Group {
    pub const ALL: [Group; 7] = [
        Group::General,
        Group::Sidebar,
        Group::Item,
        Group::ConversationAndCode,
        Group::Code,
        Group::Find,
        Group::Composer,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Sidebar => "Sidebar",
            Group::Item => "Item",
            Group::ConversationAndCode => "Conversation & Code",
            Group::Code => "Code",
            Group::Find => "Find",
            Group::Composer => "Composer",
        }
    }
}

/// Where a key is heard. Two actions can share a key only when they are
/// heard in different places: the composer's formatting keys are its own,
/// and Cmd-G there suggests a change while elsewhere it finds the next
/// match. The window hears its keys first, though, and some of them from a
/// composer too; see [`Action::heard_in_composer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Window,
    Composer,
}

macro_rules! actions {
    ($(
        $variant:ident {
            id: $id:literal,
            title: $title:literal,
            group: $group:ident,
            context: $context:ident,
            keys: [$($key:literal),* $(,)?] $(,)?
        }
    ),* $(,)?) => {
        /// A command a key can run.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action {
            $($variant,)*
        }

        impl Action {
            /// Every action, in the order Settings and the docs list them.
            pub const ALL: &'static [Action] = &[$(Action::$variant,)*];

            /// Its key under `[keybindings]`: snake_case, with no dot, so
            /// it is a bare TOML key and one segment of a dotted one.
            pub fn id(self) -> &'static str {
                match self {
                    $(Action::$variant => $id,)*
                }
            }

            /// Its name, as Settings shows it.
            pub fn title(self) -> &'static str {
                match self {
                    $(Action::$variant => $title,)*
                }
            }

            pub fn group(self) -> Group {
                match self {
                    $(Action::$variant => Group::$group,)*
                }
            }

            pub fn context(self) -> Context {
                match self {
                    $(Action::$variant => Context::$context,)*
                }
            }

            /// Its keys when config.toml names none, as config.toml writes
            /// them. Some actions have none.
            pub fn defaults(self) -> &'static [&'static str] {
                match self {
                    $(Action::$variant => &[$($key),*],)*
                }
            }
        }
    };
}

actions! {
    NewWork { id: "new_work", title: "New Work", group: General, context: Window, keys: ["super+n"] },
    NewWindow { id: "new_window", title: "New Window", group: General, context: Window, keys: ["super+shift+n"] },
    Open { id: "open", title: "Open…", group: General, context: Window, keys: ["super+o"] },
    Settings { id: "settings", title: "Settings", group: General, context: Window, keys: ["super+,"] },
    Close { id: "close", title: "Close", group: General, context: Window, keys: ["super+w"] },
    Back { id: "back", title: "Back to the Sidebar", group: General, context: Window, keys: ["super+["] },
    HistoryBack { id: "history_back", title: "Go Back", group: General, context: Window, keys: ["ctrl+-"] },
    HistoryForward { id: "history_forward", title: "Go Forward", group: General, context: Window, keys: ["ctrl+shift+-"] },
    ToggleAgents { id: "toggle_agents", title: "Toggle Agents", group: General, context: Window, keys: ["super+shift+a"] },
    SendFeedback { id: "send_feedback", title: "Send Feedback", group: General, context: Window, keys: [] },

    ToggleSidebar { id: "toggle_sidebar", title: "Toggle Sidebar", group: Sidebar, context: Window, keys: ["super+\\"] },
    FocusSearch { id: "focus_search", title: "Search the Sidebar", group: Sidebar, context: Window, keys: ["super+1", "/"] },
    Inbox { id: "inbox", title: "Show Inbox", group: Sidebar, context: Window, keys: ["super+2"] },
    Worktrees { id: "worktrees", title: "Show Worktrees", group: Sidebar, context: Window, keys: ["super+3"] },
    Browse { id: "browse", title: "Show Browse", group: Sidebar, context: Window, keys: ["super+4"] },
    NextItem { id: "next_item", title: "Next Item", group: Sidebar, context: Window, keys: ["j"] },
    PreviousItem { id: "previous_item", title: "Previous Item", group: Sidebar, context: Window, keys: ["k"] },
    MarkDone { id: "mark_done", title: "Mark Done", group: Sidebar, context: Window, keys: ["e"] },
    MarkUnread { id: "mark_unread", title: "Mark Unread", group: Sidebar, context: Window, keys: ["shift+u"] },
    Pin { id: "pin", title: "Pin", group: Sidebar, context: Window, keys: ["p"] },

    ShowConversation { id: "show_conversation", title: "Show Conversation", group: Item, context: Window, keys: ["super+shift+1"] },
    ShowCode { id: "show_code", title: "Show Code", group: Item, context: Window, keys: ["super+shift+2"] },
    NextTab { id: "next_tab", title: "Next Tab", group: Item, context: Window, keys: ["super+shift+]"] },
    PreviousTab { id: "previous_tab", title: "Previous Tab", group: Item, context: Window, keys: ["super+shift+["] },
    Refresh { id: "refresh", title: "Refresh", group: Item, context: Window, keys: ["super+r"] },
    OpenOnGithub { id: "open_on_github", title: "Open on GitHub", group: Item, context: Window, keys: ["super+shift+o"] },
    CopyLink { id: "copy_link", title: "Copy Link", group: Item, context: Window, keys: ["super+shift+c"] },
    CopyBranchName { id: "copy_branch_name", title: "Copy Branch Name", group: Item, context: Window, keys: [] },
    StartWork { id: "start_work", title: "Start Work", group: Item, context: Window, keys: [] },
    ToggleReview { id: "toggle_review", title: "Toggle Review", group: Item, context: Window, keys: ["super+shift+r"] },
    ToggleMerge { id: "toggle_merge", title: "Toggle Merge", group: Item, context: Window, keys: ["super+shift+m"] },

    Comment { id: "comment", title: "Comment", group: ConversationAndCode, context: Window, keys: ["a"] },
    PageUp { id: "page_up", title: "Page Up", group: ConversationAndCode, context: Window, keys: ["pageup"] },
    PageDown { id: "page_down", title: "Page Down", group: ConversationAndCode, context: Window, keys: ["pagedown"] },
    ScrollToTop { id: "scroll_to_top", title: "Scroll to Top", group: ConversationAndCode, context: Window, keys: ["super+up"] },
    ScrollToEnd { id: "scroll_to_end", title: "Scroll to End", group: ConversationAndCode, context: Window, keys: ["super+down"] },

    GoToFile { id: "go_to_file", title: "Go to File", group: Code, context: Window, keys: ["super+p"] },
    ToggleFiles { id: "toggle_files", title: "Toggle Files", group: Code, context: Window, keys: ["super+shift+e"] },
    NextChange { id: "next_change", title: "Next Change", group: Code, context: Window, keys: ["]"] },
    PreviousChange { id: "previous_change", title: "Previous Change", group: Code, context: Window, keys: ["["] },
    ToggleViewed { id: "toggle_viewed", title: "Toggle Viewed", group: Code, context: Window, keys: ["v"] },
    NextUnviewed { id: "next_unviewed", title: "Next Unviewed File", group: Code, context: Window, keys: ["shift+]"] },
    PreviousUnviewed { id: "previous_unviewed", title: "Previous Unviewed File", group: Code, context: Window, keys: ["shift+["] },
    ToggleWrap { id: "toggle_wrap", title: "Toggle Word Wrap", group: Code, context: Window, keys: [] },
    GoToDefinition { id: "go_to_definition", title: "Go to Definition", group: Code, context: Window, keys: ["f12"] },
    GoToTypeDefinition { id: "go_to_type_definition", title: "Go to Type Definition", group: Code, context: Window, keys: [] },
    GoToImplementations { id: "go_to_implementations", title: "Go to Implementations", group: Code, context: Window, keys: ["super+f12"] },
    FindReferences { id: "find_references", title: "Find References", group: Code, context: Window, keys: ["shift+f12"] },
    CallHierarchy { id: "call_hierarchy", title: "Show Call Hierarchy", group: Code, context: Window, keys: ["alt+shift+h"] },

    Find { id: "find", title: "Find", group: Find, context: Window, keys: ["super+f"] },
    FindNext { id: "find_next", title: "Find Next", group: Find, context: Window, keys: ["super+g"] },
    FindPrevious { id: "find_previous", title: "Find Previous", group: Find, context: Window, keys: ["super+shift+g"] },
    FindMatchCase { id: "find_match_case", title: "Match Case", group: Find, context: Window, keys: ["alt+super+c"] },
    FindWholeWord { id: "find_whole_word", title: "Match Whole Word", group: Find, context: Window, keys: ["alt+super+w"] },
    FindRegex { id: "find_regex", title: "Use Regular Expression", group: Find, context: Window, keys: ["alt+super+r"] },

    Bold { id: "bold", title: "Bold", group: Composer, context: Composer, keys: ["super+b"] },
    Italic { id: "italic", title: "Italic", group: Composer, context: Composer, keys: ["super+i"] },
    Code { id: "code", title: "Code", group: Composer, context: Composer, keys: ["super+e"] },
    Link { id: "link", title: "Link", group: Composer, context: Composer, keys: ["super+k"] },
    Quote { id: "quote", title: "Quote", group: Composer, context: Composer, keys: ["super+shift+."] },
    NumberedList { id: "numbered_list", title: "Numbered List", group: Composer, context: Composer, keys: ["super+shift+7"] },
    BulletList { id: "bullet_list", title: "Unordered List", group: Composer, context: Composer, keys: ["super+shift+8"] },
    TaskList { id: "task_list", title: "Task List", group: Composer, context: Composer, keys: ["super+shift+l"] },
    Suggest { id: "suggest", title: "Suggest a Change", group: Composer, context: Composer, keys: ["super+g"] },
    TogglePreview { id: "toggle_preview", title: "Toggle Preview", group: Composer, context: Composer, keys: ["super+shift+p"] },
}

impl Action {
    /// The action whose id this is.
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.id() == id)
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Whether a window action's key runs while a composer has the
    /// keyboard. The window's keys are caught on the way down, before the
    /// composer's own, so a composer action on the same key would never
    /// run. The rest wait while a composer is being typed in, Find and its
    /// keys among them, or are heard only after the composer has had the
    /// key. See the window's key handler in `ui/mod.rs`. A composer action
    /// is never one.
    pub fn heard_in_composer(self) -> bool {
        matches!(
            self,
            Action::NewWork
                | Action::NewWindow
                | Action::Open
                | Action::Settings
                | Action::Close
                | Action::HistoryBack
                | Action::HistoryForward
                | Action::ToggleAgents
                | Action::SendFeedback
                | Action::Inbox
                | Action::Worktrees
                | Action::Browse
                | Action::NextItem
                | Action::PreviousItem
                | Action::MarkDone
                | Action::MarkUnread
                | Action::OpenOnGithub
                | Action::ToggleReview
                | Action::ToggleMerge
                | Action::PageUp
                | Action::PageDown
                | Action::GoToFile
                | Action::ToggleFiles
        )
    }

    /// Whether a key `self` and `other` share would leave one of them
    /// never running: both heard in the same place, or a composer's key
    /// the window takes first. A key that types is never a composer's, nor
    /// the window's while a composer has the keyboard.
    fn clashes_with(self, other: Action, key: &Shortcut) -> bool {
        if self.context() == other.context() {
            return true;
        }
        let window = if self.context() == Context::Window {
            self
        } else {
            other
        };
        window.heard_in_composer() && !key.is_bare()
    }

    /// What it does, in a line, as Settings says it under the title.
    pub fn description(self) -> &'static str {
        match self {
            Action::NewWork => "Open New Work, to start an agent on a new branch.",
            Action::NewWindow => "Open another window.",
            Action::Open => "Open a pull request or issue from its link or number.",
            Action::Settings => "Open this page.",
            Action::Close => "Close the open file, then the open item, then the window.",
            Action::Back => "Close the open item or New Work, back to the sidebar.",
            Action::HistoryBack => "Go back to where you were before.",
            Action::HistoryForward => "Go forward again after going back.",
            Action::ToggleAgents => "Show or hide the agents running across Perch.",
            Action::SendFeedback => "Send a bug report or an idea.",
            Action::ToggleSidebar => "Show or hide the sidebar.",
            Action::FocusSearch => "Put the keyboard in the sidebar's search box.",
            Action::Inbox => "Show the Inbox in the sidebar.",
            Action::Worktrees => "Show the worktrees in the sidebar.",
            Action::Browse => "Show Browse in the sidebar.",
            Action::NextItem => "Open the sidebar's next row.",
            Action::PreviousItem => "Open the sidebar's row before.",
            Action::MarkDone => "Mark the notification under the pointer, or the open one, done.",
            Action::MarkUnread => {
                "Mark the notification under the pointer, or the open one, unread."
            }
            Action::Pin => "Pin the item under the pointer, or the open one.",
            Action::ShowConversation => "Show the open item's Conversation tab.",
            Action::ShowCode => "Show the open item's Code tab.",
            Action::NextTab => "Go to the next file tab.",
            Action::PreviousTab => "Go to the file tab before.",
            Action::Refresh => "Fetch the open item from GitHub again.",
            Action::OpenOnGithub => "Open the item in the browser.",
            Action::CopyLink => "Copy the open item's link.",
            Action::CopyBranchName => "Copy the open pull request's branch name.",
            Action::StartWork => "Start work on the open item in a new worktree.",
            Action::ToggleReview => "Open or close the Review box.",
            Action::ToggleMerge => "Open or close the Merge box.",
            Action::Comment => "Comment on the selected lines, or on the item as a whole.",
            Action::PageUp => "Scroll up a page.",
            Action::PageDown => "Scroll down a page.",
            Action::ScrollToTop => "Scroll to the top.",
            Action::ScrollToEnd => "Scroll to the end.",
            Action::GoToFile => "Find a file by name and open it.",
            Action::ToggleFiles => "Show or hide the list of files.",
            Action::NextChange => "Go to the next change in the file.",
            Action::PreviousChange => "Go to the change before in the file.",
            Action::ToggleViewed => "Mark the open file viewed, or not viewed.",
            Action::NextUnviewed => "Go to the next file not yet viewed.",
            Action::PreviousUnviewed => "Go to the file before not yet viewed.",
            Action::ToggleWrap => "Wrap long lines in the code, or stop wrapping them.",
            Action::GoToDefinition => "Go to where the symbol is defined.",
            Action::GoToTypeDefinition => "Go to where the symbol's type is defined.",
            Action::GoToImplementations => "List what implements the symbol.",
            Action::FindReferences => "List where the symbol is used.",
            Action::CallHierarchy => "Show what calls the function, and what it calls.",
            Action::Find => "Find text in the code or the conversation.",
            Action::FindNext => "Go to the next match.",
            Action::FindPrevious => "Go to the match before.",
            Action::FindMatchCase => "Find with case, or without.",
            Action::FindWholeWord => "Find whole words only, or any text.",
            Action::FindRegex => "Find by a regular expression, or plain text.",
            Action::Bold => "Make the selection bold.",
            Action::Italic => "Make the selection italic.",
            Action::Code => "Make the selection code.",
            Action::Link => "Make the selection a link.",
            Action::Quote => "Quote the selected lines.",
            Action::NumberedList => "Make the selected lines a numbered list.",
            Action::BulletList => "Make the selected lines a list.",
            Action::TaskList => "Make the selected lines a task list.",
            Action::Suggest => "Suggest a change to the lines commented on.",
            Action::TogglePreview => "Show the comment as it will read, or go back to writing it.",
        }
    }
}

/// The keys each action has: its defaults, or what config.toml gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    /// Indexed by `Action::index`.
    bindings: Vec<Vec<Shortcut>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::defaults()
    }
}

impl Keymap {
    /// Every action on its default keys.
    pub fn defaults() -> Self {
        let bindings = Action::ALL
            .iter()
            .map(|action| {
                action
                    .defaults()
                    .iter()
                    .map(|keys| {
                        Shortcut::parse(keys).unwrap_or_else(|| {
                            panic!("{} has a default it can't read: {keys}", action.id())
                        })
                    })
                    .collect()
            })
            .collect();
        Keymap { bindings }
    }

    /// The defaults, with each action `[keybindings]` names on the keys it
    /// gives instead: one key as a string, several as a list, none as `""`
    /// or `[]`. An id no action has, a value of another kind and a key that
    /// doesn't read are left out, each with a warning; an action whose keys
    /// don't all read keeps the ones that do.
    pub fn from_config(table: &BTreeMap<String, toml::Value>) -> (Keymap, Vec<String>) {
        let mut keymap = Keymap::defaults();
        let mut warnings = Vec::new();
        let warn = |id: &str, value: &toml::Value, why: &str| {
            format!("config.toml: [keybindings] {id} = {value} was not read: {why}")
        };
        for (id, value) in table {
            let Some(action) = Action::from_id(id) else {
                warnings.push(warn(id, value, "Perch has no action by that name"));
                continue;
            };
            let written: Vec<&str> = match value {
                toml::Value::String(keys) if keys.trim().is_empty() => Vec::new(),
                toml::Value::String(keys) => vec![keys.as_str()],
                toml::Value::Array(list) => {
                    let mut written = Vec::new();
                    for item in list {
                        match item.as_str() {
                            Some(keys) if !keys.trim().is_empty() => written.push(keys),
                            Some(_) => {}
                            None => warnings.push(warn(
                                id,
                                value,
                                &format!("{item} is not keys written as a string"),
                            )),
                        }
                    }
                    written
                }
                _ => {
                    warnings.push(warn(id, value, "write keys as a string, or a list of them"));
                    continue;
                }
            };
            let mut keys: Vec<Shortcut> = Vec::new();
            for written in written {
                match Shortcut::parse(written) {
                    Some(key) if !keys.contains(&key) => keys.push(key),
                    Some(_) => {}
                    None => warnings.push(warn(
                        id,
                        value,
                        &format!("{written:?} is not a key Perch knows"),
                    )),
                }
            }
            keymap.bindings[action.index()] = keys;
        }
        (keymap, warnings)
    }

    /// The keys that run `action`, the first the one menus and tips show.
    pub fn keys(&self, action: Action) -> &[Shortcut] {
        &self.bindings[action.index()]
    }

    /// How the platform writes the first of `action`'s keys, for a tip or
    /// a menu row; None when it has no keys.
    pub fn label(&self, action: Action) -> Option<String> {
        self.keys(action).first().map(Shortcut::label)
    }

    /// Whether `key` is one of `action`'s. A key that types, a letter with
    /// no Cmd say, never is while `typing`: it belongs to the box.
    pub fn matches(&self, action: Action, key: &KeyboardEvent, typing: bool) -> bool {
        self.keys(action)
            .iter()
            .any(|shortcut| shortcut.matches(key) && !(typing && shortcut.is_bare()))
    }

    /// The other actions that share a key with `action` where only one of
    /// them can run, in `Action::ALL`'s order.
    pub fn conflicts(&self, action: Action) -> Vec<Action> {
        let keys = self.keys(action);
        Action::ALL
            .iter()
            .copied()
            .filter(|other| {
                *other != action
                    && self
                        .keys(*other)
                        .iter()
                        .any(|k| keys.contains(k) && action.clashes_with(*other, k))
            })
            .collect()
    }
}

thread_local! {
    /// The keymap every key is matched by. Its own scope, so no view's
    /// going takes it with it.
    static KEYMAP: RwSignal<Rc<Keymap>> = Scope::new().create_rw_signal(Rc::new(Keymap::defaults()));
}

/// Put `keymap` in place of the one keys are matched by. A no-op when
/// nothing changes, since every tip that read a label would run again.
pub fn set(keymap: Keymap) {
    KEYMAP.with(|k| {
        if k.with_untracked(|now| **now != keymap) {
            k.set(Rc::new(keymap));
            crate::app_menu::take_up_keys();
        }
    });
}

/// The keymap config.toml's `[keybindings]` gives, put in place. Its
/// warnings are the backend's to show, from its own read of the file.
pub fn take_up(table: &BTreeMap<String, toml::Value>) {
    set(Keymap::from_config(table).0);
}

/// Add what's wrong with `config`'s `[keybindings]` to its warnings, so
/// they are said as the file's other warnings are.
pub fn add_warnings(config: &mut perch_github::Config) {
    let (_, warnings) = Keymap::from_config(&config.keybindings);
    config.warnings.extend(warnings);
}

/// The keymap now, without tracking it.
pub fn current() -> Rc<Keymap> {
    KEYMAP.with(|k| k.get_untracked())
}

/// Whether `key` runs `action`; see [`Keymap::matches`]. Read untracked,
/// as a key handler is not a view to update.
pub fn pressed(action: Action, key: &KeyboardEvent, typing: bool) -> bool {
    KEYMAP.with(|k| k.with_untracked(|keymap| keymap.matches(action, key, typing)))
}

/// How the platform writes `action`'s first key, tracked, so a tip or a
/// menu row that shows it changes with the keymap.
pub fn label(action: Action) -> Option<String> {
    KEYMAP.with(|k| k.with(|keymap| keymap.label(action)))
}

/// `text` with `action`'s first key after it, "Bold (⌘B)", as a tip says
/// it; `text` alone while the action has no key. Tracked, as `label` is.
pub fn tip(text: &str, action: Action) -> String {
    match label(action) {
        Some(key) => format!("{text} ({key})"),
        None => text.to_string(),
    }
}

/// `action`'s keys now, tracked.
#[cfg(test)]
pub fn keys(action: Action) -> Vec<Shortcut> {
    KEYMAP.with(|k| k.with(|keymap| keymap.keys(action).to_vec()))
}

#[cfg(test)]
thread_local! {
    /// How many [`Installed`] guards are alive, so the last one going puts
    /// the defaults back.
    static INSTALLED: RefCell<usize> = const { RefCell::new(0) };
}

/// A keymap a test put in place, which goes back to the defaults when this
/// is dropped, so the next test on the thread starts from them.
#[cfg(test)]
#[must_use]
pub struct Installed(());

#[cfg(test)]
impl Drop for Installed {
    fn drop(&mut self) {
        let last = INSTALLED.with(|n| {
            let mut n = n.borrow_mut();
            *n -= 1;
            *n == 0
        });
        if last {
            set(Keymap::defaults());
        }
    }
}

/// Put `keymap` in place for a test, until the guard is dropped.
#[cfg(test)]
pub fn install(keymap: Keymap) -> Installed {
    INSTALLED.with(|n| *n.borrow_mut() += 1);
    set(keymap);
    Installed(())
}

/// The defaults with `[keybindings]` text from config.toml on top, for a
/// test; panics on a warning.
#[cfg(test)]
pub fn with_bindings(toml_text: &str) -> Keymap {
    let table: BTreeMap<String, toml::Value> = toml::from_str(toml_text).unwrap();
    let (keymap, warnings) = Keymap::from_config(&table);
    assert!(warnings.is_empty(), "{warnings:?}");
    keymap
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use floem::ui_events::keyboard::{Code, Key, KeyState, Location, Modifiers};

    use super::*;

    fn table(text: &str) -> BTreeMap<String, toml::Value> {
        toml::from_str(text).unwrap()
    }

    fn press(keys: &str) -> KeyboardEvent {
        let shortcut = Shortcut::parse(keys).unwrap();
        KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character(String::new()),
            code: shortcut.code,
            location: Location::Standard,
            modifiers: shortcut.modifiers,
            repeat: false,
            is_composing: false,
        }
    }

    #[test]
    fn every_action_has_its_own_dot_free_snake_case_id() {
        let mut seen = HashSet::new();
        for (i, action) in Action::ALL.iter().enumerate() {
            assert_eq!(action.index(), i, "{action:?} is out of ALL's order");
            let id = action.id();
            assert!(seen.insert(id), "{id} twice");
            assert!(
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{id}"
            );
            assert_eq!(Action::from_id(id), Some(*action));
            assert!(!action.title().is_empty());
        }
        let groups: HashSet<Group> = Action::ALL.iter().map(|a| a.group()).collect();
        assert_eq!(groups.len(), Group::ALL.len());
    }

    #[test]
    fn the_defaults_have_no_conflicts() {
        let keymap = Keymap::defaults();
        for action in Action::ALL {
            assert_eq!(keymap.conflicts(*action), vec![], "{action:?}");
            assert_eq!(keymap.keys(*action).len(), action.defaults().len());
        }
    }

    /// docs/reference.md's Keyboard shortcuts has a row for every action,
    /// in Settings' order, with its defaults as config.toml writes them,
    /// and an example Perch reads with no warning.
    #[test]
    fn the_reference_lists_every_action_with_its_defaults() {
        let reference = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/reference.md"
        ));
        let section = reference
            .split("\n## Keyboard shortcuts\n")
            .nth(1)
            .expect("docs/reference.md has a Keyboard shortcuts section");
        let section = section.split("\n## ").next().unwrap();
        let example = section
            .split("```toml\n[keybindings]\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("the section has an example");
        let keymap = with_bindings(example);
        assert_eq!(keymap.keys(Action::Pin), &[]);
        assert_eq!(keymap.keys(Action::Refresh).len(), 2);
        let rows: Vec<(&str, String)> = section
            .lines()
            .filter_map(|line| {
                let mut cells = line.strip_prefix("| `")?.split(" | ");
                let id = cells.next()?.strip_suffix('`')?;
                Some((id, cells.next()?.to_string()))
            })
            .collect();
        let expected: Vec<(&str, String)> = Action::ALL
            .iter()
            .map(|action| {
                let keys = match action.defaults() {
                    [] => "none".to_string(),
                    keys => keys
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                };
                (action.id(), keys)
            })
            .collect();
        assert_eq!(rows, expected);
    }

    #[test]
    fn a_key_can_be_had_in_two_contexts() {
        let keymap = Keymap::defaults();
        assert_eq!(keymap.keys(Action::FindNext), keymap.keys(Action::Suggest));
        assert!(keymap.matches(Action::FindNext, &press("super+g"), false));
        assert!(keymap.matches(Action::Suggest, &press("super+g"), false));
    }

    #[test]
    fn the_defaults_are_the_keys_perch_had() {
        let keymap = Keymap::defaults();
        let os = crate::ui::OS_MOD;
        let one = |action: Action| keymap.keys(action)[0];
        assert_eq!(
            one(Action::GoToFile),
            Shortcut {
                modifiers: os,
                code: Code::KeyP
            }
        );
        assert_eq!(
            one(Action::HistoryForward),
            Shortcut {
                modifiers: Modifiers::CONTROL | Modifiers::SHIFT,
                code: Code::Minus
            }
        );
        assert_eq!(
            one(Action::CallHierarchy),
            Shortcut::parse("shift+alt+KeyH").unwrap()
        );
        assert_eq!(one(Action::Pin), Shortcut::key(Code::KeyP));
        assert_eq!(
            keymap.keys(Action::FocusSearch),
            &[
                Shortcut {
                    modifiers: os,
                    code: Code::Digit1
                },
                Shortcut::key(Code::Slash)
            ]
        );
        assert!(keymap.keys(Action::GoToTypeDefinition).is_empty());
        assert_eq!(keymap.label(Action::GoToTypeDefinition), None);
    }

    #[test]
    fn a_keybinding_override_replaces_the_defaults() {
        let (keymap, warnings) = Keymap::from_config(&table(
            "go_to_file = \"super+t\"\nfocus_search = [\"super+2\"]\n",
        ));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            keymap.keys(Action::GoToFile),
            &[Shortcut::parse("super+t").unwrap()]
        );
        assert!(keymap.matches(Action::GoToFile, &press("super+t"), false));
        assert!(!keymap.matches(Action::GoToFile, &press("super+p"), false));
        assert_eq!(
            keymap.keys(Action::FocusSearch),
            &[Shortcut::parse("super+2").unwrap()]
        );
        assert!(!keymap.matches(Action::FocusSearch, &press("/"), false));
        // The rest keep theirs.
        assert!(keymap.matches(Action::Close, &press("super+w"), false));
    }

    #[test]
    fn several_keys_can_run_one_action() {
        let keymap = with_bindings("pin = [\"p\", \"super+shift+p\", \"p\"]");
        assert_eq!(keymap.keys(Action::Pin).len(), 2);
        assert!(keymap.matches(Action::Pin, &press("super+shift+p"), false));
        assert_eq!(keymap.label(Action::Pin).as_deref(), Some("P"));
    }

    #[test]
    fn an_empty_string_or_list_leaves_an_action_unbound() {
        let (keymap, warnings) = Keymap::from_config(&table("pin = \"\"\ncomment = []\n"));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(keymap.keys(Action::Pin).is_empty());
        assert!(keymap.keys(Action::Comment).is_empty());
        assert!(!keymap.matches(Action::Pin, &press("p"), false));
        assert_eq!(keymap.label(Action::Comment), None);
    }

    #[test]
    fn unknown_ids_and_bad_keys_warn_and_the_rest_is_read() {
        let (keymap, warnings) = Keymap::from_config(&table(
            "fly = \"super+y\"\npin = \"hyper+p\"\ncomment = [\"c\", \"nope\"]\nclose = 3\ngo_to_file = \"super+t\"\n",
        ));
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        for warning in &warnings {
            assert!(
                warning.starts_with("config.toml: [keybindings] "),
                "{warning}"
            );
            assert!(warning.contains("was not read"), "{warning}");
        }
        assert!(warnings.iter().any(|w| w.contains("fly = \"super+y\"")));
        assert!(warnings.iter().any(|w| w.contains("\"hyper+p\"")));
        assert!(warnings.iter().any(|w| w.contains("\"nope\"")));
        assert!(warnings.iter().any(|w| w.contains("close = 3")));
        // A key that didn't read leaves its action with what did.
        assert!(keymap.keys(Action::Pin).is_empty());
        assert_eq!(keymap.keys(Action::Comment), &[Shortcut::key(Code::KeyC)]);
        // A value of another kind leaves the defaults.
        assert!(keymap.matches(Action::Close, &press("super+w"), false));
        assert!(keymap.matches(Action::GoToFile, &press("super+t"), false));
    }

    #[test]
    fn bare_keys_yield_to_typing() {
        let keymap = with_bindings("toggle_files = \"t\"");
        assert!(keymap.matches(Action::ToggleFiles, &press("t"), false));
        assert!(!keymap.matches(Action::ToggleFiles, &press("t"), true));
        // `/` searches, but not from a box.
        assert!(keymap.matches(Action::FocusSearch, &press("/"), false));
        assert!(!keymap.matches(Action::FocusSearch, &press("/"), true));
        assert!(keymap.matches(Action::FocusSearch, &press("super+1"), true));
        assert!(!keymap.matches(Action::NextUnviewed, &press("shift+]"), true));
        // Keys that don't type are heard while typing.
        assert!(keymap.matches(Action::GoToDefinition, &press("f12"), true));
        assert!(keymap.matches(Action::PageUp, &press("pageup"), true));
    }

    #[test]
    fn a_match_takes_the_modifiers_exactly() {
        let keymap = Keymap::defaults();
        assert!(!keymap.matches(Action::Back, &press("super+shift+["), false));
        assert!(keymap.matches(Action::PreviousTab, &press("super+shift+["), false));
        assert!(!keymap.matches(Action::Comment, &press("shift+a"), false));
        assert!(!keymap.matches(Action::Find, &press("super+alt+f"), false));
    }

    #[test]
    fn conflicts_name_the_other_actions_on_a_key() {
        let keymap = with_bindings("pin = \"super+w\"\nbold = \"super+w\"");
        assert_eq!(keymap.conflicts(Action::Pin), vec![Action::Close]);
        // Cmd-W closes from a composer too, so Bold would never run; Pin
        // waits for the composer, which keeps the key.
        assert_eq!(
            keymap.conflicts(Action::Close),
            vec![Action::Pin, Action::Bold]
        );
        assert_eq!(keymap.conflicts(Action::Bold), vec![Action::Close]);
    }

    /// A composer's key conflicts with a window key the window hears first
    /// from a composer, and not with one that waits while a composer has
    /// the keyboard, as Find's keys do.
    #[test]
    fn a_composer_key_conflicts_with_a_window_key_heard_in_a_composer() {
        let keymap = with_bindings("bold = \"super+shift+e\"\nitalic = \"super+f\"");
        assert_eq!(keymap.conflicts(Action::Bold), vec![Action::ToggleFiles]);
        assert_eq!(keymap.conflicts(Action::ToggleFiles), vec![Action::Bold]);
        assert_eq!(keymap.conflicts(Action::Italic), vec![]);
        assert_eq!(keymap.conflicts(Action::Find), vec![]);
        let keymap = with_bindings("toggle_files = \"super+b\"");
        assert_eq!(keymap.conflicts(Action::Bold), vec![Action::ToggleFiles]);
        // A key that types is the composer's text, whatever it runs.
        let keymap = with_bindings("bold = \"j\"");
        assert_eq!(keymap.conflicts(Action::Bold), vec![]);
        for action in Action::ALL {
            if action.context() == Context::Composer {
                assert!(!action.heard_in_composer(), "{action:?}");
            }
        }
        assert!(!Action::FindNext.heard_in_composer());
    }

    #[test]
    fn the_global_keymap_is_the_defaults_until_one_is_installed() {
        assert!(pressed(Action::GoToFile, &press("super+p"), false));
        assert_eq!(
            label(Action::GoToFile),
            Keymap::defaults().label(Action::GoToFile)
        );
        {
            let _keymap = install(with_bindings("go_to_file = \"super+t\""));
            assert!(!pressed(Action::GoToFile, &press("super+p"), false));
            assert!(pressed(Action::GoToFile, &press("super+t"), false));
            assert_eq!(
                keys(Action::GoToFile),
                vec![Shortcut::parse("super+t").unwrap()]
            );
        }
        assert!(pressed(Action::GoToFile, &press("super+p"), false));
        assert_eq!(*current(), Keymap::defaults());
    }

    #[test]
    fn a_label_follows_the_keymap() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let effect_seen = seen.clone();
        floem::reactive::Effect::new(move |_| {
            effect_seen.borrow_mut().push(label(Action::Refresh));
        });
        let _keymap = install(with_bindings("refresh = \"f5\""));
        // The same keymap again is no change.
        set(with_bindings("refresh = \"f5\""));
        assert_eq!(
            *seen.borrow(),
            vec![
                Keymap::defaults().label(Action::Refresh),
                Some("F5".to_string())
            ]
        );
    }
}
