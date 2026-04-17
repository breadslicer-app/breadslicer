/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{AboutWindow, MainWindow};

pub(crate) fn wire_about_window(ui: &MainWindow) {
    let about_window: Rc<RefCell<Option<AboutWindow>>> = Rc::new(RefCell::new(None));
    ui.on_show_about_requested(move || {
        let mut slot = about_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.show();
            return;
        }
        match build_about_window() {
            Ok(about) => {
                if let Err(err) = about.show() {
                    eprintln!("Failed to show About window: {err}");
                    return;
                }
                *slot = Some(about);
            }
            Err(err) => eprintln!("Failed to create About window: {err}"),
        }
    });
}

fn build_about_window() -> Result<AboutWindow, slint::PlatformError> {
    let about = AboutWindow::new()?;
    about.set_app_version(env!("CARGO_PKG_VERSION").into());

    let about_weak = about.as_weak();
    about.on_close_requested(move || {
        if let Some(w) = about_weak.upgrade() {
            let _ = w.hide();
        }
    });

    Ok(about)
}
