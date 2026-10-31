fn main() {
    // Cupertino style for the std-widgets in the settings window (the notch uses custom widgets).
    let config = slint_build::CompilerConfiguration::new().with_style("cupertino".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");
}
