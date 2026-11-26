/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Tools → Find dead pixels (#783): the card that walks through printing
//! the screen sheet, laying it on the lit screen and marking the dark cells
//! on the plate with the dead zones' tool (#726).

use std::rc::Rc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::dead_pixel_pictures::{Step, picture};
use crate::i18n::ctr;
use crate::plate_sheet::screen::{ScreenSheet, ScreenTexts};
use crate::preferences_bus::PreferencesBus;
use crate::viewport::ViewportController;
use crate::{DeadPixels, MainWindow};

/// The sheet's words, in the app's language.
fn texts() -> ScreenTexts {
    let (page, pages) = ("{page}", "{pages}");
    ScreenTexts {
        title: ctr!("ScreenSheet", "Dead pixel sheet").to_string(),
        front: ctr!("PlateSheet", "FRONT").to_string(),
        actual_size: ctr!(
            "ScreenSheet",
            "Print at actual size (100 %): this ruler measures 50 mm."
        )
        .to_string(),
        cut: ctr!(
            "ScreenSheet",
            "Cut along the dashed line. Lay the sheet on the screen with the vat out, its thick edge on the lit edge of the screen, and run the printer's cleaning or exposure test: the dead pixels show dark."
        )
        .to_string(),
        page: ctr!("PlateSheet", "Page {} of {}", page, pages).to_string(),
        join: ctr!(
            "ScreenSheet",
            "Cut the edges with no band exactly on their line and butt them against the next page."
        )
        .to_string(),
    }
}

/// The printer chosen in the slice panel, by its name.
fn printer_name(ui: &MainWindow) -> String {
    let options: ModelRc<SharedString> = ui.get_slice_printer_options();
    usize::try_from(ui.get_slice_printer_current_index())
        .ok()
        .and_then(|i| options.row_data(i))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn trim(mm: f32) -> String {
    let s = format!("{mm:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Fills the card for the printer in use and opens it.
fn open(ui: &MainWindow, ctrl: &ViewportController, prefs: &PreferencesBus) {
    let volume = ctrl.build_volume();
    let (w, d) = (volume.width_mm(), volume.depth_mm());
    let aspect = if d > 0.0 { w / d } else { 1.6 };
    let pictures: Vec<Image> = Step::ALL
        .iter()
        .map(|step| {
            Image::load_from_svg_data(picture(*step, aspect).as_bytes()).unwrap_or_default()
        })
        .collect();
    let has_printer = prefs.snapshot().default_printer.is_some();
    let name = printer_name(ui);
    // The card over the plate: the cells' tool, if out, goes away.
    ctrl.set_dead_zone_tool(false);
    let global = ui.global::<DeadPixels>();
    global.set_printer(if has_printer && !name.is_empty() {
        format!("{name} · {} × {} mm", trim(w), trim(d)).into()
    } else {
        SharedString::default()
    });
    global.set_has_printer(has_printer);
    global.set_pictures(ModelRc::new(VecModel::from(pictures)));
    global.set_zones(crate::dead_zones::current().len() as i32);
    global.set_sheet_note(SharedString::default());
    global.set_page(0);
    global.set_open(true);
    crate::breadcrumbs::note("dead pixels: opened");
}

/// Wires Tools → Find dead pixels and its card.
pub(crate) fn wire(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
    {
        let weak = ui.as_weak();
        let ctrl = ctrl.clone();
        let prefs = prefs.clone();
        ui.on_dead_pixels_requested(move || {
            if let Some(ui) = weak.upgrade() {
                open(&ui, &ctrl, &prefs);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let ctrl = ctrl.clone();
        ui.global::<DeadPixels>().on_mark_cells(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            ui.global::<DeadPixels>().set_open(false);
            ctrl.set_dead_zone_tool(true);
        });
    }
    let weak = ui.as_weak();
    ui.global::<DeadPixels>().on_save_sheet(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let volume = ctrl.build_volume();
        let sheet = ScreenSheet {
            printer: printer_name(&ui),
            version: crate::VERSION.to_owned(),
            width_mm: volume.width_mm(),
            depth_mm: volume.depth_mm(),
            zones: crate::dead_zones::current(),
            texts: texts(),
        };
        let dialog = crate::error_dialog::file_dialog_over(ui.window());
        let weak = ui.as_weak();
        let task = async move {
            let stem = if sheet.printer.is_empty() {
                "Breadslicer".to_owned()
            } else {
                sheet.printer.clone()
            };
            let name = format!("{stem} - {}.pdf", ctr!("ScreenSheet", "dead pixels"));
            let Some(path) = dialog
                .add_filter(ctr!("FileDialog", "PDF documents").to_string(), &["pdf"])
                .set_file_name(name)
                .save_file()
                .await
                .map(|h| h.path().to_path_buf())
            else {
                return;
            };
            let (tx, rx) = futures_channel::oneshot::channel();
            let target = path.clone();
            std::thread::Builder::new()
                .name("screen-sheet".into())
                .spawn(move || {
                    let written = crate::plate_sheet::screen::write(&sheet).and_then(|bytes| {
                        crate::plate_sheet::write_atomically(&target, &bytes)
                            .map_err(|e| e.to_string())
                    });
                    let _ = tx.send(written);
                })
                .ok();
            let note = match rx.await {
                Ok(Ok(())) => {
                    crate::plate_sheet::open_file(&path);
                    let file = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ctr!("DeadPixels", "Saved: {}. It opens to print.", file)
                }
                Ok(Err(err)) => ctr!("DeadPixels", "The sheet was not saved: {}", err),
                Err(_) => SharedString::default(),
            };
            if let Some(ui) = weak.upgrade() {
                ui.global::<DeadPixels>().set_sheet_note(note);
            }
        };
        if let Err(err) = slint::spawn_local(task) {
            eprintln!("Failed to spawn the screen sheet: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_s_words_are_whole() {
        let t = texts();
        assert!(t.page.contains("{page}") && t.page.contains("{pages}"));
        assert!(!t.title.is_empty() && !t.cut.is_empty());
    }
}
