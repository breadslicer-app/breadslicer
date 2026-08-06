/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the flipped-face trio variants: each is broken in the
//! way the health sweep detects (winding, hence edge-manifoldness),
//! each is repairable by the default passes, and repairing restores
//! the pristine donor byte for byte.

use sc_mesh_core::indexed_mesh::{MeshFix, RepairPasses};
use sc_mesh_core::primitives::FaceIndex;

use super::*;

fn is_edge_manifold(mesh: &IndexedMesh) -> bool {
    mesh.validated_faces()
        .is_ok_and(|faces| faces.validated_edge_manifold().is_ok())
}

fn trio() -> [(&'static str, IndexedMesh, IndexedMesh, Vec<usize>); 3] {
    [
        (
            "unit_cube",
            unit_cube_with_flipped_faces(),
            unit_cube_mesh(),
            UNIT_CUBE_FLIPPED_FACES.to_vec(),
        ),
        (
            "z_tunnel",
            cuboid_with_z_tunnel_and_flipped_faces(),
            cuboid_with_z_tunnel(),
            Z_TUNNEL_FLIPPED_FACES.to_vec(),
        ),
        (
            "y_tunnel",
            cuboid_with_y_tunnel_and_flipped_faces(),
            cuboid_with_y_tunnel(),
            Y_TUNNEL_FLIPPED_FACES.to_vec(),
        ),
    ]
}

#[test]
fn each_variant_breaks_the_edge_manifold_witness() {
    for (name, broken, pristine, _) in trio() {
        assert!(
            is_edge_manifold(&pristine),
            "pristine {name} must be edge-manifold"
        );
        assert!(
            !is_edge_manifold(&broken),
            "broken {name} must fail the witness mint"
        );
    }
}

#[test]
fn each_variant_flips_a_minority_of_faces() {
    // The orientation pass refuses when at least faces / 2 faces
    // read as inside-out; the variants must stay strictly below it.
    for (name, broken, _, flipped) in trio() {
        assert!(
            flipped.len() < broken.faces.len() / 2,
            "{name} flips too many faces to stay repairable"
        );
    }
}

#[test]
fn default_repair_restores_each_variant_to_its_pristine_donor() {
    for (name, mut broken, pristine, flipped) in trio() {
        let report = broken.repair(RepairPasses::default_repair());
        assert_eq!(report.refusal, None, "{name} must repair cleanly");
        assert!(
            report.edge_manifold_after,
            "{name} must be edge-manifold after repair"
        );
        let mut fixed: Vec<FaceIndex> = report
            .fixes
            .iter()
            .map(|fix| match fix {
                MeshFix::FlippedFaceOrientation { face_idx } => *face_idx,
                other => panic!("{name}: unexpected fix {other:?}"),
            })
            .collect();
        fixed.sort_unstable();
        let expected: Vec<FaceIndex> = flipped.into_iter().map(FaceIndex::from).collect();
        assert_eq!(fixed, expected, "{name} must fix exactly the flipped faces");
        assert_eq!(
            broken, pristine,
            "{name} must be byte-identical to its donor after repair"
        );
    }
}
