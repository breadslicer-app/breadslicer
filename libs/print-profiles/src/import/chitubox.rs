/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! CHITUBOX's printer settings export (`.cfgx`), as its exported files show
//! it: JSON. The printer is `printerinfo` (`machinename`, `resolutionx`,
//! `machinewidth`…, each value under `currentvalue`); its resins are the
//! `slice` entries of each `material`, with their print settings under
//! `print` and the light and compensation under `advanced`.
//!
//! The lift runs in 2 stages: `…liftheight` at `…liftspeed` near the film,
//! then `…liftheight2` at `…liftspeed2`; the retract comes down at
//! `…dropspeed`, then the last `…dropheight2` at `…dropspeed2`, the slow
//! approach to the film, which is our first stage's retract. The waits are
//! the rest times when `printwaitmode` is 1; otherwise the light-off time
//! is the wait before each exposure.

use serde_json::Value;

use super::{
    ImportError, ImportedPrinter, ImportedResin, empty_settings, mm, per_min, pwm,
    scale_from_percent, sec, stage, wait,
};

/// A setting of the file: its `currentvalue`. CHITUBOX stores single
/// precision floats: each is given back as the shortest decimal that is the
/// same `f32` (1.4, not 1.399999976158142).
fn number(value: &Value, path: &[&str]) -> Option<f64> {
    let v = path.iter().try_fold(value, |v, key| v.get(key))?;
    let v = v.get("currentvalue").unwrap_or(v);
    let n = v.as_f64().or_else(|| v.as_bool().map(f64::from))?;
    Some((n as f32).to_string().parse().unwrap_or(n))
}

fn text(value: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(value, |v, key| v.get(key))
        .map(|v| v.get("currentvalue").unwrap_or(v))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The resin of one `slice` entry; `brand` names the material it is under.
fn resin(slice: &Value, brand: &str) -> ImportedResin {
    let print = |key: &str| number(slice, &["print", key]);
    let advanced = |key: &str| number(slice, &["advanced", key]);
    let mut settings = empty_settings();
    settings.normal_exposure_sec = sec(print("normalexposuretime"));
    settings.bottom_exposure_sec = sec(print("bottomlayerexposuretime"));
    settings.bottom_layer_count = print("bottomlayercount")
        .filter(|c| *c >= 0.0 && c.fract() == 0.0)
        .map(|c| c as u32);
    if print("printwaitmode") == Some(1.0) {
        settings.wait_before_cure_sec = wait(print("normalresttimeafterretract"));
        settings.wait_after_cure_sec = wait(print("normalresttimebeforelift"));
        settings.wait_after_lift_sec = wait(print("normalresttimeafterlift"));
        settings.bottom_wait_before_cure_sec = wait(print("bottomresttimeafterretract"));
        settings.bottom_wait_after_cure_sec = wait(print("bottomresttimebeforelift"));
        settings.bottom_wait_after_lift_sec = wait(print("bottomresttimeafterlift"));
    } else {
        settings.wait_before_cure_sec = wait(print("lightofftime"));
        settings.bottom_wait_before_cure_sec = wait(print("bottomlightofftime"));
    }
    // The 2 stages: near the film, then on.
    let two = |layer: &str| print(&format!("{layer}layerliftheight2")).is_some_and(|h| h > 0.0);
    settings.lift_distance_mm = mm(print("normallayerliftheight"));
    settings.lift_speed_mm_min = per_min(print("normallayerliftspeed"));
    settings.bottom_lift_distance_mm = mm(print("bottomlayerliftheight"));
    settings.bottom_lift_speed_mm_min = per_min(print("bottomlayerliftspeed"));
    if two("normal") {
        settings.retract_speed_mm_min = per_min(print("normallayerdropspeed2"));
        settings.second_travel = stage(
            print("normallayerliftheight2"),
            print("normallayerliftspeed2"),
            print("normallayerdropspeed"),
        );
    } else {
        settings.retract_speed_mm_min = per_min(print("normallayerdropspeed"));
    }
    if two("bottom") {
        settings.bottom_retract_speed_mm_min = per_min(print("bottomlayerdropspeed2"));
        settings.bottom_second_travel = stage(
            print("bottomlayerliftheight2"),
            print("bottomlayerliftspeed2"),
            print("bottomlayerdropspeed"),
        );
    } else {
        settings.bottom_retract_speed_mm_min = per_min(print("bottomlayerdropspeed"));
    }
    // The light's power from 0 to 255, ours a percentage.
    let percent = |v: Option<f64>| v.map(|v| v * 100.0 / 255.0);
    settings.light_pwm = pwm(percent(advanced("normallightintensitypwm")));
    settings.bottom_light_pwm = pwm(percent(advanced("bottomlightintensitypwm")));
    let scale = if advanced("enable_scale_compensation") == Some(1.0) {
        match (
            advanced("compensation_x"),
            advanced("compensation_y"),
            advanced("compensation_z"),
        ) {
            (Some(x), Some(y), Some(z)) => scale_from_percent(x, y, z),
            _ => None,
        }
    } else {
        None
    };
    let mut not_imported = Vec::new();
    for (key, idle) in [
        ("transitionlayercount", 0.0),
        ("edgeexposuremm", 0.0),
        ("bottomedgeexposuremm", 0.0),
    ] {
        if print(key).is_some_and(|v| v != idle) {
            not_imported.push(key.to_owned());
        }
    }
    if advanced("bantialiasing") == Some(1.0) {
        not_imported.push("bantialiasing".to_owned());
    }
    if two("normal") && print("normallayerdropheight2") != print("normallayerliftheight2") {
        not_imported.push("normallayerdropheight2".to_owned());
    }
    ImportedResin {
        brand: brand.to_owned(),
        name: text(slice, &["name"]),
        layer_height_mm: print("layerheight"),
        settings,
        scale,
        not_imported,
    }
}

/// The printer and resins of a `.cfgx` file.
pub fn read_chitubox(bytes: &[u8]) -> Result<Vec<ImportedPrinter>, ImportError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| ImportError::Unreadable(format!("not a CHITUBOX export ({e})")))?;
    let info = value
        .get("printerinfo")
        .ok_or_else(|| ImportError::Unreadable("not a CHITUBOX export".to_owned()))?;
    let (Some(x), Some(y), Some(w), Some(d)) = (
        number(info, &["resolutionx"]),
        number(info, &["resolutiony"]),
        number(info, &["machinewidth"]),
        number(info, &["machinedepth"]),
    ) else {
        return Err(ImportError::Unreadable(
            "the printer has no panel or plate size".to_owned(),
        ));
    };
    let resins: Vec<ImportedResin> = value
        .get("material")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|material| {
            // A material the person named is the brand; "Default" says none.
            let name = text(material, &["name"]);
            let brand = if name.is_empty() || name == "Default" {
                "CHITUBOX".to_owned()
            } else {
                name
            };
            material
                .get("slice")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |slice| resin(slice, &brand))
                .collect::<Vec<_>>()
        })
        .filter(|r| !r.name.trim().is_empty())
        .collect();
    if resins.is_empty() {
        return Err(ImportError::Empty);
    }
    let mut name = text(info, &["machinename"]);
    if name.is_empty() {
        name = text(info, &["machinetype"]);
    }
    Ok(vec![ImportedPrinter {
        name,
        resolution: (x as u32, y as u32),
        plate_mm: (w, d),
        resins,
    }])
}
