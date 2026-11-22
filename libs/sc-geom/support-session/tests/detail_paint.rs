//! Painted detail is avoided, never forbidden (#186): a tip moves off it
//! where bare surface lies within half its spacing, takes the finest tip
//! where none does, and none is dropped for it.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, NodeId, Scene};
use bread_support_session::zone_presets::minis_30;
use bread_support_session::{SupportOptions, SupportRequest, start_supports};

/// A slab 20 x 20 x 4 mm; its first two faces are its underside, split
/// along the diagonal x + y = 20.
fn slab() -> IndexedMesh {
    let (lo, hi) = ([0.0f32; 3], [20.0f32, 20.0, 4.0]);
    let vertices = (0..8)
        .map(|i| {
            Vertex::new([
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ])
        })
        .collect();
    let faces = [
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ]
    .map(IndexedTriangle::from_raw_indices)
    .to_vec();
    IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces)
}

/// The tips of a run on the slab held 6 mm over the plate, with the
/// underside's first face painted or not: where each is, and its width.
fn tips(painted: bool) -> Vec<([f64; 3], f64)> {
    let mut g = Scene::new();
    let leaf: NodeId = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 6.0])),
            LeafPayload::Mesh(Arc::new(slab())),
        )
        .unwrap();
    g.add_root(leaf).unwrap();
    let mut detail = vec![false; 12];
    detail[0] = true;
    let request = SupportRequest {
        options: SupportOptions {
            zones: Some(minis_30()),
            ..SupportOptions::default()
        },
        detail: if painted {
            vec![(leaf, Arc::from(detail))]
        } else {
            Vec::new()
        },
        ..SupportRequest::default()
    };
    let outcome = start_supports(&g, request).unwrap().join().unwrap();
    let session = &outcome.sets[0].session;
    session
        .tips()
        .into_iter()
        .map(|(id, p)| {
            let width = session.shape_of(&id).map_or(0.0, |s| s.tip_diameter_mm);
            (p, width)
        })
        .collect()
}

#[test]
fn tips_on_painted_detail_take_the_finest_tip_and_none_is_dropped() {
    let finest = minis_30()
        .rules
        .sizes
        .iter()
        .map(|s| s.tip_diameter_mm)
        .fold(f64::MAX, f64::min);
    // The painted half, away from the diagonal where a tip could move off.
    let on_paint = |tips: &[([f64; 3], f64)]| -> Vec<f64> {
        tips.iter()
            .filter(|(p, _)| p[2] < 6.3 && p[0] + p[1] < 17.0)
            .map(|(_, d)| *d)
            .collect()
    };
    let bare = tips(false);
    let painted = tips(true);
    assert!(
        on_paint(&bare).len() > 10,
        "{} on the half",
        on_paint(&bare).len()
    );
    assert!(
        on_paint(&bare).iter().any(|&d| d > finest + 1e-6),
        "the bare run uses wider tips there"
    );
    let widths = on_paint(&painted);
    assert!(
        widths.iter().all(|&d| (d - finest).abs() < 1e-6),
        "the finest tip on the paint: {widths:?}"
    );
    assert!(
        painted.len() * 10 >= bare.len() * 9,
        "none dropped: {} against {}",
        painted.len(),
        bare.len()
    );
}
