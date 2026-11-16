/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Photon Workshop's printer settings export (`.pwsp`), as its exported
//! files show it: JSON. The printer is `printer_param.machine_type`; the
//! resins the person set up are `printer_param.machine_extern.user_resins`
//! (the factory resins Anycubic ships are left out: our catalog carries
//! Anycubic's own tables). Speeds are in mm/s, and a two-stage motion's
//! second height is on top of the first, as the Anycubic files Breadslicer
//! writes hold it.

use serde_json::Value;

use super::{
    ImportError, ImportedPrinter, ImportedResin, empty_settings, mm, per_min, scale_from_percent,
    sec, stage, wait,
};

/// A number of the file. Workshop stores single-precision floats: each is
/// given back as the shortest decimal that is the same `f32`, the value
/// the person typed (2.2, not 2.200000047683716).
fn number(value: &Value, path: &[&str]) -> Option<f64> {
    path.iter()
        .try_fold(value, |v, key| v.get(key))
        .and_then(Value::as_f64)
        .map(|v| (v as f32).to_string().parse().unwrap_or(v))
}

fn text(value: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(value, |v, key| v.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn resin(value: &Value) -> ImportedResin {
    let slice = |key: &str| number(value, &["slicepara", key]);
    let ext = |path: &[&str]| {
        let mut full = vec!["slice_extpara"];
        full.extend_from_slice(path);
        number(value, &full)
    };
    let per_sec = |v: Option<f64>| v.map(|v| v * 60.0);
    let mut settings = empty_settings();
    settings.normal_exposure_sec = sec(slice("exposure_time"));
    settings.bottom_exposure_sec = sec(slice("bott_time"));
    settings.bottom_layer_count = slice("bott_layers")
        .filter(|c| *c >= 0.0 && c.fract() == 0.0)
        .map(|c| c as u32);
    settings.wait_before_cure_sec = wait(slice("off_time"));
    settings.wait_after_cure_sec = wait(slice("wait_before_lift"));
    settings.wait_after_lift_sec = wait(slice("wait_after_lift"));
    settings.bottom_wait_before_cure_sec = wait(slice("bott_off_time"));
    settings.bottom_wait_after_cure_sec = wait(slice("bott_wait_before_lift"));
    settings.bottom_wait_after_lift_sec = wait(slice("bott_wait_after_lift"));
    if ext(&["multi_state_used"]) == Some(1.0) {
        let first = |state: &str, key: &str| ext(&["multi_state_paras", state, key]);
        settings.lift_distance_mm = mm(first("normal_0", "height"));
        settings.lift_speed_mm_min = per_min(per_sec(first("normal_0", "up_speed")));
        settings.retract_speed_mm_min = per_min(per_sec(first("normal_0", "down_speed")));
        settings.second_travel = stage(
            first("normal_1", "height"),
            per_sec(first("normal_1", "up_speed")),
            per_sec(first("normal_1", "down_speed")),
        );
        settings.bottom_lift_distance_mm = mm(first("bott_0", "height"));
        settings.bottom_lift_speed_mm_min = per_min(per_sec(first("bott_0", "up_speed")));
        settings.bottom_retract_speed_mm_min = per_min(per_sec(first("bott_0", "down_speed")));
        settings.bottom_second_travel = stage(
            first("bott_1", "height"),
            per_sec(first("bott_1", "up_speed")),
            per_sec(first("bott_1", "down_speed")),
        );
    } else {
        settings.lift_distance_mm = mm(slice("zup_height"));
        settings.lift_speed_mm_min = per_min(per_sec(slice("zup_speed")));
        settings.retract_speed_mm_min = per_min(per_sec(slice("zdown_speed")));
    }
    let scale = match (
        ext(&["material_scale_xyz", "x"]),
        ext(&["material_scale_xyz", "y"]),
        ext(&["material_scale_xyz", "z"]),
    ) {
        // Factors: 1.01 is 101 %.
        (Some(x), Some(y), Some(z)) => scale_from_percent(x * 100.0, y * 100.0, z * 100.0),
        _ => None,
    };
    let mut not_imported = Vec::new();
    if ext(&["transition_layercount"]).is_some_and(|n| n > 0.0) {
        not_imported.push("transition_layercount".to_owned());
    }
    if ext(&["exposure_compensate"]).is_some_and(|n| n != 0.0) {
        not_imported.push("exposure_compensate".to_owned());
    }
    if slice("use_indivi_layerpara") == Some(1.0) {
        not_imported.push("use_indivi_layerpara".to_owned());
    }
    let property = |key: &str| text(value, &["property", key]);
    // The setting and the film tell apart the resins of one product.
    let tags: Vec<String> = [property("setting_name"), property("film_name")]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    let name = if tags.is_empty() {
        property("resin_name")
    } else {
        format!("{} ({})", property("resin_name"), tags.join(", "))
    };
    ImportedResin {
        brand: property("brand_name"),
        name,
        layer_height_mm: slice("zthick"),
        settings,
        scale,
        not_imported,
    }
}

/// The printer and resins of a `.pwsp` file.
pub fn read_photon_workshop(bytes: &[u8]) -> Result<Vec<ImportedPrinter>, ImportError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| ImportError::Unreadable(format!("not a Photon Workshop export ({e})")))?;
    let param = value
        .get("printer_param")
        .ok_or_else(|| ImportError::Unreadable("not a Photon Workshop export".to_owned()))?;
    let machine = param
        .get("machine_type")
        .ok_or_else(|| ImportError::Unreadable("no printer in the file".to_owned()))?;
    let res = |key: &str| number(machine, &[key]).map(|v| v as u32);
    let (Some(x), Some(y), Some(px), Some(py)) = (
        res("res_x"),
        res("res_y"),
        number(machine, &["print_xsize"]),
        number(machine, &["print_ysize"]),
    ) else {
        return Err(ImportError::Unreadable(
            "the printer has no panel or plate size".to_owned(),
        ));
    };
    let resins: Vec<ImportedResin> = param
        .get("machine_extern")
        .and_then(|e| e.get("user_resins"))
        .and_then(Value::as_array)
        .map(|resins| resins.iter().map(resin).collect())
        .unwrap_or_default();
    if resins.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(vec![ImportedPrinter {
        name: text(machine, &["name"]),
        resolution: (x, y),
        plate_mm: (px, py),
        resins,
    }])
}
