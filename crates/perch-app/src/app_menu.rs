//! Check for Updates…, New Window, Open…, Send Feedback… and Star Perch on
//! GitHub in the Perch menu. The menu is
//! winit's; the items' target is Perch's own, not a window's, so they
//! still work with no window left.

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenuItem};
    use objc2_foundation::{ns_string, NSObjectProtocol};

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
        // About and Check for Updates…, a separator, then New Window,
        // Open…, Send Feedback…, Star Perch on GitHub and a separator of
        // their own.
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
                ns_string!("n"),
            )
        };
        item.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        );
        unsafe { item.setTarget(Some(&target)) };
        let open = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Open…"),
                Some(sel!(openReference:)),
                ns_string!("o"),
            )
        };
        open.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
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
        menu.insertItem_atIndex(&updates, menu.numberOfItems().min(1));
        let at = menu.numberOfItems().min(3);
        menu.insertItem_atIndex(&item, at);
        menu.insertItem_atIndex(&open, at + 1);
        menu.insertItem_atIndex(&feedback, at + 2);
        menu.insertItem_atIndex(&star, at + 3);
        menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), at + 4);
        // A menu item holds its target weakly, and this one is for the
        // app's life.
        std::mem::forget(target);
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

#[cfg(not(target_os = "macos"))]
pub fn install() {}
