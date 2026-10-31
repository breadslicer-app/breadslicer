/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! The print settings these containers keep once for the whole file.
//!
//! A [`PrintProgram`] records every layer in full, while CXDLP keeps one set of
//! settings for the bottom layers and one for the rest, and the Anycubic ZIP
//! family keeps those 2 sets beside a short per-layer list. The bottom set is
//! the first layer's, and the normal set is the first layer past the bottom and
//! transition layers (the last layer when the print ends sooner).

use sc_print_primitives::LightPwm;
use sc_sla_formats::{LayerProgram, PrintProgram};

/// One set of settings, in the units of the program.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StageSettings {
    pub exposure_sec: f32,
    /// Pause after the platform returns, before the light comes on.
    pub wait_before_cure_sec: f32,
    /// Pause after the light goes off, before the lift.
    pub wait_after_cure_sec: f32,
    /// Pause at the top of the lift.
    pub wait_after_lift_sec: f32,
    pub lift_mm: f32,
    pub lift_mm_min: f32,
    pub retract_mm_min: f32,
    /// The second stage of the travel, `(distance, lift speed, retract speed)`.
    pub second_stage: Option<(f32, f32, f32)>,
    /// The light's duty on the 0..=255 scale these containers store.
    pub pwm: u8,
}

impl StageSettings {
    /// Takes one layer's settings, or zeros when there is no layer.
    pub(crate) fn of(layer: Option<&LayerProgram>) -> Self {
        let Some(layer) = layer else {
            return Self::default();
        };
        let second_stage = layer.second_lift.map(|lift| {
            let retract = layer
                .second_retract
                .map_or(lift.speed_mm_min.value(), |stage| {
                    stage.speed_mm_min.value()
                });
            (
                lift.distance_mm.value() as f32,
                lift.speed_mm_min.value() as f32,
                retract as f32,
            )
        });
        Self {
            exposure_sec: layer.exposure_sec.value() as f32,
            wait_before_cure_sec: layer.wait_before_cure_sec.value() as f32,
            wait_after_cure_sec: layer.wait_after_cure_sec.value() as f32,
            wait_after_lift_sec: layer.wait_after_lift_sec.value() as f32,
            lift_mm: layer.lift.distance_mm.value() as f32,
            lift_mm_min: layer.lift.speed_mm_min.value() as f32,
            retract_mm_min: layer.retract.speed_mm_min.value() as f32,
            second_stage,
            pwm: pwm_byte(layer.light_pwm),
        }
    }

    /// The whole distance the platform rises, both stages together.
    pub(crate) fn total_lift_mm(&self) -> f32 {
        self.lift_mm + self.second_stage.map_or(0.0, |(distance, _, _)| distance)
    }
}

/// The bottom and the normal settings of a program.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProgramSummary {
    pub bottom: StageSettings,
    pub normal: StageSettings,
}

impl ProgramSummary {
    pub(crate) fn of(program: &PrintProgram) -> Self {
        let layers = &program.layers;
        let first_normal = program
            .bottom_layer_count
            .saturating_add(program.transition_layer_count) as usize;
        let normal = layers.get(first_normal).or_else(|| layers.last());
        Self {
            bottom: StageSettings::of(layers.first()),
            normal: StageSettings::of(normal),
        }
    }
}

/// Converts a duty cycle to the 0..=255 scale these containers store.
pub(crate) fn pwm_byte(pwm: LightPwm) -> u8 {
    (pwm.percent() / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Converts millimetres per minute to the millimetres per second these
/// containers store.
pub(crate) fn mm_per_sec(mm_per_min: f32) -> f32 {
    mm_per_min / 60.0
}

/// The height of each layer's slab and the height of its top, in layer order.
/// The first slab starts at the plate.
pub(crate) fn slabs(program: &PrintProgram) -> Vec<(f32, f32)> {
    let mut previous_top = 0.0_f64;
    program
        .layers
        .iter()
        .map(|layer| {
            let top = layer.slab_top_mm.value();
            let thickness = top - previous_top;
            previous_top = top;
            (thickness as f32, top as f32)
        })
        .collect()
}

/// Reports whether any layer departs from the settings of its group, which
/// the file flags so the machine reads the per-layer list.
pub(crate) fn uses_per_layer_settings(program: &PrintProgram, summary: &ProgramSummary) -> bool {
    program.layers.iter().enumerate().any(|(index, layer)| {
        let group = if (index as u32) < program.bottom_layer_count {
            &summary.bottom
        } else {
            &summary.normal
        };
        let this = StageSettings::of(Some(layer));
        this.exposure_sec != group.exposure_sec
            || this.lift_mm != group.lift_mm
            || this.lift_mm_min != group.lift_mm_min
    })
}

/// Reports whether a layer departs from what the bottom, transition and normal
/// settings give it, so the file must ask the machine to read the per-layer
/// list. The transition layers step linearly from the bottom exposure to the
/// normal one over `transition_layer_count + 1` steps, as the machine ramps
/// them itself; a layer within a hundredth of a second of that counts as
/// following it. UVtools decides the flag the same way.
pub(crate) fn uses_per_layer_list(program: &PrintProgram, summary: &ProgramSummary) -> bool {
    let (bottom, normal) = (&summary.bottom, &summary.normal);
    let bottom_count = program.bottom_layer_count;
    let steps = program.transition_layer_count as f32 + 1.0;
    program.layers.iter().enumerate().any(|(index, layer)| {
        let index = index as u32;
        let (group, exposure) = if index < bottom_count {
            (bottom, bottom.exposure_sec)
        } else if index - bottom_count < program.transition_layer_count {
            let step = (index - bottom_count + 1) as f32;
            let ramp =
                bottom.exposure_sec + (normal.exposure_sec - bottom.exposure_sec) * step / steps;
            (normal, ramp)
        } else {
            (normal, normal.exposure_sec)
        };
        let this = StageSettings::of(Some(layer));
        (this.exposure_sec - exposure).abs() > 0.01
            || this.lift_mm != group.lift_mm
            || this.lift_mm_min != group.lift_mm_min
    })
}

/// Above this, .NET leaves a float as it is when asked to round it.
const SINGLE_ROUND_LIMIT: f32 = 1.0e8;

/// Rounds to `digits` decimals, halves away from zero, as .NET's
/// `MathF.Round` does for UVtools: the value is scaled in single precision, so
/// a product such as 1733 x 0.035 rounds as the float it is.
pub(crate) fn round_away(value: f32, digits: i32) -> f32 {
    if value.abs() >= SINGLE_ROUND_LIMIT {
        return value;
    }
    let power = 10_f32.powi(digits);
    let scaled = value * power;
    let whole = scaled.trunc();
    let fraction = scaled - whole;
    let rounded = if fraction.abs() >= 0.5 {
        whole + fraction.signum()
    } else {
        whole
    };
    rounded / power
}

/// Rounds to `digits` decimals, halves to even, as .NET's `MathF.Round`
/// does for UVtools: the value is scaled in single precision.
pub(crate) fn round_even(value: f32, digits: i32) -> f32 {
    if value.abs() >= SINGLE_ROUND_LIMIT {
        return value;
    }
    let power = 10_f32.powi(digits);
    (value * power).round_ties_even() / power
}

/// Rounds a height as UVtools does: 3 decimals, halves away from zero.
pub(crate) fn round_height(value: f32) -> f32 {
    round_away(value, 3)
}

/// Rounds a panel size or position as UVtools does: 3 decimals, halves to even.
pub(crate) fn round_display(value: f32) -> f32 {
    round_even(value, 3)
}

#[cfg(test)]
mod program_tests {
    use super::*;

    #[test]
    fn rounding_scales_in_single_precision_as_dotnet_does() {
        // 1733 x 0.035 is 60.655 as a float, a hair over the half: UVtools
        // rounds it up, where rounding in double precision would not.
        assert_eq!(round_even(1733.0 * 0.035, 2), 60.66);
        assert_eq!(round_even(3865.0 * 0.017, 2), 65.70);
        assert_eq!(round_even(0.125, 2), 0.12);
        assert_eq!(round_away(0.125, 2), 0.13);
        assert_eq!(round_away(-0.125, 2), -0.13);
        assert_eq!(round_height(0.0499999), 0.05);
        assert_eq!(round_away(1.0e9, 2), 1.0e9);
    }
}
