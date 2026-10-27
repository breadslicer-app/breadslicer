//! A support session kept as data (#145): its document rebuilds the same
//! supports on the same models, the rebuilt session still edits by hand, and
//! a document for other models, or from a newer build, is refused.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};
use support_gen_df_sc::{
    DF_SUPPORT_DOCUMENT_VERSION, DfSupportRequest, DfSupportSession, RestoreError,
    start_df_supports,
};

/// A closed 10 mm cube, outward-wound.
fn cube() -> IndexedMesh {
    let s = 10.0;
    let v = |x: f32, y: f32, z: f32| Vertex::new([x * s, y * s, z * s]);
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
    IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces)
}

fn scene_with(meshes: Vec<(IndexedMesh, [f64; 3])>) -> Scene {
    let mut g = Scene::new();
    for (mesh, offset) in meshes {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(offset)),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
    }
    g
}

fn run_on(scene: &Scene) -> DfSupportSession {
    start_df_supports(scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .session
}

#[test]
fn a_document_rebuilds_the_same_supports_and_they_stay_editable() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let mut session = run_on(&scene);
    // A hand edit first, so the id counter it moved is part of what is kept.
    session
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    let mesh = session.mesh().unwrap().expect("a support mesh");
    let document = session.to_document().expect("a run from options is data");

    // The reopened scene: a fresh graph with other node ids, holding the
    // support leaf too, which is not a model.
    let reopened = scene_with(vec![
        (mesh.clone(), [0.0, 0.0, 0.0]),
        (cube(), [0.0, 0.0, 8.0]),
    ]);
    let mut rebuilt =
        DfSupportSession::from_document(&reopened, DF_SUPPORT_DOCUMENT_VERSION, &document)
            .expect("the same models");
    assert_eq!(rebuilt.state(), session.state());
    let again = rebuilt.mesh().unwrap().expect("a support mesh");
    assert_eq!(again.vertices(), mesh.vertices());
    assert_eq!(again.faces(), mesh.faces());

    // Edits go on: a removal, and a new support whose ids do not collide
    // with the hand-placed one kept in the document.
    let roots = rebuilt.state().collections.roots.len();
    rebuilt
        .remove_near([1.5, 1.5, 7.9])
        .expect("a support there");
    rebuilt
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    assert_eq!(rebuilt.state().collections.roots.len(), roots);
}

#[test]
fn a_document_for_other_models_is_refused() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let document = run_on(&scene).to_document().unwrap();
    let moved = scene_with(vec![(cube(), [5.0, 0.0, 8.0])]);
    assert!(matches!(
        DfSupportSession::from_document(&moved, DF_SUPPORT_DOCUMENT_VERSION, &document),
        Err(RestoreError::ModelsChanged)
    ));
    let none = Scene::new();
    assert!(matches!(
        DfSupportSession::from_document(&none, DF_SUPPORT_DOCUMENT_VERSION, &document),
        Err(RestoreError::ModelsChanged)
    ));
}

#[test]
fn a_session_fits_its_models_however_the_scene_holds_them() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let session = run_on(&scene);
    let mesh = session.mesh().unwrap().expect("a support mesh");
    assert!(session.fits_models_of(&scene));
    // The support leaf is not a model, and node ids do not matter.
    let with_supports = scene_with(vec![
        (mesh, [0.0, 0.0, 0.0]),
        (cube(), [0.0, 0.0, 8.0]),
    ]);
    assert!(session.fits_models_of(&with_supports));
    assert!(!session.fits_models_of(&scene_with(vec![(cube(), [5.0, 0.0, 8.0])])));
    assert!(!session.fits_models_of(&Scene::new()));
}

#[test]
fn a_newer_or_damaged_document_is_refused() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let document = run_on(&scene).to_document().unwrap();
    assert!(matches!(
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION + 1, &document),
        Err(RestoreError::NewerVersion(_))
    ));
    assert!(matches!(
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION, b"{"),
        Err(RestoreError::Malformed(_))
    ));
}
