/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{AaLevel, BuildVolume, Mirror, PanelRotation, PixelResolution};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};
use sc_sl1cer::raster::rle::RleRun;

use super::{Frames, PLACEHOLDER_GREY, lit_layer_count, write_layer_images};
use crate::raster_setup::RasterSetup;

const PIXELS: u32 = 20;
const FRAME: usize = (PIXELS * PIXELS) as usize;

fn raster() -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(10.0, 10.0, 100.0).unwrap(),
        PixelResolution::new(PIXELS, PIXELS).unwrap(),
        LayerSchedule::uniform(0.05, 0.05, 1).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

/// A plate-centred rectangle, `x` and `y` its spans in millimetres.
fn rect(x: (f64, f64), y: (f64, f64)) -> Polygon {
    Polygon::new_unchecked(
        ClosedRing::new(vec![
            Point2D::new(x.0, y.0),
            Point2D::new(x.1, y.0),
            Point2D::new(x.1, y.1),
            Point2D::new(x.0, y.1),
        ]),
        Vec::new(),
    )
}

fn stack(layers: Vec<Vec<Polygon>>) -> ContourStack {
    ContourStack::new_unchecked(
        layers
            .into_iter()
            .enumerate()
            .map(|(index, polygons)| ContourLayer::new(0.05 * (index as f64 + 0.5), polygons))
            .collect(),
    )
}

fn block() -> Vec<Polygon> {
    vec![rect((-2.0, 3.0), (-1.0, 2.0))]
}

fn other_block() -> Vec<Polygon> {
    vec![rect((-4.0, -3.0), (2.0, 4.0))]
}

/// A square far smaller than a pixel, off every pixel centre.
fn sliver() -> Vec<Polygon> {
    vec![rect((0.1, 0.11), (0.1, 0.11))]
}

/// Every image the writers are handed, one grey per pixel.
fn images(stack: &ContourStack, raster: &RasterSetup, dimmest_stored: u8) -> Vec<Vec<u8>> {
    let mut images = Vec::new();
    write_layer_images(
        stack,
        raster,
        dimmest_stored,
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

fn lit(image: &[u8]) -> Vec<(usize, u8)> {
    image
        .iter()
        .enumerate()
        .filter(|&(_, &grey)| grey > 0)
        .map(|(at, &grey)| (at, grey))
        .collect()
}

fn first_full(image: &[u8]) -> usize {
    image.iter().position(|&grey| grey == u8::MAX).unwrap()
}

#[test]
fn a_dark_layer_between_lit_ones_gets_one_pixel_on_the_layer_below() {
    let images = images(
        &stack(vec![block(), Vec::new(), other_block()]),
        &raster(),
        1,
    );
    assert_eq!(images.len(), 3);
    assert!(images.iter().all(|image| image.len() == FRAME));
    assert_eq!(
        lit(&images[1]),
        [(first_full(&images[0]), PLACEHOLDER_GREY)]
    );
}

#[test]
fn lit_layers_are_handed_over_untouched() {
    let alone = images(&stack(vec![block()]), &raster(), 1);
    let around_a_gap = images(&stack(vec![block(), Vec::new(), block()]), &raster(), 1);
    assert_eq!(around_a_gap[0], alone[0]);
    assert_eq!(around_a_gap[2], alone[0]);
}

#[test]
fn dark_layers_under_the_first_lit_one_take_the_pixel_from_above() {
    let images = images(
        &stack(vec![Vec::new(), Vec::new(), other_block()]),
        &raster(),
        1,
    );
    let at = first_full(&images[2]);
    assert_eq!(lit(&images[0]), [(at, PLACEHOLDER_GREY)]);
    assert_eq!(lit(&images[1]), [(at, PLACEHOLDER_GREY)]);
}

#[test]
fn contours_too_thin_to_light_a_pixel_get_the_placeholder_too() {
    let images = images(&stack(vec![block(), sliver()]), &raster(), 1);
    assert_eq!(
        lit(&images[1]),
        [(first_full(&images[0]), PLACEHOLDER_GREY)]
    );
}

#[test]
fn a_plate_with_nothing_lit_gets_the_pixel_on_the_panel_centre() {
    let images = images(&stack(vec![Vec::new(), sliver()]), &raster(), 1);
    let centre = (PIXELS / 2 * PIXELS + PIXELS / 2) as usize;
    for image in &images {
        assert_eq!(lit(image), [(centre, PLACEHOLDER_GREY)]);
    }
}

#[test]
fn a_turned_panel_places_the_pixel_in_the_turned_frame() {
    let raster = raster().with_panel_rotation(PanelRotation::Ccw90);
    let images = images(&stack(vec![block(), Vec::new()]), &raster, 1);
    assert_eq!(
        lit(&images[1]),
        [(first_full(&images[0]), PLACEHOLDER_GREY)]
    );
}

#[test]
fn a_grey_the_codec_stores_dark_leaves_the_frame_dark() {
    let raster = raster();
    let run = |value, length| RleRun { value, length };
    let faint = [run(0, 5), run(9, 3), run(0, 2)];
    assert_eq!(Frames::new(&[], &raster, 16).lit_pixel(&faint), None);
    assert_eq!(Frames::new(&[], &raster, 1).lit_pixel(&faint), Some(5));
    // A fully lit pixel is preferred to an edge's grey before it.
    let edge = [run(0, 2), run(100, 1), run(0, 4), run(255, 1)];
    assert_eq!(Frames::new(&[], &raster, 1).lit_pixel(&edge), Some(7));
}

#[test]
fn the_lit_layer_count_stops_at_the_last_lit_layer() {
    let raster = raster();
    let count = |layers| lit_layer_count(&stack(layers), &raster);
    assert_eq!(
        count(vec![block(), Vec::new(), block(), Vec::new(), sliver()]),
        3
    );
    assert_eq!(count(vec![block(), block()]), 2);
    assert_eq!(count(vec![Vec::new(), sliver()]), 0);
    assert_eq!(count(Vec::new()), 0);
}
