//! Whether a scroll's bars fade out while it is still, as macOS has it.
//!
//! System Settings > Appearance > Show scroll bars decides: "When
//! scrolling" overlays them, "Always" keeps them, and "Automatically", the
//! default, overlays them with a trackpad and keeps them while a mouse is
//! plugged in. AppKit sums that up as the preferred scroller style, and
//! says when it changes.

use floem::ext_event::ExtSendTrigger;

/// True while macOS overlays scroll bars, showing them only as a view
/// scrolls. Off the main thread, where AppKit cannot be asked, the answer
/// is macOS's own default on a Mac with a trackpad.
#[cfg(target_os = "macos")]
pub fn overlaid() -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScroller, NSScrollerStyle};

    MainThreadMarker::new()
        .is_none_or(|mtm| NSScroller::preferredScrollerStyle(mtm) == NSScrollerStyle::Overlay)
}

#[cfg(not(target_os = "macos"))]
pub fn overlaid() -> bool {
    false
}

/// Notifies `changed` each time the preferred style changes, for as long
/// as the app runs: with "Automatically", on plugging in a mouse or
/// unplugging the last one.
#[cfg(target_os = "macos")]
pub fn watch(changed: ExtSendTrigger) {
    use block2::RcBlock;
    use objc2_app_kit::NSPreferredScrollerStyleDidChangeNotification;
    use objc2_foundation::NSNotificationCenter;

    // Only the app's window asks; a test's has no AppKit to hear from.
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let block = RcBlock::new(move |_| floem::ext_event::register_ext_trigger(changed));
    // SAFETY: no object filter or queue, and the block only hands a
    // `Send` trigger to floem, from whichever thread AppKit posts on.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSPreferredScrollerStyleDidChangeNotification),
            None,
            None,
            &block,
        )
    };
    // The center holds the block until the observer is removed, which is
    // never.
    std::mem::forget(observer);
}

#[cfg(not(target_os = "macos"))]
pub fn watch(_changed: ExtSendTrigger) {}
