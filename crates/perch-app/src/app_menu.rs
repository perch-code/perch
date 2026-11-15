//! Check for Updates…, Settings…, New Window, Open…, Send Feedback… and
//! Star Perch on GitHub in the Perch menu. The menu is
//! winit's; the items' target is Perch's own, not a window's, so they
//! still work with no window left.
//!
//! New Window, Open… and Settings… hold a key from the keymap, which the
//! menu hears before any window does. The window hears their other keys.
//! While a shortcut in Settings waits for keys the menu holds none, so the
//! key pressed reaches it.

use std::cell::Cell;

use floem::ui_events::keyboard::{Code, KeyboardEvent, Modifiers};

use crate::keymap::{self, Action};
use crate::ui::menu::Shortcut;

/// The actions with an item of their own in the Perch menu.
const ITEMS: [Action; 3] = [Action::NewWindow, Action::Open, Action::Settings];

thread_local! {
    /// How many [`KeysLetThrough`] guards are alive.
    static LETTING_THROUGH: Cell<usize> = const { Cell::new(0) };
}

/// While it lives the Perch menu holds no key, and its items have their
/// keys back once the last one goes. On the main thread, as the menu is.
#[must_use]
pub struct KeysLetThrough(());

impl Drop for KeysLetThrough {
    fn drop(&mut self) {
        // A window's views can go as the thread does, when there is no
        // menu left to give keys to.
        let last = LETTING_THROUGH
            .try_with(|n| {
                n.set(n.get() - 1);
                n.get() == 0
            })
            .unwrap_or(false);
        if last {
            take_up_keys();
        }
    }
}

/// Take the Perch menu's keys off its items until the guard goes, for a
/// shortcut waiting for keys to hear Cmd-O rather than the menu opening
/// Open….
pub fn let_keys_through() -> KeysLetThrough {
    let first = LETTING_THROUGH.with(|n| {
        n.set(n.get() + 1);
        n.get() == 1
    });
    if first {
        take_up_keys();
    }
    KeysLetThrough(())
}

/// The key the Perch menu's item for `action` holds: on macOS, the first
/// of the action's keys, unless it types, as a menu item would take it
/// from a box being typed in, or is a key a menu can't show. None off
/// macOS, where there is no such menu, and while the menu lets keys
/// through.
pub fn held(action: Action) -> Option<Shortcut> {
    if !cfg!(target_os = "macos") || !ITEMS.contains(&action) {
        return None;
    }
    if LETTING_THROUGH.with(Cell::get) > 0 {
        return None;
    }
    keymap::current()
        .keys(action)
        .first()
        .copied()
        .filter(|key| !key.is_bare() && key_equivalent(key.code).is_some())
}

/// Whether `key` runs `action` from the window: one of its keys that the
/// Perch menu doesn't hold.
pub fn pressed_in_window(action: Action, key: &KeyboardEvent, typing: bool) -> bool {
    keymap::pressed(action, key, typing) && held(action).is_none_or(|held| !held.matches(key))
}

/// What AppKit calls a key in a menu item: the character on it, or for a
/// key that writes none, the private-use character AppKit gives it. None
/// for a key it has no character for.
fn key_equivalent(code: Code) -> Option<String> {
    let function = |n: u32| char::from_u32(n).map(String::from);
    match code {
        Code::Enter => Some("\r".to_string()),
        Code::Escape => Some("\u{1b}".to_string()),
        Code::Tab => Some("\t".to_string()),
        Code::Space => Some(" ".to_string()),
        Code::Backspace => Some("\u{8}".to_string()),
        Code::ArrowUp => function(0xF700),
        Code::ArrowDown => function(0xF701),
        Code::ArrowLeft => function(0xF702),
        Code::ArrowRight => function(0xF703),
        Code::Delete => function(0xF728),
        Code::Home => function(0xF729),
        Code::End => function(0xF72B),
        Code::PageUp => function(0xF72C),
        Code::PageDown => function(0xF72D),
        _ => {
            // A letter, a digit or punctuation is written as itself.
            let written = Shortcut::key(code).to_config();
            if written.chars().count() == 1 {
                return Some(written);
            }
            let n = written.strip_prefix('f')?.parse::<u32>().ok()?;
            (1..=20).contains(&n).then(|| function(0xF704 + n - 1))?
        }
    }
}

/// The modifiers as a menu item's mask has them.
#[cfg(target_os = "macos")]
fn mask(modifiers: Modifiers) -> objc2_app_kit::NSEventModifierFlags {
    use objc2_app_kit::NSEventModifierFlags as Flags;
    let mut mask = Flags::empty();
    for (modifier, flag) in [
        (Modifiers::META, Flags::Command),
        (Modifiers::SHIFT, Flags::Shift),
        (Modifiers::ALT, Flags::Option),
        (Modifiers::CONTROL, Flags::Control),
    ] {
        if modifiers.contains(modifier) {
            mask |= flag;
        }
    }
    mask
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenuItem};
    use objc2_foundation::{ns_string, NSObjectProtocol, NSString};

    use crate::keymap::Action;

    thread_local! {
        /// The items that hold a key, for the keymap to set again.
        static KEYED: RefCell<Vec<(Action, Retained<NSMenuItem>)>> = const { RefCell::new(Vec::new()) };
    }

    /// Give each item that holds a key the keymap's, or none.
    pub fn take_up_keys() {
        let _ = KEYED.try_with(|keyed| {
            for (action, item) in keyed.borrow().iter() {
                let (equivalent, mask) = match super::held(*action) {
                    Some(key) => (
                        super::key_equivalent(key.code).unwrap_or_default(),
                        super::mask(key.modifiers),
                    ),
                    None => (String::new(), NSEventModifierFlags::empty()),
                };
                item.setKeyEquivalent(&NSString::from_str(&equivalent));
                item.setKeyEquivalentModifierMask(mask);
            }
        });
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "PerchMenuTarget"]
        struct Target;

        unsafe impl NSObjectProtocol for Target {}

        impl Target {
            #[unsafe(method(newWindow:))]
            fn new_window(&self, _sender: Option<&AnyObject>) {
                crate::windows::open_new();
            }

            #[unsafe(method(openReference:))]
            fn open_reference(&self, _sender: Option<&AnyObject>) {
                crate::windows::show_open_box();
            }

            #[unsafe(method(checkForUpdate:))]
            fn check_for_update(&self, _sender: Option<&AnyObject>) {
                crate::windows::check_for_update();
            }

            #[unsafe(method(openSettings:))]
            fn open_settings(&self, _sender: Option<&AnyObject>) {
                crate::windows::show_settings();
            }

            #[unsafe(method(sendFeedback:))]
            fn send_feedback(&self, _sender: Option<&AnyObject>) {
                crate::windows::show_feedback();
            }

            #[unsafe(method(starOnGithub:))]
            fn star_on_github(&self, _sender: Option<&AnyObject>) {
                crate::windows::star_on_github();
            }
        }
    );

    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // About and Check for Updates…, a separator, Settings… and a
        // separator, as macOS apps have it, then New Window, Open…, Send
        // Feedback…, Star Perch on GitHub and a separator of their own.
        let Some(menu) = app
            .mainMenu()
            .and_then(|bar| bar.itemAtIndex(0))
            .and_then(|item| item.submenu())
        else {
            return;
        };
        let target: Retained<Target> = unsafe { msg_send![super(mtm.alloc().set_ivars(())), init] };
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("New Window"),
                Some(sel!(newWindow:)),
                ns_string!(""),
            )
        };
        unsafe { item.setTarget(Some(&target)) };
        let open = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Open…"),
                Some(sel!(openReference:)),
                ns_string!(""),
            )
        };
        unsafe { open.setTarget(Some(&target)) };
        let feedback = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Send Feedback…"),
                Some(sel!(sendFeedback:)),
                ns_string!(""),
            )
        };
        unsafe { feedback.setTarget(Some(&target)) };
        let star = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Star Perch on GitHub"),
                Some(sel!(starOnGithub:)),
                ns_string!(""),
            )
        };
        unsafe { star.setTarget(Some(&target)) };
        let updates = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Check for Updates…"),
                Some(sel!(checkForUpdate:)),
                ns_string!(""),
            )
        };
        unsafe { updates.setTarget(Some(&target)) };
        let settings = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Settings…"),
                Some(sel!(openSettings:)),
                ns_string!(""),
            )
        };
        unsafe { settings.setTarget(Some(&target)) };
        menu.insertItem_atIndex(&updates, menu.numberOfItems().min(1));
        let at = menu.numberOfItems().min(3);
        menu.insertItem_atIndex(&settings, at);
        menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), at + 1);
        let at = at + 2;
        menu.insertItem_atIndex(&item, at);
        menu.insertItem_atIndex(&open, at + 1);
        menu.insertItem_atIndex(&feedback, at + 2);
        menu.insertItem_atIndex(&star, at + 3);
        menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), at + 4);
        KEYED.with(|keyed| {
            *keyed.borrow_mut() = vec![
                (Action::NewWindow, item),
                (Action::Open, open),
                (Action::Settings, settings),
            ];
        });
        take_up_keys();
        // A menu item holds its target weakly, and this one is for the
        // app's life.
        std::mem::forget(target);
    }
}

#[cfg(target_os = "macos")]
pub use mac::{install, take_up_keys};

#[cfg(not(target_os = "macos"))]
pub fn install() {}

/// Give the Perch menu's items the keymap's keys. Off macOS there is no
/// such menu.
#[cfg(not(target_os = "macos"))]
pub fn take_up_keys() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_holds_the_first_key_of_new_window_open_and_settings() {
        let held_now = || ITEMS.map(held);
        if !cfg!(target_os = "macos") {
            assert_eq!(held_now(), [None; 3]);
            return;
        }
        assert_eq!(
            held_now(),
            [
                Shortcut::parse("super+shift+n"),
                Shortcut::parse("super+o"),
                Shortcut::parse("super+,"),
            ]
        );
        assert_eq!(held(Action::GoToFile), None);
        assert_eq!(key_equivalent(Code::KeyN).as_deref(), Some("n"));
        assert_eq!(key_equivalent(Code::Comma).as_deref(), Some(","));
        assert_eq!(key_equivalent(Code::F12).as_deref(), Some("\u{F70F}"));
        assert_eq!(key_equivalent(Code::PageDown).as_deref(), Some("\u{F72D}"));
        assert_eq!(key_equivalent(Code::NumpadEnter), None);
    }

    /// While a shortcut waits for keys the menu holds none, so Cmd-O
    /// reaches it, and the menu has them back when the last wait is over.
    #[test]
    fn the_menu_lets_its_keys_through_while_a_shortcut_waits() {
        let first = let_keys_through();
        let second = let_keys_through();
        assert_eq!(held(Action::Open), None);
        drop(first);
        assert_eq!(held(Action::Open), None, "the other still waits");
        drop(second);
        assert_eq!(
            held(Action::Open),
            Shortcut::parse("super+o").filter(|_| cfg!(target_os = "macos"))
        );
    }

    /// A key that types stays with the window, which gives it to a box
    /// being typed in; the menu holds none then, and the window hears all
    /// of the action's keys.
    #[test]
    fn a_key_that_types_or_a_second_key_is_the_windows() {
        let _keymap = keymap::install(keymap::with_bindings(
            "settings = [\"s\", \"super+,\"]\nopen = [\"super+o\", \"f2\"]",
        ));
        let press = |keys: &str| {
            let key = Shortcut::parse(keys).unwrap();
            KeyboardEvent {
                state: floem::ui_events::keyboard::KeyState::Down,
                key: floem::ui_events::keyboard::Key::Character(String::new()),
                code: key.code,
                location: Default::default(),
                modifiers: key.modifiers,
                repeat: false,
                is_composing: false,
            }
        };
        assert_eq!(held(Action::Settings), None);
        assert!(pressed_in_window(Action::Settings, &press("s"), false));
        assert!(!pressed_in_window(Action::Settings, &press("s"), true));
        assert!(pressed_in_window(
            Action::Settings,
            &press("super+,"),
            false
        ));
        assert!(pressed_in_window(Action::Open, &press("f2"), false));
        assert_eq!(
            pressed_in_window(Action::Open, &press("super+o"), false),
            !cfg!(target_os = "macos"),
            "the menu has Cmd-O"
        );
    }
}
