/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The dialog that adds a calibration block to a resin through its extension
//! file, which the Preferences window opens.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use sc_print_profiles::{EffectiveMaterial, user_library_dir};

use crate::material_form::{
    CalibrationFormInput, EditedProfileKind, calibration_form_from_block, parse_calibration_form,
    save_edited_profile,
};
use crate::preferences_bus::PreferencesBus;
use crate::profile_catalog::{ProfileCatalog, load_profile_for_view, refresh_catalog};
use crate::profile_extending::{build_extension_update, copy_sources, uncovered_targets};
use crate::profile_view::profile_display_name;
use crate::slint_ui::ExtendDialog;

/// Opens the extend dialog for the resin at `index`: pick one of the targets
/// the resin does not cover yet, enter the calibration values (partial
/// allowed), and write the block into the resin's extension file (created on
/// the first extend, appended to afterwards).
pub(crate) fn open_extend_dialog(
    bus: &Rc<PreferencesBus>,
    catalog_handle: &Rc<RefCell<Rc<ProfileCatalog>>>,
    refresh_view: &Rc<dyn Fn()>,
    extend_slot: &Rc<RefCell<Option<ExtendDialog>>>,
    index: i32,
) {
    let catalog = catalog_handle.borrow().clone();
    let Some((base, extension, _)) = load_profile_for_view(&catalog, index) else {
        return;
    };

    let snapshot = bus.snapshot();
    let default_printer = catalog.effective_default_printer(snapshot.default_printer.as_ref());
    // The dialog writes to the raw files, so `base` and `extension` stay
    // around; the composed view feeds the target complement and the blocks a
    // new one can be copied from.
    let (material, issues) = EffectiveMaterial::compose(base.clone(), extension.clone());
    for issue in &issues {
        eprintln!("warning: {issue}");
    }
    let targets = uncovered_targets(&material, catalog.printers(), default_printer.as_ref());
    if targets.is_empty() {
        return;
    }
    let sources = copy_sources(&material, catalog.printers());

    let dialog = match ExtendDialog::new() {
        Ok(dialog) => dialog,
        Err(err) => {
            eprintln!("failed to create the extend dialog: {err}");
            return;
        }
    };
    dialog.set_theme(snapshot.theme);
    dialog.set_source_name(profile_display_name(&base.identity).into());
    dialog.set_target_labels(ModelRc::new(VecModel::from(
        targets
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_target_index(-1);
    dialog.set_copy_source_labels(ModelRc::new(VecModel::from(
        sources
            .iter()
            .map(|source| source.label.clone())
            .collect::<Vec<_>>(),
    )));
    dialog.set_copy_source_index(-1);

    let base = Rc::new(base);
    let extension = Rc::new(extension);
    let targets = Rc::new(targets);
    let sources = Rc::new(sources);
    let dialog_weak = dialog.as_weak();

    dialog.on_copy_requested({
        let dialog_weak = dialog_weak.clone();
        let sources = sources.clone();
        move |picked| {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some(source) = sources.get(picked as usize) else {
                return;
            };
            let seeded = calibration_form_from_block(&source.values);
            d.set_normal_exposure_text(seeded.normal_exposure_sec.into());
            d.set_bottom_exposure_text(seeded.bottom_exposure_sec.into());
            d.set_bottom_count_text(seeded.bottom_layer_count.into());
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
        let extend_slot = extend_slot.clone();
        let catalog_handle = catalog_handle.clone();
        let refresh_view = refresh_view.clone();
        let base = base.clone();
        let extension = extension.clone();
        let targets = targets.clone();
        move || {
            let Some(d) = dialog_weak.upgrade() else {
                return;
            };
            let Some((target, _)) = targets.get(d.get_target_index() as usize) else {
                return;
            };
            let input = CalibrationFormInput {
                normal_exposure_sec: d.get_normal_exposure_text().to_string(),
                bottom_exposure_sec: d.get_bottom_exposure_text().to_string(),
                bottom_layer_count: d.get_bottom_count_text().to_string(),
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
                bottom_second_lift_speed_mm_min: d.get_bottom_second_lift_speed_text().to_string(),
                bottom_second_retract_speed_mm_min: d
                    .get_bottom_second_retract_speed_text()
                    .to_string(),
                light_pwm: d.get_light_pwm_text().to_string(),
                bottom_light_pwm: d.get_bottom_light_pwm_text().to_string(),
            };
            let block = match parse_calibration_form(&input) {
                Ok(block) => block,
                Err(err) => {
                    d.set_error_message(err.user_message());
                    return;
                }
            };
            let update = build_extension_update(&base, (*extension).as_ref(), target, block);
            let user_lib = match user_library_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    d.set_error_message(err.to_string().into());
                    return;
                }
            };
            if let Err(message) =
                save_edited_profile(&user_lib, &update, EditedProfileKind::Extension)
            {
                d.set_error_message(message.into());
                return;
            }
            refresh_catalog(&catalog_handle);
            refresh_view();
            let _ = d.hide();
            *extend_slot.borrow_mut() = None;
        }
    });

    dialog.on_cancel_requested({
        let extend_slot = extend_slot.clone();
        move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            *extend_slot.borrow_mut() = None;
        }
    });

    if let Err(err) = dialog.show() {
        eprintln!("failed to show the extend dialog: {err}");
        return;
    }
    *extend_slot.borrow_mut() = Some(dialog);
}
