/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Projection from a resolved print configuration to the program a
//! printer file hands the machine.

use std::fmt;

use chrono::Local;
use sc_print_primitives::{
    LightPwm, MassOutOfRange, NegativeDuration, NonEmptyString, NonNegativeFiniteSec,
    PositiveFiniteMm, PositiveFiniteSec, VolumeMm3, VolumeOutOfRange,
};
use sc_print_profiles::{MaterialResolved, ResolvedPrintConfig};
use sc_scene::Scene;
use sc_scene::SceneError;
use sc_sl1cer::profile::SliceProfile;
use sc_sla_formats::{CreationTime, LayerProgram, MotionStage, PrintProgram, RasterSetup};

const SEC_PER_MIN: f64 = 60.0;

/// Returned when a schedule places a layer's slab top outside the
/// range a length can hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlabTopOutOfRange {
    /// Position of the offending layer in the schedule.
    pub layer_index: u32,
    /// The offending slab top.
    pub z_mm: f64,
}

impl fmt::Display for SlabTopOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "layer {} sits at z = {} mm, which is not a positive finite height",
            self.layer_index, self.z_mm,
        )
    }
}

impl std::error::Error for SlabTopOutOfRange {}

/// Why a scene and its resolved configuration yield no print program.
#[derive(Clone, Debug, PartialEq)]
pub enum UnprojectablePrint {
    /// A scheduled layer has no representable height.
    SlabTop(SlabTopOutOfRange),
    /// The scene could not be traversed for the material it encloses.
    Scene(SceneError),
    /// The traversal's total is not a volume.
    Volume(VolumeOutOfRange),
    /// That volume at the resin's density is not a mass.
    Mass(MassOutOfRange),
    /// The schedule, once the printer's calibration is applied, is
    /// not a duration.
    PrintTime(NegativeDuration),
}

impl fmt::Display for UnprojectablePrint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlabTop(err) => err.fmt(f),
            Self::Scene(err) => write!(f, "cannot measure what the scene encloses: {err}"),
            Self::Volume(err) => err.fmt(f),
            Self::Mass(err) => err.fmt(f),
            Self::PrintTime(err) => write!(f, "the print takes no expressible time: {err}"),
        }
    }
}

impl std::error::Error for UnprojectablePrint {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SlabTop(err) => Some(err),
            Self::Scene(err) => Some(err),
            Self::Volume(err) => Some(err),
            Self::Mass(err) => Some(err),
            Self::PrintTime(err) => Some(err),
        }
    }
}

/// Build the [`PrintProgram`] for a scene under a resolved
/// configuration, one record per layer of `profile`'s schedule.
///
/// `scene` must be the one that was sliced and `profile` the one the
/// contours were sliced under.
///
/// The reported volume is everything `scene` encloses, so geometry
/// the machine will not reach still counts towards it, and the
/// reported weight is that volume at the resin's cured density, which
/// is what the print's resin weighs rather than what pouring it costs.
/// The reported duration is what the layer schedule adds up to under
/// the printer's own time calibration.
///
/// # Errors
///
/// [`UnprojectablePrint`] when a scheduled layer has no representable
/// height, when the scene yields no volume to weigh, or when the
/// calibrated schedule overflows a duration.
pub fn project_to_print_program(
    scene: &Scene,
    config: &ResolvedPrintConfig,
    profile: &SliceProfile,
    created_at: CreationTime,
    slicer_version: &'static str,
) -> Result<PrintProgram, UnprojectablePrint> {
    let material = &config.material_resolved;
    let transition_count = config.printer.capabilities.transition_layers.unwrap_or(0);

    let layers = (0u32..)
        .zip(profile.layer_schedule().iter_slab_tops())
        .map(|(layer_index, slab_top)| {
            let slab_top_mm = PositiveFiniteMm::new(slab_top).map_err(|_| {
                UnprojectablePrint::SlabTop(SlabTopOutOfRange {
                    layer_index,
                    z_mm: slab_top,
                })
            })?;
            let waits = waits_for(layer_index, material);
            let motion = motion_for(layer_index, material);
            Ok(LayerProgram {
                slab_top_mm,
                exposure_sec: exposure_for(layer_index, transition_count, material),
                wait_before_cure_sec: waits.before_cure_sec,
                wait_after_cure_sec: waits.after_cure_sec,
                wait_after_lift_sec: waits.after_lift_sec,
                lift: motion.lift,
                retract: motion.retract,
                second_lift: motion.second_lift,
                second_retract: motion.second_retract,
                light_pwm: light_pwm_for(layer_index, material),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let volume_mm3 = plate_volume(scene)?;
    let mass_g = material
        .cured_density_g_per_cm3
        .mass_of(volume_mm3)
        .map_err(UnprojectablePrint::Mass)?;
    let corrected = config
        .printer
        .time_calibration
        .corrected_sec(schedule_duration_sec(&layers));
    let print_time_sec =
        NonNegativeFiniteSec::new(corrected).map_err(UnprojectablePrint::PrintTime)?;

    Ok(PrintProgram {
        raster: RasterSetup::from_profile(profile),
        machine_name: config.printer.identity.name.clone(),
        material_name: material.identity.name.clone(),
        layers,
        layer_height_mm: material.layer_height_mm,
        bottom_layer_count: material.bottom_layer_count,
        transition_layer_count: transition_count,
        volume_mm3,
        mass_g,
        print_time_sec,
        created_at,
        slicer_version,
    })
}

/// Measure the material a scene encloses, as the validated volume a
/// printer file carries.
fn plate_volume(scene: &Scene) -> Result<VolumeMm3, UnprojectablePrint> {
    let total = scene
        .world_volume_mm3()
        .map_err(UnprojectablePrint::Scene)?;
    VolumeMm3::new(total).map_err(UnprojectablePrint::Volume)
}

/// Add up what the layer records ask for: light, pauses and travel,
/// second stages included.
///
/// A real machine takes longer, spending time on ramps and per-layer
/// overhead that no record carries. The printer's time calibration is
/// what adds those back.
fn schedule_duration_sec(layers: &[LayerProgram]) -> f64 {
    layers.iter().map(layer_duration_sec).sum()
}

fn layer_duration_sec(layer: &LayerProgram) -> f64 {
    let travel: f64 = [
        Some(layer.lift),
        Some(layer.retract),
        layer.second_lift,
        layer.second_retract,
    ]
    .into_iter()
    .flatten()
    .map(travel_duration_sec)
    .sum();

    layer.exposure_sec.value()
        + layer.wait_before_cure_sec.value()
        + layer.wait_after_cure_sec.value()
        + layer.wait_after_lift_sec.value()
        + travel
}

/// Time one stage of platform travel takes, from the per-minute
/// speed these files record.
fn travel_duration_sec(stage: MotionStage) -> f64 {
    stage.distance_mm.value() / stage.speed_mm_min.value() * SEC_PER_MIN
}

/// Take the current local time as a printer file's creation time.
///
/// The rendering is the writer's to choose, since the formats that
/// keep one as text never parse it back; this is the one this
/// workspace writes. A clock set before the unix epoch yields 0
/// seconds, which is what the oldest real files carry anyway.
pub fn creation_time_now() -> CreationTime {
    let now = Local::now();
    let rendered = now.format("%Y-%m-%d %H:%M:%S").to_string();
    CreationTime {
        rendered: NonEmptyString::new(&rendered).expect("a formatted timestamp is never empty"),
        unix_sec: u64::try_from(now.timestamp()).unwrap_or_default(),
    }
}

/// The three pauses one layer's record carries.
#[derive(Clone, Copy, Debug)]
struct LayerWaits {
    before_cure_sec: NonNegativeFiniteSec,
    after_cure_sec: NonNegativeFiniteSec,
    after_lift_sec: NonNegativeFiniteSec,
}

/// Pick one layer's pauses: the bottom layers hold the bottom waits
/// and every other layer the normal ones. The transition band takes
/// the normal waits, because only the exposure changes across it.
fn waits_for(layer_index: u32, material: &MaterialResolved) -> LayerWaits {
    if layer_index < material.bottom_layer_count {
        LayerWaits {
            before_cure_sec: material.bottom_wait_before_cure_sec,
            after_cure_sec: material.bottom_wait_after_cure_sec,
            after_lift_sec: material.bottom_wait_after_lift_sec,
        }
    } else {
        LayerWaits {
            before_cure_sec: material.wait_before_cure_sec,
            after_cure_sec: material.wait_after_cure_sec,
            after_lift_sec: material.wait_after_lift_sec,
        }
    }
}

/// The platform travel one layer's record carries.
#[derive(Clone, Copy, Debug)]
struct LayerMotion {
    lift: MotionStage,
    retract: MotionStage,
    second_lift: Option<MotionStage>,
    second_retract: Option<MotionStage>,
}

/// Pick one layer's platform travel: the bottom layers hold the
/// bottom distance and speeds and every other layer the normal ones.
/// The transition band takes the normal travel, because only the
/// exposure changes across it.
///
/// The platform returns the distance it rose. One stage returns it in
/// a single move; a split travel returns the 2 parts in reverse order,
/// so the retract carries the second lift's distance and the second
/// retract carries the first lift's.
fn motion_for(layer_index: u32, material: &MaterialResolved) -> LayerMotion {
    let (distance_mm, lift_speed_mm_min, retract_speed_mm_min, second) =
        if layer_index < material.bottom_layer_count {
            (
                material.bottom_lift_distance_mm,
                material.bottom_lift_speed_mm_min,
                material.bottom_retract_speed_mm_min,
                material.bottom_second_travel,
            )
        } else {
            (
                material.lift_distance_mm,
                material.lift_speed_mm_min,
                material.retract_speed_mm_min,
                material.second_travel,
            )
        };
    let lift = MotionStage {
        distance_mm,
        speed_mm_min: lift_speed_mm_min,
    };
    let Some(second) = second else {
        return LayerMotion {
            lift,
            retract: MotionStage {
                distance_mm,
                speed_mm_min: retract_speed_mm_min,
            },
            second_lift: None,
            second_retract: None,
        };
    };
    // The platform covers the same 2 distances on the way down in
    // reverse order, so the first part of the return is the second
    // lift's distance and the last part is the first lift's. Each
    // stage states its own return speed; none is derived from a lift
    // speed.
    LayerMotion {
        lift,
        retract: MotionStage {
            distance_mm: second.lift_distance_mm,
            speed_mm_min: second.retract_speed_mm_min,
        },
        second_lift: Some(MotionStage {
            distance_mm: second.lift_distance_mm,
            speed_mm_min: second.lift_speed_mm_min,
        }),
        second_retract: Some(MotionStage {
            distance_mm,
            speed_mm_min: retract_speed_mm_min,
        }),
    }
}

/// Pick how hard one layer drives the light: the bottom layers hold
/// the bottom duty and every other layer the normal one. The
/// transition band takes the normal duty, because only the exposure
/// changes across it.
fn light_pwm_for(layer_index: u32, material: &MaterialResolved) -> LightPwm {
    if layer_index < material.bottom_layer_count {
        material.bottom_light_pwm
    } else {
        material.light_pwm
    }
}

/// Pick one layer's exposure: the bottom layers hold the bottom
/// exposure, the transition band steps from it towards the normal
/// exposure, and every layer after the band holds the normal
/// exposure.
fn exposure_for(
    layer_index: u32,
    transition_count: u32,
    material: &MaterialResolved,
) -> PositiveFiniteSec {
    if layer_index < material.bottom_layer_count {
        return material.bottom_exposure_sec;
    }
    let band_end = material.bottom_layer_count.saturating_add(transition_count);
    if layer_index >= band_end {
        return material.normal_exposure_sec;
    }

    let step = f64::from(layer_index - material.bottom_layer_count) + 1.0;
    // Stepping over one more layer than the band holds keeps its last
    // layer above the normal exposure, which the layer after the band
    // is the first to carry.
    let steps = f64::from(transition_count) + 1.0;
    material
        .bottom_exposure_sec
        .interpolate_towards(material.normal_exposure_sec, step / steps)
}

#[cfg(test)]
mod schedule_tests {
    use sc_print_primitives::PositiveFiniteMmPerMin;

    use super::*;

    fn stage(distance_mm: f64, speed_mm_min: f64) -> MotionStage {
        MotionStage {
            distance_mm: PositiveFiniteMm::new(distance_mm).unwrap(),
            speed_mm_min: PositiveFiniteMmPerMin::new(speed_mm_min).unwrap(),
        }
    }

    /// A layer costing 2.5 s of light, 4.1 s of pauses, a 5 mm lift
    /// at 120 mm/min and a 5 mm retract at 150 mm/min, so 2.5 s and
    /// 2 s of travel.
    fn layer() -> LayerProgram {
        LayerProgram {
            slab_top_mm: PositiveFiniteMm::new(0.05).unwrap(),
            exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
            wait_before_cure_sec: NonNegativeFiniteSec::new(0.4).unwrap(),
            wait_after_cure_sec: NonNegativeFiniteSec::new(1.2).unwrap(),
            wait_after_lift_sec: NonNegativeFiniteSec::new(2.5).unwrap(),
            lift: stage(5.0, 120.0),
            retract: stage(5.0, 150.0),
            second_lift: None,
            second_retract: None,
            light_pwm: LightPwm::FULL,
        }
    }

    const LAYER_SEC: f64 = 11.1;

    #[test]
    fn a_layer_costs_its_light_its_pauses_and_its_travel() {
        let total = schedule_duration_sec(&[layer()]);
        assert!((total - LAYER_SEC).abs() < 1e-9, "total: {total}");
    }

    #[test]
    fn every_layer_adds_to_the_schedule() {
        let total = schedule_duration_sec(&[layer(), layer(), layer()]);
        assert!((total - 3.0 * LAYER_SEC).abs() < 1e-9, "total: {total}");
    }

    #[test]
    fn a_split_travel_adds_both_of_its_stages() {
        let mut split = layer();
        split.second_lift = Some(stage(2.0, 60.0));
        split.second_retract = Some(stage(2.0, 40.0));

        // 2 mm at 60 mm/min takes 2 s, and at 40 mm/min 3 s.
        let total = schedule_duration_sec(&[split]);
        assert!((total - (LAYER_SEC + 5.0)).abs() < 1e-9, "total: {total}");
    }

    #[test]
    fn an_empty_schedule_takes_no_time() {
        assert_eq!(schedule_duration_sec(&[]), 0.0);
    }
}
