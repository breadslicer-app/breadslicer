/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The in-place editor of a custom resin profile, which the Preferences window
//! and the fork dialog open.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use sc_print_profiles::{
    load_material_extension_from_library, load_material_profile_from_library, user_library_dir,
};

use crate::i18n::ctr;
use crate::material_form::{
    CalibrationFormInput, DefaultsFormInput, EditedProfileKind, calibration_form_from_block,
    defaults_form_from, parse_calibration_form, parse_defaults_form, save_edited_profile,
};
use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, ProfileSource, refresh_catalog};
use crate::profile_view::{
    identity_section, intent_section, ordered_calibration_targets, profile_display_name,
};
use crate::slint_ui::ProfileViewDialog;

/// Opens the in-place editor for a custom row. The extension row edits the
/// extension file; a custom profile row edits the full profile, falling back to
/// the extension file for an orphaned extension entry. The defaults are
/// editable for full profiles only (an extension inherits them from its
/// parent), and 1 calibration pair per session can be picked from the profile's
/// own blocks and edited.
pub(crate) fn open_profile_editor(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    dialog_slot: &Rc<RefCell<Option<ProfileViewDialog>>>,
    index: i32,
    is_extension: bool,
    note: Option<SharedString>,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some(entry) = catalog.materials().get(index as usize) else {
        return;
    };
    let user_lib = match user_library_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("cannot resolve the user profile library directory: {err}");
            return;
        }
    };

    let (profile, kind) = if is_extension {
        match load_material_extension_from_library(&user_lib, &entry.id) {
            Ok(profile) => (profile, EditedProfileKind::Extension),
            Err(err) => {
                eprintln!("{err}");
                return;
            }
        }
    } else {
        match load_material_profile_from_library(&user_lib, &entry.id) {
            Ok(profile) => (profile, EditedProfileKind::Full),
            Err(full_err) => match load_material_extension_from_library(&user_lib, &entry.id) {
                Ok(orphan_extension) => (orphan_extension, EditedProfileKind::Extension),
                Err(_) => {
                    eprintln!("{full_err}");
                    return;
                }
            },
        }
    };

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());

    // Read-only context above the edit cards: identity, plus intent for full
    // profiles (an extension's intent is inherited).
    let mut sections = vec![identity_section(&profile.identity, ProfileSource::User)];
    if kind == EditedProfileKind::Full {
        sections.push(intent_section(&profile.intent));
    }

    let targets =
        ordered_calibration_targets(&profile, catalog.printers(), default_printer.as_ref());

    let dialog = match ProfileViewDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the profile editor: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    let display_name = match kind {
        EditedProfileKind::Full => SharedString::from(profile_display_name(&profile.identity)),
        EditedProfileKind::Extension => ctr!(
            "PreferencesPage",
            "{} (extended)",
            profile_display_name(&profile.identity)
        ),
    };
    dialog.set_profile_name(display_name);
    dialog.set_note(note.unwrap_or_default());
    dialog.set_sections(ModelRc::new(VecModel::from(sections)));
    dialog.set_editable(true);
    dialog.set_defaults_editable(kind == EditedProfileKind::Full);
    if kind == EditedProfileKind::Full {
        let seeded = defaults_form_from(&profile.defaults);
        dialog.set_scale_x_text(seeded.scale_x_pct.into());
        dialog.set_scale_y_text(seeded.scale_y_pct.into());
        dialog.set_scale_z_text(seeded.scale_z_pct.into());
        dialog.set_defaults_bottom_count_text(seeded.bottom_layer_count.into());
        dialog.set_defaults_wait_before_cure_text(seeded.wait_before_cure_sec.into());
        dialog.set_defaults_wait_after_cure_text(seeded.wait_after_cure_sec.into());
        dialog.set_defaults_wait_after_lift_text(seeded.wait_after_lift_sec.into());
        dialog.set_defaults_light_pwm_text(seeded.light_pwm.into());
        dialog.set_defaults_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
    }
    dialog.set_edit_target_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_edit_target_index(-1);

    let profile = Rc::new(RefCell::new(profile));
    let targets = Rc::new(targets);
    let dialog_weak = dialog.as_weak();

    dialog.on_edit_target_changed({
        let dialog_weak = dialog_weak.clone();
        let profile = profile.clone();
        let targets = targets.clone();
        move |picked| {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some((id, _)) = targets.get(picked as usize) else {
                return;
            };
            let profile = profile.borrow();
            let Some(block) = profile.per_printer.get(id) else {
                return;
            };
            let seeded = calibration_form_from_block(block);
            d.set_normal_exposure_text(seeded.normal_exposure_sec.into());
            d.set_bottom_exposure_text(seeded.bottom_exposure_sec.into());
            d.set_block_bottom_count_text(seeded.bottom_layer_count.into());
            d.set_wait_before_cure_text(seeded.wait_before_cure_sec.into());
            d.set_wait_after_cure_text(seeded.wait_after_cure_sec.into());
            d.set_wait_after_lift_text(seeded.wait_after_lift_sec.into());
            d.set_bottom_wait_before_cure_text(seeded.bottom_wait_before_cure_sec.into());
            d.set_bottom_wait_after_cure_text(seeded.bottom_wait_after_cure_sec.into());
            d.set_bottom_wait_after_lift_text(seeded.bottom_wait_after_lift_sec.into());
            d.set_lift_distance_text(seeded.lift_distance_mm.into());
            d.set_lift_speed_text(seeded.lift_speed_mm_min.into());
            d.set_retract_speed_text(seeded.retract_speed_mm_min.into());
            d.set_second_lift_distance_text(seeded.second_lift_distance_mm.into());
            d.set_second_lift_speed_text(seeded.second_lift_speed_mm_min.into());
            d.set_second_retract_speed_text(seeded.second_retract_speed_mm_min.into());
            d.set_bottom_lift_distance_text(seeded.bottom_lift_distance_mm.into());
            d.set_bottom_lift_speed_text(seeded.bottom_lift_speed_mm_min.into());
            d.set_bottom_retract_speed_text(seeded.bottom_retract_speed_mm_min.into());
            d.set_bottom_second_lift_distance_text(seeded.bottom_second_lift_distance_mm.into());
            d.set_bottom_second_lift_speed_text(seeded.bottom_second_lift_speed_mm_min.into());
            d.set_bottom_second_retract_speed_text(
                seeded.bottom_second_retract_speed_mm_min.into(),
            );
            d.set_light_pwm_text(seeded.light_pwm.into());
            d.set_bottom_light_pwm_text(seeded.bottom_light_pwm.into());
        }
    });

    dialog.on_save_requested({
        let dialog_weak = dialog_weak.clone();
        let dialog_slot = dialog_slot.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let profile = profile.clone();
        let targets = targets.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let mut profile = profile.borrow_mut();
            if kind == EditedProfileKind::Full {
                let input = DefaultsFormInput {
                    scale_x_pct: d.get_scale_x_text().to_string(),
                    scale_y_pct: d.get_scale_y_text().to_string(),
                    scale_z_pct: d.get_scale_z_text().to_string(),
                    bottom_layer_count: d.get_defaults_bottom_count_text().to_string(),
                    wait_before_cure_sec: d.get_defaults_wait_before_cure_text().to_string(),
                    wait_after_cure_sec: d.get_defaults_wait_after_cure_text().to_string(),
                    wait_after_lift_sec: d.get_defaults_wait_after_lift_text().to_string(),
                    light_pwm: d.get_defaults_light_pwm_text().to_string(),
                    bottom_light_pwm: d.get_defaults_bottom_light_pwm_text().to_string(),
                };
                match parse_defaults_form(&input) {
                    Ok(defaults) => profile.defaults = defaults,
                    Err(err) => {
                        d.set_error_message(err.user_message());
                        return;
                    }
                }
            }
            let picked = d.get_edit_target_index();
            if picked >= 0 {
                let Some((id, _)) = targets.get(picked as usize) else {
                    return;
                };
                let input = CalibrationFormInput {
                    normal_exposure_sec: d.get_normal_exposure_text().to_string(),
                    bottom_exposure_sec: d.get_bottom_exposure_text().to_string(),
                    bottom_layer_count: d.get_block_bottom_count_text().to_string(),
                    wait_before_cure_sec: d.get_wait_before_cure_text().to_string(),
                    wait_after_cure_sec: d.get_wait_after_cure_text().to_string(),
                    wait_after_lift_sec: d.get_wait_after_lift_text().to_string(),
                    bottom_wait_before_cure_sec: d.get_bottom_wait_before_cure_text().to_string(),
                    bottom_wait_after_cure_sec: d.get_bottom_wait_after_cure_text().to_string(),
                    bottom_wait_after_lift_sec: d.get_bottom_wait_after_lift_text().to_string(),
                    lift_distance_mm: d.get_lift_distance_text().to_string(),
                    lift_speed_mm_min: d.get_lift_speed_text().to_string(),
                    retract_speed_mm_min: d.get_retract_speed_text().to_string(),
                    second_lift_distance_mm: d.get_second_lift_distance_text().to_string(),
                    second_lift_speed_mm_min: d.get_second_lift_speed_text().to_string(),
                    second_retract_speed_mm_min: d.get_second_retract_speed_text().to_string(),
                    bottom_lift_distance_mm: d.get_bottom_lift_distance_text().to_string(),
                    bottom_lift_speed_mm_min: d.get_bottom_lift_speed_text().to_string(),
                    bottom_retract_speed_mm_min: d.get_bottom_retract_speed_text().to_string(),
                    bottom_second_lift_distance_mm: d
                        .get_bottom_second_lift_distance_text()
                        .to_string(),
                    bottom_second_lift_speed_mm_min: d
                        .get_bottom_second_lift_speed_text()
                        .to_string(),
                    bottom_second_retract_speed_mm_min: d
                        .get_bottom_second_retract_speed_text()
                        .to_string(),
                    light_pwm: d.get_light_pwm_text().to_string(),
                    bottom_light_pwm: d.get_bottom_light_pwm_text().to_string(),
                };
                match parse_calibration_form(&input) {
                    Ok(block) => {
                        profile.per_printer.insert(id.clone(), block);
                    }
                    Err(err) => {
                        d.set_error_message(err.user_message());
                        return;
                    }
                }
            }
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    return;
                }
            };
            if let Err(message) = save_edited_profile(&user_lib, &profile, kind) {
                d.set_error_message(message.into());
                return;
            }
            drop(profile);
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *dialog_slot.borrow_mut() = None;
        }
    });

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
        eprintln!("failed to show the profile editor: {err}");
        return;
    }
    *dialog_slot.borrow_mut() = Some(dialog);
}
