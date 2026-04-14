/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;

// Includes the code generated with the slint-build crate from the build script.
// After calling slint_build::compile in our build.rs build script, the use of
// this macro includes the generated Rust code and makes the exported types
// available for us to instantiate.
slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    let ui = MainWindow::new()?;

    ui.run()?;

    Ok(())
}
