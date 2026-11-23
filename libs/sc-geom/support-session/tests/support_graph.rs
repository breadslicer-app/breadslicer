//! The supports as a beam frame (#243): a generated set, loaded through its
//! tips, holds the body on its roots.

use std::sync::Arc;

use bread_structure::{BodyLoad, CheckOptions, Material, SupportGraph, check_layer};
use bread_support_session::zone_presets::minis_30;
use bread_support_session::{SupportOptions, SupportRequest, SupportSession, start_supports};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};

/// The box `lo`..`hi` (looking in when `inward`), meshed.
fn mesh_of(boxes: &[([f32; 3], [f32; 3], bool)]) -> IndexedMesh {
    let (mut vertices, mut faces) = (Vec::new(), Vec::new());
    for &(lo, hi, inward) in boxes {
        let at = vertices.len() as u32;
        for i in 0..8 {
            vertices.push(Vertex::new([
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ]));
        }
        for f in [
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
        ] {
            let [a, b, c] = f.map(|k| at + k);
            faces.push(IndexedTriangle::from_raw_indices(if inward {
                [a, c, b]
            } else {
                [a, b, c]
            }));
        }
    }
    IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces)
}

fn run_on(mesh: IndexedMesh, at: [f64; 3]) -> SupportSession {
    let mut g = Scene::new();
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(at)),
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .unwrap();
    g.add_root(leaf).unwrap();
    let options = SupportOptions {
        zones: Some(minis_30()),
        ..SupportOptions::default()
    };
    start_supports(
        &g,
        SupportRequest {
            options,
            ..SupportRequest::default()
        },
    )
    .unwrap()
    .join()
    .unwrap()
    .sets
    .remove(0)
    .session
}

const OPTIONS: CheckOptions = CheckOptions {
    material: Material::GREEN_RESIN,
    strength_mpa: 8.0,
    lateral_share: 0.05,
    directions: 8,
};

/// One body over the whole graph, held by every tip and foot, pulled at
/// `at` with `force_n`.
fn whole_body(graph: &SupportGraph, at: [f64; 3], force_n: f64) -> BodyLoad {
    BodyLoad {
        force_n,
        at,
        held: false,
        tips: graph.contacts().iter().map(|c| c.tip).collect(),
        feet: graph.feet().iter().map(|f| f.at).collect(),
    }
}

#[test]
fn a_braced_set_holds_its_body_on_its_roots() {
    let session = run_on(
        mesh_of(&[([0.0; 3], [30.0, 30.0, 3.0], false)]),
        [0.0, 0.0, 25.0],
    );
    let c = &session.state().collections;
    assert!(!c.braces.is_empty(), "braced");
    let graph = session.support_graph();
    assert!(!graph.contacts().is_empty());
    let fixed = graph
        .frame()
        .holds()
        .iter()
        .filter(|h| **h == bread_structure::Hold::Fixed)
        .count();
    assert!(
        fixed > 0 && fixed <= c.roots.len(),
        "{fixed} fixed nodes for {} roots",
        c.roots.len()
    );
    assert!(graph.feet().is_empty(), "nothing stands on the model");

    let body = whole_body(&graph, [15.0, 15.0, 25.0], 5.0);
    let check = check_layer(std::slice::from_ref(&graph), 25.0, &[body], &OPTIONS).unwrap();
    assert_eq!(check.tips_missed, 0);
    assert_eq!(check.tips_found, graph.contacts().len());
    assert_eq!(check.bodies_unheld, 0);
    assert!(check.residual < 1e-8, "residual {}", check.residual);
    assert!(check.worst > 0.0 && check.worst.is_finite());
    // The braces carry some of the sideways share.
    let braced = graph
        .owners()
        .iter()
        .zip(&check.owners[0])
        .filter(|(id, u)| c.braces.contains_key(id.as_str()) && **u > 0.0)
        .count();
    assert!(braced > 0, "no brace carries anything");

    // Every tip alone reaches the raft through its support.
    for contact in graph.contacts().iter().step_by(7) {
        let one = BodyLoad {
            force_n: 0.1,
            at: contact.tip,
            held: false,
            tips: vec![contact.tip],
            feet: Vec::new(),
        };
        let check = check_layer(std::slice::from_ref(&graph), 25.0, &[one], &OPTIONS).unwrap();
        assert_eq!(
            check.bodies_unheld, 0,
            "a tip on nothing: {:?}",
            contact.tip
        );
    }
}

#[test]
fn sticks_inside_a_hollow_join_its_floor_to_its_ceiling() {
    let session = run_on(
        mesh_of(&[
            ([0.0; 3], [40.0, 40.0, 30.0], false),
            ([5.0, 5.0, 5.0], [35.0, 35.0, 25.0], true),
        ]),
        [0.0, 0.0, 5.0],
    );
    assert!(!session.state().collections.sticks.is_empty());
    let graph = session.support_graph();
    let body = whole_body(&graph, [20.0, 20.0, 35.0], 10.0);
    let check = check_layer(std::slice::from_ref(&graph), 35.0, &[body], &OPTIONS).unwrap();
    assert_eq!(check.tips_missed, 0);
    assert_eq!(check.bodies_unheld, 0);
    assert!(check.residual < 1e-8, "residual {}", check.residual);
}

/// The graph is the same each time it is read.
#[test]
fn the_graph_is_the_same_each_time() {
    let session = run_on(
        mesh_of(&[([0.0; 3], [20.0, 20.0, 3.0], false)]),
        [0.0, 0.0, 15.0],
    );
    let (a, b) = (session.support_graph(), session.support_graph());
    assert_eq!(a.frame().nodes(), b.frame().nodes());
    assert_eq!(a.frame().members(), b.frame().members());
    assert_eq!(a.owners(), b.owners());
}
