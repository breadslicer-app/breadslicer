/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::{CutError, CutModel, Fit, Plane, Rect, is_closed, volume};

/// An axis-aligned box from `lo` to `hi`, faces outward, or inward when
/// `inward` (a cavity).
fn cuboid(lo: [f32; 3], hi: [f32; 3], inward: bool) -> Shell {
    let corner = |k: u32| {
        [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    let vertices = (0..8).map(corner).collect();
    let quads = [
        [0, 2, 3, 1], // -z
        [4, 5, 7, 6], // +z
        [0, 1, 5, 4], // -y
        [2, 6, 7, 3], // +y
        [0, 4, 6, 2], // -x
        [1, 3, 7, 5], // +x
    ];
    let mut faces = Vec::new();
    for [a, b, c, d] in quads {
        if inward {
            faces.push([a, c, b]);
            faces.push([a, d, c]);
        } else {
            faces.push([a, b, c]);
            faces.push([a, c, d]);
        }
    }
    (vertices, faces)
}

/// The vertices and triangles of one shell.
type Shell = (Vec<[f32; 3]>, Vec<[u32; 3]>);

fn mesh(parts: &[Shell]) -> Arc<IndexedMesh> {
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    for (v, f) in parts {
        let base = vertices.len() as u32;
        vertices.extend(v.iter().map(|&p| Vertex::new(p)));
        faces.extend(
            f.iter()
                .map(|t| IndexedTriangle::from_raw_indices(t.map(|i| i + base))),
        );
    }
    Arc::new(IndexedMesh::new(None, vertices, faces))
}

fn level(z: f64) -> Plane {
    Plane::through([0.0, 0.0, z], [0.0, 0.0, 1.0]).unwrap()
}

fn all(n: usize) -> Vec<bool> {
    vec![true; n]
}

#[test]
fn a_box_cut_across_leaves_two_closed_pieces_of_the_same_volume() {
    let model = mesh(&[cuboid([0.0; 3], [10.0, 10.0, 10.0], false)]);
    assert!((volume(&model) - 1000.0).abs() < 1e-6);
    let section = CutModel::new(model).unwrap().section(level(3.0)).unwrap();
    assert_eq!(section.pieces().len(), 1);
    assert!((section.pieces()[0].area_mm2() - 100.0).abs() < 1e-6);
    let cut = section.cut(&all(1)).unwrap();
    assert_eq!(cut.pieces.len(), 2);
    assert!(!cut.by_fallback);
    for piece in &cut.pieces {
        assert!(is_closed(piece));
    }
    assert!(
        (volume(&cut.pieces[0]) - 700.0).abs() < 1e-3,
        "the top first"
    );
    assert!((volume(&cut.pieces[1]) - 300.0).abs() < 1e-3);
}

#[test]
fn a_tilted_cut_keeps_the_volume() {
    let model = mesh(&[cuboid([0.0; 3], [10.0, 10.0, 10.0], false)]);
    let plane = Plane::through([5.0, 5.0, 5.0], [0.3, -0.5, 0.8]).unwrap();
    let cut = CutModel::new(model)
        .unwrap()
        .section(plane)
        .unwrap()
        .cut(&all(1))
        .unwrap();
    assert_eq!(cut.pieces.len(), 2);
    let total: f64 = cut.pieces.iter().map(volume).sum();
    assert!((total - 1000.0).abs() < 1e-3, "{total}");
    assert!(cut.pieces.iter().all(is_closed));
}

/// A box with a cavity: the cross-section is a ring, and both pieces keep
/// their part of the cavity, closed.
#[test]
fn a_hollow_box_cuts_into_two_closed_shells() {
    let model = mesh(&[
        cuboid([0.0; 3], [10.0, 10.0, 10.0], false),
        cuboid([2.0; 3], [8.0, 8.0, 8.0], true),
    ]);
    assert!((volume(&model) - (1000.0 - 216.0)).abs() < 1e-3);
    let section = CutModel::new(model).unwrap().section(level(5.0)).unwrap();
    let [ring] = section.pieces() else {
        panic!("one piece with a hole");
    };
    assert_eq!(ring.holes.len(), 1);
    assert!((ring.area_mm2() - 64.0).abs() < 1e-6);
    let cut = section.cut(&all(1)).unwrap();
    assert_eq!(cut.pieces.len(), 2);
    for piece in &cut.pieces {
        assert!(is_closed(piece));
        assert!((volume(piece) - 392.0).abs() < 1e-3);
    }
}

/// Two boxes side by side: cutting only one leaves the other whole, and
/// the result is the two halves and the other box.
#[test]
fn cutting_one_piece_of_the_section_leaves_the_others_whole() {
    let model = mesh(&[
        cuboid([0.0; 3], [10.0, 10.0, 10.0], false),
        cuboid([20.0, 0.0, 0.0], [30.0, 10.0, 10.0], false),
    ]);
    let section = CutModel::new(model).unwrap().section(level(5.0)).unwrap();
    assert_eq!(section.pieces().len(), 2);
    let left = section.piece_at([5.0, 5.0, 5.0]).expect("the left box");
    assert_eq!(section.piece_at([15.0, 5.0, 5.0]), None);
    let mut chosen = vec![false; 2];
    chosen[left] = true;
    let cut = section.cut(&chosen).unwrap();
    assert_eq!(cut.pieces.len(), 3);
    assert!(cut.pieces.iter().all(is_closed));
    let mut volumes: Vec<f64> = cut.pieces.iter().map(volume).collect();
    volumes.sort_by(f64::total_cmp);
    for (got, want) in volumes.iter().zip([500.0, 500.0, 1000.0]) {
        assert!((got - want).abs() < 1e-3, "{volumes:?}");
    }
}

/// A frame (a box with a tunnel), cut upright through one of its two sides
/// only: the other side still joins it, so the cut leaves one piece and says
/// which part of the section joins it. Through both sides, two pieces.
#[test]
fn a_cut_that_leaves_one_piece_says_what_joins_it() {
    let model = Arc::new(sc_mesh_fixtures::cuboid_with_z_tunnel());
    let section = CutModel::new(model)
        .unwrap()
        .section(Plane::through([0.0; 3], [1.0, 0.0, 0.0]).unwrap())
        .unwrap();
    assert_eq!(section.pieces().len(), 2);
    let front = section.piece_at([0.0, -0.7, 1.0]).unwrap();
    let mut chosen = vec![false; 2];
    chosen[front] = true;
    let cut = section.cut(&chosen).unwrap();
    assert_eq!(cut.pieces.len(), 1);
    assert_eq!(cut.joined_by, [1 - front]);
    assert!(is_closed(&cut.pieces[0]));

    let cut = section.cut(&all(2)).unwrap();
    assert_eq!(cut.pieces.len(), 2);
    assert!(cut.pieces.iter().all(is_closed));
    assert!(cut.joined_by.is_empty());
}

/// A shell the cut does not reach goes with the piece it overlaps: a head
/// on a body cut at the waist goes with the top.
#[test]
fn a_shell_the_cut_misses_goes_with_the_piece_it_overlaps() {
    let model = mesh(&[
        cuboid([0.0; 3], [10.0, 10.0, 10.0], false),
        cuboid([4.0, 4.0, 8.0], [6.0, 6.0, 12.0], false),
        cuboid([20.0, 0.0, 0.0], [30.0, 10.0, 10.0], false),
    ]);
    let section = CutModel::new(model).unwrap().section(level(5.0)).unwrap();
    let body = section.piece_at([5.0, 5.0, 5.0]).unwrap();
    let mut chosen = vec![false; section.pieces().len()];
    chosen[body] = true;
    let cut = section.cut(&chosen).unwrap();
    let mut volumes: Vec<f64> = cut.pieces.iter().map(volume).collect();
    volumes.sort_by(f64::total_cmp);
    assert_eq!(volumes.len(), 3, "{volumes:?}");
    for (got, want) in volumes.iter().zip([500.0, 516.0, 1000.0]) {
        assert!((got - want).abs() < 1e-3, "{volumes:?}");
    }
}

#[test]
fn the_rectangle_says_which_pieces_it_takes() {
    let model = mesh(&[
        cuboid([0.0; 3], [10.0, 10.0, 10.0], false),
        cuboid([20.0, 0.0, 0.0], [30.0, 10.0, 10.0], false),
    ]);
    let section = CutModel::new(model).unwrap().section(level(5.0)).unwrap();
    let left = section.piece_at([5.0, 5.0, 5.0]).unwrap();
    let right = 1 - left;
    let around_left = Rect {
        center: [5.0, 5.0],
        half: [8.0, 8.0],
    };
    let fits = section.fits(&around_left);
    assert_eq!(fits[left], Fit::Inside);
    assert_eq!(fits[right], Fit::Outside);
    let half_of_left = Rect {
        center: [0.0, 5.0],
        half: [4.0, 8.0],
    };
    assert_eq!(section.fits(&half_of_left)[left], Fit::Crossed);
    let inside_left = Rect {
        center: [5.0, 5.0],
        half: [1.0, 1.0],
    };
    assert_eq!(section.fits(&inside_left)[left], Fit::Crossed);
}

#[test]
fn the_fill_covers_the_piece() {
    let model = mesh(&[
        cuboid([0.0; 3], [10.0, 10.0, 10.0], false),
        cuboid([2.0; 3], [8.0, 8.0, 8.0], true),
    ]);
    let section = CutModel::new(model).unwrap().section(level(5.0)).unwrap();
    let (points, indices) = section.fill(0);
    let area: f64 = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| points[i as usize].map(f64::from));
            ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
        })
        .sum();
    assert!((area - 64.0).abs() < 1e-6, "{area}");
}

#[test]
fn an_open_mesh_or_a_plane_that_misses_is_refused() {
    let (v, mut f) = cuboid([0.0; 3], [10.0, 10.0, 10.0], false);
    f.pop();
    assert_eq!(
        CutModel::new(mesh(&[(v, f)])).unwrap_err(),
        CutError::NotClosed
    );
    let model = CutModel::new(mesh(&[cuboid([0.0; 3], [10.0, 10.0, 10.0], false)])).unwrap();
    assert_eq!(model.section(level(20.0)).unwrap_err(), CutError::Misses);
    let section = model.section(level(5.0)).unwrap();
    assert_eq!(section.cut(&[false]).unwrap_err(), CutError::NothingChosen);
}

#[test]
fn the_fallback_cuts_a_box_as_ours_does() {
    let model = mesh(&[cuboid([0.0; 3], [10.0, 10.0, 10.0], false)]);
    let cut = crate::fallback::cut(&model, level(3.0)).unwrap();
    assert!(cut.by_fallback);
    assert_eq!(cut.pieces.len(), 2);
    assert!((volume(&cut.pieces[0]) - 700.0).abs() < 1e-3);
    assert!((volume(&cut.pieces[1]) - 300.0).abs() < 1e-3);
}

#[test]
fn a_level_plane_has_x_and_y_for_its_frame() {
    let (u, v) = level(1.0).frame();
    assert_eq!(u, [1.0, 0.0, 0.0]);
    assert_eq!(v, [0.0, 1.0, 0.0]);
    let upright = Plane::through([0.0; 3], [1.0, 0.0, 0.0]).unwrap();
    let (u, v) = upright.frame();
    assert_eq!(u, [0.0, 1.0, 0.0]);
    assert_eq!(v, [0.0, 0.0, 1.0]);
}

/// Cuts real models, one per line of `CUT_MODELS` (STL paths), with planes
/// at 30, 50 and 70 % of their height, tilted 30°, upright, and `CUT_RANDOM`
/// random ones (default 0); prints one line per cut. Models that are not
/// closed are repaired first, as the app does.
#[test]
#[ignore = "reads the models listed in CUT_MODELS"]
fn real_models() {
    use std::time::Instant;

    use sc_mesh_core::indexed_mesh::RepairPasses;
    use sc_mesh_core::indexed_mesh::repair::repaired_copy;

    let list = std::env::var("CUT_MODELS").expect("CUT_MODELS");
    let random: usize = std::env::var("CUT_RANDOM")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let (mut cuts, mut closed, mut fallback) = (0, 0, 0);
    for path in list.split(';').filter(|p| !p.is_empty()) {
        let loaded =
            sc_mesh_formats::load_mesh_from_path(std::path::Path::new(path), None, None).unwrap();
        let graph = loaded.graph;
        let leaf = graph.roots()[0];
        let mut mesh = graph.leaf_mesh_arc(leaf).unwrap().as_ref().clone();
        if let Some(found) = sc_mesh_core::presupported::find_supports(&mesh) {
            mesh = found.model;
        }
        if !is_closed(&mesh) {
            let (_, fixed) = repaired_copy(&mesh, RepairPasses::default_repair());
            mesh = fixed.expect("repaired");
        }
        let name = std::path::Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let start = Instant::now();
        let model = CutModel::new(Arc::new(mesh)).expect("closed after repair");
        let prepare_ms = start.elapsed().as_secs_f64() * 1e3;
        let total = volume(model.mesh());
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for v in model.mesh().vertices() {
            for k in 0..3 {
                lo[k] = lo[k].min(f64::from(v.as_array()[k]));
                hi[k] = hi[k].max(f64::from(v.as_array()[k]));
            }
        }
        let mid = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let mut planes: Vec<(String, Plane)> = [0.3, 0.5, 0.7]
            .iter()
            .map(|f| {
                let z = lo[2] + (hi[2] - lo[2]) * f;
                (format!("z{:.0}", f * 100.0), level(z))
            })
            .collect();
        let (s, c) = 30f64.to_radians().sin_cos();
        planes.push(("tilt30".into(), Plane::through(mid, [s, 0.0, c]).unwrap()));
        planes.push(("x50".into(), Plane::through(mid, [1.0, 0.0, 0.0]).unwrap()));
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for i in 0..random {
            let z = rnd() * 2.0 - 1.0;
            let a = rnd() * std::f64::consts::TAU;
            let r = (1.0 - z * z).sqrt();
            let at = [0, 1, 2].map(|k| lo[k] + (hi[k] - lo[k]) * (0.15 + 0.7 * rnd()));
            planes.push((
                format!("r{i}"),
                Plane::through(at, [r * a.cos(), r * a.sin(), z]).unwrap(),
            ));
        }
        for (label, plane) in planes {
            let start = Instant::now();
            let section = model.section(plane);
            let section_ms = start.elapsed().as_secs_f64() * 1e3;
            let Ok(section) = section else {
                println!("{name}\t{label}\tsection {section:?}");
                continue;
            };
            let start = Instant::now();
            let cut = section.cut(&vec![true; section.pieces().len()]);
            let cut_ms = start.elapsed().as_secs_f64() * 1e3;
            cuts += 1;
            match cut {
                Ok(cut) => {
                    let ok = cut.pieces.iter().all(is_closed);
                    closed += usize::from(ok);
                    fallback += usize::from(cut.by_fallback);
                    let drift: f64 = (cut.pieces.iter().map(volume).sum::<f64>() - total) / total;
                    println!(
                        "{name}\t{label}\tpieces {}\tsection {section_ms:.0} ms\tcut {cut_ms:.0} ms\tclosed {ok}\tfallback {}\tvolume {:+.5} %",
                        cut.pieces.len(),
                        cut.by_fallback,
                        drift * 100.0
                    );
                }
                Err(e) => println!("{name}\t{label}\tERROR {e}\tcut {cut_ms:.0} ms"),
            }
        }
        println!(
            "{name}\tprepare {prepare_ms:.0} ms\tfaces {}",
            model.mesh().faces().len()
        );
    }
    println!("TOTAL cuts {cuts} closed {closed} fallback {fallback}");
}
