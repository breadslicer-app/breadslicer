/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Lychee Slicer's printer export (`.lyp`, Printers > Export), as its
//! exported files show it: MessagePack, every byte raised by the byte of
//! a 36-byte key that repeats along the file. The value is a list of
//! `{printer, resins}`; numbers with a unit are `{value, unit}`.

use super::msgpack::{self, Value};
use super::{
    ImportError, ImportedPrinter, ImportedResin, empty_settings, mm, per_min, pwm,
    scale_from_percent, sec, stage, wait,
};

/// The key each byte of an exported file is raised by.
const KEY: &[u8; 36] = b"a8ee1146-8d03-4b69-8a67-59009a3f9ee7";

/// The bytes of a `.lyp` as they were before Lychee raised them.
fn lowered(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .zip(KEY.iter().cycle())
        .map(|(b, k)| b.wrapping_sub(*k))
        .collect()
}

/// A number's value in `unit` (`{value, unit}` or a bare number), or
/// `None`. Micrometres are given in millimetres.
fn number(value: &Value, key: &str) -> Option<f64> {
    let field = value.get(key)?;
    if let Some(n) = field.as_f64() {
        return Some(n);
    }
    let n = field.get("value")?.as_f64()?;
    Some(match field.get("unit").and_then(Value::as_str) {
        Some("um") => n / 1000.0,
        _ => n,
    })
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn on(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The settings Lychee holds that our profiles have no place for, when
/// they do something: tilting vats, slow sections, a gradual first
/// layers, exposure compensation, a longer exposure for supports.
fn not_imported(resin: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "useTiltAbove",
        "useTiltBelow",
        "slowSection",
        "burnLayerDecreasing",
        "burnSupportsEnabled",
    ] {
        if on(resin, key) {
            out.push(key.to_owned());
        }
    }
    for key in ["normalLayerCompensation", "burnLayerCompensation"] {
        if number(resin, key).is_some_and(|n| n != 0.0) {
            out.push(key.to_owned());
        }
    }
    if on(resin, "supportCompensation") {
        out.push("supportCompensation".to_owned());
    }
    out
}

fn resin(value: &Value) -> ImportedResin {
    let n = |key: &str| number(value, key);
    let mut settings = empty_settings();
    settings.normal_exposure_sec = sec(n("normalLayerExposureTime"));
    settings.bottom_exposure_sec = sec(n("burnLayerExposureTime"));
    settings.bottom_layer_count = n("burnLayerNumber")
        .filter(|c| *c >= 0.0 && c.fract() == 0.0)
        .map(|c| c as u32);
    if on(value, "waitingModeIsLightOffDelay") {
        // One pause, with the light off, before each layer.
        settings.wait_before_cure_sec = wait(n("normalLayerLightOffDelay"));
        settings.bottom_wait_before_cure_sec = wait(n("burnLayerLightOffDelay"));
    } else {
        settings.wait_before_cure_sec = wait(n("normalLayerWaitBeforePrint"));
        settings.wait_after_cure_sec = wait(n("normalLayerWaitAfterPrint"));
        settings.wait_after_lift_sec = wait(n("normalLayerWaitAfterLift"));
        settings.bottom_wait_before_cure_sec = wait(n("burnLayerWaitBeforePrint"));
        settings.bottom_wait_after_cure_sec = wait(n("burnLayerWaitAfterPrint"));
        settings.bottom_wait_after_lift_sec = wait(n("burnLayerWaitAfterLift"));
    }
    settings.lift_distance_mm = mm(n("normalLayerZLiftDistance"));
    settings.lift_speed_mm_min = per_min(n("normalLayerLiftSpeed"));
    settings.retract_speed_mm_min = per_min(n("normalLayerBottomSpeed"));
    settings.bottom_lift_distance_mm = mm(n("burnLayerZLiftDistance"));
    settings.bottom_lift_speed_mm_min = per_min(n("burnLayerLiftSpeed"));
    settings.bottom_retract_speed_mm_min = per_min(n("burnLayerBottomSpeed"));
    settings.light_pwm = pwm(n("normalLayerPwm"));
    settings.bottom_light_pwm = pwm(n("burnLayerPwm"));
    // Two-stage motion: the second stage's distance is taken on top of the
    // first, as Photon Workshop's files hold it: if Lychee means a total,
    // the platform rises higher, never less.
    if on(value, "isTSMCOn") {
        settings.second_travel = stage(
            n("normalLayerZLiftDistanceTSMC"),
            n("normalLayerLiftSpeedTSMC"),
            n("normalLayerBottomSpeedTSMC"),
        );
        settings.bottom_second_travel = stage(
            n("burnLayerZLiftDistanceTSMC"),
            n("burnLayerLiftSpeedTSMC"),
            n("burnLayerBottomSpeedTSMC"),
        );
    }
    let scale = match (n("shrinkScaleX"), n("shrinkScaleY"), n("shrinkScaleZ")) {
        (Some(x), Some(y), Some(z)) => scale_from_percent(x, y, z),
        _ => None,
    };
    ImportedResin {
        brand: text(value, "brand"),
        name: text(value, "name"),
        layer_height_mm: n("normalLayerHeight"),
        settings,
        scale,
        not_imported: not_imported(value),
    }
}

/// The printers and resins of a `.lyp` file.
pub fn read_lychee(bytes: &[u8]) -> Result<Vec<ImportedPrinter>, ImportError> {
    let value = msgpack::decode(&lowered(bytes))
        .map_err(|e| ImportError::Unreadable(format!("not a Lychee printer export ({})", e.0)))?;
    let Value::Array(entries) = value else {
        return Err(ImportError::Unreadable(
            "not a Lychee printer export".to_owned(),
        ));
    };
    let printers: Vec<ImportedPrinter> = entries
        .iter()
        .filter_map(|entry| {
            let printer = entry.get("printer")?;
            let resolution = (
                number(printer, "resWidth")? as u32,
                number(printer, "resHeight")? as u32,
            );
            let plate_mm = (
                number(printer, "printableX").or_else(|| number(printer, "x"))?,
                number(printer, "printableY").or_else(|| number(printer, "y"))?,
            );
            let resins = match entry.get("resins") {
                Some(Value::Array(resins)) => resins.iter().map(resin).collect(),
                _ => Vec::new(),
            };
            Some(ImportedPrinter {
                name: text(printer, "name"),
                resolution,
                plate_mm,
                resins,
            })
        })
        .filter(|printer| !printer.resins.is_empty())
        .collect();
    if printers.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(printers)
}

/// `bytes` as a `.lyp` holds them: raised by the key (for the tests).
#[cfg(test)]
pub(super) fn raised(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .zip(KEY.iter().cycle())
        .map(|(b, k)| b.wrapping_add(*k))
        .collect()
}
