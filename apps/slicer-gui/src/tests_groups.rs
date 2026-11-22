/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Groups of models (#475): picked as one, arranged as one body, kept in
//! the project, undone with their members where they stand.

use std::path::PathBuf;
use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_scene::{LeafPayload, NodeId, Scene};

use crate::arrange_run::{ArrangeOptions, arrange_now};
use crate::print_selection::PrintDefaults;
use crate::scene_document::SceneDocument;
use crate::selection::{LeafId, Selection};

/// A closed 4 mm cube from the plate up.
fn cube() -> IndexedMesh {
    let v = |x: f32, y: f32, z: f32| Vertex::new([x * 4.0, y * 4.0, z * 4.0]);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(1.0, 1.0, 0.0),
        v(0.0, 1.0, 0.0),
        v(0.0, 0.0, 1.0),
        v(1.0, 0.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(0.0, 1.0, 1.0),
    ];
    let f = |a, b, c| IndexedTriangle::from_raw_indices([a, b, c]);
    let faces = vec![
        f(0, 2, 1),
        f(0, 3, 2),
        f(4, 5, 6),
        f(4, 6, 7),
        f(0, 1, 5),
        f(0, 5, 4),
        f(1, 2, 6),
        f(1, 6, 5),
        f(2, 3, 7),
        f(2, 7, 6),
        f(3, 0, 4),
        f(3, 4, 7),
    ];
    IndexedMesh::new(None, vertices, faces)
}

/// A document with three cubes standing at `at`.
fn three_cubes(at: [[f64; 3]; 3]) -> (std::rc::Rc<SceneDocument>, Vec<NodeId>) {
    let mut graph = Scene::new();
    let mesh = Arc::new(cube());
    let mut leaves = Vec::new();
    for p in at {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(p)),
                LeafPayload::Mesh(mesh.clone()),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    let document = SceneDocument::new();
    document.install_graph(graph, PathBuf::from("cubes.stl"));
    (document, leaves)
}

fn place(document: &SceneDocument, leaf: NodeId) -> [f64; 3] {
    let t = document
        .with_scene(|s| s.leaf(leaf).map(|q| q.world_transform()))
        .flatten()
        .expect("a leaf");
    match t {
        Transform3D::Affine(a) => *a.translation(),
        Transform3D::Identity => [0.0; 3],
    }
}

#[test]
fn a_group_is_picked_as_one_and_arranged_as_one_body() {
    let (document, leaves) = three_cubes([[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [40.0, 30.0, 0.0]]);
    assert!(!document.group(&leaves[..1]), "one model is no group");
    assert!(document.group(&leaves[..2]));
    assert_eq!(document.group_of(leaves[1]), Some(leaves[..2].to_vec()));
    assert_eq!(document.group_of(leaves[2]), None);

    // Picking one member picks the group.
    document.set_selection(Selection::One(LeafId(leaves[0])));
    let picked = document.selection_node_ids();
    assert_eq!(picked.len(), 2, "{picked:?}");
    assert!(picked.contains(&leaves[1]));

    // Arranged, the members keep their places to each other.
    let before = [place(&document, leaves[0]), place(&document, leaves[1])];
    let plate = bread_arrange::Plate {
        min: [-60.0, -60.0],
        max: [60.0, 60.0],
        floor: 0.0,
    };
    document.set_selection(Selection::None);
    arrange_now(&document, &plate, ArrangeOptions::default(), false).expect("arranged");
    let after = [place(&document, leaves[0]), place(&document, leaves[1])];
    let gap = |p: [[f64; 3]; 2]| [p[1][0] - p[0][0], p[1][1] - p[0][1]];
    let (a, b) = (gap(before), gap(after));
    assert!(
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
        "{a:?} {b:?}"
    );

    // Kept in the project by the members' rows.
    let payload = document.compose_project_payload(&PrintDefaults::default());
    assert_eq!(payload.groups, vec![vec![0, 1]]);

    // Undone: each on its own again, where it stands.
    assert!(document.ungroup(&leaves[1..2]));
    assert!(document.groups().is_empty());
    document.set_selection(Selection::One(LeafId(leaves[0])));
    assert_eq!(document.selection_node_ids(), vec![leaves[0]]);
    assert_eq!(place(&document, leaves[0]), after[0]);
}

#[test]
fn grouping_a_member_takes_in_its_whole_group() {
    let (document, leaves) = three_cubes([[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
    assert!(document.group(&leaves[..2]));
    assert!(document.group(&leaves[1..]));
    assert_eq!(
        document.groups(),
        vec![leaves.clone()],
        "one group of three"
    );
}
