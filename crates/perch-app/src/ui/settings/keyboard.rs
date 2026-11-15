//! Keyboard shortcuts: the keys each of Perch's commands is on, one group
//! for each part of the window. What the rows show is what config.toml's
//! `[keybindings]` gives, so a key written here, or by hand, shows once
//! the backend reads the file again, as every other setting does.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use perch_github::Setting;

use super::controls;
use super::{Group, Page, Row};
use crate::keymap::{self, Action, Keymap};

/// A `[keybindings]` table and the keymap made from it.
type Read = (BTreeMap<String, toml::Value>, Rc<Keymap>);

thread_local! {
    /// The keymap last made from the file, since every row's label and
    /// description asks for it.
    static READ: RefCell<Option<Read>> = const { RefCell::new(None) };
}

/// The keymap the file gives, tracked.
fn keymap(page: &Page) -> Rc<Keymap> {
    page.with_config(|config| {
        READ.with(|read| {
            let mut read = read.borrow_mut();
            if let Some((table, keymap)) = read.as_ref() {
                if *table == config.keybindings {
                    return keymap.clone();
                }
            }
            let keymap = Rc::new(Keymap::from_config(&config.keybindings).0);
            *read = Some((config.keybindings.clone(), keymap.clone()));
            keymap
        })
    })
}

pub(super) fn groups(page: &Page) -> Vec<Group> {
    keymap::Group::ALL
        .into_iter()
        .map(|group| {
            let rows = Action::ALL
                .iter()
                .copied()
                .filter(|action| action.group() == group)
                .map(|action| row(page, action))
                .collect();
            Group::new(group.title(), rows)
        })
        .collect()
}

/// Titles as a sentence lists them: "A", "A and B", "A, B and C".
fn listed(titles: &[&str]) -> String {
    match titles {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn row(page: &Page, action: Action) -> Row {
    let description = {
        let page = page.clone();
        move || {
            let keymap = keymap(&page);
            let others: Vec<&str> = keymap
                .conflicts(action)
                .into_iter()
                .map(Action::title)
                .collect();
            let mut description = action.description().to_string();
            if !others.is_empty() {
                description.push_str(&format!(" Shares a key with {}.", listed(&others)));
            }
            // Written by hand: the button refuses them.
            let kept: Vec<String> = keymap
                .keys(action)
                .iter()
                .filter(|keys| controls::taken(**keys))
                .map(|keys| keys.label())
                .collect();
            if !kept.is_empty() {
                description.push_str(&format!(
                    " macOS keeps {} for itself, so {}.",
                    kept.join(" and "),
                    if kept.len() == 1 {
                        "it never runs"
                    } else {
                        "they never run"
                    }
                ));
            }
            description
        }
    };
    let control = {
        let page = page.clone();
        move || {
            let (keys, enabled, write) = (page.clone(), page.clone(), page.clone());
            controls::shortcut(
                page.state.settings,
                move || keymap(&keys).keys(action).to_vec(),
                move || enabled.writable(),
                move |keys| {
                    write.write(Setting::Keybinding {
                        action: action.id(),
                        keys: Some(keys.map(|k| k.to_config()).into_iter().collect()),
                    })
                },
                action.id(),
            )
        }
    };
    Row::new(
        format!("shortcut:{}", action.id()),
        action.title(),
        description,
        control,
    )
    .keywords(&[action.id(), "shortcut", "keybinding", "key"])
    .reset(
        format!("keybindings.{}", action.id()),
        Setting::Keybinding {
            action: action.id(),
            keys: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use floem::prelude::*;
    use floem::reactive::{SignalGet, SignalUpdate};

    use super::super::tests::{
        click, escape, file, goes_dark, key, named, open_on, settle, shown, window, written,
    };
    use super::super::{found, words, Section};
    use super::*;
    use crate::ui::menu::Shortcut;

    /// `keys` as config.toml writes them here: `super` is Ctrl off macOS.
    fn written_as(keys: &str) -> Setting {
        Setting::Keybinding {
            action: "new_work",
            keys: Some(vec![Shortcut::parse(keys).unwrap().to_config()]),
        }
    }

    fn press(harness: &mut floem::headless::HeadlessHarness, code: Code, c: &str, mods: Modifiers) {
        harness.dispatch_event(key(code, Key::Character(c.into()), mods));
        settle(harness);
    }

    /// A click sets a shortcut waiting; the next key, with its modifiers,
    /// is written in config.toml's spelling and the wait is over.
    /// Modifiers alone wait on for a key.
    #[test]
    fn a_shortcut_writes_the_keys_pressed() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        assert!(shown(&harness, "settings-group:General"));
        assert!(shown(&harness, "settings-row:shortcut:new_work"));

        click(&mut harness, "settings-shortcut:new_work");
        let button = named(harness.root_id(), "settings-shortcut:new_work").unwrap();
        assert_eq!(state.settings.capturing.get_untracked(), Some(button));
        assert!(harness.is_focused(button));
        press(&mut harness, Code::ShiftLeft, "", Modifiers::SHIFT);
        assert!(written(&mut commands).is_empty(), "a modifier waits");
        assert!(state.settings.capturing.get_untracked().is_some());

        press(
            &mut harness,
            Code::KeyT,
            "t",
            crate::ui::OS_MOD | Modifiers::SHIFT,
        );
        assert_eq!(written(&mut commands), vec![written_as("super+shift+t")]);
        assert_eq!(state.settings.capturing.get_untracked(), None);
        // Enter on the button waits again; a key that types is a key too.
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert_eq!(state.settings.capturing.get_untracked(), Some(button));
        press(&mut harness, Code::Slash, "/", Modifiers::empty());
        assert_eq!(written(&mut commands), vec![written_as("/")]);
        goes_dark(&mut harness);
    }

    /// A key going down, or up, or repeating as it is held.
    fn stroke(
        harness: &mut floem::headless::HeadlessHarness,
        code: Code,
        key: Key,
        mods: Modifiers,
        state: KeyState,
        repeat: bool,
    ) {
        harness.dispatch_event(floem::event::Event::Key(KeyboardEvent {
            state,
            key,
            code,
            location: Default::default(),
            modifiers: mods,
            repeat,
            is_composing: false,
        }));
        settle(harness);
    }

    /// Enter or Space pressed and let go, as a reader does.
    fn tap(harness: &mut floem::headless::HeadlessHarness, code: Code, key: Key) {
        let none = Modifiers::empty();
        stroke(harness, code, key.clone(), none, KeyState::Down, false);
        stroke(harness, code, key, none, KeyState::Up, false);
    }

    /// Tab round the page until `name` has the keyboard.
    fn tab_to(harness: &mut floem::headless::HeadlessHarness, name: &str) -> floem::ViewId {
        let id = named(harness.root_id(), name).unwrap();
        for _ in 0..400 {
            if harness.is_focused(id) {
                return id;
            }
            press(harness, Code::Tab, "", Modifiers::empty());
        }
        panic!("Tab never reached {name}");
    }

    /// Return on a shortcut the keyboard is on sets it waiting, and the key
    /// coming up, which floem makes a click of, leaves it waiting, so the
    /// next key is written. Space given to it is written once, and coming
    /// up doesn't set it waiting again. Return on the × clears once.
    #[test]
    fn enter_and_space_on_a_shortcut_are_heard_once() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        let button = tab_to(&mut harness, "settings-shortcut:new_work");
        tap(&mut harness, Code::Enter, Key::Named(NamedKey::Enter));
        assert_eq!(state.settings.capturing.get_untracked(), Some(button));
        press(&mut harness, Code::KeyT, "t", crate::ui::OS_MOD);
        assert_eq!(written(&mut commands), vec![written_as("super+t")]);
        assert_eq!(state.settings.capturing.get_untracked(), None);

        tap(&mut harness, Code::Space, Key::Character(" ".into()));
        assert_eq!(state.settings.capturing.get_untracked(), Some(button));
        tap(&mut harness, Code::Space, Key::Character(" ".into()));
        assert_eq!(written(&mut commands), vec![written_as("space")]);
        assert_eq!(
            state.settings.capturing.get_untracked(),
            None,
            "waiting again"
        );
        press(&mut harness, Code::Tab, "", Modifiers::SHIFT);
        assert!(written(&mut commands).is_empty(), "Shift-Tab moved on");

        let clear = named(harness.root_id(), "settings-shortcut-clear:new_work").unwrap();
        assert!(harness.is_focused(clear));
        tap(&mut harness, Code::Enter, Key::Named(NamedKey::Enter));
        assert_eq!(
            written(&mut commands),
            vec![Setting::Keybinding {
                action: "new_work",
                keys: Some(vec![]),
            }]
        );
    }

    /// The key just given to a shortcut, held down, repeats, and the
    /// repeats are not the page's: a held Cmd-W doesn't shut it, nor a
    /// held Tab move the keyboard on. Pressed again, it is the page's.
    #[test]
    fn a_key_held_past_its_capture_repeats_into_nothing() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        let button = named(harness.root_id(), "settings-shortcut:new_work").unwrap();
        let held = |harness: &mut floem::headless::HeadlessHarness,
                    code: Code,
                    key: Key,
                    mods: Modifiers| {
            for _ in 0..3 {
                stroke(harness, code, key.clone(), mods, KeyState::Down, true);
            }
        };

        click(&mut harness, "settings-shortcut:new_work");
        press(&mut harness, Code::KeyW, "w", crate::ui::OS_MOD);
        held(
            &mut harness,
            Code::KeyW,
            Key::Character("w".into()),
            crate::ui::OS_MOD,
        );
        assert!(state.settings.open.get_untracked(), "the page is up");
        assert_eq!(written(&mut commands), vec![written_as("super+w")]);
        assert!(harness.is_focused(button));

        click(&mut harness, "settings-shortcut:new_work");
        press(&mut harness, Code::Tab, "", Modifiers::empty());
        held(
            &mut harness,
            Code::Tab,
            Key::Named(NamedKey::Tab),
            Modifiers::empty(),
        );
        assert_eq!(written(&mut commands), vec![written_as("tab")]);
        assert!(harness.is_focused(button), "Tab moved the keyboard");

        // A key held as the wait starts is not the key it waits for.
        let (enter, none) = (Key::Named(NamedKey::Enter), Modifiers::empty());
        stroke(
            &mut harness,
            Code::Enter,
            enter.clone(),
            none,
            KeyState::Down,
            false,
        );
        held(&mut harness, Code::Enter, enter.clone(), none);
        stroke(&mut harness, Code::Enter, enter, none, KeyState::Up, false);
        assert_eq!(state.settings.capturing.get_untracked(), Some(button));
        assert!(written(&mut commands).is_empty());
        escape(&mut harness);

        press(&mut harness, Code::KeyW, "w", crate::ui::OS_MOD);
        assert!(!state.settings.open.get_untracked(), "Cmd-W shut the page");
    }

    /// The Perch menu lets its keys through from the moment a shortcut
    /// waits until the key it was given comes up, so neither the key nor
    /// its repeats are menu items'. Escape gives them back at once, and so
    /// does the window shutting while a shortcut waits.
    #[test]
    fn the_menu_has_its_keys_back_once_the_key_given_is_up() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        let letting = || state.settings.menu_keys.with_untracked(Option::is_some);
        let (o, cmd) = (Key::Character("o".into()), crate::ui::OS_MOD);

        click(&mut harness, "settings-shortcut:open");
        assert!(letting(), "waiting, the menu holds Cmd-O");
        stroke(
            &mut harness,
            Code::KeyO,
            o.clone(),
            cmd,
            KeyState::Down,
            false,
        );
        assert_eq!(
            written(&mut commands),
            vec![Setting::Keybinding {
                action: "open",
                keys: Some(vec!["super+o".to_string()]),
            }]
        );
        stroke(
            &mut harness,
            Code::KeyO,
            o.clone(),
            cmd,
            KeyState::Down,
            true,
        );
        assert!(letting(), "the menu took Cmd-O back while it was down");
        stroke(&mut harness, Code::KeyO, o, cmd, KeyState::Up, false);
        assert!(!letting());

        click(&mut harness, "settings-shortcut:open");
        assert!(letting());
        escape(&mut harness);
        assert!(!letting());

        click(&mut harness, "settings-shortcut:open");
        assert!(letting());
        state.settings.window_closed();
        assert!(!letting());
        assert_eq!(state.settings.capturing.get_untracked(), None);
        assert!(written(&mut commands).is_empty());
    }

    /// A shortcut waiting as config.toml stops reading, which leaves the
    /// button unable to hear keys, stops waiting, and the page's keys and
    /// the menu's are back.
    #[test]
    fn a_shortcut_stops_waiting_when_the_file_cannot_be_written() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        click(&mut harness, "settings-shortcut:new_work");
        assert!(state.settings.capturing.get_untracked().is_some());
        state.settings_file.update(|file| {
            if let Some(file) = file {
                file.error = Some("expected `=`".into());
            }
        });
        settle(&mut harness);
        assert_eq!(state.settings.capturing.get_untracked(), None);
        assert!(state.settings.menu_keys.with_untracked(Option::is_none));
        escape(&mut harness);
        assert!(!state.settings.open.get_untracked(), "Escape shut the page");
        assert!(written(&mut commands).is_empty());
    }

    /// Escape stops the wait and writes nothing; the page stays up, and a
    /// second Escape shuts it.
    #[test]
    fn escape_stops_a_shortcut_waiting_with_no_change() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        click(&mut harness, "settings-shortcut:new_work");
        assert!(state.settings.capturing.get_untracked().is_some());
        escape(&mut harness);
        assert_eq!(state.settings.capturing.get_untracked(), None);
        assert!(written(&mut commands).is_empty());
        assert!(state.settings.open.get_untracked());
        escape(&mut harness);
        assert!(!state.settings.open.get_untracked());
    }

    /// While a shortcut waits, Cmd-W is a key for it rather than shutting
    /// the page, and Tab is a key rather than moving on. The keyboard
    /// going elsewhere stops the wait.
    #[test]
    fn a_waiting_shortcut_takes_cmd_w_and_tab() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        let button = named(harness.root_id(), "settings-shortcut:new_work").unwrap();

        click(&mut harness, "settings-shortcut:new_work");
        press(&mut harness, Code::KeyW, "w", crate::ui::OS_MOD);
        assert!(state.settings.open.get_untracked(), "the page is up");
        assert_eq!(written(&mut commands), vec![written_as("super+w")]);

        click(&mut harness, "settings-shortcut:new_work");
        harness.dispatch_event(key(
            Code::Tab,
            Key::Named(NamedKey::Tab),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert_eq!(written(&mut commands), vec![written_as("tab")]);
        assert!(harness.is_focused(button), "Tab didn't move the keyboard");

        click(&mut harness, "settings-shortcut:new_work");
        assert!(state.settings.capturing.get_untracked().is_some());
        click(&mut harness, "settings-search");
        assert_eq!(state.settings.capturing.get_untracked(), None);
        assert!(written(&mut commands).is_empty());
    }

    /// The × takes an action's keys away, writing an empty list; it isn't
    /// there for an action with no keys.
    #[test]
    fn clearing_a_shortcut_writes_no_keys() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        assert!(shown(&harness, "settings-shortcut-clear:new_work"));
        assert!(!shown(&harness, "settings-shortcut-clear:send_feedback"));
        click(&mut harness, "settings-shortcut-clear:new_work");
        assert_eq!(
            written(&mut commands),
            vec![Setting::Keybinding {
                action: "new_work",
                keys: Some(vec![]),
            }]
        );
        state
            .settings_file
            .set(Some(file("[keybindings]\nnew_work = []\n")));
        settle(&mut harness);
        assert!(!shown(&harness, "settings-shortcut-clear:new_work"));
    }

    /// The reset arrow shows while the file sets the action's keys, and
    /// takes them out of it.
    #[test]
    fn a_shortcut_resets_only_when_the_file_sets_it() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        assert!(!shown(&harness, "settings-reset:shortcut:new_work"));
        state
            .settings_file
            .set(Some(file("[keybindings]\nnew_work = \"super+t\"\n")));
        settle(&mut harness);
        assert!(shown(&harness, "settings-reset:shortcut:new_work"));
        assert!(!shown(&harness, "settings-reset:shortcut:new_window"));
        click(&mut harness, "settings-reset:shortcut:new_work");
        assert_eq!(
            written(&mut commands),
            vec![Setting::Keybinding {
                action: "new_work",
                keys: None,
            }]
        );
    }

    /// The rows show the file's keys, and say when another action that
    /// would keep it from running has one of them.
    #[test]
    fn a_shortcut_shows_the_files_keys_and_says_what_shares_them() {
        let (_harness, state, _commands) = window(
            1400.0,
            900.0,
            "[keybindings]\npin = \"super+w\"\nbold = \"super+w\"\nnew_work = []\n",
        );
        let page = Page {
            state,
            backend: Rc::new(crate::backend::Backend::detached()),
        };
        let keymap = keymap(&page);
        assert!(keymap.keys(Action::NewWork).is_empty());
        assert_eq!(
            keymap.keys(Action::Pin),
            &[Shortcut::parse("super+w").unwrap()]
        );
        let description = |action: Action| (row(&page, action).description)();
        assert_eq!(
            description(Action::Pin),
            format!("{} Shares a key with Close.", Action::Pin.description())
        );
        assert!(description(Action::Close).ends_with("Shares a key with Pin and Bold."));
        // Pin waits for a composer to have the key, but Close doesn't.
        assert!(description(Action::Bold).ends_with("Shares a key with Close."));
        assert_eq!(description(Action::Italic), Action::Italic.description());
        assert_eq!(listed(&["A", "B", "C"]), "A, B and C");
    }

    /// Search finds a shortcut by its title, and every shortcut by
    /// "shortcut".
    #[test]
    fn search_finds_shortcuts() {
        let (mut harness, state, _commands) = window(1400.0, 900.0, "");
        let page = Page {
            state,
            backend: Rc::new(crate::backend::Backend::detached()),
        };
        let rows = |query: &str| -> Vec<String> {
            found(&page, &words(query))
                .into_iter()
                .filter(|(section, _)| *section == Section::KeyboardShortcuts)
                .flat_map(|(_, groups)| groups.into_iter().flat_map(|g| g.rows))
                .map(|row| row.name)
                .collect()
        };
        assert!(rows("go to file").contains(&"shortcut:go_to_file".to_string()));
        assert_eq!(
            rows("toggle merge"),
            vec!["shortcut:toggle_merge".to_string()]
        );
        assert_eq!(
            rows("toggle_merge"),
            vec!["shortcut:toggle_merge".to_string()]
        );
        assert_eq!(rows("shortcut").len(), Action::ALL.len());

        super::super::show(state, None);
        settle(&mut harness);
        state.settings.query.set("go to file".into());
        settle(&mut harness);
        assert!(shown(&harness, "settings-row:shortcut:go_to_file"));
        assert!(shown(&harness, "settings-group:Keyboard shortcuts · Code"));
    }

    /// A key macOS keeps for itself that reaches the button, Cmd-M, is
    /// refused with a note, and the shortcut goes on waiting. One written
    /// by hand, such as Cmd-Opt-H, which the app menu has first, is said
    /// on the row.
    #[test]
    #[cfg(target_os = "macos")]
    fn keys_macos_keeps_are_refused() {
        let (mut harness, state, mut commands) = window(1400.0, 900.0, "");
        open_on(&mut harness, state, Section::KeyboardShortcuts);
        assert!(!shown(&harness, "settings-shortcut-note:new_work"));
        click(&mut harness, "settings-shortcut:new_work");
        press(&mut harness, Code::KeyM, "m", Modifiers::META);
        assert!(written(&mut commands).is_empty());
        assert!(shown(&harness, "settings-shortcut-note:new_work"));
        assert!(state.settings.capturing.get_untracked().is_some());
        press(
            &mut harness,
            Code::KeyN,
            "n",
            Modifiers::META | Modifiers::ALT,
        );
        assert_eq!(written(&mut commands), vec![written_as("alt+super+n")]);
        assert!(!shown(&harness, "settings-shortcut-note:new_work"));

        let (_harness, state, _commands) = window(
            1400.0,
            900.0,
            "[keybindings]\npin = [\"alt+super+h\", \"super+q\"]\n",
        );
        let page = Page {
            state,
            backend: Rc::new(crate::backend::Backend::detached()),
        };
        assert_eq!(
            (row(&page, Action::Pin).description)(),
            format!(
                "{} macOS keeps ⌥⌘H and ⌘Q for itself, so they never run.",
                Action::Pin.description()
            )
        );
    }
}
