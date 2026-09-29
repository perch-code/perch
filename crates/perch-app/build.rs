//! Embed the app's Info.plist in the binary, so a bare `cargo run` is named
//! Perch in the menu bar and the Dock rather than after its file, `perch`.

fn main() {
    let plist = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/Info.plist");
    println!("cargo:rerun-if-changed={plist}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
}
