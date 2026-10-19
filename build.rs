fn main() {
    println!("cargo:rerun-if-changed=src/moli/listener_macos.c");
    // Test the compilation target, not the machine running this build script.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/moli/listener_macos.c")
            .warnings_into_errors(true)
            .compile("lexmount_moli_listener");
    }
}
