fn main() {
    println!("cargo:rerun-if-changed=assets/atelier.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/atelier.ico")
            .compile()
            .expect("Failed to embed Windows executable icon");
    }
}
