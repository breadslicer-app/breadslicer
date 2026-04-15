/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use slint::ComponentHandle;

// Includes the code generated with the slint-build crate from the build script.
// After calling slint_build::compile in our build.rs build script, the use of
// this macro includes the generated Rust code and makes the exported types
// available for us to instantiate.
slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    let ui = MainWindow::new()?;

    ui.on_open_file_requested(|| {
        let dialog = rfd::AsyncFileDialog::new()
            .add_filter("All files", &["obj", "stl", "stla", "stlb"])
            .add_filter("OBJ files", &["obj"])
            .add_filter("STL files", &["stl", "stla", "stlb"]);

        if let Err(err) = slint::spawn_local(async move {
            if let Some(handle) = dialog.pick_file().await {
                println!("Selected file: {}", handle.path().display());
            }
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
        }
    });

    let about_window: Rc<RefCell<Option<AboutWindow>>> = Rc::new(RefCell::new(None));
    ui.on_show_about_requested(move || {
        let mut slot = about_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.show();
            return;
        }

        let about = match AboutWindow::new() {
            Ok(w) => w,
            Err(err) => {
                eprintln!("Failed to create About window: {err}");
                return;
            }
        };
        about.set_app_version(env!("CARGO_PKG_VERSION").into());

        let about_weak = about.as_weak();
        about.on_close_requested(move || {
            if let Some(w) = about_weak.upgrade() {
                let _ = w.hide();
            }
        });

        if let Err(err) = about.show() {
            eprintln!("Failed to show About window: {err}");
            return;
        }
        *slot = Some(about);
    });

    ui.run()?;

    Ok(())
}
