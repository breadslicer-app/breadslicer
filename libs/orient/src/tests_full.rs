/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The terms of a Full verdict (#185): cups on fine voxels with a sealed
//! cavity counted, supports that land on the model (T6) and on the paint
//! (D3).

use std::sync::atomic::AtomicBool;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::tests_shapes::Shape;
use crate::*;

fn analyse(mesh: &IndexedMesh, paint: Option<&[u8]>) -> OrientAnalysis {
    OrientAnalysis::new(
        mesh,
        &AffineTransform3D::identity(),
        paint,
        &AtomicBool::new(false),
    )
    .expect("not cancelled")
}

fn ctx(mode: SearchMode) -> OrientContext {
    OrientContext {
        mode,
        ..OrientContext::new(Preset::Balanced)
    }
}

/// A 20 mm box with a sealed 16 mm cavity: a hollowed model with no drain.
fn hollow_box() -> IndexedMesh {
    let outer = Shape::default().boxed([0.0; 3], [20.0; 3], 4).mesh();
    let inner = Shape::default().boxed([2.0; 3], [18.0; 3], 4).mesh();
    // The cavity's faces turn inward.
    let n = outer.vertices.len() as u32;
    let mut vertices = outer.vertices.clone();
    vertices.extend(inner.vertices.iter().copied());
    let mut faces = outer.faces.clone();
    faces.extend(inner.faces.iter().map(|f| {
        let [a, b, c] = f.vertices.map(|v| v.get() + n);
        sc_mesh_core::IndexedTriangle::from_raw_indices([a, c, b])
    }));
    IndexedMesh::new(None, vertices, faces)
}

#[test]
fn a_sealed_cavity_is_a_cup_whichever_way_it_stands() {
    let solid = analyse(&Shape::default().boxed([0.0; 3], [20.0; 3], 4).mesh(), None);
    let hollow = analyse(&hollow_box(), None);
    for mode in [SearchMode::Quick, SearchMode::Full] {
        for up in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, -0.6, 0.8]] {
            let c = ctx(mode);
            assert_eq!(solid.evaluate(up, &c).cup_area_mm2, 0.0);
            let cost = hollow.evaluate(up, &c);
            assert!(cost.cup_area_mm2 > 100.0, "{mode:?} {up:?}: {cost:?}");
            assert!((cost.cup_force_n - c.peel.cup_n_per_mm2 * cost.cup_area_mm2).abs() < 1e-9);
        }
    }
}

#[test]
fn a_full_search_never_leaves_a_bowl_open_towards_the_film() {
    let bowl = Shape::default().bowl(20.0, 17.0, 48, 12).mesh();
    let a = analyse(&bowl, None);
    let o = a
        .search(
            &ctx(SearchMode::Full),
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .expect("not cancelled");
    let best = o.recommended();
    // The bowl opens towards +Z: the pose found never faces it up, and
    // its cups are a fraction of the open bowl's.
    assert!(best.up[2] <= 0.5, "opening faces up: {:?}", best.up);
    let open = a.evaluate([0.0, 0.0, 1.0], &ctx(SearchMode::Full));
    assert!(open.cup_area_mm2 > 500.0, "{open:?}");
    assert!(
        best.cost.cup_area_mm2 < open.cup_area_mm2 / 4.0,
        "{:?}",
        best.cost
    );
}

/// A mushroom: a 30 × 30 × 4 base, a post, and a 20 × 20 cap above the
/// base's top, which is painted.
fn mushroom() -> (IndexedMesh, Vec<u8>) {
    let mesh = Shape::default()
        .boxed([0.0, 0.0, 0.0], [30.0, 30.0, 4.0], 6)
        .boxed([13.0, 13.0, 4.0], [17.0, 17.0, 14.0], 2)
        .boxed([5.0, 5.0, 14.0], [25.0, 25.0, 16.0], 6)
        .mesh();
    let paint = mesh
        .faces
        .iter()
        .map(|f| {
            let p = f
                .vertices
                .map(|v| mesh.vertices[usize::from(v)].coords().map(f64::from));
            let top = p.iter().all(|q| (q[2] - 4.0).abs() < 1e-6);
            if top { 255 } else { 0 }
        })
        .collect();
    (mesh, paint)
}

#[test]
fn supports_under_a_hat_land_on_the_painted_face_below_it() {
    let (mesh, paint) = mushroom();
    let a = analyse(&mesh, Some(&paint));
    // Upright, the cap's underside hangs over the painted base.
    let full = a.evaluate([0.0, 0.0, 1.0], &ctx(SearchMode::Full));
    assert!(full.model_landings > 1.0, "{full:?}");
    let d = full.detail.expect("painted");
    assert!(d.landings > 1.0, "{d:?}");
    // A quick verdict does not measure them.
    let quick = a.evaluate([0.0, 0.0, 1.0], &ctx(SearchMode::Quick));
    assert_eq!(quick.model_landings, 0.0);
    assert_eq!(quick.detail.expect("painted").landings, 0.0);

    // A Full search finds a pose that keeps the marks off the paint.
    let o = a
        .search(
            &ctx(SearchMode::Full),
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .expect("not cancelled");
    let best = o.recommended();
    assert_eq!(
        best.cost.detail.expect("painted").landings,
        0.0,
        "{:?}",
        best.cost
    );
}
