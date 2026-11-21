//! Inside a hollow, supports share a stick from the cavity's floor (#632);
//! a zone preset's adaptive base and braces that follow their trunks
//! (#471).

use std::collections::HashMap;
use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};
use support_session::zone_presets::minis_30;
use support_session::{SupportOptions, SupportRequest, SupportSession, start_supports};

/// The 8 corners and 12 faces of the box `lo`..`hi`, looking out, or in
/// when `inward`, added to `vertices` and `faces`.
fn add_box(
    vertices: &mut Vec<Vertex>,
    faces: &mut Vec<IndexedTriangle>,
    lo: [f32; 3],
    hi: [f32; 3],
    inward: bool,
) {
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

fn mesh_of(boxes: &[([f32; 3], [f32; 3], bool)]) -> IndexedMesh {
    let (mut vertices, mut faces) = (Vec::new(), Vec::new());
    for &(lo, hi, inward) in boxes {
        add_box(&mut vertices, &mut faces, lo, hi, inward);
    }
    IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces)
}

fn run_on(mesh: IndexedMesh, at: [f64; 3], options: SupportOptions) -> SupportSession {
    let mut g = Scene::new();
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(at)),
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .unwrap();
    g.add_root(leaf).unwrap();
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

fn minis() -> SupportOptions {
    SupportOptions {
        zones: Some(minis_30()),
        ..SupportOptions::default()
    }
}

#[test]
fn contacts_inside_a_cavity_share_a_stick_from_its_floor() {
    // A 40 mm box hollowed to a 30 mm cavity, held over the plate: its
    // ceiling hangs over the cavity's floor.
    let session = run_on(
        mesh_of(&[
            ([0.0; 3], [40.0, 40.0, 30.0], false),
            ([5.0, 5.0, 5.0], [35.0, 35.0, 25.0], true),
        ]),
        [0.0, 0.0, 5.0],
        minis(),
    );
    let c = &session.state().collections;
    let stick_of: HashMap<&str, &str> = c
        .sticks
        .iter()
        .flat_map(|(id, s)| s.segments.iter().map(move |g| (g.id.as_str(), id.as_str())))
        .collect();
    let mut members: HashMap<&str, usize> = HashMap::new();
    let parents = c
        .leaves
        .values()
        .map(|l| l.parent_knot_id.as_str())
        .chain(c.branches.values().map(|b| b.parent_knot_id.as_str()));
    for knot in parents {
        if let Some(stick) = c
            .knots
            .get(knot)
            .and_then(|k| stick_of.get(k.parent_shaft_id.as_str()))
        {
            *members.entry(stick).or_default() += 1;
        }
    }
    assert!(!c.sticks.is_empty(), "sticks inside the cavity");
    assert!(
        !members.is_empty(),
        "contacts hang from sticks already there: {} sticks",
        c.sticks.len()
    );
    assert!(members.values().all(|&n| n <= 4), "{members:?}");
    let mesh = session.mesh().unwrap().expect("supports");
    assert!(mesh.is_sliceable());
}

#[test]
fn an_adaptive_base_is_wider_under_a_tall_support() {
    let block = || mesh_of(&[([0.0; 3], [6.0, 6.0, 4.0], false)]);
    let widest = |session: &SupportSession| {
        session
            .state()
            .collections
            .roots
            .values()
            .map(|r| r.diameter)
            .fold(0.0, f64::max)
    };
    let mut adaptive = minis();
    adaptive.zones.as_mut().unwrap().rules.adaptive_base = true;
    let low = run_on(block(), [0.0, 0.0, 10.0], adaptive.clone());
    let high = run_on(block(), [0.0, 0.0, 70.0], adaptive);
    let plain = run_on(block(), [0.0, 0.0, 70.0], minis());
    assert!(widest(&plain) > 0.0);
    assert!(
        (widest(&low) - widest(&plain)).abs() < 1e-9,
        "low: as the preset"
    );
    assert!(
        (widest(&high) - 1.5 * widest(&plain)).abs() < 1e-6,
        "{} {}",
        widest(&high),
        widest(&plain)
    );
}

#[test]
fn braces_that_follow_their_trunks_are_a_share_of_them() {
    let slab = || mesh_of(&[([0.0; 3], [30.0, 30.0, 3.0], false)]);
    let mut follow = minis();
    follow.zones.as_mut().unwrap().rules.braces_follow_trunks = true;
    let session = run_on(slab(), [0.0, 0.0, 25.0], follow);
    let c = &session.state().collections;
    assert!(!c.braces.is_empty(), "braced");
    // A brace is drawn at its knots' diameter, a share of the trunk.
    for brace in c.braces.values() {
        let knot = &c.knots[&brace.start_knot_id];
        let d = knot.diameter.expect("a diameter");
        assert!(d > 0.85 && d < 1.0, "{d}");
    }
}
