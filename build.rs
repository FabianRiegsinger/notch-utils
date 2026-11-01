fn main() {
    // Cupertino style for the std-widgets in the settings window (the notch uses custom widgets).
    let config = slint_build::CompilerConfiguration::new().with_style("cupertino".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");

    // macOS: the Now Playing bridge (see native/nowplaying.m), embedded into the binary.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=native/nowplaying.m");
        let out =
            std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("libnotchnp.dylib");
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "arm64",
            _ => "x86_64",
        };
        let status = std::process::Command::new("xcrun")
            .args([
                "clang",
                "-dynamiclib",
                "-fobjc-arc",
                "-O2",
                "-arch",
                arch,
                "-mmacosx-version-min=11.0",
            ])
            .args(["-framework", "Foundation", "-o"])
            .arg(&out)
            .arg("native/nowplaying.m")
            .status()
            .expect("running xcrun clang");
        assert!(status.success(), "building native/nowplaying.m failed");
    }
}
