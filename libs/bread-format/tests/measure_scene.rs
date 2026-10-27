/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Times saving and opening a real model's scene as a `.bread` file (#145):
//! `SC_MEASURE_STL=model.stl cargo test --release -p sc-bread-format
//! --test measure_scene -- --ignored --nocapture`.

use std::time::Instant;

use sc_bread_format::{read_scene, write_scene};
use sc_mesh_formats::load_mesh_from_path;

#[test]
#[ignore = "needs SC_MEASURE_STL"]
fn measure_scene_save_and_open() {
    let Ok(path) = std::env::var("SC_MEASURE_STL") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let scene = load_mesh_from_path(&path, None, None)
        .expect("the model loads")
        .graph;
    let leaf = scene.first_leaf_id().expect("a model");
    let mesh = scene.leaf_mesh_arc(leaf).expect("a mesh");
    println!(
        "{}: {} triangles, {} vertices, STL {:.1} MB",
        path.display(),
        mesh.faces().len(),
        mesh.vertices().len(),
        std::fs::metadata(&path).unwrap().len() as f64 / 1e6
    );
    for round in 0..3 {
        let started = Instant::now();
        let bytes = write_scene(&scene, Vec::new()).expect("the scene writes");
        let save = started.elapsed();
        let started = Instant::now();
        let back = read_scene(bytes.as_slice()).expect("the scene reads");
        let open = started.elapsed();
        let back_mesh = back.leaf_mesh_arc(back.first_leaf_id().unwrap()).unwrap();
        assert_eq!(back_mesh.faces(), mesh.faces());
        assert!(
            back_mesh
                .vertices()
                .iter()
                .zip(mesh.vertices())
                .all(|(a, b)| a.as_array().map(f32::to_bits) == b.as_array().map(f32::to_bits))
        );
        println!(
            "round {round}: .bread {:.2} MB, save {:.3} s, open {:.3} s",
            bytes.len() as f64 / 1e6,
            save.as_secs_f64(),
            open.as_secs_f64()
        );
    }
}
