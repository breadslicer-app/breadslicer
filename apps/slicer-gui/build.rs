use slint_build::{CompilerConfiguration, compile_with_config};

fn main() {
    let config = CompilerConfiguration::new().with_bundled_translations("i18n");
    compile_with_config("ui/main-window.slint", config).expect("Slint build failed");
}
