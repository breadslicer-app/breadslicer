/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Projection from a resolved print configuration to the program a
//! printer file hands the machine.

use std::fmt;

use chrono::Local;
use sc_print_primitives::{
    LightPwm, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteSec, VolumeMm3,
    VolumeOutOfRange,
};
use sc_print_profiles::{MaterialResolved, ResolvedPrintConfig};
use sc_scene::Scene;
use sc_scene::SceneError;
use sc_sl1cer::profile::SliceProfile;
use sc_sla_formats::{LayerProgram, MotionStage, PrintProgram, RasterSetup};

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
}

impl fmt::Display for UnprojectablePrint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlabTop(err) => err.fmt(f),
            Self::Scene(err) => write!(f, "cannot measure what the scene encloses: {err}"),
            Self::Volume(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for UnprojectablePrint {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SlabTop(err) => Some(err),
            Self::Scene(err) => Some(err),
            Self::Volume(err) => Some(err),
        }
    }
}

/// Build the [`PrintProgram`] for a scene under a resolved
/// configuration, one record per layer of `profile`'s schedule.
///
/// `scene` must be the one that was sliced, `profile` the one the
/// contours were sliced under, and `created_at` must already be
/// rendered the way the target file records a creation time.
///
/// The reported volume is everything `scene` encloses, so geometry
/// the machine will not reach still counts towards it.
///
/// # Errors
///
/// [`UnprojectablePrint`] when a scheduled layer has no representable
/// height, or when the scene yields no volume.
pub fn project_to_print_program(
    scene: &Scene,
    config: &ResolvedPrintConfig,
    profile: &SliceProfile,
    created_at: NonEmptyString,
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
                second_lift: None,
                second_retract: None,
                light_pwm: light_pwm_for(layer_index, material),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PrintProgram {
        raster: RasterSetup::from_profile(profile),
        machine_name: config.printer.identity.name.clone(),
        material_name: material.identity.name.clone(),
        layers,
        layer_height_mm: material.layer_height_mm,
        bottom_layer_count: material.bottom_layer_count,
        transition_layer_count: transition_count,
        volume_mm3: plate_volume(scene)?,
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

/// Render the current local time as a printer file's creation time.
///
/// These formats carry the creation time as a free-form string, so its
/// shape is the writer's to choose. This is the one this workspace
/// writes, and the rendering [`project_to_print_program`] expects
/// `created_at` to arrive in.
pub fn creation_time_now() -> NonEmptyString {
    let rendered = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    NonEmptyString::new(&rendered).expect("a formatted timestamp is never empty")
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
}

/// Pick one layer's platform travel: the bottom layers hold the
/// bottom distance and speeds and every other layer the normal ones.
/// The transition band takes the normal travel, because only the
/// exposure changes across it.
///
/// The platform returns the distance it rose, so the retract repeats
/// the lift's distance at its own speed.
fn motion_for(layer_index: u32, material: &MaterialResolved) -> LayerMotion {
    let (distance_mm, lift_speed_mm_min, retract_speed_mm_min) =
        if layer_index < material.bottom_layer_count {
            (
                material.bottom_lift_distance_mm,
                material.bottom_lift_speed_mm_min,
                material.bottom_retract_speed_mm_min,
            )
        } else {
            (
                material.lift_distance_mm,
                material.lift_speed_mm_min,
                material.retract_speed_mm_min,
            )
        };
    LayerMotion {
        lift: MotionStage {
            distance_mm,
            speed_mm_min: lift_speed_mm_min,
        },
        retract: MotionStage {
            distance_mm,
            speed_mm_min: retract_speed_mm_min,
        },
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
