/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Small prints for the tests: a plate, a program and a contour stack, and the
//! layer images the slicer's own rasteriser makes of them.

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{CreationTime, LayerProgram, MotionStage, PrintProgram, RasterSetup};

pub(crate) const PLATE_MM: f64 = 10.0;
pub(crate) const PIXELS_X: u32 = 40;
pub(crate) const PIXELS_Y: u32 = 30;
pub(crate) const LAYER_HEIGHT_MM: f64 = 0.05;

pub(crate) fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn speed(value: f64) -> PositiveFiniteMmPerMin {
    PositiveFiniteMmPerMin::new(value).unwrap()
}

fn sec(value: f64) -> NonNegativeFiniteSec {
    NonNegativeFiniteSec::new(value).unwrap()
}

pub(crate) fn raster(aa: AaLevel, mirror: Mirror) -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(PLATE_MM, PLATE_MM * 0.75, 100.0).unwrap(),
        PixelResolution::new(PIXELS_X, PIXELS_Y).unwrap(),
        LayerSchedule::uniform(LAYER_HEIGHT_MM, LAYER_HEIGHT_MM, 3).unwrap(),
        mirror,
        aa,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

pub(crate) fn layer(slab_top_mm: f64, exposure_sec: f64, bottom: bool) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(slab_top_mm),
        exposure_sec: PositiveFiniteSec::new(exposure_sec).unwrap(),
        wait_before_cure_sec: sec(if bottom { 3.0 } else { 1.5 }),
        wait_after_cure_sec: sec(if bottom { 1.0 } else { 0.5 }),
        wait_after_lift_sec: sec(0.25),
        lift: MotionStage {
            distance_mm: mm(if bottom { 7.0 } else { 6.0 }),
            speed_mm_min: speed(if bottom { 60.0 } else { 90.0 }),
        },
        retract: MotionStage {
            distance_mm: mm(if bottom { 7.0 } else { 6.0 }),
            speed_mm_min: speed(150.0),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: LightPwm::FULL,
    }
}

pub(crate) fn program(machine: &str, raster: RasterSetup, layers: usize) -> PrintProgram {
    PrintProgram {
        raster,
        machine_name: NonEmptyString::new(machine).unwrap(),
        material_name: NonEmptyString::new("Validation Resin").unwrap(),
        layers: (0..layers)
            .map(|index| {
                let bottom = index < 2;
                layer(
                    LAYER_HEIGHT_MM * (index + 1) as f64,
                    if bottom { 35.0 } else { 2.5 },
                    bottom,
                )
            })
            .collect(),
        layer_height_mm: mm(LAYER_HEIGHT_MM),
        bottom_layer_count: 2,
        transition_layer_count: 0,
        volume_mm3: VolumeMm3::new(12.5).unwrap(),
        mass_g: MassG::new(0.015).unwrap(),
        print_time_sec: NonNegativeFiniteSec::new(321.5).unwrap(),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-10-27 10:00:00").unwrap(),
            unix_sec: 1_793_095_200,
        },
        slicer_version: "0.0.1-test",
    }
}

fn ring(points: &[(f64, f64)]) -> ClosedRing {
    ClosedRing::new(points.iter().map(|&(x, y)| Point2D::new(x, y)).collect())
}

/// An asymmetric shape per layer, plate-centred: an L with a hole, plus a
/// slanted triangle whose edges anti-alias.
pub(crate) fn stack(layers: usize) -> ContourStack {
    ContourStack::new_unchecked(
        (0..layers)
            .map(|index| {
                let grow = index as f64 * 0.2;
                let l_shape = Polygon::new_unchecked(
                    ring(&[
                        (-4.0, -3.0),
                        (-1.0 + grow, -3.0),
                        (-1.0 + grow, -1.0),
                        (-2.5, -1.0),
                        (-2.5, 2.5),
                        (-4.0, 2.5),
                    ]),
                    vec![ring(&[
                        (-3.5, -2.5),
                        (-3.5, -1.5),
                        (-3.0, -1.5),
                        (-3.0, -2.5),
                    ])],
                );
                let triangle = Polygon::new_unchecked(
                    ring(&[(0.5, -2.0), (4.3, -0.7), (1.1, 3.1)]),
                    Vec::new(),
                );
                ContourLayer::new(
                    LAYER_HEIGHT_MM * (index as f64 + 0.5),
                    vec![l_shape, triangle],
                )
            })
            .collect(),
    )
}

/// The slicer's own layer images: rows in file order, one grey per pixel.
pub(crate) fn expected_images(stack: &ContourStack, raster: &RasterSetup) -> Vec<Vec<u8>> {
    let mut images = Vec::new();
    write_layer_images(
        stack,
        raster,
        |_, runs| {
            let mut image = Vec::new();
            for run in runs {
                image.extend(std::iter::repeat_n(run.value, run.length as usize));
            }
            image
        },
        |_, image| -> Result<(), ()> {
            images.push(image);
            Ok(())
        },
    )
    .unwrap();
    images
}
