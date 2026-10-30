//! Back and Forward by swipe. Logitech's Options+ sends a mouse's back
//! and forward buttons as the swipe Safari and Finder go back and forward
//! on, not as buttons 4 and 5, and so does a three-finger swipe on a
//! trackpad set to swipe between pages. winit passes swipes over, so they
//! are heard from AppKit directly.

use std::cell::RefCell;

use floem::ext_event::ExtSendTrigger;

thread_local! {
    /// Swipes heard and not yet acted on, oldest first: true for Back.
    static PENDING: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
}

/// The swipes heard since the last call, oldest first: true for Back,
/// false for Forward.
pub fn take() -> Vec<bool> {
    PENDING.with_borrow_mut(std::mem::take)
}

/// Notifies `swiped` after each sideways swipe, for as long as the app
/// runs, with the way it went left for `take`.
#[cfg(target_os = "macos")]
pub fn watch(swiped: ExtSendTrigger) {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask};

    // Only the app asks; a test has no AppKit to hear from.
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event.
        let dx = unsafe { event.as_ref() }.deltaX();
        tracing::debug!(dx, "swipe");
        // Left is positive, as WebKit and Chromium read it: a swipe left,
        // or the back button, goes back. Options+ sends each press as a
        // swipe of 0 and then one of 1 or -1.
        if dx.abs() > 0.5 {
            PENDING.with_borrow_mut(|pending| pending.push(dx > 0.0));
            floem::ext_event::register_ext_trigger(swiped);
        }
        // Passed on unchanged, for whatever else listens.
        event.as_ptr()
    });
    // SAFETY: the block returns the event it was given, which is valid.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Swipe, &block)
    };
    // AppKit holds the block until the monitor is removed, which is never.
    std::mem::forget(monitor);
}

#[cfg(not(target_os = "macos"))]
pub fn watch(_swiped: ExtSendTrigger) {}
