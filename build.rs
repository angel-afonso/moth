fn main() {
    println!("cargo:rerun-if-changed=moth.rc");
    println!("cargo:rerun-if-changed=assets/branding/moth.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // gpui takes the window and taskbar icon from resource ID 1; Explorer shows it too.
        embed_resource::compile("moth.rc", embed_resource::NONE)
            .manifest_required()
            .expect("embed the Moth icon");
    }
}
