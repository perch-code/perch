//! New Window in the Perch menu. The menu is winit's; the item's target is
//! Perch's own, not a window's, so it still opens one with none left.

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
        }
    );

    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // About, a separator, then New Window and a separator of its own.
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
        let at = menu.numberOfItems().min(2);
        menu.insertItem_atIndex(&item, at);
        menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), at + 1);
        // A menu item holds its target weakly, and this one is for the
        // app's life.
        std::mem::forget(target);
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

#[cfg(not(target_os = "macos"))]
pub fn install() {}
