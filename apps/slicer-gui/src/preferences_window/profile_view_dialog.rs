/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The read-only resin profile viewer, which the Preferences window opens.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use sc_print_profiles::EffectiveMaterial;

use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, load_profile_for_view};
use crate::profile_view::{build_profile_view_model, profile_display_name};
use crate::slint_ui::ProfileViewDialog;

/// Opens the read-only profile viewer for the resin at `index`: the base
/// profile's sections plus its extension's calibration blocks, with the block
/// the default printer would use highlighted.
pub(super) fn open_profile_view_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((profile, extension, source)) = load_profile_for_view(&catalog, index) else {
        return;
    };
    let (material, issues) = EffectiveMaterial::compose(profile, extension);
    for issue in &issues {
        eprintln!("warning: {issue}");
    }

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    let sections = build_profile_view_model(
        &material,
        catalog.printers(),
        default_printer.as_ref(),
        source,
    );

    let dialog = match ProfileViewDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the profile viewer: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    dialog.set_profile_name(profile_display_name(material.identity()).into());
    dialog.set_sections(ModelRc::new(VecModel::from(sections)));

    let dialog_weak = dialog.as_weak();
    dialog.on_close_requested({
        let dialog_slot = dialog_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *dialog_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the profile viewer: {err}");
        return;
    }
    *dialog_slot.borrow_mut() = Some(dialog);
}
