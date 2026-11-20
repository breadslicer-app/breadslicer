/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::f64::consts::PI;

use super::*;

/// The runs of the pixels whose centres `inside` accepts, on a `size` x
/// `size` screen of pixels `pixel_mm`, the screen's corner at the origin.
fn raster(
    pixel_mm: [f64; 2],
    size: u32,
    inside: impl Fn(f64, f64) -> bool,
) -> Vec<(u32, u32, u32)> {
    let mut runs = Vec::new();
    for y in 0..size {
        let cy = (f64::from(y) + 0.5) * pixel_mm[1];
        let mut start = None;
        for x in 0..=size {
            let lit = x < size && inside((f64::from(x) + 0.5) * pixel_mm[0], cy);
            match (lit, start) {
                (true, None) => start = Some(x),
                (false, Some(x0)) => {
                    runs.push((y, x0, x));
                    start = None;
                }
                _ => {}
            }
        }
    }
    runs
}

/// Whether `(x, y)` lies in the square of side `side` centred at `centre`
/// and turned by `angle`.
fn in_square(centre: [f64; 2], side: f64, angle: f64) -> impl Fn(f64, f64) -> bool {
    move |x, y| {
        let (dx, dy) = (x - centre[0], y - centre[1]);
        let (s, c) = angle.sin_cos();
        let u = dx * c + dy * s;
        let v = -dx * s + dy * c;
        u.abs() <= side / 2.0 && v.abs() <= side / 2.0
    }
}

fn close(measured: f64, truth: f64, tolerance: f64) {
    let error = (measured - truth).abs() / truth;
    assert!(
        error <= tolerance,
        "measured {measured:.4}, true {truth:.4}, {:.2} % off",
        error * 100.0
    );
}

#[test]
fn a_disc_measures_its_circumference() {
    let pixel = [0.05, 0.05];
    let runs = raster(pixel, 600, |x, y| (x - 15.0).hypot(y - 15.0) <= 10.0);
    close(perimeter_of_rows(pixel, runs), 2.0 * PI * 10.0, 0.015);
}

#[test]
fn a_square_measures_its_sides_at_any_angle() {
    let pixel = [0.05, 0.05];
    for degrees in [0.0, 10.0, 22.5, 30.0, 45.0, 60.0, 80.0] {
        let angle = f64::to_radians(degrees);
        let runs = raster(pixel, 600, in_square([15.0, 15.0], 12.0, angle));
        close(perimeter_of_rows(pixel, runs), 48.0, 0.025);
    }
}

#[test]
fn rectangular_pixels_measure_the_same() {
    // The M7 Pro's pixels: 16.8 x 24.8 µm.
    let pixel = [0.0168, 0.0248];
    let size = 1000;
    for degrees in [0.0, 30.0, 45.0] {
        let angle = f64::to_radians(degrees);
        let runs = raster(pixel, size, in_square([8.0, 12.0], 6.0, angle));
        close(perimeter_of_rows(pixel, runs), 24.0, 0.025);
    }
    let runs = raster(pixel, size, |x, y| (x - 8.0).hypot(y - 12.0) <= 4.0);
    close(perimeter_of_rows(pixel, runs), 2.0 * PI * 4.0, 0.015);
}

#[test]
fn a_ring_counts_both_edges() {
    let pixel = [0.05, 0.05];
    let runs = raster(pixel, 600, |x, y| {
        let r = (x - 15.0).hypot(y - 15.0);
        (5.0..=10.0).contains(&r)
    });
    close(perimeter_of_rows(pixel, runs), 2.0 * PI * 15.0, 0.02);
}

#[test]
fn moving_a_shape_changes_nothing() {
    let pixel = [0.05, 0.05];
    let runs = raster(pixel, 600, |x, y| (x - 15.0).hypot(y - 15.0) <= 7.0);
    let moved: Vec<(u32, u32, u32)> = runs
        .iter()
        .map(|&(y, x0, x1)| (y + 37, x0 + 11, x1 + 11))
        .collect();
    assert_eq!(
        perimeter_of_rows(pixel, runs),
        perimeter_of_rows(pixel, moved)
    );
}

#[test]
fn rows_given_in_pieces_and_with_gaps_read_as_one_layer() {
    let pixel = [0.05, 0.05];
    let runs = raster(pixel, 400, |x, y| {
        (x - 5.0).hypot(y - 5.0) <= 3.0 || (x - 14.0).hypot(y - 14.0) <= 3.0
    });
    let whole = perimeter_of_rows(pixel, runs.iter().copied());
    // Each span split in two touching halves, given as separate rows.
    let mut meter = PerimeterMeter::new(pixel);
    for &(y, x0, x1) in &runs {
        let mid = x0 + (x1 - x0) / 2;
        meter.push_row(y, [(mid, x1)]);
        meter.push_row(y, [(x0, mid)]);
    }
    assert!((meter.finish() - whole).abs() < 1e-9);
    // Two discs apart measure as the sum of each.
    let one = perimeter_of_rows(
        pixel,
        raster(pixel, 400, |x, y| (x - 5.0).hypot(y - 5.0) <= 3.0),
    );
    let two = perimeter_of_rows(
        pixel,
        raster(pixel, 400, |x, y| (x - 14.0).hypot(y - 14.0) <= 3.0),
    );
    assert!((one + two - whole).abs() < 1e-9);
}

#[test]
fn nothing_has_no_perimeter() {
    assert_eq!(perimeter_of_rows([0.05, 0.05], []), 0.0);
    // One pixel is sampled at its centre: less than its 4 sides.
    let one = perimeter_of_rows([1.0, 1.0], [(3, 3, 4)]);
    assert!(one > 1.0 && one < 4.0, "{one}");
}
