/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Compares saving a real sliced plate in a 3MF project with saving it in a
//! `.bread` slices chunk (#145). Takes the plate from a CLI `.slices.3mf`:
//! `SC_MEASURE_SLICES_3MF=plate.slices.3mf cargo test --release
//! -p sc-project-formats measure_slices -- --ignored --nocapture`.

use std::io::Cursor;
use std::time::Instant;

use sc_3mf::model::parse_model;
use sc_3mf::package::Package;
use sc_bread_format::{BreadReader, BreadWriter, SLICES_TAG, SLICES_VERSION};
use sc_scene::Scene;
use sc_sl1cer::contour::merge_stacks;

use crate::threemf::slice_mapping::contours_from_slice_stack;
use crate::{
    EmbedProfiles, PlateSlices, ProjectFileType, ProjectPayload, read_project, write_project,
};

#[test]
#[ignore = "needs SC_MEASURE_SLICES_3MF"]
fn measure_slices_in_threemf_and_bread() {
    let Ok(path) = std::env::var("SC_MEASURE_SLICES_3MF") else {
        return;
    };
    let file = std::fs::read(path).expect("the slices file reads");
    let package = Package::read_from(Cursor::new(file)).expect("a 3MF package");
    let stacks: Vec<_> = package
        .part_names()
        .filter(|name| name.as_str().ends_with(".model"))
        .filter_map(|name| parse_model(package.part_data(name)?).ok())
        .flat_map(|document| document.slice_stacks)
        // The CLI file also has root stacks that only reference the parts.
        .filter_map(|stack| contours_from_slice_stack(&stack).ok())
        .collect();
    let contours = merge_stacks(stacks).expect("the stacks share a layer grid");
    let slab_tops: Vec<f64> = contours.layers().iter().map(|l| l.z_mm() + 0.025).collect();
    let points: usize = contours
        .layers()
        .iter()
        .flat_map(|l| l.polygons())
        .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
        .map(|r| r.len())
        .sum();
    println!(
        "plate: {} layers, {points} vertices",
        contours.layers().len()
    );
    let plate = PlateSlices {
        contours,
        slab_tops,
    };
    let scene = Scene::new();

    for round in 0..3 {
        let started = Instant::now();
        let mut threemf = Vec::new();
        write_project(
            &scene,
            &ProjectPayload::default(),
            EmbedProfiles::No,
            Some(&plate),
            Some("measure"),
            ProjectFileType::ThreeMf,
            &mut threemf,
        )
        .expect("the 3MF project writes");
        let threemf_save = started.elapsed();
        let started = Instant::now();
        let loaded = read_project(&mut Cursor::new(&threemf), ProjectFileType::ThreeMf)
            .expect("the 3MF project reads");
        let threemf_open = started.elapsed();
        assert!(loaded.slices.is_some(), "the slices come back");

        let started = Instant::now();
        let mut writer = BreadWriter::new(Vec::new()).unwrap();
        writer
            .chunk(
                SLICES_TAG,
                SLICES_VERSION,
                &sc_bread_format::encode_slices(&plate.contours, &plate.slab_tops),
            )
            .unwrap();
        let bread = writer.finish().unwrap();
        let bread_save = started.elapsed();
        let started = Instant::now();
        let mut reader = BreadReader::new(bread.as_slice()).unwrap();
        let chunk = reader.next_chunk().unwrap().unwrap();
        let (back, _) = sc_bread_format::decode_slices(&chunk.payload).unwrap();
        let bread_open = started.elapsed();
        assert_eq!(back.layers().len(), plate.contours.layers().len());

        println!(
            "round {round}: 3MF save {:.3} s, open {:.3} s, {:.1} MB | .bread save {:.3} s, open {:.3} s, {:.1} MB",
            threemf_save.as_secs_f64(),
            threemf_open.as_secs_f64(),
            threemf.len() as f64 / 1e6,
            bread_save.as_secs_f64(),
            bread_open.as_secs_f64(),
            bread.len() as f64 / 1e6,
        );
    }
}
