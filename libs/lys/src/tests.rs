/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use super::*;
use crate::fixture::*;

fn placed(model: &LysModel) -> Vec<[f64; 3]> {
    model
        .mesh
        .vertices()
        .iter()
        .map(|v| model.transform.apply_to_vertex(*v).coords().map(f64::from))
        .collect()
}

fn bounds(points: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn a_model_stands_turned_scaled_and_lifted_on_its_plate() {
    let scene = scene(
        vec![(
            "o1",
            object(
                "Box",
                "pl1",
                v3(188.0, 5.0, 3.0),
                v3(0.0, 0.0, 90.0),
                v3(2.0, 1.0, 1.0),
            ),
        )],
        two_plates(),
    );
    let read = read_lys(&file("3.1.0", &scene, &box_entry(4))).unwrap();
    assert_eq!(read.version, "3.1.0");
    assert_eq!(read.app_version.as_deref(), Some("7.0.0"));
    assert_eq!(read.plates.len(), 2);
    assert_eq!(read.plates[1].name, "Second");
    let model = &read.models[0];
    assert_eq!(model.name, "Box");
    assert_eq!(model.plate, 1);
    assert!(!model.hidden);
    assert_eq!(model.mesh.faces().len(), 12);
    // Scaled ×2 along its own X (4 mm), then turned 90° about Z: 4 mm along
    // the world's Y, 4 mm along X; 10 mm right of plate 1's centre, its
    // bottom 3 mm up.
    let (lo, hi) = bounds(&placed(model));
    assert!(
        close(hi[0] - lo[0], 4.0) && close(hi[1] - lo[1], 4.0),
        "{lo:?} {hi:?}"
    );
    assert!(
        close((lo[0] + hi[0]) / 2.0, 10.0) && close((lo[1] + hi[1]) / 2.0, 5.0),
        "{lo:?} {hi:?}"
    );
    assert!(close(lo[2], 3.0) && close(hi[2], 9.0), "{lo:?} {hi:?}");
    assert_eq!(read.printer.as_deref(), Some("Creality Halot One"));
    assert_eq!(
        read.resin,
        Some(LysResin {
            brand: "Creality".to_owned(),
            name: "Standard Rigid Plus".to_owned(),
        })
    );
    assert_eq!(read.supports, 3);
    assert_eq!(read.hollowed, 1);
}

#[test]
fn the_rotations_go_about_x_then_y_then_z_of_the_world() {
    // A point at +X turned 90° about X stays; about Y it goes to -Z; about Z
    // that -Z stays. With X, Y, Z in turn (90, 90, 90) +X ends at -Z.
    let r = rotation([90.0, 90.0, 90.0]);
    let p = [r[0][0], r[1][0], r[2][0]];
    assert!(
        close(p[0], 0.0) && close(p[1], 0.0) && close(p[2], -1.0),
        "{p:?}"
    );
    // And +Y: about X to +Z, about Y to +X, about Z to +Y.
    let q = [r[0][1], r[1][1], r[2][1]];
    assert!(
        close(q[0], 0.0) && close(q[1], 1.0) && close(q[2], 0.0),
        "{q:?}"
    );
}

#[test]
fn indices_two_bytes_wide_read_alike() {
    let scene = scene(
        vec![(
            "o1",
            object(
                "Box",
                "pl0",
                v3(0.0, 0.0, 0.0),
                v3(0.0, 0.0, 0.0),
                v3(1.0, 1.0, 1.0),
            ),
        )],
        two_plates(),
    );
    let wide = read_lys(&file("3.1.0", &scene, &box_entry(4))).unwrap();
    let narrow = read_lys(&file("3.1.0", &scene, &box_entry(2))).unwrap();
    assert_eq!(wide.models[0].mesh, narrow.models[0].mesh);
}

#[test]
fn a_scene_without_plates_puts_every_model_on_one() {
    let scene = scene(
        vec![(
            "o1",
            object(
                "Box",
                "",
                v3(1.0, 2.0, 0.0),
                v3(0.0, 0.0, 0.0),
                v3(1.0, 1.0, 1.0),
            ),
        )],
        Mp::Nil,
    );
    let read = read_lys(&file("3.1.0", &scene, &box_entry(4))).unwrap();
    assert_eq!(read.plates.len(), 1);
    assert_eq!(read.models[0].plate, 0);
}

#[test]
fn what_it_cannot_read_it_says_and_opens_nothing() {
    assert!(matches!(read_lys(b"nothing"), Err(LysError::NotAScene(_))));
    // A scene that is not MessagePack: in a version it knows, a broken file;
    // in one it does not, a version it cannot read.
    let mut bad = file("3.1.0", &Mp::Nil, &box_entry(4));
    let n = bad.len();
    bad[n - 1] = 0xc1u8.wrapping_add(KEY[0]);
    assert!(matches!(read_lys(&bad), Err(LysError::NotAScene(_))));
    let mut newer = file("9.0.0", &Mp::Nil, &box_entry(4));
    let n = newer.len();
    newer[n - 1] = 0xc1u8.wrapping_add(KEY[0]);
    assert!(matches!(read_lys(&newer), Err(LysError::Version(v)) if v == "9.0.0"));
    // A model whose mesh is missing.
    let mut obj = object(
        "Box",
        "pl0",
        v3(0.0, 0.0, 0.0),
        v3(0.0, 0.0, 0.0),
        v3(1.0, 1.0, 1.0),
    );
    if let Mp::Map(pairs) = &mut obj {
        pairs.retain(|(k, _)| *k != "properties");
        pairs.push(("properties", Mp::Map(vec![("hash", Mp::Str("zzz"))])));
    }
    let scene = scene(vec![("o1", obj)], two_plates());
    assert!(matches!(
        read_lys(&file("3.1.0", &scene, &box_entry(4))),
        Err(LysError::Mesh { model, .. }) if model == "Box"
    ));
}

/// Opens every scene listed (one path per line) in `LYS_CORPUS`, and says
/// what each holds.
#[test]
#[ignore]
fn the_scenes_of_a_corpus_open() {
    let list = std::env::var("LYS_CORPUS").expect("LYS_CORPUS: a file listing .lys paths");
    let mut failed = Vec::new();
    let mut opened = 0;
    for path in std::fs::read_to_string(list)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
    {
        let bytes = std::fs::read(path).unwrap();
        let started = std::time::Instant::now();
        match read_lys(&bytes) {
            Ok(scene) => {
                opened += 1;
                let triangles: usize = scene.models.iter().map(|m| m.mesh.faces().len()).sum();
                println!(
                    "{:>6} ms  {} models, {} plates, {triangles} triangles, {} supports, printer {:?}, resin {:?}  {path}",
                    started.elapsed().as_millis(),
                    scene.models.len(),
                    scene.plates.len(),
                    scene.supports,
                    scene.printer,
                    scene
                        .resin
                        .as_ref()
                        .map(|r| format!("{} {}", r.brand, r.name)),
                );
            }
            Err(e) => {
                println!("FAILED {path}: {e}");
                failed.push(path.to_owned());
            }
        }
    }
    println!("{opened} opened, {} failed", failed.len());
    assert!(failed.is_empty(), "{failed:?}");
}
