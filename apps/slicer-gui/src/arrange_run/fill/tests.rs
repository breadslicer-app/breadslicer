/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use sc_arrange::Plate;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{LeafPayload, NodeId, PAINTED, PaintChanges, PaintKind, Scene, UNPAINTED};

use super::{Landed, Pending, Progress, fill_now, gather_fill, land, make};
use crate::arrange_run::ArrangeOptions;
use crate::scene_document::{SceneDocument, SceneEdit};
use crate::selection::{LeafId, Selection};

/// A scene of 4 mm cubes standing on the plate at `at` (their middles in
/// X/Y), each with a mesh of its own, and their leaves.
fn cubes(at: &[[f64; 2]]) -> (Rc<SceneDocument>, Vec<NodeId>) {
    let mut graph = Scene::new();
    let mut leaves = Vec::new();
    for &[x, y] in at {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                    [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
                    [x, y, 2.0],
                )),
                LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    let document = SceneDocument::new();
    document.install_graph(graph, PathBuf::from("cubes.stl"));
    (document, leaves)
}

fn plate() -> Plate {
    Plate {
        min: [0.0, 0.0],
        max: [50.0, 40.0],
        floor: 0.0,
    }
}

fn select(document: &SceneDocument, leaves: &[NodeId]) {
    document.set_selection(Selection::of(leaves.iter().map(|&l| LeafId(l))));
}

/// Every model leaf of the scene, in row order, with its mesh and world
/// transform.
fn models(document: &SceneDocument) -> Vec<(NodeId, Arc<IndexedMesh>, AffineTransform3D)> {
    document
        .with_scene(|spatial| {
            (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .filter_map(|id| {
                    let mesh = spatial.graph().leaf_mesh_arc(id)?.clone();
                    let world = match spatial.leaf(id)?.world_transform() {
                        Transform3D::Identity => AffineTransform3D::identity(),
                        Transform3D::Affine(a) => a,
                    };
                    Some((id, mesh, world))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The middles of two 4 mm cubes are at least a cube and the gap apart
/// along X or Y, whatever their turn about Z.
fn apart(a: &AffineTransform3D, b: &AffineTransform3D) -> bool {
    let (p, q) = (a.translation(), b.translation());
    // A cube turned by any angle stays within its circumscribed circle;
    // turned by 45° steps its extent is at most 4·√2.
    (p[0] - q[0]).abs().max((p[1] - q[1]).abs()) >= 4.0 + 1.0 - 1e-6
        || (p[0] - q[0]).hypot(p[1] - q[1]) >= 4.0 * std::f64::consts::SQRT_2 + 1.0 - 1e-6
}

/// Whether the fill moved the model of body `body` to let more copies in.
fn moved(filled: &sc_arrange::Filled, body: usize) -> bool {
    filled.moved.iter().any(|m| m.source == body)
}

/// #356: a model in the plate's middle splits it; it moves to let the
/// copies in rows, in the same step, and Undo puts it back.
#[test]
fn a_model_in_the_middle_moves_to_let_more_copies_in() {
    let (document, leaves) = cubes(&[[25.0, 11.0]]);
    select(&document, &leaves);
    let before = models(&document);
    // 4 mm cubes on a 50 x 22 mm plate: 12 mm high inside the margin, two
    // rows of 5 mm; the cube in the middle leaves 3.5 mm above and below.
    let plate = Plate {
        min: [0.0, 0.0],
        max: [50.0, 22.0],
        floor: 0.0,
    };
    let filled = fill_now(&document, &plate, old_margin()).expect("filled").0;
    assert!(moved(&filled, 0), "it moves");
    let after = models(&document);
    assert_ne!(after[0].2, before[0].2);
    assert_eq!(after.len(), 1 + filled.copies.len());
    // Staying, only its own row takes copies: 6.
    assert!(filled.copies.len() >= 12, "{}", filled.copies.len());
    for (i, a) in after.iter().enumerate() {
        for b in &after[i + 1..] {
            assert!(apart(&a.2, &b.2), "{:?} {:?}", a.2, b.2);
        }
    }
    assert!(document.undo().unwrap());
    let back = models(&document);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].2, before[0].2, "back where it was");
    assert!(!document.can_undo(), "one step");
}

/// #356: one model selected; its copies fill the plate around it and the
/// others, which stay where they are, share its mesh, and go in one step.
#[test]
fn copies_of_one_model_fill_the_room_left_in_one_step() {
    let (document, leaves) = cubes(&[[10.0, 10.0], [25.0, 20.0], [40.0, 30.0]]);
    select(&document, &leaves[..1]);
    let before = models(&document);
    let filled = fill_now(&document, &plate(), old_margin())
        .expect("filled")
        .0;
    let after = models(&document);
    let added = filled.copies.len();
    assert!(added > 10, "{added} copies");
    assert_eq!(after.len(), before.len() + added);
    // What was there stays, the selected model too unless moving it lets
    // more copies in.
    for (row, (b, a)) in before.iter().zip(&after).enumerate() {
        assert_eq!(b.0, a.0);
        assert!(Arc::ptr_eq(&b.1, &a.1));
        assert_eq!(b.2 == a.2, !moved(&filled, row), "{row}");
    }
    // Each copy shows the selected model's mesh, never a duplicate, on the
    // plate, inside the margin, standing as it did.
    for (_, mesh, world) in &after[before.len()..] {
        assert!(Arc::ptr_eq(mesh, &before[0].1));
        let t = world.translation();
        assert!((t[2] - 2.0).abs() < 1e-9, "still on the plate");
        assert!(
            t[0] > 5.0 && t[0] < 45.0 && t[1] > 5.0 && t[1] < 35.0,
            "{t:?}"
        );
    }
    for (i, a) in after.iter().enumerate() {
        for b in &after[i + 1..] {
            assert!(apart(&a.2, &b.2), "{:?} {:?}", a.2, b.2);
        }
    }
    // One Undo takes every copy away; Redo puts them back.
    assert!(document.undo().unwrap());
    assert_eq!(models(&document).len(), before.len());
    assert!(!document.can_undo(), "one step");
    assert!(document.redo().unwrap());
    assert_eq!(models(&document).len(), after.len());
}

/// #356: two models selected: copies of both, as many of one as of the
/// other, give or take one; the unselected one stays.
#[test]
fn copies_of_two_models_mix_evenly() {
    let (document, leaves) = cubes(&[[10.0, 10.0], [25.0, 20.0], [40.0, 30.0]]);
    // Different meshes, so the copies tell which model they copy.
    select(&document, &[leaves[0], leaves[2]]);
    let before = models(&document);
    let filled = fill_now(&document, &plate(), old_margin())
        .expect("filled")
        .0;
    let count = |source: usize| filled.copies.iter().filter(|c| c.source == source).count();
    let (a, b) = (count(0), count(2));
    assert!(a > 3 && b > 3, "{a} and {b}");
    assert!(a.abs_diff(b) <= 1, "{a} and {b}");
    assert_eq!(count(1), 0, "the unselected one is not copied");
    let after = models(&document);
    assert_eq!(before[1].2, after[1].2, "the unselected one stays");
    for (row, (b, a)) in before.iter().zip(&after).enumerate() {
        assert_eq!(b.2 == a.2, !moved(&filled, row), "{row}");
    }
    let of = |mesh: &Arc<IndexedMesh>| {
        after[before.len()..]
            .iter()
            .filter(|(_, m, _)| Arc::ptr_eq(m, mesh))
            .count()
    };
    assert_eq!(of(&before[0].1), a);
    assert_eq!(of(&before[2].1), b);
    assert_eq!(of(&before[1].1), 0);
}

/// #356: Max copies caps the count; a full plate adds none; nothing
/// selected fills nothing.
#[test]
fn a_fill_takes_at_most_the_copies_asked_for() {
    let (document, leaves) = cubes(&[[10.0, 10.0]]);
    assert!(gather_fill(&document, None).is_none(), "nothing selected");
    select(&document, &leaves);
    let options = ArrangeOptions {
        max_copies: 3,
        ..ArrangeOptions::default()
    };
    let filled = fill_now(&document, &plate(), options).expect("filled").0;
    assert_eq!(filled.copies.len(), 3);
    assert_eq!(models(&document).len(), 4);

    let small = Plate {
        min: [3.0, 3.0],
        max: [17.0, 17.0],
        floor: 0.0,
    };
    let none = fill_now(&document, &small, old_margin()).expect("ran").0;
    assert!(none.copies.is_empty());
    assert_eq!(models(&document).len(), 4, "nothing added");
}

/// #183: each copy keeps the paint of its model, on its own mask, and the
/// one Undo of the fill takes the copies and their paint.
#[test]
fn a_filled_copy_keeps_the_paint_of_its_model() {
    let (document, leaves) = cubes(&[[10.0, 10.0]]);
    let edit = SceneEdit::PaintSurface {
        node_id: leaves[0],
        kind: PaintKind::Detail,
        changes: PaintChanges::from_faces(vec![(0, UNPAINTED, PAINTED), (1, UNPAINTED, PAINTED)]),
    };
    document.apply_edit(edit.clone()).unwrap();
    document.push_edit(edit);
    select(&document, &leaves);
    let options = ArrangeOptions {
        max_copies: 2,
        ..ArrangeOptions::default()
    };
    fill_now(&document, &plate(), options).expect("filled");
    let all = models(&document);
    assert_eq!(all.len(), 3);
    for (id, _, _) in &all {
        let tags = document
            .surface_tags(*id, PaintKind::Detail)
            .expect("painted");
        assert_eq!(tags.painted_faces(), 2);
    }
    document.undo().unwrap();
    assert_eq!(models(&document).len(), 1);
    assert!(
        document
            .surface_tags(leaves[0], PaintKind::Detail)
            .is_some()
    );
}

/// The card's options with the edge margin these plates were sized for.
fn old_margin() -> ArrangeOptions {
    ArrangeOptions {
        margin_mm: 5.0,
        ..ArrangeOptions::default()
    }
}

/// #356: a cancelled fill makes nothing, and a result worked out for a
/// scene that changed since adds nothing.
#[test]
fn a_cancelled_or_stale_fill_adds_nothing() {
    let (document, leaves) = cubes(&[[10.0, 10.0]]);
    select(&document, &leaves);
    let (bodies, _, sources) = gather_fill(&document, None).expect("a source");
    let settings = old_margin().settings();
    let cancelled = make(
        &bodies,
        &sources,
        &plate(),
        &settings,
        None,
        &Progress::default(),
        &AtomicBool::new(true),
    );
    assert!(cancelled.is_none());

    let pending = Pending {
        revision: document.scene_revision(),
        sources: sources.clone(),
        started: std::time::Instant::now(),
    };
    let made = make(
        &bodies,
        &sources,
        &plate(),
        &settings,
        None,
        &Progress::default(),
        &AtomicBool::new(false),
    )
    .expect("made");
    assert!(!made.filled.copies.is_empty());
    // The model moves meanwhile.
    let moved = SceneEdit::SetLocalTransform {
        node_id: leaves[0],
        before: document
            .with_scene(|s| s.leaf(leaves[0]).map(|q| q.local_transform()))
            .flatten()
            .unwrap(),
        after: Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
            [30.0, 20.0, 2.0],
        )),
    };
    document.apply_edit(moved.clone()).unwrap();
    document.push_edit(moved);
    assert_eq!(land(&document, &pending, made), Landed::Stale);
    assert_eq!(models(&document).len(), 1, "nothing added");
}
