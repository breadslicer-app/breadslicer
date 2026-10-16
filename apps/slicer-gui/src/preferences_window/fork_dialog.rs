/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The dialog that forks a resin profile into the user library, which the
//! Preferences window opens.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use sc_print_primitives::MaterialId;
use sc_print_profiles::user_library_dir;

use crate::i18n::ctr;
use crate::material_form::{EditedProfileKind, save_edited_profile};
use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, load_profile_for_view, refresh_catalog};
use crate::profile_forking::{ForkParts, build_fork};
use crate::profile_view::{ordered_calibration_targets, profile_display_name};
use crate::slint_ui::{ForkDialog, ProfileViewDialog};

use super::profile_editor::open_profile_editor;

/// Opens the fork dialog for the resin at `index`. The source is the resolved
/// profile behind the row (an extension row forks the same resolved base); the
/// offered parts are strictly the source's own blocks. On confirm the fork is
/// written to the user library and the in-place editor opens on it, with a note
/// that the profile is already saved so cancelling there keeps the untouched
/// fork.
pub(super) fn open_fork_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    fork_slot: &Rc<RefCell<Option<ForkDialog>>>,
    editor_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((source, _, _)) = load_profile_for_view(&catalog, index) else {
        return;
    };

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    let targets =
        ordered_calibration_targets(&source, catalog.printers(), default_printer.as_ref());

    let dialog = match ForkDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the fork dialog: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    dialog.set_source_name(profile_display_name(&source.identity).into());
    dialog.set_fork_name(ctr!(
        "ForkDialog",
        "{} (fork)",
        source.identity.name.as_str()
    ));
    dialog.set_fork_brand(source.identity.brand.as_str().into());
    dialog.set_block_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));

    // The checkboxes' state, mirrored from the dialog; every part starts
    // selected, matching the checkboxes' initial state.
    let blocks_checked = Rc::new(RefCell::new(vec![true; targets.len()]));
    dialog.on_block_toggled({
        let blocks_checked = blocks_checked.clone();
        move |index, checked| {
            if let Some(slot) = blocks_checked.borrow_mut().get_mut(index as usize) {
                *slot = checked;
            }
        }
    });

    let source = Rc::new(source);
    let targets = Rc::new(targets);
    let dialog_weak = dialog.as_weak();

    dialog.on_confirm_requested({
        let dialog_weak = dialog_weak.clone();
        let fork_slot = fork_slot.clone();
        let editor_slot = editor_slot.clone();
        let bus = bus.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let source = source.clone();
        let targets = targets.clone();
        let blocks_checked = blocks_checked.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let catalog = catalog_handle.borrow().clone();
            let existing: BTreeSet<MaterialId> = catalog
                .materials()
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            let parts = ForkParts {
                intent: d.get_copy_intent(),
                defaults: d.get_copy_defaults(),
                blocks: targets
                    .iter()
                    .zip(blocks_checked.borrow().iter())
                    .filter(|(_, checked)| **checked)
                    .map(|((id, _), _)| id.clone())
                    .collect(),
            };
            let fork = match build_fork(
                &source,
                &parts,
                &d.get_fork_name(),
                &d.get_fork_brand(),
                &existing,
            ) {
                Ok(fork) => fork,
                Err(err) => {
                    d.set_error_message(err.user_message());
                    return;
                }
            };
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    return;
                }
            };
            if let Err(message) = save_edited_profile(&user_lib, &fork, EditedProfileKind::Full) {
                d.set_error_message(message.into());
                return;
            }
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *fork_slot.borrow_mut() = None;

            let catalog = catalog_handle.borrow().clone();
            let Some(fork_index) = catalog.material_index(&fork.identity.id) else {
                return;
            };
            open_profile_editor(
                &bus,
                &catalog_handle,
                &refresh_view,
                &editor_slot,
                fork_index as i32,
                false,
                Some(ctr!(
                    "ProfileViewDialog",
                    "The forked profile is already in your library. Close keeps it unchanged.",
                )),
            );
        }
    });

    dialog.on_cancel_requested({
        let fork_slot = fork_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *fork_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the fork dialog: {err}");
        return;
    }
    *fork_slot.borrow_mut() = Some(dialog);
}
