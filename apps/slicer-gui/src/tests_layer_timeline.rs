/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The layer timeline's figures (#94).

use std::sync::Arc;

use sc_sla_formats::layer_images::LitMeasure;

use super::*;

/// `areas` of square layers, a second each, lifting at the reference speed.
fn held(areas: &[f64]) -> HeldLayers {
    HeldLayers {
        measures: areas
            .iter()
            .map(|&area_mm2| LitMeasure {
                area_mm2,
                perimeter_mm: 4.0 * area_mm2.sqrt(),
            })
            .collect::<Vec<_>>()
            .into(),
        times_sec: vec![1.0; areas.len()],
        lift_mm_min: vec![Some(60.0); areas.len()],
    }
}

#[test]
fn each_layer_pulls_with_its_area_and_its_cups() {
    let model = PeelModel::default();
    let timeline = Timeline::of(&held(&[100.0, 100.0, 400.0]), Some(&[0.0, 10.0]), &model);
    assert_eq!(timeline.layers.len(), 3);
    assert!((timeline.layers[0].force_n - 1.5).abs() < 1e-9);
    assert!((timeline.layers[1].force_n - 2.0).abs() < 1e-9);
    assert!((timeline.layers[2].force_n - 6.0).abs() < 1e-9);
    assert_eq!(timeline.layers[1].cup_mm2, 10.0);
    assert_eq!(timeline.peak, Some(2));
    // Without a review, no cups.
    let bare = Timeline::of(&held(&[100.0, 100.0]), None, &model);
    assert!((bare.layers[1].force_n - 1.5).abs() < 1e-9);
}

#[test]
fn a_sudden_surface_is_marked_where_it_starts() {
    let mut areas = vec![20.0; 60];
    areas.extend([300.0; 60]);
    let timeline = Timeline::of(&held(&areas), None, &PeelModel::default());
    assert_eq!(timeline.spikes, vec![60]);
}

#[test]
fn the_path_spans_the_box_and_keeps_every_spike() {
    // More layers than points: a one-layer spike still reaches the top.
    let mut values = vec![1.0; 5000];
    values[3333] = 10.0;
    let path = chart_path(&values);
    assert!(path.starts_with("M 0.0 100"), "{path}");
    assert!(path.ends_with("L 1000.0 100 Z"), "{path}");
    assert!(path.contains(" 0.00"), "the spike reaches the top");
    let points = path.matches(" L ").count();
    assert_eq!(points, CHART_POINTS + 1);
    assert!(chart_path(&[]).is_empty());
    // One layer stands in the middle; nothing at all lies flat.
    assert!(chart_path(&[3.0]).contains("L 500.0 0.00"));
    assert!(chart_path(&[0.0, 0.0]).contains("L 1000.0 100.00"));
}

#[test]
fn the_readout_names_the_layer_from_one() {
    let timeline = Timeline::of(
        &held(&[100.0, 25.0]),
        Some(&[0.0, 5.0]),
        &PeelModel::default(),
    );
    let line = timeline.readout(1);
    assert!(line.contains('2'), "{line}");
    assert!(line.contains("25.0 mm²"), "{line}");
    assert!(line.contains("20.0 mm"), "{line}");
    assert!(line.contains("5.0 mm²"), "{line}");
    assert!(timeline.readout(7).is_empty());
    assert!(
        timeline.peak_line().contains("1.5 N"),
        "{}",
        timeline.peak_line()
    );
    let measures: Arc<[LitMeasure]> = Vec::new().into();
    let empty = Timeline::of(
        &HeldLayers {
            measures,
            times_sec: Vec::new(),
            lift_mm_min: Vec::new(),
        },
        None,
        &PeelModel::default(),
    );
    assert!(empty.peak_line().is_empty());
}
