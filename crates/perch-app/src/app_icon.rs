//! Set the Dock icon even when launched as a bare binary with `cargo run`.

#[cfg(target_os = "macos")]
pub fn install() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let mtm = MainThreadMarker::new().expect("app icon must be installed on the main thread");
    let data = NSData::with_bytes(include_bytes!("../../../assets/Perch.icns"));
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .expect("bundled Perch icon must be a valid image");
    // SAFETY: called on the main thread with a valid, non-null NSImage.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}
