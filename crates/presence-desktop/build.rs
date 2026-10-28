fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("Could not compile the native Slint studio");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/icon.ico")
            .set("ProductName", "Presence Control")
            .set("FileDescription", "Presence Control — Native Rust Studio")
            .compile()
            .expect("Could not embed Windows application icon");
    }
}
