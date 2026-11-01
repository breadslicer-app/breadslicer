/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::env;

use slint_build::{CompilerConfiguration, compile_with_config};

/// The interface and its translations live with the app crate.
const MAIN_WINDOW: &str = "../../ui/main-window.slint";
const I18N_DIR: &str = "../../i18n";

/// Slint bundles the files `i18n/<lang>/LC_MESSAGES/<domain>.po` and takes the
/// domain from `CARGO_PKG_NAME`. The catalogs are named after the app crate,
/// whose translations they are.
const DOMAIN: &str = "sc-slicer-gui";

fn main() {
    // Slint reads the domain from the environment. Set before any thread
    // starts.
    set_translation_domain();
    // The Slint resolver recurses through the component graph; on Windows the
    // default 1 MiB thread stack overflows, so run the build on a worker thread
    // with a generous stack. Cheap on every platform.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(run_build)
        .expect("failed to spawn build thread")
        .join()
        .expect("build thread panicked");
}

#[allow(unsafe_code)]
fn set_translation_domain() {
    // SAFETY: the first thing `main` does, before any thread exists, so no
    // other thread can read or write the environment at the same time.
    unsafe { env::set_var("CARGO_PKG_NAME", DOMAIN) };
}

fn run_build() {
    // Development builds (tests among them) carry the element ids and type
    // names Slint's element queries need, so layout tests can measure what is
    // drawn. Release builds do not.
    let debug_info = env::var("PROFILE").as_deref() == Ok("debug");
    let config = CompilerConfiguration::new()
        .with_bundled_translations(I18N_DIR)
        .with_debug_info(debug_info);
    compile_with_config(MAIN_WINDOW, config).expect("Slint build failed");
}
