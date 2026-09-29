//! Set the Dock icon and the menu bar's app name even when launched as a
//! bare binary with `cargo run`.

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
    set_display_name();
}

/// LaunchServices names a bare binary after its file, `perch`, and the menu
/// bar's bold title follows it; the Info.plist that build.rs embeds names
/// only the menu's items. The call is private, Chromium's way of naming its
/// processes, so it is looked up at run time and skipped if it is gone.
#[cfg(target_os = "macos")]
fn set_display_name() {
    use objc2_foundation::ns_string;
    use std::ffi::{c_char, c_int, c_void};

    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    type GetAsn = unsafe extern "C" fn() -> *const c_void;
    type SetItem = unsafe extern "C" fn(
        c_int,
        *const c_void,
        *const c_void,
        *const c_void,
        *mut c_void,
    ) -> i32;
    const RTLD_LAZY: c_int = 1;
    // kLSDefaultSessionID.
    const DEFAULT_SESSION: c_int = -2;

    // SAFETY: the symbols are LaunchServices' own, with Chromium's
    // signatures, and each is checked for null before use.
    unsafe {
        let ls = dlopen(
            c"/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/LaunchServices".as_ptr(),
            RTLD_LAZY,
        );
        if ls.is_null() {
            return;
        }
        let get_asn = dlsym(ls, c"_LSGetCurrentApplicationASN".as_ptr());
        let set_item = dlsym(ls, c"_LSSetApplicationInformationItem".as_ptr());
        let key = dlsym(ls, c"_kLSDisplayNameKey".as_ptr()) as *const *const c_void;
        if get_asn.is_null() || set_item.is_null() || key.is_null() {
            return;
        }
        let get_asn: GetAsn = std::mem::transmute(get_asn);
        let set_item: SetItem = std::mem::transmute(set_item);
        let asn = get_asn();
        if asn.is_null() {
            return;
        }
        let name: *const _ = ns_string!("Perch");
        set_item(
            DEFAULT_SESSION,
            asn,
            *key,
            name.cast(),
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}
