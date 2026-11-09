/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Manual measure of the pre-supported classifier (#258) on real files.
//!
//! `PRESUPPORTED_CORPUS` names a list, one `name|path` per line; a line whose
//! name ends in `--unsupported` is the twin without supports of the file
//! named before it. Each file is read as the app imports it (exact duplicate
//! faces dropped), and the table says how its faces split, how long it
//! took, and for a file with a twin how many faces the twin's model has.
//! Run with `cargo test -p sc-slicer-cli --test main presupported_corpus --
//! --ignored --nocapture`.

use std::collections::HashMap;
use std::time::Instant;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::presupported::{find_supports, fused_supports};
use sc_mesh_formats::loader::drop_scene_duplicate_faces;
use sc_mesh_formats::stl::read_stl;
use sc_scene::Scene;

fn first_mesh(scene: &Scene) -> Option<IndexedMesh> {
    let mut found = None;
    scene
        .for_each_expanded_leaf::<sc_scene::SceneError, _>(
            sc_scene::CsgPolicy::Descend,
            |_, _, mesh| {
                found.get_or_insert_with(|| mesh.clone());
                Ok(())
            },
        )
        .ok()?;
    found
}

fn read(path: &str) -> Option<IndexedMesh> {
    let file = std::fs::File::open(path).ok()?;
    let (mut scene, _) = read_stl(&mut std::io::BufReader::new(file)).ok()?;
    drop_scene_duplicate_faces(&mut scene);
    first_mesh(&scene)
}

#[test]
#[ignore = "manual measure on files outside the repository"]
fn presupported_corpus() {
    let Ok(list) = std::env::var("PRESUPPORTED_CORPUS") else {
        eprintln!("skipping: PRESUPPORTED_CORPUS is not set");
        return;
    };
    let list = std::fs::read_to_string(list).expect("read the corpus list");
    let entries: Vec<(&str, &str)> = list
        .lines()
        .filter_map(|line| line.split_once('|'))
        .collect();
    let twins: HashMap<&str, &str> = entries
        .iter()
        .filter_map(|(name, path)| name.strip_suffix("--unsupported").map(|base| (base, *path)))
        .collect();
    println!("file\tfaces\tmodel\tsupports\tsupport shells\traft\tfused\ttwin model\tms");
    for (name, path) in entries.iter().filter(|(n, _)| !n.ends_with("--unsupported")) {
        let Some(mesh) = read(path) else {
            println!("{name}\tunreadable");
            continue;
        };
        let start = Instant::now();
        let found = find_supports(&mesh);
        let fused = found.is_none() && fused_supports(&mesh);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let twin = twins
            .get(name)
            .and_then(|p| read(p))
            .map_or(String::from("-"), |t| t.faces().len().to_string());
        let (model, supports, shells, raft) = found.as_ref().map_or(
            (mesh.faces().len(), 0, 0, false),
            |f| (f.model.faces().len(), f.supports.faces().len(), f.support_shells, f.raft),
        );
        let total = mesh.faces().len().max(1) as f64;
        println!(
            "{name}\t{}\t{model} ({:.1} %)\t{supports} ({:.1} %)\t{shells}\t{raft}\t{fused}\t{twin}\t{ms:.0}",
            mesh.faces().len(),
            100.0 * model as f64 / total,
            100.0 * supports as f64 / total,
        );
    }
}
