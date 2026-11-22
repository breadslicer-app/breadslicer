/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The stress view's chart and bands (#245). How a tip's load finds its
//! support is tested with real supports in `support_run`'s tests.

use super::*;

/// The pixel at column `x`, `y` rows from the bottom.
fn at(buffer: &SharedPixelBuffer<Rgba8Pixel>, x: usize, from_bottom: usize) -> Rgba8Pixel {
    let w = CHART_W as usize;
    let h = CHART_H as usize;
    buffer.as_slice()[(h - 1 - from_bottom) * w + x]
}

#[test]
fn each_bar_stands_as_high_as_its_force_in_the_colour_of_its_load() {
    // Two layers: the first half as strong and held by the plate, the
    // second at its peak with a tip at all it holds.
    let buffer = chart_pixels(&[1.0, 2.0], &[0.0, 1.0]);
    let w = CHART_W as usize;
    let h = CHART_H as usize;
    let left = at(&buffer, 10, 2);
    let right = at(&buffer, w - 10, 2);
    let plain = PLAIN.map(|v| (v * 255.0).round() as u8);
    let high = HIGH.map(|v| (v * 255.0).round() as u8);
    assert_eq!([left.r, left.g, left.b], plain, "the plate holds it: plain");
    assert_eq!([right.r, right.g, right.b], high, "all a tip holds: red");
    // Half as high: nothing above the middle on the left, the right to the top.
    assert_eq!(at(&buffer, 10, h / 2 + 4).a, 0);
    assert!(at(&buffer, 10, h / 2 - 4).a > 0);
    assert!(at(&buffer, w - 10, h - 1).a > 0);
}

#[test]
fn a_column_keeps_the_hardest_of_its_layers() {
    let mut force = vec![1.0; 5000];
    let mut share = vec![0.0; 5000];
    force[2500] = 10.0;
    share[2500] = 0.5;
    let buffer = chart_pixels(&force, &share);
    let column = 2500 * CHART_W as usize / 5000;
    let top = at(&buffer, column, CHART_H as usize - 1);
    assert!(top.a > 0, "the one-layer spike reaches the top");
    let mid = at(&buffer, column, 2);
    let yellow = MID.map(|v| (v * 255.0).round() as u8);
    assert_eq!([mid.r, mid.g, mid.b], yellow);
    assert!(chart_pixels(&[], &[]).as_slice().iter().all(|p| p.a == 0));
}

#[test]
fn the_load_runs_from_green_through_yellow_to_red() {
    let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6);
    assert!(near(load_color(0.0), LOW));
    assert!(near(load_color(0.5), MID));
    assert!(near(load_color(1.0), HIGH));
    assert!(near(load_color(4.0), HIGH));
}

#[test]
fn the_bands_follow_each_layer_s_height() {
    // Four layers of 0.05 mm, then one of 0.1: the last takes the top
    // third of the bands.
    let tops = [0.05, 0.1, 0.15, 0.2, 0.3];
    let share = [0.0, 0.0, 0.2, 0.0, 0.9];
    let bands = height_bands(&share, &tops, None).expect("bands");
    assert_eq!(bands.top_mm, 0.3);
    assert_eq!(bands.bands.len(), STRESS_BANDS);
    let band = |z: f64| bands.bands[((z / 0.3) * STRESS_BANDS as f64) as usize];
    assert_eq!(band(0.02), 0.0);
    assert_eq!(band(0.12), 0.2);
    assert_eq!(band(0.25), 0.9);
    assert_eq!(band(0.299), 0.9);
    assert!(height_bands(&share, &[], None).is_none());
}
